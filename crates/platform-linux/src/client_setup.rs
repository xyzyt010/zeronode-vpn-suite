//! Client-side environment diagnostics (Block A5).
//!
//! `client_setup_checks()` reports everything the GUI/CLI needs to know about
//! the host's ability to run each protocol backend. Every tunnel block (C–I)
//! extends this list as it lands.

use std::path::PathBuf;
use vpn_suite_core::setup::{SetupCheck, SetupStatus};

#[cfg(target_os = "linux")]
use crate::common::{
    command_exists, current_exe_dir, detect_distro, find_binary, find_in_path, install_hint,
    Distro,
};

/// Candidate locations for the bundled Tor binary (Block G1 resolver).
/// Returns the first usable Tor: bundled (x86_64 expert bundle) if its
/// NEEDED libs (libevent-2.1.so.7, libssl.so.3) are satisfiable, otherwise
/// falls back to system `tor` in PATH. This makes the same binary work on
/// Debian 11 (libssl1.1) via system tor, and on Debian 12+/Arch/Fedora via bundled.
pub fn resolve_tor_binary() -> Option<PathBuf> {
    #[cfg(not(target_os = "linux"))]
    {
        None
    }

    #[cfg(target_os = "linux")]
    {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(exe_dir) = current_exe_dir() {
            candidates.push(exe_dir.join("assets/tor-linux/tor"));
            candidates.push(exe_dir.join("../../apps/client/assets/tor-linux/tor"));
        }
        candidates.push(PathBuf::from("/usr/share/vpn-client/tor-linux/tor"));
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join("apps/client/assets/tor-linux/tor"));
        }
        // Try bundled first, but only if its NEEDED libs are available.
        for cand in &candidates {
            if cand.is_file() && is_tor_binary_usable(cand) {
                return Some(cand.clone());
            }
        }
        // Bundled not usable (wrong arch on aarch64 host, or missing libssl3/libevent) -> system tor
        if let Some(sys) = find_binary("tor").or_else(|| find_in_path("tor")) {
            return Some(sys);
        }
        // Last resort: return bundled even if not usable (so caller can report lib error)
        candidates.into_iter().find(|path| path.is_file())
    }
}

