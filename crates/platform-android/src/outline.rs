//! Outline (Shadowsocks) on Android — embedded sslocal + tun2proxy on VpnService TUN.

use anyhow::{bail, Context, Result};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tokio::sync::oneshot;

use crate::progress::set_progress;
use crate::socks_tun::{start_socks_system_tunnel, stop_socks_system_tunnel};

static OUTLINE: OnceLock<Mutex<Option<OutlineHandle>>> = OnceLock::new();

struct OutlineHandle {
    stop_tx: Option<oneshot::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
    socks_port: u16,
}

fn slot() -> &'static Mutex<Option<OutlineHandle>> {
    OUTLINE.get_or_init(|| Mutex::new(None))
}

pub fn is_outline_running() -> bool {
    slot().lock().map(|g| g.is_some()).unwrap_or(false)
}

pub fn outline_socks_port() -> Option<u16> {
    slot()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|h| h.socks_port))
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
    if let Ok(addrs) = (host, 0u16).to_socket_addrs() {
        for addr in addrs {
            push_ip(&mut out, addr.ip());
        }
    }
    out
}

fn build_ss_config(
    method: &str,
    password: &str,
    host: &str,
    port: u16,
    local_port: u16,
) -> Result<shadowsocks_service::config::Config> {
    use shadowsocks_service::config::{Config, ConfigType};
    let json = serde_json::json!({
        "server": host,
        "server_port": port,
        "local_address": "127.0.0.1",
        "local_port": local_port,
        "password": password,
        "method": method,
        "mode": "tcp_and_udp",
        "timeout": 300
    });
    Config::load_from_str(&json.to_string(), ConfigType::Local)
        .map_err(|e| anyhow::anyhow!("invalid Outline/Shadowsocks config: {e}"))
}

/// Start embedded SS local SOCKS and (if `tun_fd >= 0`) system tun2proxy tunnel.
pub fn start_outline(
    method: &str,
    password: &str,
    host: &str,
    port: u16,
    tun_fd: i32,
) -> Result<u16> {
    stop_outline()?;
    set_progress("outline", 0.1, "starting shadowsocks");

    let local_port = pick_free_port()?;
    let config = build_ss_config(method, password, host, port, local_port)?;
    config
        .check_integrity()
        .map_err(|e| anyhow::anyhow!("Outline config integrity: {e}"))?;
    let (stop_tx, stop_rx) = oneshot::channel::<()>();

    let worker = thread::Builder::new()
        .name(String::from("zeronode-outline-ss"))
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
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
        .context("spawn outline ss")?;

    if !wait_for_port(local_port, Duration::from_secs(8)) {
        let _ = stop_tx.send(());
        let _ = worker.join();
        bail!("Outline local SOCKS did not become ready on 127.0.0.1:{local_port}");
    }

    set_progress("outline", 0.45, format!("SOCKS ready :{local_port}"));

    *slot()
        .lock()
        .map_err(|_| anyhow::anyhow!("outline lock poisoned"))? = Some(OutlineHandle {
        stop_tx: Some(stop_tx),
        worker: Some(worker),
        socks_port: local_port,
    });

    if tun_fd >= 0 {
        let bypass = resolve_server_bypass_cidrs(host);
        set_progress(
            "outline",
            0.6,
            format!("system tunnel bypass={}", bypass.join(",")),
        );
        start_socks_system_tunnel(tun_fd, local_port, &bypass)
            .context("outline system tunnel")?;
    }

    set_progress("outline", 1.0, "active");
    Ok(local_port)
}

pub fn stop_outline() -> Result<()> {
    let _ = stop_socks_system_tunnel();
    let mut guard = slot()
        .lock()
        .map_err(|_| anyhow::anyhow!("outline lock poisoned"))?;
    if let Some(mut handle) = guard.take() {
        if let Some(tx) = handle.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(worker) = handle.worker.take() {
            // Never block the service thread forever — join with a short wait
            // in a helper thread so Disconnect always returns.
            thread::spawn(move || {
                let _ = worker.join();
            });
            thread::sleep(Duration::from_millis(120));
        }
    }
    Ok(())
}
