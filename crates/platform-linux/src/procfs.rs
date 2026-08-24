//! Process discovery and termination via procfs.
//!
//! Linux counterpart of the Windows `silent_cmd::kill_process_image`:
//! scans `/proc/<pid>/exe` for matching image names and terminates matches
//! with a SIGTERM → SIGKILL ladder. Never kills our own process.

#[cfg(target_os = "linux")]
mod imp {
    /// Normalize an image name for comparison: lowercase, strip any `.exe`
    /// suffix so Windows-style call sites (`"tor.exe"`) behave identically.
    pub(super) fn normalize_image(name: &str) -> String {
        name.trim().to_ascii_lowercase()
            .trim_end_matches(".exe")
            .to_owned()
    }

    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    /// Executable basename of a running pid, normalized. `None` when the
    /// process vanished or is not readable by the current user.
    fn exe_name_of(pid: u32) -> Option<String> {
        let link = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        let name = link.file_name()?.to_str()?;
        Some(normalize_image(name))
    }

    /// All pids whose executable basename matches `image` (e.g. `"tor"` or
    /// `"tor.exe"`). Self is excluded.
    pub fn find_pids_by_name(image: &str) -> Vec<u32> {
        let target = normalize_image(image);
        let self_pid = std::process::id();
        let mut pids = Vec::new();
        let Ok(entries) = fs::read_dir("/proc") else {
            return pids;
        };
        for entry in entries.flatten() {
            let Some(file_name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            let Ok(pid) = file_name.parse::<u32>() else {
                continue;
            };
            if pid == self_pid {
                continue;
            }
            if exe_name_of(pid).as_deref() == Some(target.as_str()) {
                pids.push(pid);
            }
        }
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// True while the pid exists and is not a zombie.
    pub fn process_exists(pid: u32) -> bool {
        if pid == std::process::id() {
            return true;
        }
        let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat,
            Err(_) => return false,
        };
        // state is the field after the parenthesized comm; comm can contain
        // spaces, so split from the last ')'.
        match stat.rsplit_once(')') {
            Some((_, rest)) => {
                let mut fields = rest.split_whitespace();
                matches!(fields.next(), Some("R") | Some("S") | Some("D") | Some("T"))
            }
            None => false,
        }
    }

    /// Read a pidfile and return the pid only when that process is still alive.
    pub fn pid_from_pidfile(path: &Path) -> Option<u32> {
        let contents = fs::read_to_string(path).ok()?;
        let pid = contents.trim().parse::<u32>().ok()?;
        process_exists(pid).then_some(pid)
    }

    /// Terminate every process whose image name matches (e.g. `"tor.exe"`),
    /// SIGTERM first with a grace window, then SIGKILL stragglers.
    ///
    /// Returns how many processes were terminated.
    pub fn kill_process_by_name(image: &str) -> u32 {
        let pids: BTreeSet<u32> = find_pids_by_name(image).into_iter().collect();
        if pids.is_empty() {
            return 0;
        }

        let mut killed = 0u32;
        for pid in &pids {
            // SAFETY: libc::kill only sends a signal to the pid.
            let rc = unsafe { libc::kill(*pid as libc::pid_t, libc::SIGTERM) };
            if rc == 0 {
                killed += 1;
            } else if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                continue;
            } else {
                // EPERM or other failure — try SIGKILL anyway in case perms differ.
                let rc = unsafe { libc::kill(*pid as libc::pid_t, libc::SIGKILL) };
                if rc == 0 {
                    killed += 1;
                }
            }
        }

        // Grace window: wait up to 3 s for TERM'd processes to exit.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining: Vec<u32> =
                pids.iter().copied().filter(|p| process_exists(*p)).collect();
            if remaining.is_empty() || Instant::now() >= deadline {
                for pid in remaining {
                    // SAFETY: libc::kill only sends a signal to the pid.
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGKILL);
                    }
                    tracing::debug!("proc: SIGKILL sent to leftover pid {pid} ({image})");
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        killed
    }

    /// Write a pidfile atomically enough for our purposes (single writer).
    /// Consumed by the OpenVPN/PPTP/Tor lifecycle blocks.
    #[allow(dead_code)]
    pub fn write_pidfile(path: &PathBuf, pid: u32) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, format!("{pid}\n"))
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use std::path::Path;

    pub fn find_pids_by_name(_image: &str) -> Vec<u32> {
        Vec::new()
    }

    pub fn process_exists(_pid: u32) -> bool {
        false
    }

    pub fn pid_from_pidfile(_path: &Path) -> Option<u32> {
        None
    }

    pub fn kill_process_by_name(_image: &str) -> u32 {
        0
    }
}

pub use imp::{find_pids_by_name, kill_process_by_name, pid_from_pidfile, process_exists};

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::imp::{normalize_image, write_pidfile};
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn normalize_handles_exe_suffix_and_case() {
        assert_eq!(normalize_image("Tor.EXE"), "tor");
        assert_eq!(normalize_image(" openvpn "), "openvpn");
    }

    #[test]
    fn missing_process_yields_no_pids_and_no_kills() {
        assert!(find_pids_by_name("zeronode-definitely-not-running-xyz").is_empty());
        assert_eq!(
            kill_process_by_name("zeronode-definitely-not-running-xyz"),
            0
        );
    }

    #[test]
    fn self_process_exists() {
        assert!(process_exists(std::process::id()));
        assert!(!process_exists(u32::MAX - 1));
    }

    #[test]
    fn pidfile_round_trip_rejects_dead_pid() {
        let dir = std::env::temp_dir().join(format!("zn-proc-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = PathBuf::from(&dir).join("test.pid");

        imp::write_pidfile(&path, std::process::id()).unwrap();
        assert_eq!(pid_from_pidfile(&path), Some(std::process::id()));

        imp::write_pidfile(&path, u32::MAX - 1).unwrap();
        assert_eq!(pid_from_pidfile(&path), None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
