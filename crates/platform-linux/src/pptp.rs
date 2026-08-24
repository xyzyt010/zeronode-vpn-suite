//! PPTP tunnel via pppd + pptp-linux (Linux).
//!
//! Direct `pppd` invocation, not `pon`, for lifecycle control.
//! Manages `/etc/ppp/chap-secrets` and `/etc/ppp/peers/zeronode-pptp`.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

const PEER_NAME: &str = "zeronode-pptp";
const PEER_FILE: &str = "/etc/ppp/peers/zeronode-pptp";
const CHAP_SECRETS: &str = "/etc/ppp/chap-secrets";
const CHAP_MARKER_BEGIN: &str = "# BEGIN ZeroNode PPTP";
const CHAP_MARKER_END: &str = "# END ZeroNode PPTP";

static PPTP_STATE: OnceLock<Mutex<Option<PptpHandle>>> = OnceLock::new();

struct PptpHandle {
    pid: Option<u32>,
    server: String,
}

fn pptp_slot() -> &'static Mutex<Option<PptpHandle>> {
    PPTP_STATE.get_or_init(|| Mutex::new(None))
}

fn pppd_exists() -> bool {
    crate::common::command_exists("pppd") || Path::new("/usr/sbin/pppd").exists()
}

fn pptp_exists() -> bool {
    crate::common::command_exists("pptp") || Path::new("/usr/sbin/pptp").exists()
}

pub fn is_pptp_running() -> bool {
    // Check pidfile first
    {
        let slot = pptp_slot().lock().unwrap();
        if let Some(handle) = slot.as_ref() {
            if let Some(pid) = handle.pid {
                if crate::procfs::process_exists(pid) {
                    return true;
                }
            }
        }
    }
    // Fallback: check for ppp0 interface or pppd process with our peer
    if Path::new("/sys/class/net/ppp0").exists() {
        return true;
    }
    // Check any pppd with our peer file
    !crate::procfs::find_pids_by_name("pppd").is_empty() && Path::new(PEER_FILE).exists()
}

pub fn start_pptp(server: &str, username: &str, password: &str) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (server, username, password);
        anyhow::bail!("PPTP is only available on Linux");
    }
    #[cfg(target_os = "linux")]
    {
        if crate::elevation::current_uid() != Some(0) {
            anyhow::bail!("PPTP requires root. Re-launch via pkexec.");
        }
        if !pppd_exists() {
            anyhow::bail!("pppd not found. Install with: sudo apt install ppp");
        }
        if !pptp_exists() {
            anyhow::bail!("pptp not found. Install with: sudo apt install pptp-linux");
        }

        stop_pptp()?;

        write_peer_file(server, username)?;
        write_chap_secrets(username, server, password)?;

        // Spawn pppd: pppd pty "pptp <server> --nolaunchpppd" call zeronode-pptp
        let pty = format!("pptp {server} --nolaunchpppd");
        let mut child = Command::new("pppd")
            .args(["pty", &pty, "call", PEER_NAME])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn pppd")?;

        let pid = child.id();

        // Detach: we don't wait, but keep pid for tracking
        std::mem::forget(child);

        {
            let mut slot = pptp_slot().lock().unwrap();
            *slot = Some(PptpHandle {
                pid: Some(pid),
                server: server.to_string(),
            });
        }

        // Poll for ppp0 up to 15s
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            if Path::new("/sys/class/net/ppp0").exists() {
                tracing::info!("PPTP connected: ppp0 up");
                return Ok(());
            }
            // Check if pppd died
            if let Some(pid) = pid.into() {
                if !crate::procfs::process_exists(pid) {
                    anyhow::bail!("pppd exited prematurely; check /var/log/syslog for details");
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }

        anyhow::bail!("PPTP connection timed out; ppp0 did not appear within 15s")
    }
}

pub fn stop_pptp() -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let pid = {
            let mut slot = pptp_slot().lock().unwrap();
            slot.take().and_then(|h| h.pid)
        };

        if let Some(pid) = pid {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while std::time::Instant::now() < deadline {
                if !crate::procfs::process_exists(pid) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if crate::procfs::process_exists(pid) {
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
            }
        }

        // Fallback: kill any pppd with our peer
        let pids = crate::procfs::find_pids_by_name("pppd");
        for pid in pids {
            // Check cmdline contains our peer
            if let Ok(cmdline) = fs::read_to_string(format!("/proc/{pid}/cmdline")) {
                if cmdline.contains(PEER_NAME) {
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGTERM);
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        // SIGKILL any remaining
        for pid in crate::procfs::find_pids_by_name("pppd") {
            if let Ok(cmdline) = fs::read_to_string(format!("/proc/{pid}/cmdline")) {
                if cmdline.contains(PEER_NAME) {
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGKILL);
                    }
                }
            }
        }

        // Remove peer file (keep chap entry for reuse)
        let _ = fs::remove_file(PEER_FILE);

        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn write_peer_file(server: &str, username: &str) -> Result<()> {
    let dir = Path::new("/etc/ppp/peers");
    fs::create_dir_all(dir).context("failed to create /etc/ppp/peers")?;

    let content = format!(
        r#"# ZeroNode PPTP peer — managed
pty "pptp {server} --nolaunchpppd"
name {username}
remotename {PEER_NAME}
require-mppe-128
file /etc/ppp/options.pptp
ipparam {PEER_NAME}
"#
    );
    fs::write(PEER_FILE, content).context("failed to write peer file")?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn write_chap_secrets(username: &str, server: &str, password: &str) -> Result<()> {
    let existing = fs::read_to_string(CHAP_SECRETS).unwrap_or_default();

    // Remove old managed block
    let mut new_lines = Vec::new();
    let mut in_block = false;
    for line in existing.lines() {
        if line.trim() == CHAP_MARKER_BEGIN {
            in_block = true;
            continue;
        }
        if line.trim() == CHAP_MARKER_END {
            in_block = false;
            continue;
        }
        if !in_block {
            new_lines.push(line.to_string());
        }
    }

    new_lines.push(CHAP_MARKER_BEGIN.to_string());
    // Format: client server secret IP addresses — use * for server and IPs
    new_lines.push(format!("{username} {PEER_NAME} \"{password}\" *"));
    new_lines.push(format!("{username} {server} \"{password}\" *"));
    new_lines.push(CHAP_MARKER_END.to_string());

    let new_content = new_lines.join("\n") + "\n";
    fs::write(CHAP_SECRETS, new_content).context("failed to write chap-secrets")?;

    // Secure permissions
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(CHAP_SECRETS)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(CHAP_SECRETS, perms)?;

    Ok(())
}
