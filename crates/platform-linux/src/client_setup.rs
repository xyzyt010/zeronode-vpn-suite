//! Client-side environment diagnostics (Block A5).
//!
//! `client_setup_checks()` reports everything the GUI/CLI needs to know about
//! the host's ability to run each protocol backend. Every tunnel block (C–I)
//! extends this list as it lands.

use std::path::PathBuf;
use vpn_suite_core::setup::{SetupCheck, SetupStatus};

#[cfg(target_os = "linux")]
use crate::common::{command_exists, current_exe_dir, find_in_path};

/// Candidate locations for the bundled Tor binary (Block G1 resolver).
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
        candidates
            .into_iter()
            .find(|path| path.is_file())
            .or_else(|| find_in_path("tor"))
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
        SetupCheck {
            name: String::from("Elevation prompt"),
            status: SetupStatus::Warn,
            detail: String::from("pkexec not found; elevation prompts will fail"),
            remedy: Some(String::from("sudo apt install policykit-1")),
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
        SetupCheck {
            name: String::from("iproute2"),
            status: SetupStatus::Fail,
            detail: String::from("ip(8) not found in PATH"),
            remedy: Some(String::from("sudo apt install iproute2")),
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
            remedy: Some(String::from("sudo apt install nftables")),
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
        SetupCheck {
            name: String::from("DNS manager"),
            status: SetupStatus::Warn,
            detail: String::from("no systemd-resolved; DNS overrides edit /etc/resolv.conf with backup/restore"),
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
    match find_openvpn_binary() {
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
            remedy: Some(String::from("sudo apt install openvpn")),
        },
    }
}

#[cfg(target_os = "linux")]
fn pptp_check() -> SetupCheck {
    let pptp = command_exists("pptp");
    let pppd = command_exists("pppd");
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
            remedy: Some(String::from("sudo apt install pptp-linux ppp")),
        },
        (false, false) => SetupCheck {
            name: String::from("PPTP"),
            status: SetupStatus::Warn,
            detail: String::from("pptp/pppd are not installed; the PPTP tab will be unavailable"),
            remedy: Some(String::from("sudo apt install pptp-linux ppp")),
        },
    }
}

#[cfg(target_os = "linux")]
fn tor_check() -> SetupCheck {
    match resolve_tor_binary() {
        Some(path) => SetupCheck {
            name: String::from("Tor"),
            status: SetupStatus::Pass,
            detail: format!("tor binary resolved at {}", path.display()),
            remedy: None,
        },
        None => SetupCheck {
            name: String::from("Tor"),
            status: SetupStatus::Warn,
            detail: String::from("no tor binary found in the bundle, /usr/share/vpn-client or PATH"),
            remedy: Some(String::from(
                "run tools/fetch-tor-linux.sh before packaging, or sudo apt install tor",
            )),
        },
    }
}