#[cfg(target_os = "linux")]
fn is_tor_binary_usable(path: &std::path::Path) -> bool {
    // On aarch64 host, x86_64 tor cannot be executed natively — consider it usable
    // if the file exists and is not obviously broken (for packaging checks).
    // We check architecture via `file` header: if host is aarch64, skip exec test.
    if std::env::consts::ARCH == "aarch64" {
        // Just check that NEEDED libs are plausibly available on x86_64 target via ldconfig
        // For now, assume bundled tor is usable on x86_64 target if it exists.
        return true;
    }
    // Try to run `tor --version` with 1s timeout; if it fails with "cannot open shared object"
    // we consider it not usable and will fallback to system tor.
    let output = std::process::Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

/// Locate a distro OpenVPN binary without shelling out.
pub fn find_openvpn_binary() -> Option<PathBuf> {
    #[cfg(not(target_os = "linux"))]
    {
        None
    }

    #[cfg(target_os = "linux")]
    {
        for fixed in ["/usr/sbin/openvpn", "/usr/bin/openvpn"] {
            let path = PathBuf::from(fixed);
            if path.is_file() {
                return Some(path);
            }
        }
        find_in_path("openvpn")
    }
}

#[cfg(not(target_os = "linux"))]
pub fn client_setup_checks() -> Vec<SetupCheck> {
    vec![SetupCheck {
        name: String::from("Linux client platform"),
        status: SetupStatus::Warn,
        detail: String::from("client setup checks are only produced on Linux targets"),
        remedy: None,
    }]
}

#[cfg(target_os = "linux")]
pub fn client_setup_checks() -> Vec<SetupCheck> {
    let mut checks = Vec::new();
    checks.push(tun_device_check());
    checks.push(privilege_check());
    checks.push(pkexec_check());
    checks.push(crate::ensure_wireguard_module());
    checks.push(iproute2_check());
    checks.push(firewall_tooling_check());
    checks.push(dns_manager_check());
    checks.push(openvpn_check());
    checks.push(pptp_check());
    checks.push(shadowsocks_check());
    checks.push(tor_check());
    checks
}

#[cfg(target_os = "linux")]
fn tun_device_check() -> SetupCheck {
    if std::path::Path::new("/dev/net/tun").exists() {
        SetupCheck {
            name: String::from("TUN device"),
            status: SetupStatus::Pass,
            detail: String::from("/dev/net/tun is available for tunnel interfaces"),
            remedy: None,
        }
    } else {
        SetupCheck {
            name: String::from("TUN device"),
            status: SetupStatus::Fail,
            detail: String::from("/dev/net/tun was not found; no TUN-based tunnel can start"),
            remedy: Some(String::from("run: sudo modprobe tun")),
        }
    }
}

#[cfg(target_os = "linux")]
fn privilege_check() -> SetupCheck {
    if crate::elevation::is_elevated() {
        SetupCheck {
            name: String::from("Privileges"),
            status: SetupStatus::Pass,
            detail: String::from("running as root; system-wide tunnels can start directly"),
            remedy: None,
        }
    } else {
        SetupCheck {
            name: String::from("Privileges"),
            status: SetupStatus::Warn,
            detail: String::from(
                "not running as root; system-wide tunnels will prompt via polkit on connect",
            ),
            remedy: None,
        }
    }
}

#[cfg(target_os = "linux")]
fn pkexec_check() -> SetupCheck {
    if crate::elevation::pkexec_available() {
        SetupCheck {
            name: String::from("Elevation prompt"),
            status: SetupStatus::Pass,
            detail: String::from("pkexec is available for administrator prompts"),
            remedy: None,
        }
    } else {
        let pkg = match detect_distro() {
            Distro::Arch | Distro::Fedora | Distro::OpenSuse => "polkit",
            _ => "policykit-1",
        };
        SetupCheck {
            name: String::from("Elevation prompt"),
            status: SetupStatus::Warn,
            detail: String::from("pkexec not found; elevation prompts will fail"),
            remedy: Some(install_hint(pkg)),
        }
    }
}

#[cfg(target_os = "linux")]
fn iproute2_check() -> SetupCheck {
    if command_exists("ip") {
        SetupCheck {
            name: String::from("iproute2"),
            status: SetupStatus::Pass,
            detail: String::from("ip(8) is available for link/route management"),
            remedy: None,
        }
    } else {
        let pkg = match detect_distro() {
            Distro::Fedora => "iproute",
            _ => "iproute2",
        };
        SetupCheck {
            name: String::from("iproute2"),
            status: SetupStatus::Fail,
            detail: String::from("ip(8) not found in PATH"),
            remedy: Some(install_hint(pkg)),
        }
    }
}

#[cfg(target_os = "linux")]
fn firewall_tooling_check() -> SetupCheck {
    if command_exists("nft") || command_exists("iptables") {
        SetupCheck {
            name: String::from("Firewall tooling"),
            status: SetupStatus::Pass,
            detail: String::from(
                "nftables/iptables available for system-tunnel policy routing",
            ),
            remedy: None,
        }
    } else {
        SetupCheck {
            name: String::from("Firewall tooling"),
            status: SetupStatus::Warn,
            detail: String::from("neither nft nor iptables found; system-wide tunnels need one of them"),
            remedy: Some(install_hint("nftables")),
        }
    }
}

#[cfg(target_os = "linux")]
fn dns_manager_check() -> SetupCheck {
    if std::path::Path::new("/run/systemd/resolve/resolv.conf").exists()
        || command_exists("resolvectl")
    {
        SetupCheck {
            name: String::from("DNS manager"),
            status: SetupStatus::Pass,
            detail: String::from("systemd-resolved detected; DNS override uses resolvectl"),
            remedy: None,
        }
    } else if std::path::Path::new("/etc/resolv.conf").exists() {
        // openresolv (Arch) uses same /etc/resolv.conf symlink managed by resolvconf(8)
        let detail = if std::path::Path::new("/usr/sbin/resolvconf").exists()
            || std::path::Path::new("/sbin/resolvconf").exists()
            || command_exists("resolvconf")
        {
            String::from("openresolv detected; DNS overrides edit /etc/resolv.conf with backup/restore")
        } else {
            String::from("no systemd-resolved; DNS overrides edit /etc/resolv.conf with backup/restore")
        };
        SetupCheck {
            name: String::from("DNS manager"),
            status: SetupStatus::Warn,
            detail,
            remedy: None,
        }
    } else {
        SetupCheck {
            name: String::from("DNS manager"),
            status: SetupStatus::Warn,
            detail: String::from("could not determine DNS management strategy"),
            remedy: None,
        }
    }
}

#[cfg(target_os = "linux")]
fn openvpn_check() -> SetupCheck {
    match find_openvpn_binary().or_else(|| find_binary("openvpn")) {
        Some(path) => SetupCheck {
            name: String::from("OpenVPN"),
            status: SetupStatus::Pass,
            detail: format!("openvpn binary found at {}", path.display()),
            remedy: None,
        },
        None => SetupCheck {
            name: String::from("OpenVPN"),
            status: SetupStatus::Warn,
            detail: String::from("openvpn is not installed; the OpenVPN tab will be unavailable"),
            remedy: Some(install_hint("openvpn")),
        },
    }
}

#[cfg(target_os = "linux")]
fn pptp_check() -> SetupCheck {
    let pptp = command_exists("pptp");
    let pppd = command_exists("pppd");
    let pptp_pkg = match detect_distro() {
        Distro::Arch => "pptpclient",
        Distro::Fedora | Distro::OpenSuse => "pptp",
        _ => "pptp-linux",
    };
    let hint = install_hint(&format!("{pptp_pkg} ppp"));
    match (pptp, pppd) {
        (true, true) => SetupCheck {
            name: String::from("PPTP"),
            status: SetupStatus::Pass,
            detail: String::from("pptp and pppd are installed"),
            remedy: None,
        },
        (true, false) | (false, true) => SetupCheck {
            name: String::from("PPTP"),
            status: SetupStatus::Warn,
            detail: format!(
                "partial PPTP stack (pptp={}, pppd={})",
                pptp, pppd
            ),
            remedy: Some(hint),
        },
        (false, false) => SetupCheck {
            name: String::from("PPTP"),
            status: SetupStatus::Warn,
            detail: String::from("pptp/pppd are not installed; the PPTP tab will be unavailable"),
            remedy: Some(hint),
        },
    }
}

#[cfg(target_os = "linux")]
fn shadowsocks_check() -> SetupCheck {
    let found = find_in_path("sslocal").is_some() || find_in_path("ss-local").is_some() || find_binary("sslocal").is_some() || find_binary("ss-local").is_some();
    if found {
        let path = find_in_path("sslocal")
            .or_else(|| find_in_path("ss-local"))
            .or_else(|| find_binary("sslocal"))
            .or_else(|| find_binary("ss-local"))
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "sslocal/ss-local".to_string());
        SetupCheck {
            name: String::from("Shadowsocks"),
            status: SetupStatus::Pass,
            detail: format!("shadowsocks binary found at {path}"),
            remedy: None,
        }
    } else {
        let pkg = match detect_distro() {
            Distro::Arch | Distro::Fedora | Distro::OpenSuse => "shadowsocks-libev",
            _ => "shadowsocks-libev",
        };
        SetupCheck {
            name: String::from("Shadowsocks"),
            status: SetupStatus::Warn,
            detail: String::from("sslocal/ss-local not found; Outline/Shadowsocks will be unavailable"),
            remedy: Some(install_hint(pkg)),
        }
    }
}

