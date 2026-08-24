//! Linux platform implementation for the ZeroNode VPN suite.
//!
//! Server-side kernel WireGuard lives here directly (historical layout).
//! Client-side tunnel/elevation/process infrastructure is split into
//! modules mirroring `crates/platform-windows`:
//!
//! * [`common`]       — command runner, PATH resolution
//! * [`elevation`]    — root detection + pkexec relaunch (UAC parity)
//! * [`procfs`]       — process discovery/termination via /proc
//! * [`client_setup`] — client environment diagnostics

mod client_setup;
mod common;
mod elevation;
mod leak_protect;
mod openvpn;
mod outline;
mod pptp;
mod procfs;
mod socks_tun;
mod wireguard;

pub use client_setup::{client_setup_checks, find_openvpn_binary, resolve_tor_binary};
pub use common::silent_output;
pub use elevation::{
    exit_after_relaunch, is_elevated, relaunch_elevated, relaunch_elevated_with_args,
};
pub use leak_protect::{disable_all as ipv6_disable_all, restore as ipv6_restore};
pub use openvpn::{is_openvpn_running, openvpn_status, start_openvpn, stop_openvpn};
pub use outline::{
    find_sslocal, is_outline_embedded, is_outline_running, outline_socks_port, start_outline,
    stop_outline,
};
pub use pptp::{is_pptp_running, start_pptp, stop_pptp};
pub use procfs::{find_pids_by_name, kill_process_by_name, pid_from_pidfile, process_exists};
pub use socks_tun::{
    is_socks_tunnel_running, is_tor_tunnel_running, start_socks_system_tunnel,
    start_tor_system_tunnel, stop_socks_system_tunnel, stop_tor_system_tunnel,
};
pub use wireguard::{
    is_global_running as is_wireguard_running, is_wintun_available, parse_client_config as parse_wireguard_config,
    start_global as start_wireguard_global, stop_global as stop_wireguard_global,
    TunnelConfig as WireGuardTunnelConfig,
};

// ---------------------------------------------------------------------------
// Compatibility shims — Windows API surface expected by `apps/client`
// ---------------------------------------------------------------------------

/// Linux alias for Windows `kill_process_image` (takes "tor.exe" etc.).
pub fn kill_process_image(image: &str) -> u32 {
    kill_process_by_name(image)
}

/// No-op on Linux (registry proxy hint only exists on Windows).
pub fn clear_stale_wininet_socks_hint() {}

/// Stub for Windows `CREATE_NO_WINDOW` flag — value unused on Linux.
pub const CREATE_NO_WINDOW: u32 = 0;

/// Linux stub for `silent_command` — returns a Command with piped stdio.
pub fn silent_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    cmd
}

use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
use common::{command_exists, run_command, CommandOutcome};
use vpn_suite_core::{
    config::{ClientConfig, ServerConfig},
    model::{ControlSessionLease, ServerSummary},
    setup::{SetupCheck, SetupStatus},
};
#[cfg(target_os = "linux")]
use {
    anyhow::{Context, Result},
    std::net::IpAddr,
    wireguard_control::{Backend, Device, DeviceUpdate, InterfaceName, Key, PeerConfigBuilder},
};

pub const SERVER_INTERFACE: &str = "znwg0";
pub const CLIENT_INTERFACE: &str = "znclient0";

#[derive(Clone, Debug)]
pub struct LinuxPlatformSummary {
    pub service_model: &'static str,
    pub tunnel_backend: &'static str,
    pub firewall_strategy: &'static str,
}

pub fn describe_server_platform(_config: &ServerConfig) -> LinuxPlatformSummary {
    LinuxPlatformSummary {
        service_model: "systemd unit packaged with Debian maintainer scripts",
        tunnel_backend: "kernel WireGuard via wireguard-control; userspace fallback pending",
        firewall_strategy: "nftables/iptables preflight checked; rule application pending",
    }
}

