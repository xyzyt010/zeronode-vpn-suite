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
        // For Tor the exit cannot reliably reach external DNS like 8.8.8.8:53
        // (many exits block port 53). Use Virtual DNS so every hostname gets
        // a fake 198.18.x.y that tun2proxy maps back to the domain and
        // connects via SOCKS5h — Tor then resolves via its own DNS. For
        // non-Tor tunnels (Outline/Shadowsocks) the remote can reach 8.8.8.8
        // so OverTcp is fine.
        if tun_name == "ZeroNodeTor" {
            args.dns = ArgDns::Virtual;
        } else {
            args.dns = ArgDns::OverTcp;
        }
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
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
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
///
/// The system route must not flip while Tor is still bootstrapping: if it
/// does, Tor's own guard connections get routed into the TUN → SOCKS loop
/// whose only exit is Tor itself → deadlock at ~18% forever. So we wait
/// (up to 10 s) until Tor actually holds ESTABLISHED guard connections and
/// bypass exactly those /32s before installing the route.
pub fn start_tor_system_tunnel(socks_port: u16) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = socks_port;
        anyhow::bail!("Tor system tunnel is only available on Linux");
    }
    #[cfg(target_os = "linux")]
    {
        // Clean any stale TUN/routes left by a previous SIGKILL before we
        // attempt to discover guards — otherwise Tor can't reach its guards
        // at all and we deadlock at 18%.
        let _ = stop_socks_system_tunnel();
        // Also belt-and-braces tproxy cleanup in case the worker was detached.
        {
            let _ = std::thread::Builder::new()
                .name("zn-tproxy-pre-clean".into())
                .spawn(|| {
                    if let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(1)
                        .enable_all()
                        .build()
                    {
                        let _ = rt.block_on(async { tproxy_config::tproxy_remove(None).await });
                    }
                })
                .map(|h| h.join());
        }

        let mut bypass = Vec::new();
        for attempt in 0..30 {
            bypass = tor_guard_bypass_strings();
            if !bypass.is_empty() {
                if attempt > 0 {
                    tunnel_log(&format!("tor bypass: found {len} guards after {attempt} retries", len = bypass.len()));
                }
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            if attempt == 4 || attempt == 10 || attempt == 20 {
                tunnel_log(&format!(
                    "tor bypass: no established guards yet (attempt {attempt}/30) — waiting (bootstrap typically 10-25s)"
                ));
            }
        }
        tunnel_log(&format!("tor guard bypass count={} list={:?}", bypass.len(), bypass));
        if bypass.is_empty() {
            // Empty bypass would route Tor's own guard connections into the
            // TUN → SOCKS loop (deadlock, DNS blackhole, "no internet").
            // Fail fast so the GUI can retry via ApplyTorSystemRoute (it
            // already does 24×5s retries when geo_ready). If Tor really has
            // no guards yet, the retry will find them once the consensus
            // loads.
            tunnel_log("tor guard bypass empty after 15s — failing so GUI can retry (prevents routing loop)");
            anyhow::bail!(
                "no Tor guard connections found (Tor still bootstrapping or proc parsing failed) — retrying in 5s"
            );
        }
        start_socks_system_tunnel(socks_port, "ZeroNodeTor", &bypass)
    }
}

