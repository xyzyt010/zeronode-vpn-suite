//! Tor on Android via the official expert bundle (`libTor.so` + geoip + PTs).
//!
//! Bundle layout expected under the app files / native lib dir:
//! - `{native_lib_dir}/libTor.so`          — PIE tor binary packaged as .so
//! - `{tor_home}/data/geoip`, `geoip6`
//! - `{tor_home}/pluggable_transports/lyrebird`
//! - generated `{tor_home}/torrc` + `{tor_home}/data/` DataDirectory

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::progress::set_progress;
use crate::socks_tun::{start_socks_system_tunnel, stop_socks_system_tunnel};

static TOR: OnceLock<Mutex<Option<TorHandle>>> = OnceLock::new();

struct TorHandle {
    child: Option<Child>,
    socks_port: u16,
    home: PathBuf,
}

fn slot() -> &'static Mutex<Option<TorHandle>> {
    TOR.get_or_init(|| Mutex::new(None))
}

pub fn is_tor_running() -> bool {
    slot().lock().map(|g| g.is_some()).unwrap_or(false)
}

pub fn tor_socks_port() -> Option<u16> {
    slot()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|h| h.socks_port))
}

pub fn tor_bootstrap_hint() -> String {
    // Lightweight: if SOCKS accepts, report ready; else bootstrapping.
    match tor_socks_port() {
        Some(port) if port_open(port) => format!("OK socks=127.0.0.1:{port}"),
        Some(port) => format!("BOOTSTRAPPING socks_port={port}"),
        None => String::from("STOPPED"),
    }
}

fn port_open(port: u16) -> bool {
    TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).is_ok()
}

fn pick_free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if port_open(port) {
            return true;
        }
        thread::sleep(Duration::from_millis(120));
    }
    false
}

fn log_line(home: &Path, msg: &str) {
    let path = home.join("tor-tunnel.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(
            f,
            "[{}] {}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            msg
        );
    }
    eprintln!("tor: {msg}");
}

/// Prepare Tor home from Java-extracted assets.
///
/// `tor_home` should already contain:
/// - data/geoip, data/geoip6 (and optionally torrc-defaults)
/// - pluggable_transports/lyrebird (chmod +x done on Java side)
///
/// `native_lib_dir` is `ApplicationInfo.nativeLibraryDir` (contains libTor.so).
pub fn prepare_tor_home(tor_home: &Path, native_lib_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(tor_home.join("data")).context("create tor data dir")?;
    let lib_tor = native_lib_dir.join("libTor.so");
    if !lib_tor.exists() {
        bail!(
            "libTor.so not found in {}. Install the arm64-v8a Tor expert bundle.",
            native_lib_dir.display()
        );
    }
    // Mark executable best-effort (may already be).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&lib_tor) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&lib_tor, perms);
        }
        let lyrebird = tor_home.join("pluggable_transports").join("lyrebird");
        if lyrebird.exists() {
            if let Ok(meta) = fs::metadata(&lyrebird) {
                let mut perms = meta.permissions();
                perms.set_mode(0o755);
                let _ = fs::set_permissions(&lyrebird, perms);
            }
        }
    }
    Ok(lib_tor)
}

fn write_torrc(home: &Path, socks_port: u16) -> Result<PathBuf> {
    let data_dir = home.join("data");
    fs::create_dir_all(&data_dir)?;
    let geoip = data_dir.join("geoip");
    let geoip6 = data_dir.join("geoip6");
    let lyrebird = home.join("pluggable_transports").join("lyrebird");

    let mut body = String::new();
    body.push_str(&format!("DataDirectory {}\n", data_dir.display()));
    body.push_str(&format!("SocksPort 127.0.0.1:{socks_port}\n"));
    body.push_str("SocksPolicy accept 127.0.0.1\n");
    body.push_str("SocksPolicy reject *\n");
    body.push_str("AvoidDiskWrites 1\n");
    body.push_str("CookieAuthentication 0\n");
    body.push_str("ControlPort 0\n");
    body.push_str("Log notice stdout\n");
    if geoip.exists() {
        body.push_str(&format!("GeoIPFile {}\n", geoip.display()));
    }
    if geoip6.exists() {
        body.push_str(&format!("GeoIPv6File {}\n", geoip6.display()));
    }
    if lyrebird.exists() {
        body.push_str(&format!(
            "ClientTransportPlugin meek_lite,obfs2,obfs3,obfs4,scramblesuit,webtunnel exec {}\n",
            lyrebird.display()
        ));
        body.push_str(&format!(
            "ClientTransportPlugin snowflake exec {}\n",
            lyrebird.display()
        ));
    }
    let extra = home.join("user-bridges.conf");
    if extra.exists() {
        if let Ok(s) = fs::read_to_string(&extra) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                body.push('\n');
                body.push_str(trimmed);
                body.push('\n');
            }
        }
    }

    let torrc = home.join("torrc");
    fs::write(&torrc, body).context("write torrc")?;
    Ok(torrc)
}