pub fn server_platform_checks(_config: &ServerConfig) -> Vec<SetupCheck> {
    vec![
        SetupCheck {
            name: String::from("Linux service manager"),
            status: if Path::new("/run/systemd/system").exists() {
                SetupStatus::Pass
            } else {
                SetupStatus::Warn
            },
            detail: if Path::new("/run/systemd/system").exists() {
                String::from("systemd is active; Debian postinst can enable the server unit")
            } else {
                String::from(
                    "systemd is not active in this environment; service enablement will be skipped",
                )
            },
            remedy: Some(String::from(
                "install on a normal Ubuntu host for automatic service start; WSL often skips systemd",
            )),
        },
        SetupCheck {
            name: String::from("Packaged systemd unit"),
            status: if Path::new("/lib/systemd/system/zeronode-vpn-server.service").exists()
                || Path::new("apps/server/assets/debian/zeronode-vpn-server.service").exists()
            {
                SetupStatus::Pass
            } else {
                SetupStatus::Warn
            },
            detail: String::from("zeronode-vpn-server.service is expected in the Debian package"),
            remedy: Some(String::from(
                "rebuild zeronode-vpn-server if the unit is missing from dpkg-deb --contents",
            )),
        },
        SetupCheck {
            name: String::from("WireGuard kernel module"),
            status: if Path::new("/sys/module/wireguard").exists() {
                SetupStatus::Pass
            } else {
                SetupStatus::Warn
            },
            detail: if Path::new("/sys/module/wireguard").exists() {
                String::from("kernel WireGuard module is loaded")
            } else {
                String::from("kernel WireGuard module is not currently loaded")
            },
            remedy: Some(String::from(
                "load the kernel module or use the userspace backend once tunnel bring-up lands",
            )),
        },
        SetupCheck {
            name: String::from("Firewall tooling"),
            status: if command_exists("nft") || command_exists("iptables") {
                SetupStatus::Pass
            } else {
                SetupStatus::Warn
            },
            detail: if command_exists("nft") {
                String::from("nft is available")
            } else if command_exists("iptables") {
                String::from("iptables is available")
            } else {
                String::from("neither nft nor iptables was found in PATH")
            },
            remedy: Some(String::from(
                "install nftables on Ubuntu when firewall rule automation is enabled",
            )),
        },
        SetupCheck {
            name: String::from("IPv4 forwarding"),
            status: match fs::read_to_string("/proc/sys/net/ipv4/ip_forward") {
                Ok(value) if value.trim() == "1" => SetupStatus::Pass,
                Ok(_) | Err(_) => SetupStatus::Warn,
            },
            detail: match fs::read_to_string("/proc/sys/net/ipv4/ip_forward") {
                Ok(value) if value.trim() == "1" => String::from("IPv4 forwarding is enabled"),
                Ok(value) => format!("IPv4 forwarding is currently {}", value.trim()),
                Err(error) => format!("could not read ip_forward: {error}"),
            },
            remedy: Some(String::from(
                "server tunnel mode will need net.ipv4.ip_forward=1",
            )),
        },
    ]
}

pub fn apply_server_tunnel(config: &ServerConfig) -> Vec<SetupCheck> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        return vec![SetupCheck {
            name: String::from("Server WireGuard interface"),
            status: SetupStatus::Warn,
            detail: String::from("kernel WireGuard setup is only available on Linux targets"),
            remedy: None,
        }];
    }

    #[cfg(target_os = "linux")]
    {
        if effective_uid() != Some(0) {
            return vec![SetupCheck {
                name: String::from("Server WireGuard interface"),
                status: SetupStatus::Warn,
                detail: String::from("not running as root; skipped kernel interface setup"),
                remedy: Some(String::from(
                    "run the server service as root or Administrator-equivalent",
                )),
            }];
        }

        vec![
            ensure_wireguard_module(),
            apply_server_device(config),
            assign_interface_address(SERVER_INTERFACE, &server_internal_cidr(&config.vpn_subnet)),
            assign_interface_address(
                SERVER_INTERFACE,
                &server_internal_cidr_ipv6(&config.vpn_subnet_ipv6),
            ),
            set_link_up(SERVER_INTERFACE),
        ]
    }
}

#[cfg(target_os = "linux")]
pub fn apply_server_peer(lease: &ControlSessionLease) -> SetupCheck {
    match try_apply_server_peer(lease) {
        Ok(()) => SetupCheck {
            name: String::from("Server WireGuard peer"),
            status: SetupStatus::Pass,
            detail: format!(
                "peer {} allowed at {}/32",
                lease.client_name, lease.reserved_client_ip
            ),
            remedy: None,
        },
        Err(error) => SetupCheck {
            name: String::from("Server WireGuard peer"),
            status: SetupStatus::Warn,
            detail: format!("could not apply peer to kernel interface: {error:#}"),
            remedy: Some(String::from(
                "check that host-setup apply has run and the wireguard module is loaded",
            )),
        },
    }
}

#[cfg(not(target_os = "linux"))]
pub fn apply_server_peer(lease: &ControlSessionLease) -> SetupCheck {
    let _ = lease;
    SetupCheck {
        name: String::from("Server WireGuard peer"),
        status: SetupStatus::Warn,
        detail: String::from("kernel peer setup is only available on Linux targets"),
        remedy: None,
    }
}

#[cfg(target_os = "linux")]
pub fn remove_server_peer(lease: &ControlSessionLease) -> SetupCheck {
    let iface = match interface_name(SERVER_INTERFACE) {
        Ok(iface) => iface,
        Err(error) => {
            return SetupCheck {
                name: String::from("Server WireGuard peer"),
                status: SetupStatus::Warn,
                detail: format!("invalid interface name: {error:#}"),
                remedy: None,
            }
        }
    };
    let key = match Key::from_base64(&lease.client_public_key) {
        Ok(key) => key,
        Err(error) => {
            return SetupCheck {
                name: String::from("Server WireGuard peer"),
                status: SetupStatus::Warn,
                detail: format!("invalid client public key: {error}"),
                remedy: None,
            }
        }
    };

    match DeviceUpdate::new()
        .remove_peer_by_key(&key)
        .apply(&iface, Backend::Kernel)
    {
        Ok(()) => SetupCheck {
            name: String::from("Server WireGuard peer"),
            status: SetupStatus::Pass,
            detail: format!("removed peer {}", lease.client_name),
            remedy: None,
        },
        Err(error) => SetupCheck {
            name: String::from("Server WireGuard peer"),
            status: SetupStatus::Warn,
            detail: format!("could not remove peer: {error}"),
            remedy: None,
        },
    }
}

