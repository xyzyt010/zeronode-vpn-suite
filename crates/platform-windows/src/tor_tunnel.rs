//! Laptop-wide Tor routing via Wintun + tun2proxy (Rust SOCKS5 → TUN).
//!
//! This is a **real tunnel**, not a permanent WinINet / system HTTP proxy:
//! traffic is routed through a temporary Wintun adapter while the tunnel is
//! up. On stop we cancel tun2proxy (which tears down its own routes/adapter)
//! and never leave port-based OS proxy settings behind.
//!
//! Tor's own OR connections to entry guards must bypass the tunnel or they
//! loop (Tor → default route → Wintun → SOCKS → Tor). Before starting we
//! collect tor.exe remote peers and add host routes via the real gateway.

use anyhow::{Context, Result};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use tun2proxy::{
    general_run_async, ArgDns, ArgProxy, Args, CancellationToken, ProxyType, DEFAULT_MTU,
};

use crate::silent_cmd::silent_output;

static TOR_TUNNEL: OnceLock<Mutex<Option<TorTunnelHandle>>> = OnceLock::new();

struct TorTunnelHandle {
    cancel: CancellationToken,
    thread: Option<JoinHandle<()>>,
}

fn tunnel_slot() -> &'static Mutex<Option<TorTunnelHandle>> {
    TOR_TUNNEL.get_or_init(|| Mutex::new(None))
}

fn tunnel_log(msg: &str) {
    // Best-effort diagnostic next to the binary / temp so GUI (no console) runs
    // can still be debugged when the system tunnel fails.
    let line = format!(
        "[{}] {}\n",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        msg
    );
    let candidates = [
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("tor-tunnel.log"))),
        Some(std::env::temp_dir().join("zeronode-tor-tunnel.log")),
    ];
    for path in candidates.into_iter().flatten() {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = f.write_all(line.as_bytes());
            break;
        }
    }
    eprintln!("{msg}");
}

/// Collect remote IPv4 peers currently used by tor.exe so we can bypass them
/// through the physical gateway (prevents Tor OR ↔ tunnel feedback loops).
///
/// Uses `tasklist` + `netstat` with CREATE_NO_WINDOW — never PowerShell — so
/// connect/disconnect does not flash black console windows.
fn tor_or_peer_bypass_strings() -> Vec<String> {
    // PIDs of tor.exe (CSV: "tor.exe","1234",...)
    let mut tor_pids: Vec<u32> = Vec::new();
    if let Some(list) = silent_output(
        "tasklist",
        &["/FI", "IMAGENAME eq tor.exe", "/FO", "CSV", "/NH"],
    ) {
        for line in list.lines() {
            // "tor.exe","1234","Session Name","Session#","Mem Usage"
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 2 {
                let pid_s = parts[1].trim().trim_matches('"');
                if let Ok(pid) = pid_s.parse::<u32>() {
                    tor_pids.push(pid);
                }
            }
        }
    }
    if tor_pids.is_empty() {
        return Vec::new();
    }

    let mut peers = Vec::new();
    if let Some(netstat) = silent_output("netstat", &["-ano"]) {
        for line in netstat.lines() {
            let line = line.trim();
            // TCP    192.168.x.x:port    1.2.3.4:443    ESTABLISHED    1234
            if !line.starts_with("TCP") && !line.starts_with("tcp") {
                continue;
            }
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 5 {
                continue;
            }
            let state = cols[3];
            if !state.eq_ignore_ascii_case("ESTABLISHED") {
                continue;
            }
            let Ok(pid) = cols[4].parse::<u32>() else {
                continue;
            };
            if !tor_pids.contains(&pid) {
                continue;
            }
            // remote is cols[2] as ip:port (IPv4) or [ipv6]:port
            let remote = cols[2];
            let ip_str = if remote.starts_with('[') {
                continue; // skip IPv6 for /32 bypass list
            } else {
                remote.rsplit_once(':').map(|(ip, _)| ip).unwrap_or(remote)
            };
            if let Ok(IpAddr::V4(v4)) = ip_str.parse::<IpAddr>() {
                if !v4.is_loopback() && !v4.is_private() && !v4.is_link_local() {
                    let cidr = format!("{v4}/32");
                    if !peers.contains(&cidr) {
                        peers.push(cidr);
                    }
                }
            }
        }
    }
    peers
}

