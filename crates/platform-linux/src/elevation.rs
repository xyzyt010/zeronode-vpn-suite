//! Linux privilege helpers (parity with Windows `elevation.rs`).
//!
//! The client manifest-equivalent runs unprivileged so users can launch the
//! app normally. When a system-wide tunnel is requested we re-launch through
//! `pkexec` (polkit), which shows the desktop's native authentication dialog
//! on both GNOME and XFCE.
//!
//! Environment strategy (Block B3):
//! * `pkexec` resets the child environment to a safe allowlist; it forwards
//!   display-related variables (`DISPLAY`, `XAUTHORITY`, …) itself.
//! * We additionally forward `HOME` via `/usr/bin/env` so the elevated
//!   instance keeps using the invoking user's config/data directories,
//!   matching UAC same-user semantics on Windows.
//! * When the originating session is Wayland we set `ZERONODE_BACKEND=x11`
//!   for the elevated instance: root processes cannot attach to the Wayland
//!   compositor socket on mutter, but XWayland accepts them.

#[cfg(target_os = "linux")]
mod imp {
    use anyhow::{Context, Result};
    use std::ffi::{OsStr, OsString};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    /// UID of the current process (`geteuid`). Falls back to parsing
    /// `/proc/self/status` if libc is unavailable for some reason.
    pub fn current_uid() -> Option<u32> {
        // SAFETY: geteuid is a trivial syscall wrapper with no preconditions.
        let uid = unsafe { libc::geteuid() };
        if uid != u32::MAX {
            return Some(uid);
        }
        procfs_uid()
    }