#[cfg(target_os = "linux")]
fn tor_guard_bypass_strings() -> Vec<String> {
    // Most reliable: walk /proc/net/tcp directly via Tor's socket inodes.
    // This does not need `ss` and works even when ss output is capped or
    // pid column is hidden. Fall back to ss with pid filter only if /proc
    // yields nothing.
    let pids = crate::procfs::find_pids_by_name("tor");
    if pids.is_empty() {
        tunnel_log("tor bypass: no tor pids found");
        return Vec::new();
    }
    let tor_inodes = tor_socket_inodes(&pids);
    if tor_inodes.is_empty() {
        tunnel_log(&format!("tor bypass: tor pids {:?} but no socket inodes (still starting)", pids));
    } else {
        let mut peers: Vec<String> = Vec::new();
        if let Ok(tcp) = std::fs::read_to_string("/proc/net/tcp") {
            for line in tcp.lines().skip(1) {
                let cols: Vec<&str> = line.split_whitespace().collect();
                if cols.len() < 10 { continue; }
                if cols[3] != "01" { continue; } // ESTABLISHED
                if !tor_inodes.contains(cols[9]) { continue; }
                if let Some(v4) = hex_be_to_ipv4(cols[2]) {
                    if !v4.is_loopback() && !v4.is_private() && !v4.is_link_local() {
                        let cidr = format!("{v4}/32");
                        if !peers.contains(&cidr) { peers.push(cidr); }
                    }
                }
            }
        }
        if let Ok(tcp6) = std::fs::read_to_string("/proc/net/tcp6") {
            for line in tcp6.lines().skip(1) {
                let cols: Vec<&str> = line.split_whitespace().collect();
                if cols.len() < 10 { continue; }
                if cols[3] != "01" { continue; }
                if !tor_inodes.contains(cols[9]) { continue; }
                if let Some(v6) = hex_be_to_ipv6(cols[2]) {
                    if !v6.is_loopback() && !v6.is_unspecified() {
                        let cidr = format!("{v6}/128");
                        if !peers.contains(&cidr) { peers.push(cidr); }
                    }
                }
            }
        }
        if !peers.is_empty() {
            tunnel_log(&format!("tor bypass: /proc found {} guards via {} inodes", peers.len(), tor_inodes.len()));
            return peers;
        }
        tunnel_log(&format!("tor bypass: /proc found 0 guards via {} inodes — falling back to ss -tnp", tor_inodes.len()));
    }

    // Fallback: ss -tnp with pid filter (needs root, but helper is root).
    let ss_output = crate::common::silent_output("ss", &["-tnp", "state", "established"])
        .or_else(|| crate::common::silent_output("ss", &["-tn", "state", "established"]));
    if let Some(output) = ss_output {
        let uses_pids = output.contains("pid=");
        let mut peers: Vec<String> = Vec::new();
        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() || line.contains("State") || line.contains("Recv-Q") || line.starts_with("Netid") { continue; }
            if uses_pids {
                let has_tor_pid = pids.iter().any(|pid| {
                    line.contains(&format!("pid={pid},")) || line.contains(&format!("pid={pid})"))
                        || line.contains(&format!("pid={pid} ")) || line.ends_with(&format!("pid={pid}"))
                });
                if !has_tor_pid { continue; }
            }
            // Extract peer IP: second IP:port on line
            let mut ip_ports: Vec<&str> = Vec::new();
            for tok in line.split_whitespace() {
                if tok.contains(':') {
                    let clean = tok.trim_matches(|c: char| c == ',' || c == ')' || c == '(');
                    if clean.matches('.').count() >= 2 || clean.contains('[') { ip_ports.push(clean); }
                }
            }
            let peer_token = if ip_ports.len() >= 2 { ip_ports[1] } else { ip_ports.first().copied().unwrap_or("") };
            if peer_token.is_empty() { continue; }
            let ip_str = peer_token.rsplit_once(':').map(|(ip, _)| ip).unwrap_or(peer_token).trim_matches(|c| c == '[' || c == ']' || c == ',' || c == ')');
            if let Ok(ip) = ip_str.parse::<IpAddr>() {
                match ip {
                    IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_private() && !v4.is_link_local() => {
                        let cidr = format!("{v4}/32");
                        if !peers.contains(&cidr) { peers.push(cidr); }
                    }
                    IpAddr::V6(v6) if !v6.is_loopback() && !v6.is_unspecified() => {
                        let cidr = format!("{v6}/128");
                        if !peers.contains(&cidr) { peers.push(cidr); }
                    }
                    _ => {}
                }
            }
        }
        if !peers.is_empty() {
            // Without pid column peers is over-bypass but safe vs empty.
            return peers;
        }
    }
    Vec::new()
}

