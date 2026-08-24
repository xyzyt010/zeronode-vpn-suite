//! OpenVPN client (Linux).
//!
//! Spawns the distro `openvpn` binary with a runtime profile derived from the
//! imported .ovpn. Mirrors the Windows log-state-machine approach.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use crate::common::{command_exists, run_command, CommandOutcome};

const OVPN_LOG_POLL_MS: u64 = 400;
const OVPN_UP_TIMEOUT_SECS: u64 = 60;

static OVPN_STATE: OnceLock<Mutex<Option<OpenVpnHandle>>> = OnceLock::new();

struct OpenVpnHandle {
    child: Child,
    profile: String,
    log_path: PathBuf,
    pid: u32,
    ipv6_guard: Option<crate::leak_protect::Guard>,
}

fn ovpn_slot() -> &'static Mutex<Option<OpenVpnHandle>> {
    OVPN_STATE.get_or_init(|| Mutex::new(None))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OvpnTunnelState {
    Connecting,
    Up,
    AuthFailed,
    Fatal,
    Exited,
}

fn classify_ovpn_line(line: &str) -> Option<OvpnTunnelState> {
    let lower = line.to_ascii_lowercase();
    if lower.contains("auth_failed") || lower.contains("auth failure") {
        return Some(OvpnTunnelState::AuthFailed);
    }
    if lower.contains("fatal") || lower.contains("exiting due to fatal") {
        return Some(OvpnTunnelState::Fatal);
    }
    if lower.contains("initialization sequence completed")
        || lower.contains("peer connection initiated")
        && lower.contains("completed")
    {
        return Some(OvpnTunnelState::Up);
    }
    if lower.contains("initialization sequence completed") {
        return Some(OvpnTunnelState::Up);
    }
    // TUN/TAP device opened indicates progress
    if lower.contains("tun/tap device") && lower.contains("opened") {
        return Some(OvpnTunnelState::Connecting);
    }
    None
}

pub fn find_openvpn_binary() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("ZERONODE_OPENVPN") {
        let p = PathBuf::from(custom);
        if p.is_file() {
            return Some(p);
        }
    }
    for candidate in ["/usr/sbin/openvpn", "/usr/bin/openvpn"] {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    crate::common::find_in_path("openvpn")
}

