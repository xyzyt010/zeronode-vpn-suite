//! Windows privilege helpers.
//!
//! `tun2proxy` (and the embedded WireGuard tunnel) both call `route add` and
//! `netsh` commands that need an Administrator token. The client manifest is
//! `asInvoker` so the user can launch the app from a normal Explorer shell
//! without seeing a UAC prompt at every startup. When the user actually
//! wants a system-wide tunnel (the Tor "Connect" button, or the WireGuard
//! "Apply Tunnel" button), we offer to re-launch ourselves elevated via
//! `ShellExecuteW(lpVerb = "runas")`. This keeps the no-VPN-UX admin-free
//! while still allowing full-system routing when needed.

use anyhow::{Context, Result};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Returns `true` when the current process is already running with an
/// elevated Administrator token (UAC elevation active).
pub fn is_elevated() -> bool {
    is_elevated_impl().unwrap_or(false)
}

#[inline]
fn is_elevated_impl() -> Result<bool> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: OpenProcessToken is called with the pseudo-handle returned by
    // GetCurrentProcess (which is always valid for the calling thread).
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if ok == 0 {
        return Ok(false);
    }

    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned = 0u32;
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    // SAFETY: HANDLEs returned by OpenProcessToken must be closed.
    unsafe {
        CloseHandle(token);
    }

    // GetTokenInformation returns a BOOL: non-zero = success.
    // Do NOT compare against ERROR_SUCCESS (0) — that inverts the result and
    // made is_elevated() always return false even when running as admin.
    if ok == 0 || returned == 0 {
        return Ok(false);
    }
    Ok(elevation.TokenIsElevated != 0)
}

/// Re-launch the current `vpn-client.exe` elevated, preserving existing args.
pub fn relaunch_elevated() -> Result<()> {
    relaunch_elevated_with_args(&[])
}

/// Re-launch elevated and append extra CLI args (e.g. `--auto-connect-tor`).
///
/// Returns immediately after the elevated shell is invoked; the *current*
/// process is expected to exit (see `exit_after_relaunch`).
///
/// Returns an error only when we couldn't even try — for example, the
/// `ShellExecute` syscall failed. If the user dismisses the UAC consent
/// dialog, `ShellExecute` returns ≤32 and we surface a friendly error.
pub fn relaunch_elevated_with_args(extra_args: &[&str]) -> Result<()> {
    use windows_sys::Win32::UI::Shell::{ShellExecuteW, SE_ERR_ACCESSDENIED};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let exe = std::env::current_exe().context("could not resolve current executable path")?;
    let exe_w: Vec<u16> = exe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();

    let mut parts: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| {
            let s = a.to_string_lossy().into_owned();
            if s.contains(' ') {
                format!("\"{s}\"")
            } else {
                s
            }
        })
        .collect();

    for extra in extra_args {
        if !parts.iter().any(|p| p == extra) {
            parts.push((*extra).to_string());
        }
    }

    let args_str = parts.join(" ");
    let args_w: Vec<u16> = std::ffi::OsStr::new(&args_str)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let verb_w: Vec<u16> = "runas".encode_utf16().chain(std::iter::once(0)).collect();
    let cwd_w: Vec<u16> = std::env::current_dir()
        .map(|p| p.as_os_str().encode_wide().chain(std::iter::once(0)).collect())
        .unwrap_or_else(|_| vec![0]);

    // SAFETY: All four wide strings are null-terminated and remain alive for
    // the duration of the call. SW_SHOWNORMAL so the elevated GUI is visible
    // (SW_HIDE was a bug — elevated instance launched invisible).
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb_w.as_ptr(),
            exe_w.as_ptr(),
            if args_str.is_empty() {
                std::ptr::null()
            } else {
                args_w.as_ptr()
            },
            cwd_w.as_ptr(),
            SW_SHOWNORMAL as i32,
        )
    };

    let handle = result as isize;
    if handle <= 32 {
        let msg = match handle as u32 {
            SE_ERR_ACCESSDENIED => {
                "User cancelled the UAC prompt — system-wide tunnel requires Administrator."
            }
            2 => "Could not relaunch as Administrator (file not found).",
            3 => "Could not relaunch as Administrator (path not found).",
            8 => "Out of memory while relaunching as Administrator.",
            26 => "Could not share the desktop with the elevated process.",
            27 => "The user cancelled the operation.",
            other => {
                eprintln!("ShellExecuteW(run-as) returned error code {other}");
                "Could not relaunch as Administrator."
            }
        };
        anyhow::bail!(msg);
    }

    Ok(())
}

/// Exit the current (non-elevated) process after a successful elevated relaunch.
pub fn exit_after_relaunch() -> ! {
    // Give the elevated child a chance to open its window before we go.
    std::thread::sleep(std::time::Duration::from_millis(800));
    let pid = std::process::id();
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output();
    std::process::exit(0);
}