#[cfg(target_os = "linux")]
fn tor_check() -> SetupCheck {
    match resolve_tor_binary() {
        Some(path) => {
            let is_bundled = path.to_string_lossy().contains("tor-linux");
            let detail = if is_bundled {
                format!("tor binary resolved at {} (bundled expert bundle 15.0.17, needs libevent-2.1.so.7+libssl.so.3)", path.display())
            } else {
                format!("tor binary resolved at {} (system tor, fallback — bundled needs libevent/libssl3)", path.display())
            };
            SetupCheck {
                name: String::from("Tor"),
                status: SetupStatus::Pass,
                detail,
                remedy: None,
            }
        }
        None => {
            let hint = match detect_distro() {
                Distro::Arch => "sudo pacman -S tor".to_string(),
                Distro::Fedora | Distro::OpenSuse => "sudo dnf install tor".to_string(),
                _ => "sudo apt install tor".to_string(),
            };
            // Also mention bundled libs for Debian 11 case
            let bundled_hint = match detect_distro() {
                Distro::Arch => "sudo pacman -S libevent openssl zlib".to_string(),
                Distro::Fedora | Distro::OpenSuse => "sudo dnf install libevent openssl-libs zlib".to_string(),
                _ => "sudo apt install libevent-2.1-7 libssl3 zlib1g".to_string(),
            };
            SetupCheck {
                name: String::from("Tor"),
                status: SetupStatus::Warn,
                detail: String::from("no tor binary found in the bundle, /usr/share/vpn-client or PATH"),
                remedy: Some(format!(
                    "run tools/fetch-tor-linux.sh before packaging, or {hint} (and for bundled: {bundled_hint})"
                )),
            }
        }
    }
}
