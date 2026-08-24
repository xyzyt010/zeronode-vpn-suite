//! PPTP via Windows built-in RAS (`rasdial` + temporary VPN connection).
//!
//! PPTP is cryptographically weak; this exists only for legacy compatibility.
//! All child processes use CREATE_NO_WINDOW to avoid console flashes.

use anyhow::{bail, Context, Result};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use crate::silent_cmd::{silent_command, silent_output, CREATE_NO_WINDOW};

const ENTRY_NAME: &str = "ZeroNodePPTP";

static PPTP_ACTIVE: OnceLock<Mutex<Option<PptpSession>>> = OnceLock::new();

struct PptpSession {
    #[allow(dead_code)]
    entry: String,
}

fn slot() -> &'static Mutex<Option<PptpSession>> {
    PPTP_ACTIVE.get_or_init(|| Mutex::new(None))
}

/// Ensure a PPTP VPN connection entry exists (silent PowerShell).
fn ensure_connection_entry(server: &str) -> Result<()> {
    // Remove stale entry quietly, then recreate.
    let remove = format!(
        "Remove-VpnConnection -Name '{ENTRY_NAME}' -Force -ErrorAction SilentlyContinue"
    );
    let _ = silent_output("powershell.exe", &["-NoProfile", "-NonInteractive", "-Command", &remove]);

    let add = format!(
        "Add-VpnConnection -Name '{ENTRY_NAME}' -ServerAddress '{server}' -TunnelType Pptp \
         -EncryptionLevel Optional -AuthenticationMethod MsChapv2 -RememberCredential:$false \
         -SplitTunneling:$false -Force -ErrorAction Stop | Out-Null; 'OK'"
    );
    let out = silent_output(
        "powershell.exe",
        &["-NoProfile", "-NonInteractive", "-Command", &add],
    )
    .unwrap_or_default();
    if !out.contains("OK") {
        // Fallback: try Set-VpnConnection if entry already exists
        let set = format!(
            "if (Get-VpnConnection -Name '{ENTRY_NAME}' -ErrorAction SilentlyContinue) {{ \
               Set-VpnConnection -Name '{ENTRY_NAME}' -ServerAddress '{server}' -TunnelType Pptp \
               -EncryptionLevel Optional -AuthenticationMethod MsChapv2 -SplitTunneling:$false -Force; 'OK' \
             }} else {{ throw 'Add-VpnConnection failed: {out}' }}"
        );
        let out2 = silent_output(
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command", &set],
        )
        .unwrap_or_default();
        if !out2.contains("OK") {
            bail!("failed to create PPTP connection entry: {out} {out2}");
        }
    }
    Ok(())
}

/// Dial PPTP. Returns when the connection reaches Connected (or errors).
pub fn start_pptp(server: &str, username: &str, password: &str) -> Result<()> {
    stop_pptp()?;

    let server = server.trim();
    if server.is_empty() {
        bail!("PPTP server is empty");
    }
    if username.trim().is_empty() {
        bail!("PPTP username is empty");
    }

    ensure_connection_entry(server).context("create PPTP VPN entry")?;

    // rasdial "entry" user pass
    let mut cmd = silent_command("rasdial");
    cmd.arg(ENTRY_NAME).arg(username).arg(password);
    let output = cmd
        .output()
        .context("failed to run rasdial (is the Remote Access service available)?")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!(
            "rasdial failed ({}): {} {}",
            output.status,
            stdout.trim(),
            stderr.trim()
        );
    }

    // Poll until connected or timeout
    for _ in 0..30 {
        if is_pptp_connected() {
            let mut guard = slot().lock().unwrap();
            *guard = Some(PptpSession {
                entry: ENTRY_NAME.to_string(),
            });
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }

    // rasdial often returns success before fully up — check once more
    if is_pptp_connected() || stdout.to_ascii_lowercase().contains("successfully") {
        let mut guard = slot().lock().unwrap();
        *guard = Some(PptpSession {
            entry: ENTRY_NAME.to_string(),
        });
        return Ok(());
    }

    bail!("PPTP dial timed out: {} {}", stdout.trim(), stderr.trim());
}

pub fn stop_pptp() -> Result<()> {
    let mut guard = slot().lock().unwrap();
    let _ = guard.take();
    drop(guard);

    let mut cmd = silent_command("rasdial");
    cmd.arg(ENTRY_NAME).arg("/disconnect");
    let _ = cmd.output();

    // Also hang up any lingering connection with that name
    let _ = silent_output(
        "powershell.exe",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "rasdial '{ENTRY_NAME}' /disconnect 2>$null; \
                 Get-VpnConnection -Name '{ENTRY_NAME}' -ErrorAction SilentlyContinue | \
                 ForEach-Object {{ if ($_.ConnectionStatus -ne 'Disconnected') {{ rasdial $_.Name /disconnect }} }}"
            ),
        ],
    );
    Ok(())
}

pub fn is_pptp_running() -> bool {
    if slot().lock().map(|g| g.is_some()).unwrap_or(false) {
        return is_pptp_connected();
    }
    is_pptp_connected()
}

fn is_pptp_connected() -> bool {
    let script = format!(
        "$c = Get-VpnConnection -Name '{ENTRY_NAME}' -ErrorAction SilentlyContinue; \
         if ($c -and $c.ConnectionStatus -eq 'Connected') {{ 'CONNECTED' }} else {{ 'DOWN' }}"
    );
    silent_output(
        "powershell.exe",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    )
    .map(|s| s.contains("CONNECTED"))
    .unwrap_or(false)
}

/// Suppress unused warning for CREATE_NO_WINDOW re-export path.
#[allow(dead_code)]
fn _flags() -> u32 {
    CREATE_NO_WINDOW
}
