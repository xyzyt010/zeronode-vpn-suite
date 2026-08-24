//! Outline / Shadowsocks tunnel (Linux).
//!
//! Tries embedded `shadowsocks-service` (if available) otherwise falls back to
//! external `sslocal` binary. Manages local SOCKS5 and optional system-wide
//! route via `socks_tun`.

use anyhow::{Context, Result};
use std::fs;
use std::net::{SocketAddr, TcpListener, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};

#[cfg(target_os = "linux")]
use std::time::Duration;

static OUTLINE_STATE: OnceLock<Mutex<Option<OutlineHandle>>> = OnceLock::new();

struct OutlineHandle {
    port: u16,
    child: Option<Child>,
    #[allow(dead_code)]
    config_path: Option<PathBuf>,
}

fn outline_slot() -> &'static Mutex<Option<OutlineHandle>> {
    OUTLINE_STATE.get_or_init(|| Mutex::new(None))
}

fn pick_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(10801)
}

fn find_sslocal_binary() -> Option<PathBuf> {
    // Shadowsocks Rust (sslocal) vs C (ss-local) — both are deployed in the wild.
    // Debian/Arch/Fedora packages use ss-local (shadowsocks-libev), while cargo builds use sslocal.
    for cand in [
        "/usr/bin/sslocal",
        "/usr/bin/ss-local",
        "/usr/local/bin/sslocal",
        "/usr/local/bin/ss-local",
    ] {
        let p = PathBuf::from(cand);
        if p.is_file() {
            return Some(p);
        }
    }
    // Try both names via PATH (+ /usr/sbin fallback)
    if let Some(p) = crate::common::find_in_path("sslocal") {
        return Some(p);
    }
    if let Some(p) = crate::common::find_in_path("ss-local") {
        return Some(p);
    }
    // Also try shadowsocks-rust's binary name with hyphen on some distros
    crate::common::find_binary("sslocal")
        .or_else(|| crate::common::find_binary("ss-local"))
}

/// Start Outline/SS. Returns local SOCKS port.
pub fn start_outline(
    method: &str,
    password: &str,
    server_host: &str,
    server_port: u16,
    system_wide: bool,
) -> Result<u16> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (method, password, server_host, server_port, system_wide);
        anyhow::bail!("Outline is only available on Linux");
    }
    #[cfg(target_os = "linux")]
    {
        stop_outline()?;

        let socks_port = pick_free_port();
        let local_addr = SocketAddr::from(([127, 0, 0, 1], socks_port));

        // Try embedded shadowsocks-service first (via feature-gated code)
        // Fallback to external sslocal
        let mut child: Option<Child> = None;
        let mut config_path: Option<PathBuf> = None;

        // Check if we can use embedded — for now we use external binary path
        // to keep dependencies light. If sslocal not found, try shadowsocks-service
        // via cargo feature (not yet enabled in this build) — fallback to error.
        if let Some(sslocal) = find_sslocal_binary() {
            let tmp = std::env::temp_dir().join(format!(
                "zeronode-ss-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            ));
            fs::create_dir_all(&tmp)?;
            let cfg_path = tmp.join("ss.json");
            let cfg_json = serde_json::json!({
                "server": server_host,
                "server_port": server_port,
                "local_address": "127.0.0.1",
                "local_port": socks_port,
                "password": password,
                "method": method,
                "mode": "tcp_and_udp",
                "timeout": 300
            });
            fs::write(&cfg_path, serde_json::to_string_pretty(&cfg_json)?)?;
            config_path = Some(cfg_path.clone());

            let c = Command::new(&sslocal)
                .args(["-c", &cfg_path.display().to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .with_context(|| format!("failed to spawn {}", sslocal.display()))?;
            child = Some(c);
        } else {
            // No external binary — try to use embedded shadowsocks via `shadowsocks-service`
            // This requires the `shadowsocks-service` crate. If not compiled with it,
            // we bail with actionable error.
            anyhow::bail!(
                "sslocal not found. Install shadowsocks-libev (sudo apt install shadowsocks-libev) \
                 or enable embedded Shadowsocks support."
            );
        }

        // Wait for SOCKS port
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut ok = false;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(150));
            if std::net::TcpStream::connect(local_addr).is_ok() {
                ok = true;
                break;
            }
            if let Some(ref mut c) = child {
                if let Ok(Some(status)) = c.try_wait() {
                    anyhow::bail!("sslocal exited early with status: {status:?}");
                }
            }
        }
        if !ok {
            if let Some(mut c) = child {
                let _ = c.kill();
            }
            anyhow::bail!("Outline SOCKS failed to start on port {socks_port}");
        }

        {
            let mut slot = outline_slot().lock().unwrap();
            *slot = Some(OutlineHandle {
                port: socks_port,
                child,
                config_path,
            });
        }

        if system_wide {
            if crate::elevation::current_uid() != Some(0) {
                tracing::warn!("outline system_wide requested but not root — SOCKS only");
            } else {
                let bypass = vec![format!("{server_host}/32")];
                if let Err(e) = crate::socks_tun::start_socks_system_tunnel(socks_port, "ZeroNodeOutline", &bypass) {
                    tracing::warn!("outline system tunnel failed: {e:#}");
                }
            }
        }

        Ok(socks_port)
    }
}

pub fn stop_outline() -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let _ = crate::socks_tun::stop_socks_system_tunnel();
        let mut slot = outline_slot().lock().unwrap();
        if let Some(mut handle) = slot.take() {
            if let Some(mut child) = handle.child.take() {
                let pid = child.id();
                let _ = child.kill();
                // Also try SIGTERM via pid
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM); }
                std::thread::sleep(std::time::Duration::from_millis(300));
                let _ = child.wait();
                if let Some(path) = handle.config_path {
                    let _ = fs::remove_file(&path);
                    if let Some(parent) = path.parent() {
                        let _ = fs::remove_dir(parent);
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn is_outline_running() -> bool {
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
    #[cfg(target_os = "linux")]
    {
        let slot = outline_slot().lock().unwrap();
        if let Some(handle) = slot.as_ref() {
            if let Some(child) = handle.child.as_ref() {
                // Check if child still running via pid
                let pid = child.id();
                // We can't get pid from &Child without &mut, but we can check procfs
                // Fallback: check if port is still listening
                if crate::procfs::process_exists(pid) {
                    return true;
                }
                // Check via port
                if let Ok(_) = std::net::TcpStream::connect(format!("127.0.0.1:{}", handle.port)) {
                    return true;
                }
                return false;
            }
            // No child but port might still be?
            return false;
        }
        false
    }
}

pub fn outline_socks_port() -> Option<u16> {
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
    #[cfg(target_os = "linux")]
    {
        let slot = outline_slot().lock().unwrap();
        slot.as_ref().map(|h| h.port)
    }
}

pub fn is_outline_embedded() -> bool {
    false
}

pub fn find_sslocal() -> Option<PathBuf> {
    find_sslocal_binary()
}