#[cfg(not(target_os = "linux"))]
pub fn remove_server_peer(lease: &ControlSessionLease) -> SetupCheck {
    let _ = lease;
    SetupCheck {
        name: String::from("Server WireGuard peer"),
        status: SetupStatus::Warn,
        detail: String::from("kernel peer removal is only available on Linux targets"),
        remedy: None,
    }
}

#[cfg(target_os = "linux")]
pub fn server_tunnel_status() -> SetupCheck {
    match interface_name(SERVER_INTERFACE)
        .and_then(|iface| Device::get(&iface, Backend::Kernel).map_err(anyhow::Error::from))
    {
        Ok(device) => SetupCheck {
            name: String::from("Server WireGuard interface"),
            status: SetupStatus::Pass,
            detail: format!(
                "{} kernel interface exists with {} peer(s)",
                device.name,
                device.peers.len()
            ),
            remedy: None,
        },
        Err(error) => SetupCheck {
            name: String::from("Server WireGuard interface"),
            status: SetupStatus::Warn,
            detail: format!("{SERVER_INTERFACE} is not active: {error}"),
            remedy: Some(String::from(
                "run sudo vpn-server host-setup apply and restart the service",
            )),
        },
    }
}

#[cfg(target_os = "linux")]
pub fn apply_client_tunnel(
    config: &ClientConfig,
    server: &ServerSummary,
    lease: &ControlSessionLease,
) -> Vec<SetupCheck> {
    if effective_uid() != Some(0) {
        return vec![SetupCheck {
            name: String::from("Client WireGuard interface"),
            status: SetupStatus::Warn,
            detail: String::from("not running as root; skipped kernel client tunnel setup"),
            remedy: Some(String::from("rerun with sudo vpn-client tunnel-apply")),
        }];
    }

    let mut checks = vec![
        ensure_wireguard_module(),
        apply_client_device(config, server, lease),
        assign_interface_address(
            CLIENT_INTERFACE,
            &format!("{}/32", lease.reserved_client_ip),
        ),
    ];

    if let Some(ipv6) = lease.reserved_client_ipv6.as_ref() {
        checks.push(assign_interface_address(
            CLIENT_INTERFACE,
            &format!("{}/128", ipv6),
        ));
    }

    checks.push(set_link_up(CLIENT_INTERFACE));
    checks.push(add_client_route(&lease.server_internal_ip));

    if let Some(ipv6) = lease.server_internal_ipv6.as_ref() {
        checks.push(add_client_route(ipv6));
    }

    checks
}

#[cfg(not(target_os = "linux"))]
pub fn apply_client_tunnel(
    config: &ClientConfig,
    server: &ServerSummary,
    lease: &ControlSessionLease,
) -> Vec<SetupCheck> {
    let _ = (config, server, lease);
    vec![SetupCheck {
        name: String::from("Client WireGuard interface"),
        status: SetupStatus::Warn,
        detail: String::from("kernel client tunnel setup is only available on Linux targets"),
        remedy: None,
    }]
}

#[cfg(target_os = "linux")]
pub fn remove_client_tunnel() -> SetupCheck {
    remove_link(CLIENT_INTERFACE)
}

#[cfg(not(target_os = "linux"))]
pub fn remove_client_tunnel() -> SetupCheck {
    SetupCheck {
        name: String::from("Client WireGuard link"),
        status: SetupStatus::Warn,
        detail: String::from("kernel client tunnel removal is only available on Linux targets"),
        remedy: None,
    }
}

#[cfg(target_os = "linux")]
pub fn client_tunnel_status() -> SetupCheck {
    match interface_name(CLIENT_INTERFACE)
        .and_then(|iface| Device::get(&iface, Backend::Kernel).map_err(anyhow::Error::from))
    {
        Ok(device) => SetupCheck {
            name: String::from("Client WireGuard interface"),
            status: SetupStatus::Pass,
            detail: format!(
                "{} kernel interface exists with {} peer(s)",
                device.name,
                device.peers.len()
            ),
            remedy: None,
        },
        Err(error) => SetupCheck {
            name: String::from("Client WireGuard interface"),
            status: SetupStatus::Warn,
            detail: format!("{CLIENT_INTERFACE} is not active: {error}"),
            remedy: Some(String::from(
                "connect first, then run sudo vpn-client tunnel-apply",
            )),
        },
    }
}

