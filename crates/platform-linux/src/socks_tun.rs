//! Shared SOCKS5 → TUN system tunnel (Linux).
//!
//! Ported from `platform-windows::tor_tunnel` but using Linux TUN device
//! (`/dev/net/tun`) via `tun2proxy` + `tproxy-config`. Used by both
//! Outline (Shadowsocks) and Tor system-wide modes.
//!
//! Requires root (CAP_NET_ADMIN). One global tunnel slot.

use anyhow::{Context, Result};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};

#[cfg(target_os = "linux")]
use tun2proxy::{general_run_async, ArgDns, ArgProxy, Args, CancellationToken, ProxyType, DEFAULT_MTU};

#[cfg(target_os = "linux")]
static SOCKS_TUNNEL: OnceLock<Mutex<Option<SocksTunnelHandle>>> = OnceLock::new();

#[cfg(target_os = "linux")]
struct SocksTunnelHandle {
    cancel: CancellationToken,
    thread: Option<JoinHandle<()>>,
    tun_name: String,
    ipv6_guard: Option<crate::leak_protect::Guard>,
}

#[cfg(target_os = "linux")]
fn socks_slot() -> &'static Mutex<Option<SocksTunnelHandle>> {
    SOCKS_TUNNEL.get_or_init(|| Mutex::new(None))
}

fn tunnel_log(msg: &str) {
    let line = format!(
        "[{}] {msg}\n",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let candidates = [
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("socks-tunnel.log"))),
        Some(std::env::temp_dir().join("zeronode-socks-tunnel.log")),
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

/// Collect bypass CIDRs for a remote SOCKS server to avoid routing loops.
/// On Linux we resolve the hostname to /32 CIDRs.
#[cfg(target_os = "linux")]
fn resolve_server_bypass_cidrs(host: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(addrs) = format!("{host}:443").to_socket_addrs() {
        for addr in addrs {
            if let IpAddr::V4(v4) = addr.ip() {
                if !v4.is_loopback() && !v4.is_private() && !v4.is_link_local() {
                    let cidr = format!("{v4}/32");
                    if !out.contains(&cidr) {
                        out.push(cidr);
                    }
                }
            }
        }
    }
    // Also try direct IP parse
    if let Ok(IpAddr::V4(v4)) = host.parse::<IpAddr>() {
        if !v4.is_loopback() && !v4.is_private() && !v4.is_link_local() {
            let cidr = format!("{v4}/32");
            if !out.contains(&cidr) {
                out.push(cidr);
            }
        }
    }
    out
}

/// Route all system traffic through a local SOCKS5 port using TUN + tun2proxy.
/// `extra_bypass` are CIDR strings that stay on physical NIC.
pub fn start_socks_system_tunnel(
    socks_port: u16,
    tun_name: &str,
    extra_bypass: &[String],
) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (socks_port, tun_name, extra_bypass);
        anyhow::bail!("SOCKS system tunnel is only available on Linux");
    }
    #[cfg(target_os = "linux")]
    {
        stop_socks_system_tunnel()?;

        if crate::elevation::current_uid() != Some(0) {
            anyhow::bail!("system-wide SOCKS tunnel requires root. Re-launch via pkexec.");
        }

        tunnel_log(&format!(
            "start_socks_system_tunnel: socks=127.0.0.1:{socks_port} tun={tun_name}"
        ));

        if !std::path::Path::new("/dev/net/tun").exists() {
            anyhow::bail!("/dev/net/tun not available; load tun module with: sudo modprobe tun");
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
            "127.0.0.0/8".parse().context("failed to parse bypass CIDR")?,
            "10.0.0.0/8".parse().context("failed to parse bypass CIDR")?,
            "172.16.0.0/12".parse().context("failed to parse bypass CIDR")?,
            "192.168.0.0/16".parse().context("failed to parse bypass CIDR")?,
            "169.254.0.0/16".parse().context("failed to parse bypass CIDR")?,
        ];

        for peer in extra_bypass {
            match peer.parse() {
                Ok(cidr) => bypass.push(cidr),
                Err(e) => tunnel_log(&format!("skip bad bypass {peer}: {e}")),
            }
        }
        args.bypass = bypass;

        let cancel = CancellationToken::new();
        let cancel_worker = cancel.clone();
        let worker_name = format!("{tun_name}-tun2proxy");
        let tun_name_owned = tun_name.to_string();

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
                            tunnel_log(&format!("socks tun2proxy exited normally (sessions={sessions})"));
                        }
                        Err(e) => {
                            tunnel_log(&format!("socks tun2proxy exited with error: {e}"));
                        }
                    }
                });
            })
            .context("failed to spawn socks tun2proxy thread")?;

        {
            let mut slot = socks_slot().lock().unwrap();
            *slot = Some(SocksTunnelHandle {
                cancel,
                thread: Some(thread),
                tun_name: tun_name_owned,
                // tun2proxy path is IPv4-only here — block v6 leaks for the
                // session (ProtonVPN behaviour). Restored on stop.
                ipv6_guard: Some(crate::leak_protect::disable_all()),
            });
        }

        // Liveness probe
        for i in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let slot = socks_slot().lock().unwrap();
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
                    "system tunnel worker exited immediately — check socks-tunnel.log. \
                     Common causes: missing /dev/net/tun, another VPN holding routes, or route setup failure."
                );
            }
            if i >= 5 {
                break;
            }
        }

        tunnel_log("start_socks_system_tunnel: OK (worker running)");
        Ok(())
    }
}

