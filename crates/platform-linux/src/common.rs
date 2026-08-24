//! Shared helpers for the Linux platform crate.
//!
//! Hosts the command runner used by both server and client flows, PATH
//! resolution, and small filesystem utilities. The client-side modules
//! (`proc`, `elevation`, `client_setup`) build on top of these.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Default wall-clock budget for [`run_command`] before the child is killed.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum CommandOutcome {
    Success(String),
    Failed(String),
}

impl CommandOutcome {
    /// Used by tunnel modules landing in the next blocks (C–I).
    #[allow(dead_code)]
    pub fn ok(&self) -> bool {
        matches!(self, CommandOutcome::Success(_))
    }
}

/// Distro family for diagnostics and remedy hints. Detection via /etc/os-release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distro {
    Debian,
    Ubuntu,
    Mint,
    Arch,
    Fedora,
    OpenSuse,
    Unknown,
}

impl Distro {
    pub fn as_str(self) -> &'static str {
        match self {
            Distro::Debian => "debian",
            Distro::Ubuntu => "ubuntu",
            Distro::Mint => "mint",
            Distro::Arch => "arch",
            Distro::Fedora => "fedora",
            Distro::OpenSuse => "opensuse",
            Distro::Unknown => "unknown",
        }
    }
}

/// Detect current distro via `/etc/os-release` ID/ID_LIKE. Cheap, cached per call (no OnceLock — caller caches if hot).
pub fn detect_distro() -> Distro {
    if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
        let mut id = String::new();
        let mut id_like = String::new();
        for line in content.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("ID=") {
                id = v.trim_matches('"').trim_matches('\'').to_ascii_lowercase();
            } else if let Some(v) = line.strip_prefix("ID_LIKE=") {
                id_like = v.trim_matches('"').trim_matches('\'').to_ascii_lowercase();
            }
        }
        match id.as_str() {
            "debian" => return Distro::Debian,
            "ubuntu" => return Distro::Ubuntu,
            "linuxmint" | "mint" => return Distro::Mint,
            "arch" | "manjaro" | "endeavouros" => return Distro::Arch,
            "fedora" | "rhel" | "centos" | "rocky" | "almalinux" => return Distro::Fedora,
            "opensuse" | "opensuse-tumbleweed" | "opensuse-leap" => return Distro::OpenSuse,
            _ => {}
        }
        // fallback via ID_LIKE
        if id_like.contains("debian") {
            if id_like.contains("ubuntu") || id == "ubuntu" {
                return Distro::Ubuntu;
            }
            return Distro::Debian;
        }
        if id_like.contains("arch") {
            return Distro::Arch;
        }
        if id_like.contains("fedora") || id_like.contains("rhel") {
            return Distro::Fedora;
        }
    }
    Distro::Unknown
}

/// Install hint per distro, e.g. `install_hint("openvpn")` → `"sudo apt install openvpn"` on Debian/Ubuntu/Mint.
pub fn install_hint(packages: &str) -> String {
    match detect_distro() {
        Distro::Arch => format!("sudo pacman -S {packages}"),
        Distro::Fedora | Distro::OpenSuse => {
            let mgr = if matches!(detect_distro(), Distro::OpenSuse) {
                "zypper"
            } else {
                "dnf"
            };
            format!("sudo {mgr} install {packages}")
        }
        _ => format!("sudo apt install {packages}"),
    }
}

/// Resolve an executable name against `PATH` (exact file match per dir).
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let path = PathBuf::from(name);
        return path.is_file().then_some(path);
    }
    if let Some(paths) = env::var_os("PATH") {
        if let Some(found) = env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
        {
            return Some(found);
        }
    }
    // Explicit sbin fallbacks (Fedora/Arch pptp/openvpn, nft sometimes in /usr/sbin without PATH for non-root)
    for dir in ["/usr/sbin", "/usr/bin", "/sbin", "/bin"] {
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Alias for `find_in_path` but future-proof: checks PATH then sbin fallbacks. Preferred for tunnel binaries.
pub fn find_binary(name: &str) -> Option<PathBuf> {
    find_in_path(name)
}

pub fn command_exists(name: &str) -> bool {
    find_in_path(name).is_some()
}

/// Directory containing the running executable. Consumed by asset resolvers
/// in the tunnel blocks (Tor bundle discovery, OpenVPN binary lookup).
#[allow(dead_code)]
pub fn current_exe_dir() -> Option<PathBuf> {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(ToOwned::to_owned))
}

/// Run a program with args, capturing stdout/stderr with a hard timeout
/// (child is SIGKILLed past the deadline). Mirrors the historical
/// `run_command` API from lib.rs so server call sites stay unchanged.
pub fn run_command(program: &str, args: &[&str]) -> CommandOutcome {
    run_command_with_timeout(program, args, COMMAND_TIMEOUT)
}

pub fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> CommandOutcome {
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return CommandOutcome::Failed(format!("could not run {program}: {error}")),
    };

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = match child.wait_with_output() {
                    Ok(output) => output,
                    Err(error) => {
                        return CommandOutcome::Failed(format!(
                            "{program} finished but output could not be read: {error}"
                        ))
                    }
                };
                if status.success() {
                    return CommandOutcome::Success(
                        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
                    );
                }
                let detail = if output.stderr.is_empty() {
                    &output.stdout
                } else {
                    &output.stderr
                };
                return CommandOutcome::Failed(format!(
                    "{program} {} failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(detail).trim()
                ));
            }
            Ok(None) => {}
            Err(error) => {
                return CommandOutcome::Failed(format!("could not wait for {program}: {error}"))
            }
        }

        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return CommandOutcome::Failed(format!(
                "{program} {} timed out after {} ms",
                args.join(" "),
                timeout.as_millis()
            ));
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// Best-effort capture of a tool's stdout; never fails loudly.
/// Linux analogue of the Windows `silent_cmd::silent_output`.
pub fn silent_output(program: impl AsRef<Path>, args: &[&str]) -> Option<String> {
    let output = Command::new(program.as_ref())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}