#[cfg(not(target_os = "linux"))]
pub fn client_tunnel_status() -> SetupCheck {
    SetupCheck {
        name: String::from("Client WireGuard interface"),
        status: SetupStatus::Warn,
        detail: String::from("kernel client tunnel status is only available on Linux targets"),
        remedy: None,
    }
}

#[cfg(not(target_os = "linux"))]
pub fn server_tunnel_status() -> SetupCheck {
    SetupCheck {
        name: String::from("Server WireGuard interface"),
        status: SetupStatus::Warn,
        detail: String::from("kernel WireGuard status is only available on Linux targets"),
        remedy: None,
    }
}

pub fn server_host_setup_plan(config: &ServerConfig) -> Vec<String> {
    vec![
        String::from("Create /var/lib/zeronode-vpn-server with root-owned service storage"),
        String::from(
            "Reload systemd and enable/restart zeronode-vpn-server.service when installed",
        ),
        String::from("Enable /proc/sys/net/ipv4/ip_forward for server routing"),
        format!(
            "Enable NAT masquerade for VPN subnet {} via nftables/iptables",
            config.vpn_subnet
        ),
        format!(
            "Create app-owned nftables table allowing UDP {} control and UDP {} WireGuard traffic",
            config.listen_port, config.wireguard_port
        ),
        format!("Create and configure kernel WireGuard interface {SERVER_INTERFACE}"),
    ]
}

pub fn apply_server_host_setup(config: &ServerConfig) -> Vec<SetupCheck> {
    if effective_uid() != Some(0) {
        return vec![SetupCheck {
            name: String::from("Privilege check"),
            status: SetupStatus::Fail,
            detail: String::from("host setup must run as root"),
            remedy: Some(String::from("rerun with sudo vpn-server host-setup apply")),
        }];
    }

    vec![
        create_state_dir(),
        enable_systemd_service(),
        enable_ipv4_forwarding(),
        apply_nftables(config.listen_port, config.wireguard_port),
        apply_nat_masquerade(&config.vpn_subnet),
        {
            let checks = apply_server_tunnel(config);
            checks.into_iter().last().unwrap_or_else(|| SetupCheck {
                name: String::from("Server WireGuard interface"),
                status: SetupStatus::Warn,
                detail: String::from("no tunnel setup checks were produced"),
                remedy: None,
            })
        },
    ]
}

fn apply_nat_masquerade(vpn_subnet: &str) -> SetupCheck {
    // Try nft first, fallback to iptables
    if command_exists("nft") {
        // Check if masquerade already exists
        let script = format!(
            "table inet zeronode_nat {{ chain postrouting {{ type nat hook postrouting priority srcnat; ip saddr {vpn_subnet} masquerade }} }}"
        );
        // Try to add; if table exists, flush first
        let _ = run_command("nft", &["delete", "table", "inet", "zeronode_nat"]);
        match run_nft_script(&script) {
            CommandOutcome::Success(_) => {
                return SetupCheck {
                    name: String::from("NAT masquerade"),
                    status: SetupStatus::Pass,
                    detail: format!("nft masquerade for {vpn_subnet} in table inet zeronode_nat"),
                    remedy: None,
                }
            }
            CommandOutcome::Failed(detail) => {
                // Fallback to iptables
                tracing::warn!("nft masquerade failed: {detail}, trying iptables");
            }
        }
    }
    if command_exists("iptables") {
        // iptables -t nat -A POSTROUTING -s <subnet> -j MASQUERADE
        // First check if rule exists
        let check = run_command(
            "iptables",
            &["-t", "nat", "-C", "POSTROUTING", "-s", vpn_subnet, "-j", "MASQUERADE"],
        );
        if matches!(check, CommandOutcome::Success(_)) {
            return SetupCheck {
                name: String::from("NAT masquerade"),
                status: SetupStatus::Pass,
                detail: format!("iptables masquerade for {vpn_subnet} already exists"),
                remedy: None,
            };
        }
        match run_command(
            "iptables",
            &["-t", "nat", "-A", "POSTROUTING", "-s", vpn_subnet, "-j", "MASQUERADE"],
        ) {
            CommandOutcome::Success(_) => {
                return SetupCheck {
                    name: String::from("NAT masquerade"),
                    status: SetupStatus::Pass,
                    detail: format!("iptables masquerade for {vpn_subnet}"),
                    remedy: None,
                }
            }
            CommandOutcome::Failed(detail) => {
                return SetupCheck {
                    name: String::from("NAT masquerade"),
                    status: SetupStatus::Warn,
                    detail: format!("failed to add masquerade: {detail}"),
                    remedy: Some(String::from("run: iptables -t nat -A POSTROUTING -s <subnet> -j MASQUERADE")),
                }
            }
        }
    }
    SetupCheck {
        name: String::from("NAT masquerade"),
        status: SetupStatus::Warn,
        detail: String::from("neither nft nor iptables found for NAT"),
        remedy: Some(String::from("install nftables or iptables")),
    }
}