/// Start Tor SOCKS only (no system tunnel yet).
pub fn start_tor_socks(tor_home: &Path, native_lib_dir: &Path) -> Result<u16> {
    stop_tor()?;
    set_progress("tor", 0.1, "preparing expert bundle");

    let lib_tor = prepare_tor_home(tor_home, native_lib_dir)?;
    let socks_port = pick_free_port()?;
    let torrc = write_torrc(tor_home, socks_port)?;
    log_line(tor_home, &format!("starting {} -f {}", lib_tor.display(), torrc.display()));

    set_progress("tor", 0.25, format!("launching libTor.so socks={socks_port}"));

    let log_path = tor_home.join("tor-stdout.log");
    let stdout_file = fs::File::create(&log_path).ok();
    let stderr_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(tor_home.join("tor-stderr.log"))
        .ok();

    let mut cmd = Command::new(&lib_tor);
    cmd.arg("-f")
        .arg(&torrc)
        .current_dir(tor_home)
        .stdin(Stdio::null());
    if let Some(f) = stdout_file {
        cmd.stdout(Stdio::from(f));
    } else {
        cmd.stdout(Stdio::null());
    }
    if let Some(f) = stderr_file {
        cmd.stderr(Stdio::from(f));
    } else {
        cmd.stderr(Stdio::null());
    }

    let child = cmd.spawn().with_context(|| {
        format!(
            "failed to exec {}. On non-arm64 devices Tor is unavailable with this expert bundle.",
            lib_tor.display()
        )
    })?;

    set_progress("tor", 0.45, "bootstrapping (waiting for SOCKS)");
    if !wait_for_port(socks_port, Duration::from_secs(90)) {
        // Kill failed child
        let mut handle = TorHandle {
            child: Some(child),
            socks_port,
            home: tor_home.to_path_buf(),
        };
        if let Some(mut c) = handle.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        bail!(
            "Tor SOCKS did not become ready on 127.0.0.1:{socks_port} within 90s. See {}",
            log_path.display()
        );
    }

    log_line(tor_home, &format!("SOCKS ready on {socks_port}"));
    *slot()
        .lock()
        .map_err(|_| anyhow::anyhow!("tor lock poisoned"))? = Some(TorHandle {
        child: Some(child),
        socks_port,
        home: tor_home.to_path_buf(),
    });

    // 0.70 = SOCKS listening; full device tunnel still required for 1.0.
    // UI continues progress while circuits build, then attaches VpnService.
    set_progress(
        "tor",
        0.70,
        format!("SOCKS ready :{socks_port} — attaching system tunnel next"),
    );
    Ok(socks_port)
}

/// Attach system-wide routing: tun2proxy on VpnService TUN → Tor SOCKS.
pub fn start_tor_system_tunnel(tun_fd: i32) -> Result<()> {
    let port = tor_socks_port().ok_or_else(|| anyhow::anyhow!("Tor SOCKS is not running"))?;
    set_progress("tor", 0.82, format!("starting system tunnel via tun2proxy → :{port}"));
    // Tor OR connections must not loop — Java VpnService.protect / package
    // exclusion is primary; we do not add broad bypass here (guards change).
    start_socks_system_tunnel(tun_fd, port, &[])?;
    set_progress("tor", 1.0, "system tunnel active — device traffic via Tor");
    Ok(())
}

pub fn stop_tor() -> Result<()> {
    let _ = stop_socks_system_tunnel();
    let mut guard = slot()
        .lock()
        .map_err(|_| anyhow::anyhow!("tor lock poisoned"))?;
    if let Some(mut handle) = guard.take() {
        if let Some(mut child) = handle.child.take() {
            let _ = child.kill();
            // Short wait
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
        log_line(&handle.home, "tor stopped");
    }
    Ok(())
}