/// Route all system traffic through a local SOCKS5 port using Wintun + tun2proxy.
/// Shared by Tor and Outline. Requires Administrator.
///
/// `extra_bypass` is a list of CIDR strings (e.g. `"1.2.3.4/32"`) that must stay
/// on the physical NIC (remote VPN/SS server, Tor OR peers, etc.).
///
/// Does **not** write permanent Internet Settings / WinHTTP proxy values.
pub fn start_socks_system_tunnel(
    socks_port: u16,
    tun_name: &str,
    extra_bypass: &[String],
) -> Result<()> {
    stop_socks_system_tunnel()?;

    if !crate::elevation::is_elevated() {
        tunnel_log("start_socks_system_tunnel: process is NOT elevated");
        anyhow::bail!(
            "system-wide SOCKS tunnel requires Administrator. Re-launch elevated (UAC) or right-click vpn-client.exe → Run as administrator."
        );
    }

    tunnel_log(&format!(
        "start_socks_system_tunnel: elevated=true socks=127.0.0.1:{socks_port} tun={tun_name}"
    ));

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let wintun = dir.join("wintun.dll");
            if !wintun.exists() {
                tunnel_log(&format!(
                    "WARNING: wintun.dll missing at {} — tunnel create may fail",
                    wintun.display()
                ));
            }
        }
    }

    let proxy = ArgProxy {
        proxy_type: ProxyType::Socks5,
        addr: SocketAddr::from(([127, 0, 0, 1], socks_port)),
        credentials: None,
    };

    let mut args = Args::default();
    args.proxy = proxy;
    args.tun = Some(tun_name.to_string());
    args.setup = true;
    args.dns = ArgDns::OverTcp;
    args.ipv6_enabled = false;

    let mut bypass = vec![
        "127.0.0.0/8"
            .parse()
            .context("failed to parse localhost bypass CIDR")?,
        "10.0.0.0/8"
            .parse()
            .context("failed to parse RFC1918 10/8 bypass")?,
        "172.16.0.0/12"
            .parse()
            .context("failed to parse RFC1918 172.16/12 bypass")?,
        "192.168.0.0/16"
            .parse()
            .context("failed to parse RFC1918 192.168/16 bypass")?,
        "169.254.0.0/16"
            .parse()
            .context("failed to parse link-local bypass")?,
    ];

    for peer in extra_bypass {
        match peer.parse() {
            Ok(cidr) => bypass.push(cidr),
            Err(e) => tunnel_log(&format!("skip bad peer bypass {peer}: {e}")),
        }
    }
    args.bypass = bypass;

    let cancel = CancellationToken::new();
    let cancel_worker = cancel.clone();
    let worker_name = format!("{tun_name}-tun2proxy");

    let thread = thread::Builder::new()
        .name(worker_name)
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tunnel_log(&format!("socks tun2proxy runtime failed: {e}"));
                    return;
                }
            };
            rt.block_on(async move {
                match general_run_async(args, DEFAULT_MTU, false, cancel_worker).await {
                    Ok(sessions) => {
                        tunnel_log(&format!(
                            "socks tun2proxy exited normally (sessions={sessions})"
                        ));
                    }
                    Err(e) => {
                        tunnel_log(&format!("socks tun2proxy exited with error: {e}"));
                    }
                }
            });
        })
        .context("failed to spawn socks tun2proxy thread")?;

    {
        let mut slot = tunnel_slot().lock().unwrap();
        *slot = Some(TorTunnelHandle {
            cancel,
            thread: Some(thread),
        });
    }

    for i in 0..20 {
        thread::sleep(std::time::Duration::from_millis(200));
        let slot = tunnel_slot().lock().unwrap();
        let finished = slot
            .as_ref()
            .and_then(|h| h.thread.as_ref())
            .map(|t| t.is_finished())
            .unwrap_or(true);
        if finished {
            drop(slot);
            tunnel_log(&format!(
                "tunnel worker died during startup (after {} ms)",
                (i + 1) * 200
            ));
            let _ = stop_socks_system_tunnel();
            anyhow::bail!(
                "system tunnel worker exited immediately — check tor-tunnel.log next to the exe. \
                 Common causes: missing wintun.dll, another VPN holding routes, or route setup failure."
            );
        }
        if i >= 5 {
            break;
        }
    }

    if let Some(names) = silent_output("netsh", &["interface", "show", "interface"]) {
        let hit = names
            .lines()
            .filter(|l| {
                let u = l.to_ascii_lowercase();
                u.contains("zeronode") || u.contains("wintun") || u.contains("wireguard")
            })
            .take(4)
            .collect::<Vec<_>>()
            .join(" | ");
        if !hit.is_empty() {
            tunnel_log(&format!("tunnel-like interfaces after start: {hit}"));
        }
    }

    tunnel_log("start_socks_system_tunnel: OK (worker running)");
    Ok(())
}

/// Route all system traffic through a local Tor SOCKS5 port using a Wintun adapter.
pub fn start_tor_system_tunnel(socks_port: u16) -> Result<()> {
    let bypass = tor_or_peer_bypass_strings();
    tunnel_log(&format!("tor OR peer bypass count={}", bypass.len()));
    start_socks_system_tunnel(socks_port, "ZeroNodeTor", &bypass)
}

/// Tear down the Wintun adapter and restore system routes.
pub fn stop_socks_system_tunnel() -> Result<()> {
    let mut slot = tunnel_slot().lock().unwrap();
    if let Some(mut handle) = slot.take() {
        tunnel_log("stop_socks_system_tunnel: cancelling worker");
        handle.cancel.cancel();
        if let Some(thread) = handle.thread.take() {
            // Keep disconnect snappy: wait briefly for clean route restore,
            // then detach so the UI never blocks on a stuck worker.
            let timeout = std::time::Duration::from_millis(1200);
            let start = std::time::Instant::now();
            loop {
                if thread.is_finished() {
                    let _ = thread.join();
                    break;
                }
                if start.elapsed() > timeout {
                    std::mem::forget(thread);
                    tunnel_log("stop_socks_system_tunnel: worker join timed out (detached)");
                    break;
                }
                thread::sleep(std::time::Duration::from_millis(30));
            }
        }
    }
    Ok(())
}

/// Tear down the Wintun adapter and restore system routes.
///
/// This only cancels the in-process tun2proxy worker. It never writes
/// permanent Internet Options proxy settings.
pub fn stop_tor_system_tunnel() -> Result<()> {
    stop_socks_system_tunnel()
}

pub fn is_tor_tunnel_running() -> bool {
    let slot = tunnel_slot().lock().unwrap();
    slot.as_ref()
        .map(|h| h.thread.as_ref().map(|t| !t.is_finished()).unwrap_or(false))
        .unwrap_or(false)
}