    fn procfs_uid() -> Option<u32> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        status.lines().find_map(|line| {
            let value = line.strip_prefix("Uid:")?;
            value.split_whitespace().nth(1)?.parse().ok()
        })
    }

    /// Returns `true` when running as root (the Unix analogue of an elevated
    /// Administrator token).
    pub fn is_elevated() -> bool {
        current_uid() == Some(0)
    }

    pub fn pkexec_available() -> bool {
        crate::common::command_exists("pkexec")
    }

    fn session_is_wayland() -> bool {
        std::env::var("XDG_SESSION_TYPE")
            .map(|value| value.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
            || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }

    /// Build the full argv for the elevated relaunch:
    /// `pkexec env [VAR=val…] <exe> <orig-args…> <extras…>`
    /// Forwards display variables explicitly so the elevated GUI can attach
    /// to the user's X/Wayland session (pkexec's allowlist is minimal).
    ///
    /// Exposed as a pure function for tests.
    fn build_relaunch_argv(
        exe: &std::path::Path,
        original_args: &[OsString],
        extra_args: &[&str],
        home: Option<&OsStr>,
        wayland_session: bool,
    ) -> Vec<OsString> {
        let mut argv = vec![OsString::from("pkexec"), OsString::from("/usr/bin/env")];
        // Forward critical display/session vars that pkexec would otherwise strip.
        for key in ["DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"] {
            if let Some(val) = std::env::var_os(key) {
                if !val.is_empty() {
                    let mut var = OsString::from(key);
                    var.push("=");
                    var.push(val);
                    argv.push(var);
                }
            }
        }
        if let Some(home) = home {
            let mut var = OsString::from("HOME=");
            var.push(home);
            argv.push(var);
        }
        if wayland_session {
            argv.push(OsString::from("ZERONODE_BACKEND=x11"));
        }
        argv.push(exe.as_os_str().to_owned());
        argv.extend(original_args.iter().cloned());
        for extra in extra_args {
            if !original_args.iter().any(|arg| arg == *extra) {
                argv.push(OsString::from(*extra));
            }
        }
        argv
    }

    /// Re-launch the current executable as root, preserving existing CLI args.
    pub fn relaunch_elevated() -> Result<()> {
        relaunch_elevated_with_args(&[])
    }

    /// Re-launch elevated and append extra CLI args (e.g. `--auto-connect-tor`).
    ///
    /// Behaviour contract (mirrors the Windows implementation):
    /// * Returns `Ok(())` once the polkit dialog has been accepted and the
    ///   elevated instance is starting; the caller is expected to show a
    ///   notice and then call [`exit_after_relaunch`].
    /// * Returns `Err` when the user dismissed/cancelled the dialog or pkexec
    ///   could not start the program — the current process stays alive so the
    ///   UI can surface a friendly message (same as the UAC decline path).
    /// * A short probe window distinguishes instant cancellations from a
    ///   dialog that is still open; late failures after `Ok` are logged only.
    pub fn relaunch_elevated_with_args(extra_args: &[&str]) -> Result<()> {
        if !pkexec_available() {
            anyhow::bail!(
                "pkexec was not found. Install polkit (sudo apt install policykit-1) to enable \
                 administrator elevation prompts."
            );
        }

        let exe = std::env::current_exe().context("could not resolve current executable path")?;
        let original_args: Vec<OsString> = std::env::args_os().skip(1).collect();
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .filter(|_| !is_elevated());
        let argv = build_relaunch_argv(
            &exe,
            &original_args,
            extra_args,
            home.as_deref(),
            session_is_wayland(),
        );

        let (program, args) = argv.split_first().expect("argv is never empty");
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                anyhow::anyhow!(
                    "could not start pkexec ({error}). Install polkit (sudo apt install policykit-1)."
                )
            })?;

        // Probe: give the user ample time to type the password (30s). Polkit
        // cancellations exit quickly (<<1s); an open dialog keeps the process
        // alive. We return Ok only after the dialog has been accepted and the
        // elevated child is running — the caller will then exit.
        let probe_deadline = Duration::from_millis(30000);
        let started = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    // pkexec finished quickly → either cancelled or failed to exec.
                    // Since we used Stdio::null, we have no stderr; classify by code.
                    return Err(classify_pkexec_failure(status.code(), ""));
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "could not track pkexec process: {error}"
                    ))
                }
            }

            if started.elapsed() >= probe_deadline {
                // Still running after 30s → assume auth succeeded and elevated
                // vpn-client is now running (pkexec exec'd it). Detach.
                break;
            }
            std::thread::sleep(Duration::from_millis(80));
        }

        // Detach: pkexec has exec'd the elevated vpn-client; original can exit.
        // We do not wait for the child — it is now the elevated GUI.
        std::mem::forget(child);

        Ok(())
    }

    fn classify_pkexec_failure(code: Option<i32>, stderr: &str) -> anyhow::Error {
        let lowered = stderr.to_ascii_lowercase();
        if lowered.contains("not authorized")
            || lowered.contains("dismissed")
            || lowered.contains("authentication")
            || code == Some(127)
            || code == Some(3)
        {
            anyhow::anyhow!(
                "Authentication declined — system-wide tunnel requires administrator (root)."
            )
        } else if code == Some(126) {
            anyhow::anyhow!("pkexec could not execute the program (check permissions/path).")
        } else {
            anyhow::anyhow!("Elevation failed (exit {code:?}): {}", stderr.trim())
        }
    }

    /// Exit the current (non-elevated) process after a successful elevated
    /// relaunch. Gives the elevated child a head start opening its window.
    pub fn exit_after_relaunch() -> ! {
        std::thread::sleep(Duration::from_millis(1200));
        std::process::exit(0);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn uid_resolution_and_elevation_agree() {
            let uid = current_uid();
            assert!(uid.is_some());
            assert_eq!(is_elevated(), uid == Some(0));
        }

        #[test]
        fn argv_builder_orders_pkexec_env_exe_args() {
            let exe = std::path::PathBuf::from("/usr/bin/vpn-client");
            let orig = vec![OsString::from("--auto-connect-tor")];
            let argv = build_relaunch_argv(
                &exe,
                &orig,
                &["--auto-connect-tor"],
                Some(OsStr::new("/home/ubuntu")),
                true,
            );
            let flat: Vec<String> =
                argv.iter().map(|a| a.to_string_lossy().into_owned()).collect();
            assert_eq!(flat[0], "pkexec");
            assert_eq!(flat[1], "/usr/bin/env");
            assert!(flat.contains(&"HOME=/home/ubuntu".to_string()));
            assert!(flat.contains(&"ZERONODE_BACKEND=x11".to_string()));
            let exe_pos =
                flat.iter().position(|a| a == "/usr/bin/vpn-client").unwrap();
            assert_eq!(flat[exe_pos + 1], "--auto-connect-tor");
            // Extra duplicates of existing args are not appended twice.
            assert_eq!(
                flat.iter().filter(|a| *a == "--auto-connect-tor").count(),
                1
            );
        }

        #[test]
        fn failure_classifier_prefers_cancel_message() {
            let err = classify_pkexec_failure(Some(127), "Error: Not authorized");
            assert!(err.to_string().contains("declined"));
            let err = classify_pkexec_failure(Some(1), "boom");
            assert!(err.to_string().contains("boom"));
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use anyhow::{anyhow, Result};

    pub fn current_uid() -> Option<u32> {
        None
    }

    pub fn is_elevated() -> bool {
        false
    }

    #[allow(dead_code)]
    pub fn pkexec_available() -> bool {
        false
    }

    pub fn relaunch_elevated() -> Result<()> {
        relaunch_elevated_with_args(&[])
    }

    pub fn relaunch_elevated_with_args(_extra_args: &[&str]) -> Result<()> {
        Err(anyhow!("pkexec elevation is only available on Linux"))
    }

    pub fn exit_after_relaunch() -> ! {
        std::process::exit(0)
    }
}

#[cfg(target_os = "linux")]
pub use imp::{
    current_uid, exit_after_relaunch, is_elevated, pkexec_available, relaunch_elevated,
    relaunch_elevated_with_args,
};

#[cfg(not(target_os = "linux"))]
pub use imp::{current_uid, exit_after_relaunch, is_elevated, relaunch_elevated, relaunch_elevated_with_args};