/// Tor-specific wrapper that adds Tor guard bypasses.
pub fn start_tor_system_tunnel(socks_port: u16) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = socks_port;
        anyhow::bail!("Tor system tunnel is only available on Linux");
    }
    #[cfg(target_os = "linux")]
    {
        let bypass = tor_guard_bypass_strings();
        tunnel_log(&format!("tor guard bypass count={}", bypass.len()));
        start_socks_system_tunnel(socks_port, "ZeroNodeTor", &bypass)
    }
}

#[cfg(target_os = "linux")]
fn tor_guard_bypass_strings() -> Vec<String> {
    // Try to find Tor's established peers via /proc/net/tcp and pid
    let mut peers = Vec::new();
    let pids = crate::procfs::find_pids_by_name("tor");
    if pids.is_empty() {
        return peers;
    }
    // Use `ss` to find peers
    if let Some(output) = crate::common::silent_output("ss", &["-tnp", "state", "established"]) {
        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("State") {
                continue;
            }
            // Check if line contains any of our tor pids
            let has_tor_pid = pids.iter().any(|pid| line.contains(&format!("pid={pid}")));
            if !has_tor_pid {
                continue;
            }
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 4 {
                continue;
            }
            // ss output: State Recv-Q Send-Q Local Address:Port Peer Address:Port Process
            let peer = cols[3];
            let ip_str = peer.rsplit_once(':').map(|(ip, _)| ip).unwrap_or(peer);
            let ip_str = ip_str.trim_matches(|c| c == '[' || c == ']');
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

pub fn stop_socks_system_tunnel() -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let mut slot = socks_slot().lock().unwrap();
        if let Some(mut handle) = slot.take() {
            tunnel_log("stop_socks_system_tunnel: cancelling worker");
            handle.cancel.cancel();
            if let Some(thread) = handle.thread.take() {
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
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
            }
            // Routes are down — bring IPv6 back exactly as we found it.
            if let Some(guard) = handle.ipv6_guard.take() {
                crate::leak_protect::restore(guard);
                tunnel_log("stop_socks_system_tunnel: ipv6 restored");
            }
        }
        Ok(())
    }
}

pub fn stop_tor_system_tunnel() -> Result<()> {
    stop_socks_system_tunnel()
}

pub fn is_tor_tunnel_running() -> bool {
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
    #[cfg(target_os = "linux")]
    {
        let slot = socks_slot().lock().unwrap();
        slot.as_ref()
            .map(|h| h.thread.as_ref().map(|t| !t.is_finished()).unwrap_or(false))
            .unwrap_or(false)
    }
}

pub fn is_socks_tunnel_running() -> bool {
    is_tor_tunnel_running()
}