fn prepare_runtime_profile(source: &Path) -> Result<PathBuf> {
    let data = fs::read_to_string(source)
        .with_context(|| format!("failed to read ovpn profile: {}", source.display()))?;

    let tmp = std::env::temp_dir().join(format!(
        "zeronode-ovpn-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    fs::create_dir_all(&tmp)?;

    // Write auth file if profile contains auth-user-pass without inline file
    // We'll create a placeholder that openvpn will fail on if credentials needed — the app
    // should have written credentials via separate flow. For now, handle auth-user-pass line:
    // if it has no argument, we keep it; if it has a file, ensure file exists.

    let dest = tmp.join("runtime.ovpn");
    let mut out = String::new();
    for line in data.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("windows-driver") {
            continue;
        }
        // Force tun device
        if trimmed.starts_with("dev ") && !trimmed.contains("tun") {
            out.push_str("dev tun\n");
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    // Ensure tun
    if !out.contains("dev tun") {
        out.push_str("dev tun\n");
    }
    fs::write(&dest, out)?;
    Ok(dest)
}

fn wait_for_openvpn_up(log_path: &Path, timeout: Duration) -> Result<OvpnTunnelState> {
    let start = Instant::now();
    let mut last_size = 0u64;
    while start.elapsed() < timeout {
        std::thread::sleep(Duration::from_millis(OVPN_LOG_POLL_MS));
        let data = fs::read_to_string(log_path).unwrap_or_default();
        // Check from last read offset onward
        let new_data = if (data.len() as u64) > last_size {
            &data[last_size as usize..]
        } else {
            &data
        };
        last_size = data.len() as u64;
        for line in new_data.lines() {
            if let Some(state) = classify_ovpn_line(line) {
                if state == OvpnTunnelState::Up {
                    return Ok(state);
                }
                if matches!(state, OvpnTunnelState::AuthFailed | OvpnTunnelState::Fatal) {
                    return Ok(state);
                }
            }
        }
        // Also check if process died
        {
            let slot = ovpn_slot().lock().unwrap();
            if let Some(handle) = slot.as_ref() {
                // Try to see if child exited by checking pid
                if !crate::procfs::process_exists(handle.pid) {
                    return Ok(OvpnTunnelState::Exited);
                }
            }
        }
    }
    anyhow::bail!("OpenVPN did not reach 'Initialization Sequence Completed' within {timeout:?}")
}

pub fn start_openvpn(profile_path: &str, auth_file: Option<&Path>) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (profile_path, auth_file);
        anyhow::bail!("OpenVPN is only available on Linux");
    }
    #[cfg(target_os = "linux")]
    {
        if crate::elevation::current_uid() != Some(0) {
            anyhow::bail!("OpenVPN requires root for tun device. Re-launch via pkexec.");
        }

        let ovpn_bin = find_openvpn_binary()
            .ok_or_else(|| anyhow::anyhow!("openvpn binary not found. Install with: sudo apt install openvpn"))?;

        stop_openvpn()?;

        let source = Path::new(profile_path);
        let runtime = prepare_runtime_profile(source)?;
        // If the profile handles IPv6 itself (udp6/tcp6, route-ipv6,
        // ifconfig-ipv6), trust it; otherwise block v6 leaks while connected.
        let profile_text = fs::read_to_string(&runtime).unwrap_or_default();
        let profile_handles_v6 = ["proto udp6", "proto tcp6", "route-ipv6", "ifconfig-ipv6"]
            .iter()
            .any(|needle| profile_text.to_ascii_lowercase().contains(needle));

        let log_path = std::env::temp_dir().join(format!(
            "zeronode-ovpn-{}.log",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));

        let mut args: Vec<String> = vec![
            "--config".to_string(),
            runtime.display().to_string(),
            "--log".to_string(),
            log_path.display().to_string(),
            "--verb".to_string(),
            "3".to_string(),
        ];
        if let Some(auth) = auth_file {
            args.push("--auth-user-pass".to_string());
            args.push(auth.display().to_string());
        }

        let child = Command::new(&ovpn_bin)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn openvpn at {}", ovpn_bin.display()))?;

        let pid = child.id();

        {
            let ipv6_guard = if profile_handles_v6 {
                None
            } else {
                Some(crate::leak_protect::disable_all())
            };
            let mut slot = ovpn_slot().lock().unwrap();
            *slot = Some(OpenVpnHandle {
                child,
                profile: profile_path.to_string(),
                log_path: log_path.clone(),
                pid,
                ipv6_guard,
            });
        }

        match wait_for_openvpn_up(&log_path, Duration::from_secs(OVPN_UP_TIMEOUT_SECS)) {
            Ok(OvpnTunnelState::Up) => {
                tracing::info!("OpenVPN tunnel up");
                Ok(())
            }
            Ok(OvpnTunnelState::AuthFailed) => {
                let _ = stop_openvpn();
                anyhow::bail!("OpenVPN authentication failed — check username/password")
            }
            Ok(state) => {
                let _ = stop_openvpn();
                anyhow::bail!("OpenVPN failed with state: {state:?}")
            }
            Err(e) => {
                let _ = stop_openvpn();
                Err(e)
            }
        }
    }
}

pub fn stop_openvpn() -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let handle = {
            let mut slot = ovpn_slot().lock().unwrap();
            slot.take()
        };
        if let Some(mut handle) = handle {
            let guard = handle.ipv6_guard.take();
            // Try graceful TERM
            unsafe {
                libc::kill(handle.pid as libc::pid_t, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if !crate::procfs::process_exists(handle.pid) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if crate::procfs::process_exists(handle.pid) {
                unsafe {
                    libc::kill(handle.pid as libc::pid_t, libc::SIGKILL);
                }
            }
            let _ = handle.child.kill();
            let _ = handle.child.wait();
            let _ = fs::remove_file(&handle.log_path);
        }

        // Restore IPv6 after the tunnel is fully down.
        if let Some(guard) = guard {
            crate::leak_protect::restore(guard);
        }

        // Fallback: kill any openvpn with our profile
        for pid in crate::procfs::find_pids_by_name("openvpn") {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
        }
        std::thread::sleep(Duration::from_millis(300));
        for pid in crate::procfs::find_pids_by_name("openvpn") {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }

        Ok(())
    }
}

pub fn is_openvpn_running() -> bool {
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
    #[cfg(target_os = "linux")]
    {
        let slot = ovpn_slot().lock().unwrap();
        if let Some(handle) = slot.as_ref() {
            if crate::procfs::process_exists(handle.pid) {
                return true;
            }
        }
        // Fallback check
        !crate::procfs::find_pids_by_name("openvpn").is_empty()
    }
}

pub fn openvpn_status() -> String {
    if is_openvpn_running() {
        "OpenVPN tunnel is active".to_string()
    } else {
        "OpenVPN tunnel is not active".to_string()
    }
}
