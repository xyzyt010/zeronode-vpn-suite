//! Route Android VpnService TUN traffic through a local SOCKS5 proxy via tun2proxy.
//!
//! Android already owns routes (Builder.addRoute); we pass `setup = false` and
//! only run the userspace netstack on the TUN file descriptor.

use anyhow::{bail, Context, Result};
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use tun2proxy::{
    general_run_async, ArgDns, ArgProxy, Args, CancellationToken, ProxyType, DEFAULT_MTU,
};

use crate::progress::set_progress;

static SOCKS_TUN: OnceLock<Mutex<Option<SocksTunHandle>>> = OnceLock::new();

struct SocksTunHandle {
    cancel: CancellationToken,
    thread: Option<JoinHandle<()>>,
}

fn slot() -> &'static Mutex<Option<SocksTunHandle>> {
    SOCKS_TUN.get_or_init(|| Mutex::new(None))
}

pub fn is_socks_tunnel_running() -> bool {
    slot()
        .lock()
        .map(|g| g.is_some())
        .unwrap_or(false)
}

/// Start tun2proxy on an Android VpnService TUN fd → socks5://127.0.0.1:`socks_port`.
///
/// `bypass_cidrs` are remote server IPs that must not loop through the tunnel
/// (e.g. Outline SS host).
pub fn start_socks_system_tunnel(
    tun_fd: i32,
    socks_port: u16,
    bypass_cidrs: &[String],
) -> Result<()> {
    stop_socks_system_tunnel()?;
    if tun_fd < 0 {
        bail!("invalid TUN file descriptor");
    }
    if socks_port == 0 {
        bail!("invalid SOCKS port");
    }

    set_progress(
        "socks_tun",
        0.55,
        format!("starting tun2proxy → :{socks_port}"),
    );

    let proxy = ArgProxy {
        proxy_type: ProxyType::Socks5,
        addr: SocketAddr::from(([127, 0, 0, 1], socks_port)),
        credentials: None,
    };

    let mut args = Args::default();
    args.proxy = proxy;
    args.tun_fd = Some(tun_fd);
    args.close_fd_on_drop = Some(false);
    args.setup = false;
    args.dns = ArgDns::OverTcp;
    args.verbosity = tun2proxy::ArgVerbosity::Info;
    args.ipv6_enabled = false;

    // Always bypass loopback so local SOCKS/Tor stay reachable.
    let mut bypass = vec!["127.0.0.0/8"
        .parse()
        .context("failed to parse localhost bypass CIDR")?];
    for cidr in bypass_cidrs {
        let trimmed = cidr.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed.parse() {
            Ok(parsed) => bypass.push(parsed),
            Err(e) => eprintln!("socks_tun: skip bypass {trimmed}: {e}"),
        }
    }
    args.bypass = bypass;

    let cancel = CancellationToken::new();
    let cancel_worker = cancel.clone();

    let thread = thread::Builder::new()
        .name(String::from("zeronode-android-socks-tun"))
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("socks-tun-rt")
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("socks_tun: runtime failed: {e}");
                    return;
                }
            };
            let result = rt.block_on(async move {
                general_run_async(args, DEFAULT_MTU, false, cancel_worker).await
            });
            match result {
                Ok(sessions) => {
                    eprintln!("socks_tun: exited normally (sessions={sessions})");
                }
                Err(e) => {
                    eprintln!("socks_tun: exited with error: {e}");
                }
            }
        })
        .context("spawn socks_tun thread")?;

    *slot()
        .lock()
        .map_err(|_| anyhow::anyhow!("socks_tun lock poisoned"))? =
        Some(SocksTunHandle {
            cancel,
            thread: Some(thread),
        });

    set_progress("socks_tun", 0.85, "tun2proxy active");
    Ok(())
}

pub fn stop_socks_system_tunnel() -> Result<()> {
    let mut guard = slot()
        .lock()
        .map_err(|_| anyhow::anyhow!("socks_tun lock poisoned"))?;
    if let Some(mut handle) = guard.take() {
        handle.cancel.cancel();
        if let Some(thread) = handle.thread.take() {
            // Non-blocking join so VpnService disconnect never freezes the UI/notification path.
            std::thread::spawn(move || {
                let _ = thread.join();
            });
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    }
    Ok(())
}