pub fn remove_server_host_setup(_config: &ServerConfig) -> Vec<SetupCheck> {
    if effective_uid() != Some(0) {
        return vec![SetupCheck {
            name: String::from("Privilege check"),
            status: SetupStatus::Fail,
            detail: String::from("host setup removal must run as root"),
            remedy: Some(String::from("rerun with sudo vpn-server host-setup remove")),
        }];
    }

    vec![
        stop_systemd_service(),
        remove_nftables_table(),
        remove_link(SERVER_INTERFACE),
        SetupCheck {
            name: String::from("IPv4 forwarding"),
            status: SetupStatus::Warn,
            detail: String::from("left unchanged to avoid disrupting other routing workloads"),
            remedy: Some(String::from(
                "set /proc/sys/net/ipv4/ip_forward manually if this host only served ZeroNode",
            )),
        },
    ]
}

fn effective_uid() -> Option<u32> {
    elevation::current_uid()
}

fn create_state_dir() -> SetupCheck {
    match fs::create_dir_all("/var/lib/zeronode-vpn-server") {
        Ok(()) => SetupCheck {
            name: String::from("Service state directory"),
            status: SetupStatus::Pass,
            detail: String::from("/var/lib/zeronode-vpn-server exists"),
            remedy: None,
        },
        Err(error) => SetupCheck {
            name: String::from("Service state directory"),
            status: SetupStatus::Fail,
            detail: format!("could not create /var/lib/zeronode-vpn-server: {error}"),
            remedy: Some(String::from("check root filesystem permissions")),
        },
    }
}

fn enable_systemd_service() -> SetupCheck {
    if !Path::new("/run/systemd/system").exists() {
        return SetupCheck {
            name: String::from("systemd service"),
            status: SetupStatus::Warn,
            detail: String::from("systemd is not active; skipped service enablement"),
            remedy: Some(String::from(
                "install on a systemd host or use the Debian package",
            )),
        };
    }
    if !Path::new("/lib/systemd/system/zeronode-vpn-server.service").exists()
        && !Path::new("/usr/lib/systemd/system/zeronode-vpn-server.service").exists()
    {
        return SetupCheck {
            name: String::from("systemd service"),
            status: SetupStatus::Warn,
            detail: String::from("zeronode-vpn-server.service is not installed"),
            remedy: Some(String::from(
                "install zeronode-vpn-server.deb before enabling the service",
            )),
        };
    }

    let reload = run_command("systemctl", &["daemon-reload"]);
    let enable = run_command("systemctl", &["enable", "zeronode-vpn-server.service"]);
    let restart = run_command("systemctl", &["restart", "zeronode-vpn-server.service"]);
    command_check(
        "systemd service",
        &[reload, enable, restart],
        "service enabled and restarted",
        "inspect with systemctl status zeronode-vpn-server.service",
    )
}

fn stop_systemd_service() -> SetupCheck {
    if !Path::new("/run/systemd/system").exists() {
        return SetupCheck {
            name: String::from("systemd service"),
            status: SetupStatus::Warn,
            detail: String::from("systemd is not active; skipped service shutdown"),
            remedy: None,
        };
    }

    let stop = run_command("systemctl", &["stop", "zeronode-vpn-server.service"]);
    let disable = run_command("systemctl", &["disable", "zeronode-vpn-server.service"]);
    command_check(
        "systemd service",
        &[stop, disable],
        "service stopped and disabled",
        "inspect with systemctl status zeronode-vpn-server.service",
    )
}

