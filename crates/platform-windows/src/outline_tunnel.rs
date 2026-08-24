//! Outline (Shadowsocks) system tunnel on Windows — fully embedded.
//!
//! Local SOCKS5 is provided by the `shadowsocks-service` crate (same stack as
//! sslocal), running **in-process** on a background Tokio runtime. No external
//! `sslocal.exe` is required.
//!
//! System-wide routing reuses the Tor path: Wintun + tun2proxy → local SOCKS5.
//! The remote SS/Outline server **must** be bypassed via its resolved IPv4
//! address(es); a hostname `/32` is not a valid CIDR and causes a routing loop
//! (traffic never leaves the tunnel → public IP never changes).

use anyhow::{bail, Context, Result};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tokio::sync::oneshot;

use crate::tor_tunnel::{start_socks_system_tunnel, stop_socks_system_tunnel};

static OUTLINE: OnceLock<Mutex<Option<OutlineHandle>>> = OnceLock::new();

struct OutlineHandle {
    stop_tx: Option<oneshot::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
    socks_port: u16,
    system_route: bool,
}

fn slot() -> &'static Mutex<Option<OutlineHandle>> {
    OUTLINE.get_or_init(|| Mutex::new(None))
}

/// Compatibility shim — Outline is always available (compiled in).
pub fn find_sslocal() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from("embedded://shadowsocks"))
}

pub fn is_outline_embedded() -> bool {
    true
}

fn pick_free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(40));
    }
    false
}

/// Resolve Outline/SS server host to `/32` (and `/128`) bypass CIDRs.
/// Hostnames like `pl140.vpnbook.com` cannot be passed as `host/32` — tun2proxy
/// only accepts IP CIDRs. Without a real bypass the SS path loops through the
/// TUN and the system tunnel carries zero sessions.
pub fn resolve_server_bypass_cidrs(host: &str) -> Vec<String> {
    let host = host.trim().trim_matches(|c| c == '[' || c == ']');
    if host.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let push_ip = |out: &mut Vec<String>, ip: IpAddr| {
        let cidr = match ip {
            IpAddr::V4(v4) => format!("{v4}/32"),
            IpAddr::V6(v6) => format!("{v6}/128"),
        };
        if !out.contains(&cidr) {
            out.push(cidr);
        }
    };

    if let Ok(ip) = host.parse::<IpAddr>() {
        push_ip(&mut out, ip);
        return out;
    }

    // Prefer A records; try with common SS ports and :0.
    for port in [443u16, 8388, 0] {
        let query = if host.contains(':') && !host.contains('.') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        if let Ok(iter) = query.to_socket_addrs() {
            for sa in iter {
                push_ip(&mut out, sa.ip());
            }
            if !out.is_empty() {
                break;
            }
        }
    }

    // Last resort: DNS via getaddrinfo with dummy port.
    if out.is_empty() {
        if let Ok(iter) = (host, 0u16).to_socket_addrs() {
            for sa in iter {
                push_ip(&mut out, sa.ip());
            }
        }
    }
    out
}

fn build_local_config(
    method: &str,
    password: &str,
    server_host: &str,
    server_port: u16,
    socks_port: u16,
) -> Result<shadowsocks_service::config::Config> {
    use shadowsocks_service::config::{Config, ConfigType};

    let json = serde_json::json!({
        "server": server_host,
        "server_port": server_port,
        "local_address": "127.0.0.1",
        "local_port": socks_port,
        "password": password,
        "method": method,
        "mode": "tcp_and_udp",
        "timeout": 300
    });
    let text = json.to_string();
    Config::load_from_str(&text, ConfigType::Local)
        .map_err(|e| anyhow::anyhow!("invalid Outline/Shadowsocks config: {e}"))
}

/// Start embedded Shadowsocks local SOCKS5 (+ optional system-wide TUN).
pub fn start_outline(
    method: &str,
    password: &str,
    server_host: &str,
    server_port: u16,
    system_wide: bool,
) -> Result<u16> {
    // Tear down any previous session quickly (non-blocking join).
    stop_outline()?;

    let socks_port = pick_free_port()?;
    let config = build_local_config(method, password, server_host, server_port, socks_port)?;
    config
        .check_integrity()
        .map_err(|e| anyhow::anyhow!("Outline config integrity: {e}"))?;

    let (stop_tx, stop_rx) = oneshot::channel::<()>();

    let worker = thread::Builder::new()
        .name("zeronode-outline-ss".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("outline-ss-worker")
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("outline: tokio runtime failed: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                tokio::select! {
                    res = shadowsocks_service::run_local(config) => {
                        if let Err(e) = res {
                            eprintln!("outline: local SS exited: {e}");
                        }
                    }
                    _ = stop_rx => {}
                }
            });
        })
        .context("failed to spawn embedded Outline worker")?;

    if !wait_for_port(socks_port, Duration::from_secs(8)) {
        let _ = stop_tx.send(());
        // Detach — don't block the UI for a stuck worker.
        std::mem::forget(worker);
        bail!(
            "embedded Shadowsocks did not open SOCKS on 127.0.0.1:{socks_port}. \
             Check method/password/server (AEAD methods recommended)."
        );
    }

    let mut system_route = false;
    if system_wide {
        // Critical: resolve hostname → IP CIDRs so tun2proxy can pin the SS
        // path on the physical gateway (avoids TUN feedback loop).
        let mut bypass = resolve_server_bypass_cidrs(server_host);
        if bypass.is_empty() {
            // Still try literal form for pure-IP keys.
            if server_host.parse::<IpAddr>().is_ok() {
                bypass.push(format!("{server_host}/32"));
            } else {
                let _ = stop_tx.send(());
                std::mem::forget(worker);
                bail!(
                    "could not resolve Outline server host `{server_host}` to an IP for route bypass. \
                     Check DNS / try a key that uses a numeric server address."
                );
            }
        }
        eprintln!(
            "outline: system TUN bypass for {server_host} → {}",
            bypass.join(", ")
        );
        if let Err(e) = start_socks_system_tunnel(socks_port, "ZeroNodeOutline", &bypass) {
            let _ = stop_tx.send(());
            std::mem::forget(worker);
            return Err(e).context("Outline system tunnel (Wintun)");
        }
        system_route = true;
    }

    {
        let mut guard = slot().lock().unwrap();
        *guard = Some(OutlineHandle {
            stop_tx: Some(stop_tx),
            worker: Some(worker),
            socks_port,
            system_route,
        });
    }
    Ok(socks_port)
}

pub fn stop_outline() -> Result<()> {
    let mut guard = slot().lock().unwrap();
    if let Some(mut h) = guard.take() {
        // Cancel TUN first so routes restore while SS is still answering briefly.
        if h.system_route {
            let _ = stop_socks_system_tunnel();
        }
        if let Some(tx) = h.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(worker) = h.worker.take() {
            // Fast path: wait up to ~800ms then detach (daemon-style).
            let timeout = Duration::from_millis(800);
            let start = std::time::Instant::now();
            loop {
                if worker.is_finished() {
                    let _ = worker.join();
                    break;
                }
                if start.elapsed() > timeout {
                    std::mem::forget(worker);
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    } else {
        let _ = stop_socks_system_tunnel();
    }
    Ok(())
}

pub fn is_outline_running() -> bool {
    slot()
        .lock()
        .map(|g| {
            g.as_ref()
                .map(|h| {
                    TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], h.socks_port))).is_ok()
                })
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

pub fn outline_socks_port() -> Option<u16> {
    slot()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|h| h.socks_port))
}