#[cfg(target_os = "linux")]
fn tor_socket_inodes(pids: &[u32]) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    for pid in pids {
        let fd_dir = format!("/proc/{pid}/fd");
        let Ok(entries) = std::fs::read_dir(&fd_dir) else { continue; };
        for entry in entries.flatten() {
            if let Ok(link) = std::fs::read_link(entry.path()) {
                let s = link.to_string_lossy();
                // socket:[12345]
                if s.starts_with("socket:[") && s.ends_with(']') {
                    let inode = s[8..s.len()-1].to_string();
                    set.insert(inode);
                }
            }
        }
    }
    set
}

#[cfg(target_os = "linux")]
fn filter_peers_by_inode(peers: &[String], _inodes: &std::collections::HashSet<String>) -> Vec<String> {
    // ss -tn path already extracted peers from all ESTAB lines; without per-
    // line inode we can't refine per-peer, so return empty to force /proc
    // fallback. Kept for API symmetry and future ss -e inode parsing.
    Vec::new()
}

#[cfg(target_os = "linux")]
fn hex_be_to_ipv4(hex: &str) -> Option<std::net::Ipv4Addr> {
    // /proc/net/tcp stores rem_address as little-endian hex: 0100007F = 127.0.0.1
    // Actually it's little-endian per 32-bit word: hex 0100007F -> 127.0.0.1
    if hex.len() < 8 { return None; }
    let addr_hex = hex.split_once(':').map(|(ip, _)| ip).unwrap_or(hex);
    if addr_hex.len() != 8 { return None; }
    let raw = u32::from_str_radix(addr_hex, 16).ok()?;
    // byte swap because /proc is little-endian
    let b0 = (raw & 0xFF) as u8;
    let b1 = ((raw >> 8) & 0xFF) as u8;
    let b2 = ((raw >> 16) & 0xFF) as u8;
    let b3 = ((raw >> 24) & 0xFF) as u8;
    Some(std::net::Ipv4Addr::new(b0, b1, b2, b3))
}

#[cfg(target_os = "linux")]
fn hex_be_to_ipv6(hex: &str) -> Option<std::net::Ipv6Addr> {
    // /proc/net/tcp6 stores rem_address as 32 hex chars, little-endian per 32-bit word.
    // e.g. 2402a00:… is stored as four LE u32 words.
    let addr_hex = hex.split_once(':').map(|(ip, _)| ip).unwrap_or(hex);
    if addr_hex.len() != 32 { return None; }
    let mut bytes = [0u8; 16];
    for word in 0..4 {
        let chunk = &addr_hex[word * 8..(word + 1) * 8];
        let raw = u32::from_str_radix(chunk, 16).ok()?;
        // LE per word: bytes reversed within the word
        bytes[word * 4] = (raw & 0xFF) as u8;
        bytes[word * 4 + 1] = ((raw >> 8) & 0xFF) as u8;
        bytes[word * 4 + 2] = ((raw >> 16) & 0xFF) as u8;
        bytes[word * 4 + 3] = ((raw >> 24) & 0xFF) as u8;
    }
    Some(std::net::Ipv6Addr::from(bytes))
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
                let timeout = std::time::Duration::from_millis(5000);
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
            // Belt-and-braces: the tun2proxy worker removes routes/nft on drop,
            // but if it was detached we ask tproxy-config to clean up anyway.
            #[cfg(target_os = "linux")]
            {
                let _ = std::thread::Builder::new()
                    .name("zn-tproxy-cleanup".into())
                    .spawn(|| {
                        let rt = tokio::runtime::Builder::new_multi_thread()
                            .worker_threads(1)
                            .enable_all()
                            .build();
                        if let Ok(rt) = rt {
                            let _ = rt.block_on(async { tproxy_config::tproxy_remove(None).await });
                        }
                    })
                    .map(|h| h.join());
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