fn enable_ipv4_forwarding() -> SetupCheck {
    match fs::write("/proc/sys/net/ipv4/ip_forward", b"1\n") {
        Ok(()) => SetupCheck {
            name: String::from("IPv4 forwarding"),
            status: SetupStatus::Pass,
            detail: String::from("enabled net.ipv4.ip_forward for the running system"),
            remedy: Some(String::from(
                "make persistent with sysctl config in a later installer milestone",
            )),
        },
        Err(error) => SetupCheck {
            name: String::from("IPv4 forwarding"),
            status: SetupStatus::Fail,
            detail: format!("could not enable ip_forward: {error}"),
            remedy: Some(String::from(
                "check /proc/sys/net/ipv4/ip_forward permissions",
            )),
        },
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn ensure_wireguard_module() -> SetupCheck {
    if Path::new("/sys/module/wireguard").exists() {
        return SetupCheck {
            name: String::from("WireGuard kernel module"),
            status: SetupStatus::Pass,
            detail: String::from("wireguard module is loaded"),
            remedy: None,
        };
    }

    match run_command("modprobe", &["wireguard"]) {
        CommandOutcome::Success(_) if Path::new("/sys/module/wireguard").exists() => SetupCheck {
            name: String::from("WireGuard kernel module"),
            status: SetupStatus::Pass,
            detail: String::from("wireguard module loaded"),
            remedy: None,
        },
        CommandOutcome::Success(_) => SetupCheck {
            name: String::from("WireGuard kernel module"),
            status: SetupStatus::Warn,
            detail: String::from("modprobe returned success but module is not visible"),
            remedy: Some(String::from("verify kernel WireGuard support")),
        },
        CommandOutcome::Failed(detail) => SetupCheck {
            name: String::from("WireGuard kernel module"),
            status: SetupStatus::Warn,
            detail,
            remedy: Some(String::from(
                "install or enable kernel WireGuard support; userspace fallback is pending",
            )),
        },
    }
}

#[cfg(target_os = "linux")]
fn apply_server_device(config: &ServerConfig) -> SetupCheck {
    match try_apply_server_device(config) {
        Ok(()) => SetupCheck {
            name: String::from("Server WireGuard device"),
            status: SetupStatus::Pass,
            detail: format!(
                "{SERVER_INTERFACE} configured on UDP {}",
                config.wireguard_port
            ),
            remedy: None,
        },
        Err(error) => SetupCheck {
            name: String::from("Server WireGuard device"),
            status: SetupStatus::Warn,
            detail: format!("could not configure kernel WireGuard device: {error:#}"),
            remedy: Some(String::from(
                "verify CAP_NET_ADMIN/root and kernel WireGuard support",
            )),
        },
    }
}

#[cfg(target_os = "linux")]
fn try_apply_server_device(config: &ServerConfig) -> Result<()> {
    let iface = interface_name(SERVER_INTERFACE)?;
    let private_key = Key::from_base64(&config.wireguard_keys.private_key)
        .map_err(|error| anyhow::anyhow!("invalid server private key: {error}"))?;
    DeviceUpdate::new()
        .set_private_key(private_key)
        .set_listen_port(config.wireguard_port)
        .apply(&iface, Backend::Kernel)
        .context("wireguard-control kernel apply failed")
}

#[cfg(target_os = "linux")]
fn try_apply_server_peer(lease: &ControlSessionLease) -> Result<()> {
    let iface = interface_name(SERVER_INTERFACE)?;
    let public_key = Key::from_base64(&lease.client_public_key)
        .map_err(|error| anyhow::anyhow!("invalid client public key: {error}"))?;
    let allowed_ip = lease
        .reserved_client_ip
        .parse::<IpAddr>()
        .with_context(|| format!("invalid reserved client IP {}", lease.reserved_client_ip))?;
    let mut peer = PeerConfigBuilder::new(&public_key)
        .replace_allowed_ips()
        .add_allowed_ip(allowed_ip, 32);

    if let Some(ipv6_str) = lease.reserved_client_ipv6.as_ref() {
        if let Ok(ipv6_addr) = ipv6_str.parse::<IpAddr>() {
            peer = peer.add_allowed_ip(ipv6_addr, 128);
        }
    }

    DeviceUpdate::new()
        .add_peer(peer)
        .apply(&iface, Backend::Kernel)
        .context("wireguard-control peer apply failed")
}

#[cfg(target_os = "linux")]
fn apply_client_device(
    config: &ClientConfig,
    server: &ServerSummary,
    lease: &ControlSessionLease,
) -> SetupCheck {
    match try_apply_client_device(config, server, lease) {
        Ok(()) => SetupCheck {
            name: String::from("Client WireGuard device"),
            status: SetupStatus::Pass,
            detail: format!(
                "{CLIENT_INTERFACE} configured for {} via {}",
                server.name, server.wireguard_endpoint
            ),
            remedy: None,
        },
        Err(error) => SetupCheck {
            name: String::from("Client WireGuard device"),
            status: SetupStatus::Warn,
            detail: format!("could not configure kernel WireGuard client: {error:#}"),
            remedy: Some(String::from(
                "verify root/CAP_NET_ADMIN, client key material, and server endpoint",
            )),
        },
    }
}

#[cfg(target_os = "linux")]
fn try_apply_client_device(
    config: &ClientConfig,
    server: &ServerSummary,
    lease: &ControlSessionLease,
) -> Result<()> {
    let iface = interface_name(CLIENT_INTERFACE)?;
    let keys = config
        .wireguard_keys
        .as_ref()
        .filter(|keys| keys.is_complete())
        .context("client WireGuard key material is missing")?;
    let private_key = Key::from_base64(&keys.private_key)
        .map_err(|error| anyhow::anyhow!("invalid client private key: {error}"))?;
    let server_key = Key::from_base64(&server.public_key)
        .map_err(|error| anyhow::anyhow!("invalid server public key: {error}"))?;
    let endpoint = server
        .wireguard_endpoint
        .parse()
        .with_context(|| format!("invalid WireGuard endpoint {}", server.wireguard_endpoint))?;
    let server_ip = lease
        .server_internal_ip
        .parse::<IpAddr>()
        .with_context(|| format!("invalid server internal IP {}", lease.server_internal_ip))?;

    let mut peer = PeerConfigBuilder::new(&server_key)
        .set_endpoint(endpoint)
        .set_persistent_keepalive_interval(25)
        .replace_allowed_ips()
        .add_allowed_ip(server_ip, 32);

    if let Some(ipv6_str) = lease.server_internal_ipv6.as_ref() {
        if let Ok(ipv6_addr) = ipv6_str.parse::<IpAddr>() {
            peer = peer.add_allowed_ip(ipv6_addr, 128);
        }
    }

    DeviceUpdate::new()
        .set_private_key(private_key)
        .replace_peers()
        .add_peer(peer)
        .apply(&iface, Backend::Kernel)
        .context("wireguard-control client apply failed")
}

#[cfg(target_os = "linux")]
fn assign_interface_address(iface: &str, cidr: &str) -> SetupCheck {
    match run_command("ip", &["address", "replace", cidr, "dev", iface]) {
        CommandOutcome::Success(_) => SetupCheck {
            name: String::from("WireGuard address"),
            status: SetupStatus::Pass,
            detail: format!("{iface} assigned {cidr}"),
            remedy: None,
        },
        CommandOutcome::Failed(detail) => SetupCheck {
            name: String::from("WireGuard address"),
            status: SetupStatus::Warn,
            detail,
            remedy: Some(String::from("verify iproute2 and CAP_NET_ADMIN/root")),
        },
    }
}

#[cfg(target_os = "linux")]
fn add_client_route(server_internal_ip: &str) -> SetupCheck {
    let is_ipv6 = server_internal_ip.contains(':');
    let cidr = if is_ipv6 {
        format!("{server_internal_ip}/128")
    } else {
        format!("{server_internal_ip}/32")
    };
    let args = if is_ipv6 {
        vec!["-6", "route", "replace", &cidr, "dev", CLIENT_INTERFACE]
    } else {
        vec!["route", "replace", &cidr, "dev", CLIENT_INTERFACE]
    };
    match run_command("ip", &args) {
        CommandOutcome::Success(_) => SetupCheck {
            name: String::from("Client WireGuard route"),
            status: SetupStatus::Pass,
            detail: format!("routed {cidr} through {CLIENT_INTERFACE}"),
            remedy: None,
        },
        CommandOutcome::Failed(detail) => SetupCheck {
            name: String::from("Client WireGuard route"),
            status: SetupStatus::Warn,
            detail,
            remedy: Some(String::from("verify iproute2 and CAP_NET_ADMIN/root")),
        },
    }
}

#[cfg(target_os = "linux")]
fn set_link_up(iface: &str) -> SetupCheck {
    match run_command("ip", &["link", "set", "up", "dev", iface]) {
        CommandOutcome::Success(_) => SetupCheck {
            name: String::from("WireGuard link"),
            status: SetupStatus::Pass,
            detail: format!("{iface} is up"),
            remedy: None,
        },
        CommandOutcome::Failed(detail) => SetupCheck {
            name: String::from("WireGuard link"),
            status: SetupStatus::Warn,
            detail,
            remedy: Some(String::from("verify iproute2 and CAP_NET_ADMIN/root")),
        },
    }
}

#[cfg(target_os = "linux")]
fn remove_link(iface: &str) -> SetupCheck {
    match run_command("ip", &["link", "delete", "dev", iface]) {
        CommandOutcome::Success(_) => SetupCheck {
            name: String::from("WireGuard link"),
            status: SetupStatus::Pass,
            detail: format!("removed {iface}"),
            remedy: None,
        },
        CommandOutcome::Failed(detail) if detail.contains("Cannot find device") => SetupCheck {
            name: String::from("WireGuard link"),
            status: SetupStatus::Warn,
            detail: format!("{iface} was not present"),
            remedy: None,
        },
        CommandOutcome::Failed(detail) => SetupCheck {
            name: String::from("WireGuard link"),
            status: SetupStatus::Warn,
            detail,
            remedy: None,
        },
    }
}

#[cfg(not(target_os = "linux"))]
fn remove_link(iface: &str) -> SetupCheck {
    SetupCheck {
        name: String::from("WireGuard link"),
        status: SetupStatus::Warn,
        detail: format!("{iface} removal is only available on Linux targets"),
        remedy: None,
    }
}

#[cfg(target_os = "linux")]
fn interface_name(name: &str) -> Result<InterfaceName> {
    name.parse::<InterfaceName>()
        .map_err(|error| anyhow::anyhow!("{error}"))
}

#[cfg(target_os = "linux")]
fn server_internal_cidr(subnet: &str) -> String {
    let prefix = subnet.split('/').next().unwrap_or("10.44.0.0");
    let mut octets = prefix.split('.').take(3).collect::<Vec<_>>();
    while octets.len() < 3 {
        octets.push("44");
    }
    format!("{}.{}.{}.1/24", octets[0], octets[1], octets[2])
}

#[cfg(target_os = "linux")]
fn server_internal_cidr_ipv6(subnet: &str) -> String {
    let prefix = subnet.split('/').next().unwrap_or("fd44::");
    let clean = prefix.trim().trim_end_matches(':');
    format!("{}::1/64", clean)
}

fn apply_nftables(control_port: u16, wireguard_port: u16) -> SetupCheck {
    if !command_exists("nft") {
        return SetupCheck {
            name: String::from("nftables firewall"),
            status: SetupStatus::Warn,
            detail: String::from("nft command not found; firewall rule was not applied"),
            remedy: Some(String::from(
                "install nftables when firewall enforcement is required",
            )),
        };
    }

    let _ = run_command("nft", &["delete", "table", "inet", "zeronode"]);
    let script = format!(
        "table inet zeronode {{\n  chain input {{\n    type filter hook input priority filter; policy accept;\n    udp dport {control_port} accept comment \"zeronode-control\"\n    udp dport {wireguard_port} accept comment \"zeronode-wireguard\"\n  }}\n}}\n"
    );
    match run_nft_script(&script) {
        CommandOutcome::Success(_) => SetupCheck {
            name: String::from("nftables firewall"),
            status: SetupStatus::Pass,
            detail: format!(
                "allowed UDP {control_port} and UDP {wireguard_port} in app-owned table inet zeronode"
            ),
            remedy: None,
        },
        CommandOutcome::Failed(detail) => SetupCheck {
            name: String::from("nftables firewall"),
            status: SetupStatus::Fail,
            detail,
            remedy: Some(String::from(
                "run nft list ruleset to inspect firewall state",
            )),
        },
    }
}

fn remove_nftables_table() -> SetupCheck {
    if !command_exists("nft") {
        return SetupCheck {
            name: String::from("nftables firewall"),
            status: SetupStatus::Warn,
            detail: String::from("nft command not found; nothing removed"),
            remedy: None,
        };
    }

    match run_command("nft", &["delete", "table", "inet", "zeronode"]) {
        CommandOutcome::Success(_) => SetupCheck {
            name: String::from("nftables firewall"),
            status: SetupStatus::Pass,
            detail: String::from("removed app-owned table inet zeronode"),
            remedy: None,
        },
        CommandOutcome::Failed(detail) if detail.contains("No such file") => SetupCheck {
            name: String::from("nftables firewall"),
            status: SetupStatus::Warn,
            detail: String::from("app-owned table inet zeronode was not present"),
            remedy: None,
        },
        CommandOutcome::Failed(detail) => SetupCheck {
            name: String::from("nftables firewall"),
            status: SetupStatus::Fail,
            detail,
            remedy: Some(String::from(
                "run nft list ruleset to inspect firewall state",
            )),
        },
    }
}

fn run_nft_script(script: &str) -> CommandOutcome {
    let mut child = match Command::new("nft")
        .arg("-f")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return CommandOutcome::Failed(format!("could not run nft: {error}")),
    };

    match child.stdin.as_mut() {
        Some(stdin) => {
            if let Err(error) = stdin.write_all(script.as_bytes()) {
                return CommandOutcome::Failed(format!("could not write nft rules: {error}"));
            }
        }
        None => return CommandOutcome::Failed(String::from("could not open nft stdin")),
    }

    match child.wait_with_output() {
        Ok(output) if output.status.success() => {
            CommandOutcome::Success(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        }
        Ok(output) => CommandOutcome::Failed(format!(
            "nft rule load failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => CommandOutcome::Failed(format!("could not wait for nft: {error}")),
    }
}

fn command_check(
    name: &str,
    outcomes: &[CommandOutcome],
    success_detail: &str,
    remedy: &str,
) -> SetupCheck {
    if let Some(failure) = outcomes.iter().find_map(|outcome| match outcome {
        CommandOutcome::Success(_) => None,
        CommandOutcome::Failed(detail) => Some(detail.clone()),
    }) {
        SetupCheck {
            name: name.to_owned(),
            status: SetupStatus::Fail,
            detail: failure,
            remedy: Some(remedy.to_owned()),
        }
    } else {
        let command_output = outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                CommandOutcome::Success(output) if !output.is_empty() => Some(output.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("; ");
        SetupCheck {
            name: name.to_owned(),
            status: SetupStatus::Pass,
            detail: if command_output.is_empty() {
                success_detail.to_owned()
            } else {
                format!("{success_detail}: {command_output}")
            },
            remedy: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vpn_suite_core::crypto::generate_key_material;

    #[test]
    fn host_setup_plan_mentions_configured_port() {
        let config = ServerConfig {
            server_id: String::from("server"),
            name: String::from("Node"),
            country_code: String::from("LAN"),
            country_name: String::from("Local"),
            listen_port: 51821,
            wireguard_port: 51822,
            vpn_subnet: String::from("10.44.0.0/24"),
            vpn_subnet_ipv6: String::from("fd44::/64"),
            openvpn_port: Some(1194),
            openvpn_endpoint: None,
            selected_global_ipv4: Vec::new(),
            selected_global_ipv6: Vec::new(),
            created_at_unix: 0,
            password_hash: None,
            wireguard_keys: generate_key_material(),
        };

        assert!(server_host_setup_plan(&config)
            .iter()
            .any(|step| step.contains("51821")));
    }
}
