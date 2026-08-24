mod elevation;
mod outline_tunnel;
mod pptp_tunnel;
mod silent_cmd;
mod tor_tunnel;
mod wireguard_tunnel;

pub use elevation::{
    exit_after_relaunch, is_elevated, relaunch_elevated, relaunch_elevated_with_args,
};
pub use outline_tunnel::{
    find_sslocal, is_outline_embedded, is_outline_running, outline_socks_port, start_outline,
    stop_outline,
};
pub use pptp_tunnel::{is_pptp_running, start_pptp, stop_pptp};
pub use silent_cmd::{
    clear_stale_wininet_socks_hint, kill_process_image, silent_command, silent_output,
    CREATE_NO_WINDOW,
};
pub use tor_tunnel::{
    is_tor_tunnel_running, start_socks_system_tunnel, start_tor_system_tunnel,
    stop_socks_system_tunnel, stop_tor_system_tunnel,
};
pub use wireguard_tunnel::{
    is_global_running as is_wireguard_running, parse_client_config as parse_wireguard_config,
    start_global as start_wireguard_global, stop_global as stop_wireguard_global,
    TunnelConfig as WireGuardTunnelConfig,
};

use anyhow::{bail, Context, Result};
use std::{env, fs, path::PathBuf, process::Command};
use vpn_suite_core::{
    app_paths::AppPaths,
    config::ServerConfig,
    model::ControlSessionLease,
    net_info::HostNetInfo,
    setup::{SetupCheck, SetupStatus},
};

const CLIENT_TUNNEL_SERVICE: &str = "WireGuardTunnel$ZeroNodeClient";
const CLIENT_TUNNEL_NAME: &str = "ZeroNodeClient";
const SERVER_TUNNEL_NAME: &str = "ZeroNodeServer";
const SERVER_TUNNEL_SERVICE: &str = "WireGuardTunnel$ZeroNodeServer";

#[derive(Clone, Debug)]
pub struct WindowsPlatformSummary {
    pub service_model: &'static str,
    pub tunnel_backend: &'static str,
    pub firewall_strategy: &'static str,
}

pub fn describe_server_platform(_config: &ServerConfig) -> WindowsPlatformSummary {
    WindowsPlatformSummary {
        service_model: "Direct elevated process launch with host-setup firewall prep",
        tunnel_backend: "official bundled wireguard.exe tunnel-service backend",
        firewall_strategy: "Windows Firewall rule via host-setup apply/remove",
    }
}

pub fn apply_server_tunnel(paths: &AppPaths, config: &ServerConfig) -> Vec<SetupCheck> {
    apply_server_tunnel_from_peer_files(paths, config)
}

pub fn apply_server_peer(
    paths: &AppPaths,
    config: &ServerConfig,
    lease: &ControlSessionLease,
) -> SetupCheck {
    let peer_path = paths
        .profiles_dir
        .join(format!("server-peer-{}.conf", lease.session_id));

    let mut allowed_ips = vec![format!("{}/32", lease.reserved_client_ip)];
    if let Some(ipv6) = lease.reserved_client_ipv6.as_ref() {
        allowed_ips.push(format!("{}/128", ipv6));
    }
    let contents = vpn_suite_core::wireguard::render_server_peer_config(
        &lease.client_public_key,
        &allowed_ips,
    );
    if let Err(e) = fs::write(&peer_path, contents) {
        return SetupCheck {
            name: String::from("Windows server peer apply"),
            status: SetupStatus::Fail,
            detail: format!("failed to write peer conf file: {e}"),
            remedy: None,
        };
    }

    match apply_peer_live(lease) {
        Ok(()) => SetupCheck {
            name: String::from("Windows server peer apply"),
            status: SetupStatus::Pass,
            detail: format!(
                "peer key {} applied live successfully",
                lease.client_public_key
            ),
            remedy: None,
        },
        Err(e) => {
            let fallback = collapse_checks(
                "Windows server peer apply",
                apply_server_tunnel_from_peer_files(paths, config),
            );
            if fallback.status == SetupStatus::Pass {
                SetupCheck {
                    name: String::from("Windows server peer apply"),
                    status: SetupStatus::Pass,
                    detail: format!(
                        "live peer apply failed ({e}); rebuilt server tunnel service with peer config"
                    ),
                    remedy: None,
                }
            } else {
                SetupCheck {
                    name: String::from("Windows server peer apply"),
                    status: fallback.status,
                    detail: format!("live apply failed ({e}); fallback: {}", fallback.detail),
                    remedy: fallback.remedy,
                }
            }
        }
    }
}

pub fn remove_server_peer(
    paths: &AppPaths,
    config: &ServerConfig,
    lease: &ControlSessionLease,
) -> SetupCheck {
    let peer_path = paths
        .profiles_dir
        .join(format!("server-peer-{}.conf", lease.session_id));
    let _ = fs::remove_file(peer_path);

    match remove_peer_live(&lease.client_public_key) {
        Ok(()) => SetupCheck {
            name: String::from("Windows server peer remove"),
            status: SetupStatus::Pass,
            detail: format!(
                "peer key {} removed live successfully",
                lease.client_public_key
            ),
            remedy: None,
        },
        Err(e) => {
            let fallback = collapse_checks(
                "Windows server peer remove",
                apply_server_tunnel_from_peer_files(paths, config),
            );
            if fallback.status == SetupStatus::Pass {
                SetupCheck {
                    name: String::from("Windows server peer remove"),
                    status: SetupStatus::Pass,
                    detail: format!(
                        "live peer remove failed ({e}); rebuilt server tunnel service without peer config"
                    ),
                    remedy: None,
                }
            } else {
                SetupCheck {
                    name: String::from("Windows server peer remove"),
                    status: fallback.status,
                    detail: format!("live remove failed ({e}); fallback: {}", fallback.detail),
                    remedy: fallback.remedy,
                }
            }
        }
    }
}

pub fn server_tunnel_status() -> SetupCheck {
    service_running_check(
        "Windows server WireGuard tunnel status",
        SERVER_TUNNEL_SERVICE,
        SetupStatus::Warn,
        "run vpn-server.exe host-setup apply from an elevated Administrator terminal",
    )
}

pub fn apply_client_tunnel_service(profile_path: Option<&str>) -> Vec<SetupCheck> {
    let Some(profile_path) = profile_path else {
        return vec![SetupCheck {
            name: String::from("Windows WireGuard tunnel service"),
            status: SetupStatus::Fail,
            detail: String::from("no cached tunnel profile path; run vpn-client connect first"),
            remedy: Some(String::from(
                "run vpn-client connect, then vpn-client tunnel-apply",
            )),
        }];
    };

    let mut checks = Vec::new();

    // Match the normal Windows VPN model: install/start a real WireGuard tunnel
    // service, which owns the adapter, routes, DNS, and packet engine.
    checks.extend(wireguard_asset_checks());

    if let Some(wireguard_exe) = bundled_wireguard_exe() {
        let prepared_profile = match prepare_windows_client_profile(profile_path) {
            Ok(path) => {
                checks.push(SetupCheck {
                    name: String::from("Windows WireGuard profile"),
                    status: SetupStatus::Pass,
                    detail: format!("prepared stable tunnel config at {path}"),
                    remedy: None,
                });
                path
            }
            Err(error) => {
                checks.push(SetupCheck {
                    name: String::from("Windows WireGuard profile"),
                    status: SetupStatus::Fail,
                    detail: format!("could not prepare stable tunnel config: {error}"),
                    remedy: Some(String::from(
                        "rerun from an elevated Administrator terminal",
                    )),
                });
                return checks;
            }
        };

        let exe = wireguard_exe.to_string_lossy().to_string();
        let _ = wireguard_tunnel::stop_global();
        let _ = run_command(&exe, &["/uninstalltunnelservice", CLIENT_TUNNEL_NAME]);
        std::thread::sleep(std::time::Duration::from_millis(500));

        let install_outcome = run_command(&exe, &["/installtunnelservice", &prepared_profile]);
        checks.push(command_check_with_failure_status(
            "Windows WireGuard tunnel install",
            install_outcome,
            "official WireGuard tunnel service installed and started",
            "rerun from an elevated Administrator terminal; ensure the official WireGuard client is installed",
            SetupStatus::Fail,
        ));

        std::thread::sleep(std::time::Duration::from_millis(1000));
        checks.push(service_running_check(
            "Windows WireGuard tunnel status",
            CLIENT_TUNNEL_SERVICE,
            SetupStatus::Fail,
            "the tunnel service was installed but is not running; inspect Windows Services or WireGuard logs",
        ));
    } else {
        checks.push(SetupCheck {
            name: String::from("Windows WireGuard executable"),
            status: SetupStatus::Warn,
            detail: String::from("wireguard.exe is not bundled; trying embedded fallback"),
            remedy: Some(String::from(
                "run tools/build-windows.ps1 to stage wireguard.exe",
            )),
        });

        if wireguard_tunnel::is_wintun_available() {
            match try_embedded_tunnel(profile_path) {
                Ok(embedded_checks) => {
                    checks.extend(embedded_checks);
                    return checks;
                }
                Err(e) => {
                    tracing::warn!("embedded tunnel failed: {e:#}");
                    checks.push(SetupCheck {
                        name: String::from("Embedded WireGuard tunnel"),
                        status: SetupStatus::Fail,
                        detail: format!("embedded tunnel unavailable: {e}"),
                        remedy: Some(String::from(
                            "bundle wireguard.exe or install the official WireGuard client",
                        )),
                    });
                }
            }
        }

        checks.push(SetupCheck {
            name: String::from("Windows WireGuard tunnel"),
            status: SetupStatus::Fail,
            detail: String::from(
                "no tunnel backend available: wintun driver not found and wireguard.exe not bundled. \
                 Install the Wintun driver (comes with WireGuard for Windows) or bundle wireguard.exe.",
            ),
            remedy: Some(String::from(
                "install WireGuard for Windows from wireguard.com, or run as Administrator",
            )),
        });
    }

    checks
}

/// Try to start the tunnel using the embedded boringtun+wintun implementation.
fn try_embedded_tunnel(profile_path: &str) -> Result<Vec<SetupCheck>> {
    let mut checks = Vec::new();

    // Read the profile file
    let contents = std::fs::read_to_string(profile_path)
        .with_context(|| format!("failed to read tunnel profile: {profile_path}"))?;

    checks.push(SetupCheck {
        name: String::from("Embedded WireGuard"),
        status: SetupStatus::Pass,
        detail: String::from("using built-in boringtun + wintun tunnel (no wireguard.exe needed)"),
        remedy: None,
    });

    // Parse the config
    let config = wireguard_tunnel::parse_client_config(&contents)
        .context("failed to parse WireGuard client config")?;

    checks.push(SetupCheck {
        name: String::from("WireGuard config parse"),
        status: SetupStatus::Pass,
        detail: format!(
            "endpoint={} tunnel_ip={} allowed_ips={}",
            config.server_endpoint,
            config.tunnel_ip,
            config.allowed_ips.join(", ")
        ),
        remedy: None,
    });

    // Check wintun availability
    if !wireguard_tunnel::is_wintun_available() {
        bail!("wintun driver not available");
    }

    checks.push(SetupCheck {
        name: String::from("Wintun driver"),
        status: SetupStatus::Pass,
        detail: String::from("wintun driver loaded successfully"),
        remedy: None,
    });

    // Create and start the tunnel via the global handle
    wireguard_tunnel::start_global(config).context("failed to start WireGuard tunnel")?;

    checks.push(SetupCheck {
        name: String::from("WireGuard tunnel"),
        status: SetupStatus::Pass,
        detail: String::from("embedded WireGuard tunnel started successfully"),
        remedy: None,
    });

    Ok(checks)
}

pub fn remove_client_tunnel_service() -> Vec<SetupCheck> {
    let mut checks = Vec::new();

    // Stop the embedded tunnel via the global handle
    match wireguard_tunnel::stop_global() {
        Ok(()) => {
            checks.push(SetupCheck {
                name: String::from("Embedded WireGuard tunnel"),
                status: SetupStatus::Pass,
                detail: if wireguard_tunnel::is_global_running() {
                    String::from("tunnel still shutting down")
                } else {
                    String::from("embedded tunnel stopped and routes removed")
                },
                remedy: None,
            });
        }
        Err(e) => {
            checks.push(SetupCheck {
                name: String::from("Embedded WireGuard tunnel"),
                status: SetupStatus::Warn,
                detail: format!("embedded tunnel stop error: {e}"),
                remedy: None,
            });
        }
    }

    // Also try wireguard.exe cleanup if available
    if let Some(wireguard_exe) = bundled_wireguard_exe() {
        let exe = wireguard_exe.to_string_lossy().to_string();
        checks.push(command_check(
            "Windows WireGuard tunnel uninstall",
            run_command(&exe, &["/uninstalltunnelservice", "ZeroNodeClient"]),
            "official WireGuard tunnel service removed",
            "run from an elevated Administrator terminal",
        ));
    }
    checks.push(stop_named_service(
        CLIENT_TUNNEL_SERVICE,
        "Windows WireGuard tunnel stop",
    ));
    checks.push(delete_named_service(
        CLIENT_TUNNEL_SERVICE,
        "Windows WireGuard tunnel delete",
    ));
    checks
}

pub fn client_tunnel_service_status() -> SetupCheck {
    if wireguard_tunnel::is_global_running() {
        return SetupCheck {
            name: String::from("Windows WireGuard tunnel status"),
            status: SetupStatus::Pass,
            detail: String::from("embedded WireGuard tunnel is active"),
            remedy: None,
        };
    }

    service_running_check(
        "Windows WireGuard tunnel status",
        CLIENT_TUNNEL_SERVICE,
        SetupStatus::Warn,
        "run vpn-client tunnel-apply from an elevated Administrator terminal",
    )
}

pub fn wireguard_asset_checks() -> Vec<SetupCheck> {
    let exe_dir = current_exe_dir();
    let Some(exe_dir) = exe_dir else {
        return vec![SetupCheck {
            name: String::from("Windows WireGuard assets"),
            status: SetupStatus::Fail,
            detail: String::from("could not resolve current executable directory"),
            remedy: Some(String::from(
                "run from the packaged dist/windows/bin directory",
            )),
        }];
    };

    let wireguard_exe = exe_dir.join("wireguard.exe");
    let wg_exe = exe_dir.join("wg.exe");
    let mut checks = Vec::new();

    checks.push(if wireguard_exe.exists() {
        SetupCheck {
            name: String::from("Windows WireGuard executable"),
            status: SetupStatus::Pass,
            detail: format!("found {}", wireguard_exe.display()),
            remedy: None,
        }
    } else {
        SetupCheck {
            name: String::from("Windows WireGuard executable"),
            status: SetupStatus::Fail,
            detail: format!("missing {}", wireguard_exe.display()),
            remedy: Some(String::from(
                "bundle official WireGuard wireguard.exe beside vpn-client.exe",
            )),
        }
    });

    checks.push(if wg_exe.exists() {
        SetupCheck {
            name: String::from("Windows WireGuard control executable"),
            status: SetupStatus::Pass,
            detail: format!("found {}", wg_exe.display()),
            remedy: None,
        }
    } else {
        SetupCheck {
            name: String::from("Windows WireGuard control executable"),
            status: SetupStatus::Fail,
            detail: format!("missing {}", wg_exe.display()),
            remedy: Some(String::from(
                "bundle official WireGuard wg.exe beside vpn-server.exe",
            )),
        }
    });

    checks
}

pub fn run_wireguard_tunnel_service_if_requested() -> anyhow::Result<Option<i32>> {
    let args = env::args_os().collect::<Vec<_>>();
    if args.len() != 3 || args.get(1).and_then(|arg| arg.to_str()) != Some("/service") {
        return Ok(None);
    }

    #[cfg(target_os = "windows")]
    {
        run_wireguard_tunnel_service(&args[2]).map(Some)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = args;
        Ok(Some(1))
    }
}

pub fn server_host_setup_plan(config: &ServerConfig) -> Vec<String> {
    let local_ips = firewall_local_ips(config);
    let local_ip_summary = if local_ips.is_empty() {
        String::from("auto-detected selected public addresses")
    } else {
        local_ips.join(", ")
    };
    vec![
        format!(
            "Open Windows Firewall UDP {} control and UDP {} WireGuard traffic only on {}",
            config.listen_port, config.wireguard_port, local_ip_summary
        ),
        String::from(
            "Use the elevated client or server dashboard to launch vpn-server.exe directly",
        ),
    ]
}

pub fn apply_server_host_setup(config: &ServerConfig) -> Vec<SetupCheck> {
    let local_ips = firewall_local_ips(config);
    let mut checks = Vec::new();

    checks.push(enable_ip_forwarding());
    checks.push(enable_ip_forwarding_ipv6());
    checks.push(enable_interface_forwarding());
    checks.push(enable_nat(&config.vpn_subnet));

    checks.push(add_firewall_rule(
        "ZeroNode VPN Control",
        config.listen_port,
        "ZeroNode VPN control plane",
        &local_ips,
    ));
    checks.push(add_firewall_rule(
        "ZeroNode VPN WireGuard",
        config.wireguard_port,
        "ZeroNode VPN WireGuard data plane",
        &local_ips,
    ));

    checks.extend(wireguard_asset_checks());
    checks
}

pub fn remove_server_host_setup(config: &ServerConfig) -> Vec<SetupCheck> {
    vec![
        delete_firewall_rule("ZeroNode VPN Control", config.listen_port),
        delete_firewall_rule("ZeroNode VPN WireGuard", config.wireguard_port),
        uninstall_wireguard_tunnel(SERVER_TUNNEL_NAME, "Windows server WireGuard uninstall"),
        disable_nat(),
    ]
}

fn enable_nat(subnet: &str) -> SetupCheck {
    let create_cmd = format!(
        "$nat = Get-NetNat -Name 'ZeroNodeVPN-NAT' -ErrorAction SilentlyContinue; \
         if ($nat -and $nat.InternalIPInterfaceAddressPrefix -ne '{0}') {{ \
             Remove-NetNat -Name 'ZeroNodeVPN-NAT' -Confirm:$false; $nat = $null \
         }}; \
         if (-not $nat) {{ New-NetNat -Name 'ZeroNodeVPN-NAT' -InternalIPInterfaceAddressPrefix '{0}' | Out-Null }}; \
         Get-NetNat -Name 'ZeroNodeVPN-NAT' | Select-Object -ExpandProperty InternalIPInterfaceAddressPrefix",
        subnet
    );
    command_check_with_failure_status(
        "Windows NetNat translation setup",
        run_command("powershell.exe", &["-NoProfile", "-Command", &create_cmd]),
        "NetNat configured successfully for NAT routing",
        "run from an elevated Administrator terminal",
        SetupStatus::Fail,
    )
}

fn disable_nat() -> SetupCheck {
    let remove_cmd = "if (Get-NetNat -Name 'ZeroNodeVPN-NAT' -ErrorAction SilentlyContinue) { Remove-NetNat -Name 'ZeroNodeVPN-NAT' -Confirm:$false }";
    command_check(
        "Windows NetNat translation removal",
        run_command("powershell.exe", &["-NoProfile", "-Command", remove_cmd]),
        "NetNat removed successfully",
        "run from an elevated Administrator terminal",
    )
}

fn enable_ip_forwarding() -> SetupCheck {
    command_check_with_failure_status(
        "Windows IPv4 forwarding",
        run_command(
            "reg.exe",
            &[
                "add",
                r"HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters",
                "/v",
                "IPEnableRouter",
                "/t",
                "REG_DWORD",
                "/d",
                "1",
                "/f",
            ],
        ),
        "IPv4 forwarding (IPEnableRouter) enabled in registry",
        "rerun from an elevated Administrator terminal",
        SetupStatus::Fail,
    )
}

fn enable_ip_forwarding_ipv6() -> SetupCheck {
    command_check_with_failure_status(
        "Windows IPv6 forwarding",
        run_command(
            "reg.exe",
            &[
                "add",
                r"HKLM\SYSTEM\CurrentControlSet\Services\Tcpip6\Parameters",
                "/v",
                "IPEnableRouter",
                "/t",
                "REG_DWORD",
                "/d",
                "1",
                "/f",
            ],
        ),
        "IPv6 forwarding (IPEnableRouter) enabled in registry",
        "rerun from an elevated Administrator terminal",
        SetupStatus::Warn,
    )
}

fn enable_interface_forwarding() -> SetupCheck {
    let command = "Set-NetIPInterface -AddressFamily IPv4 -Forwarding Enabled; \
                   Get-NetIPInterface -AddressFamily IPv4 | \
                   Where-Object { $_.Forwarding -eq 'Enabled' } | \
                   Select-Object -First 1 -ExpandProperty InterfaceAlias";
    command_check_with_failure_status(
        "Windows interface packet forwarding",
        run_command("powershell.exe", &["-NoProfile", "-Command", command]),
        "IPv4 packet forwarding enabled on Windows interfaces",
        "rerun from an elevated Administrator terminal",
        SetupStatus::Fail,
    )
}

fn enable_tunnel_interface_forwarding(tunnel_name: &str, check_name: &str) -> SetupCheck {
    let command = format!(
        "$iface = Get-NetIPInterface -InterfaceAlias '{0}' -AddressFamily IPv4 -ErrorAction SilentlyContinue; \
         if ($iface) {{ Set-NetIPInterface -InterfaceAlias '{0}' -AddressFamily IPv4 -Forwarding Enabled; 'enabled {0}' }} \
         else {{ throw 'WireGuard tunnel adapter {0} was not found' }}",
        tunnel_name
    );
    command_check_with_failure_status(
        check_name,
        run_command("powershell.exe", &["-NoProfile", "-Command", &command]),
        "tunnel interface forwarding enabled",
        "ensure the WireGuard tunnel service is running and rerun host setup",
        SetupStatus::Warn,
    )
}

fn stop_named_service(service: &str, name: &str) -> SetupCheck {
    command_check(
        name,
        run_command("sc.exe", &["stop", service]),
        "service stop requested",
        "run from an elevated Administrator terminal",
    )
}

fn delete_named_service(service: &str, name: &str) -> SetupCheck {
    command_check(
        name,
        run_command("sc.exe", &["delete", service]),
        "service removed",
        "run from an elevated Administrator terminal",
    )
}

fn add_firewall_rule(
    name: &str,
    port: u16,
    description: &str,
    _local_ips: &[String],
) -> SetupCheck {
    let port_text = port.to_string();
    let _ = delete_firewall_rule(name, port);

    let mut all_ok = true;
    let mut detail_parts = Vec::new();

    // Create separate rules for IPv4 and IPv6 to avoid netsh issues
    // with mixed address families in a single localip= parameter.
    for (suffix, profile) in [("IPv4", "any"), ("IPv6", "any")] {
        let rule_name = format!("{name} {suffix}");
        let name_arg = format!("name={rule_name}");
        let port_arg = format!("localport={port_text}");
        let desc_arg = format!("description={description} ({suffix})");
        let profile_arg = format!("profile={profile}");
        let args = vec![
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &name_arg,
            "dir=in",
            "action=allow",
            "protocol=UDP",
            &port_arg,
            &desc_arg,
            &profile_arg,
        ];
        match run_command("netsh.exe", &args) {
            CommandOutcome::Success(msg) => {
                detail_parts.push(format!("{suffix}: ok"));
                let _ = msg;
            }
            CommandOutcome::Failed(msg) => {
                detail_parts.push(format!("{suffix}: {msg}"));
                all_ok = false;
            }
        }
    }

    SetupCheck {
        name: String::from(name),
        status: if all_ok {
            SetupStatus::Pass
        } else {
            SetupStatus::Fail
        },
        detail: format!(
            "firewall rule allows UDP {} — {}",
            port,
            detail_parts.join("; ")
        ),
        remedy: if all_ok {
            None
        } else {
            Some(String::from(
                "rerun from an elevated Administrator terminal",
            ))
        },
    }
}

fn firewall_local_ips(config: &ServerConfig) -> Vec<String> {
    let net_info = HostNetInfo::query();
    let mut selected = net_info.effective_selected_global_ipv4(&config.selected_global_ipv4);
    selected.extend(net_info.effective_selected_global_ipv6(&config.selected_global_ipv6));
    selected.sort();
    selected.dedup();
    selected
}

fn delete_firewall_rule(name: &str, port: u16) -> SetupCheck {
    // Delete both the old combined rule and the new per-family rules
    let _ = run_command(
        "netsh.exe",
        &[
            "advfirewall",
            "firewall",
            "delete",
            "rule",
            &format!("name={name}"),
            "protocol=UDP",
            &format!("localport={port}"),
        ],
    );
    for suffix in ["IPv4", "IPv6"] {
        let _ = run_command(
            "netsh.exe",
            &[
                "advfirewall",
                "firewall",
                "delete",
                "rule",
                &format!("name={name} {suffix}"),
                "protocol=UDP",
                &format!("localport={port}"),
            ],
        );
    }
    SetupCheck {
        name: String::from(name),
        status: SetupStatus::Pass,
        detail: format!("removed firewall rule for UDP {port}"),
        remedy: None,
    }
}

#[derive(Debug)]
enum CommandOutcome {
    Success(String),
    Failed(String),
}

fn run_command(program: &str, args: &[&str]) -> CommandOutcome {
    match command_no_window(program).args(args).output() {
        Ok(output) if output.status.success() => {
            CommandOutcome::Success(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        }
        Ok(output) => CommandOutcome::Failed(format!(
            "{} {} failed: {}",
            program,
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => CommandOutcome::Failed(format!("could not run {program}: {error}")),
    }
}

fn command_no_window(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn command_check(
    name: &str,
    outcome: CommandOutcome,
    success_detail: &str,
    remedy: &str,
) -> SetupCheck {
    command_check_with_failure_status(name, outcome, success_detail, remedy, SetupStatus::Warn)
}

fn command_check_with_failure_status(
    name: &str,
    outcome: CommandOutcome,
    success_detail: &str,
    remedy: &str,
    failure_status: SetupStatus,
) -> SetupCheck {
    match outcome {
        CommandOutcome::Success(output) => SetupCheck {
            name: name.to_owned(),
            status: SetupStatus::Pass,
            detail: if output.is_empty() {
                success_detail.to_owned()
            } else {
                format!("{success_detail}: {output}")
            },
            remedy: None,
        },
        CommandOutcome::Failed(detail) => SetupCheck {
            name: name.to_owned(),
            status: failure_status,
            detail,
            remedy: Some(remedy.to_owned()),
        },
    }
}

fn service_running_check(
    name: &str,
    service: &str,
    missing_or_stopped_status: SetupStatus,
    remedy: &str,
) -> SetupCheck {
    match run_command("sc.exe", &["query", service]) {
        CommandOutcome::Success(output) if output.contains("RUNNING") => SetupCheck {
            name: name.to_owned(),
            status: SetupStatus::Pass,
            detail: format!("{service} is running"),
            remedy: None,
        },
        CommandOutcome::Success(output) => SetupCheck {
            name: name.to_owned(),
            status: missing_or_stopped_status,
            detail: format!(
                "{service} exists but is not running: {}",
                compact_sc_output(&output)
            ),
            remedy: Some(remedy.to_owned()),
        },
        CommandOutcome::Failed(detail) => SetupCheck {
            name: name.to_owned(),
            status: missing_or_stopped_status,
            detail,
            remedy: Some(remedy.to_owned()),
        },
    }
}

fn compact_sc_output(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn current_exe_dir() -> Option<PathBuf> {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(ToOwned::to_owned))
}

fn bundled_wireguard_exe() -> Option<PathBuf> {
    let path = current_exe_dir()?.join("wireguard.exe");
    path.exists().then_some(path)
}

fn bundled_wg_exe() -> Option<PathBuf> {
    let path = current_exe_dir()?.join("wg.exe");
    path.exists().then_some(path)
}

fn apply_peer_live(lease: &ControlSessionLease) -> Result<()> {
    let wg_exe = bundled_wg_exe().context("wg.exe not found")?;
    let mut allowed_ips = vec![format!("{}/32", lease.reserved_client_ip)];
    if let Some(ipv6) = lease.reserved_client_ipv6.as_ref() {
        allowed_ips.push(format!("{}/128", ipv6));
    }
    let allowed_ips_str = allowed_ips.join(",");
    let output = command_no_window(wg_exe)
        .args([
            "set",
            SERVER_TUNNEL_NAME,
            "peer",
            &lease.client_public_key,
            "allowed-ips",
            &allowed_ips_str,
            "persistent-keepalive",
            "25",
        ])
        .output()
        .context("failed to run wg.exe set peer")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        bail!("wg.exe set peer failed: {}", err.trim());
    }
    Ok(())
}

fn remove_peer_live(client_public_key: &str) -> Result<()> {
    let wg_exe = bundled_wg_exe().context("wg.exe not found")?;
    let output = command_no_window(wg_exe)
        .args([
            "set",
            SERVER_TUNNEL_NAME,
            "peer",
            client_public_key,
            "remove",
        ])
        .output()
        .context("failed to run wg.exe remove peer")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        bail!("wg.exe remove peer failed: {}", err.trim());
    }
    Ok(())
}

fn apply_server_tunnel_from_peer_files(paths: &AppPaths, config: &ServerConfig) -> Vec<SetupCheck> {
    let mut checks = wireguard_asset_checks();
    let profile = match write_server_tunnel_profile(paths, config) {
        Ok(path) => {
            checks.push(SetupCheck {
                name: String::from("Windows server WireGuard profile"),
                status: SetupStatus::Pass,
                detail: format!("wrote {path}"),
                remedy: None,
            });
            path
        }
        Err(error) => {
            checks.push(SetupCheck {
                name: String::from("Windows server WireGuard profile"),
                status: SetupStatus::Fail,
                detail: format!("could not write server tunnel profile: {error:#}"),
                remedy: Some(String::from("check ProgramData permissions")),
            });
            return checks;
        }
    };

    let Some(wireguard_exe) = bundled_wireguard_exe() else {
        checks.push(SetupCheck {
            name: String::from("Windows server WireGuard install"),
            status: SetupStatus::Fail,
            detail: String::from("wireguard.exe is not bundled beside vpn-server.exe"),
            remedy: Some(String::from(
                "run tools/build-windows.ps1 to stage the Windows bundle",
            )),
        });
        return checks;
    };

    let exe = wireguard_exe.to_string_lossy().to_string();
    let _ = run_command(&exe, &["/uninstalltunnelservice", SERVER_TUNNEL_NAME]);
    std::thread::sleep(std::time::Duration::from_millis(500));

    let install_outcome = run_command(&exe, &["/installtunnelservice", &profile]);
    checks.push(command_check_with_failure_status(
        "Windows server WireGuard install",
        install_outcome,
        "official WireGuard server tunnel service installed and started",
        "rerun from an elevated Administrator terminal or Windows service account",
        SetupStatus::Fail,
    ));

    std::thread::sleep(std::time::Duration::from_millis(1000));
    checks.push(service_running_check(
        "Windows server WireGuard tunnel status",
        SERVER_TUNNEL_SERVICE,
        SetupStatus::Fail,
        "inspect Windows Services and WireGuard tunnel logs",
    ));
    checks.push(enable_tunnel_interface_forwarding(
        SERVER_TUNNEL_NAME,
        "Windows server tunnel forwarding",
    ));
    checks
}

fn write_server_tunnel_profile(paths: &AppPaths, config: &ServerConfig) -> anyhow::Result<String> {
    let base = env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    let dir = base.join("ZeroNode").join("server");
    fs::create_dir_all(&dir)?;
    let destination = dir.join(format!("{SERVER_TUNNEL_NAME}.conf"));

    let server_internal_ip_v4 = server_internal_ip(&config.vpn_subnet)?;
    let server_internal_ip_v6 = server_internal_ipv6(&config.vpn_subnet_ipv6)?;

    let mut contents = format!(
        "[Interface]\nPrivateKey = {}\nAddress = {}/24, {}/64\nListenPort = {}\n",
        config.wireguard_keys.private_key,
        server_internal_ip_v4,
        server_internal_ip_v6,
        config.wireguard_port
    );

    for peer in server_peer_files(paths)? {
        let peer_contents = fs::read_to_string(&peer)?;
        contents.push('\n');
        contents.push_str(peer_contents.trim());
        contents.push('\n');
    }

    fs::write(&destination, contents)?;
    Ok(destination.to_string_lossy().to_string())
}

fn server_peer_files(paths: &AppPaths) -> anyhow::Result<Vec<PathBuf>> {
    let mut peers = Vec::new();
    if paths.profiles_dir.exists() {
        for entry in fs::read_dir(&paths.profiles_dir)? {
            let entry = entry?;
            let path = entry.path();
            let is_peer = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("server-peer-") && name.ends_with(".conf"))
                .unwrap_or(false);
            if is_peer {
                peers.push(path);
            }
        }
    }
    peers.sort();
    Ok(peers)
}

fn server_internal_ip(subnet: &str) -> anyhow::Result<String> {
    let (base, mask) = subnet
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("vpn subnet must be CIDR"))?;
    if mask != "24" {
        anyhow::bail!("only /24 subnets are supported");
    }
    let octets = base.parse::<std::net::Ipv4Addr>()?.octets();
    Ok(format!("{}.{}.{}.1", octets[0], octets[1], octets[2]))
}

fn server_internal_ipv6(subnet: &str) -> anyhow::Result<String> {
    let (base, mask) = subnet
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("vpn subnet ipv6 must be CIDR"))?;
    if mask != "64" {
        anyhow::bail!("only /64 subnets are supported");
    }
    let clean = base.trim().trim_end_matches(':');
    Ok(format!("{clean}::1"))
}

fn prepare_windows_client_profile(source: &str) -> anyhow::Result<String> {
    let base = env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    let dir = base.join("ZeroNode").join("client");
    fs::create_dir_all(&dir)?;
    let destination = dir.join("ZeroNodeClient.conf");
    fs::copy(source, &destination)?;
    Ok(destination.to_string_lossy().to_string())
}

fn uninstall_wireguard_tunnel(tunnel_name: &str, check_name: &str) -> SetupCheck {
    if let Some(wireguard_exe) = bundled_wireguard_exe() {
        let exe = wireguard_exe.to_string_lossy().to_string();
        command_check(
            check_name,
            run_command(&exe, &["/uninstalltunnelservice", tunnel_name]),
            "official WireGuard tunnel service removed",
            "run from an elevated Administrator terminal",
        )
    } else {
        SetupCheck {
            name: check_name.to_owned(),
            status: SetupStatus::Warn,
            detail: String::from("wireguard.exe is not bundled"),
            remedy: Some(String::from(
                "run tools/build-windows.ps1 to stage the Windows bundle",
            )),
        }
    }
}

fn collapse_checks(name: &str, checks: Vec<SetupCheck>) -> SetupCheck {
    let failed = checks
        .iter()
        .find(|check| matches!(check.status, SetupStatus::Fail));
    if let Some(check) = failed {
        return SetupCheck {
            name: name.to_owned(),
            status: SetupStatus::Fail,
            detail: format!("{}: {}", check.name, check.detail),
            remedy: check.remedy.clone(),
        };
    }

    let warned = checks
        .iter()
        .find(|check| matches!(check.status, SetupStatus::Warn));
    if let Some(check) = warned {
        return SetupCheck {
            name: name.to_owned(),
            status: SetupStatus::Warn,
            detail: format!("{}: {}", check.name, check.detail),
            remedy: check.remedy.clone(),
        };
    }

    SetupCheck {
        name: name.to_owned(),
        status: SetupStatus::Pass,
        detail: String::from("server tunnel profile applied"),
        remedy: None,
    }
}

#[cfg(target_os = "windows")]
fn run_wireguard_tunnel_service(profile_path: &std::ffi::OsStr) -> anyhow::Result<i32> {
    use std::os::windows::ffi::OsStrExt;

    let tunnel_path = current_exe_dir()
        .ok_or_else(|| anyhow::anyhow!("could not resolve current executable directory"))?
        .join("tunnel.dll");
    let library = unsafe {
        // SAFETY: Loading the official WireGuard embeddable tunnel.dll from the packaged
        // executable directory. The symbol signature below matches the upstream README.
        libloading::Library::new(&tunnel_path)
    }
    .map_err(|error| anyhow::anyhow!("failed to load {}: {error}", tunnel_path.display()))?;

    let tunnel_service = unsafe {
        // SAFETY: WireGuardTunnelService is exported by official tunnel.dll and expects
        // a null-terminated UTF-16 config path. The library is kept alive for the call.
        library.get::<unsafe extern "C" fn(*const u16) -> i32>(b"WireGuardTunnelService\0")
    }
    .map_err(|error| anyhow::anyhow!("failed to resolve WireGuardTunnelService: {error}"))?;

    let mut wide = profile_path.encode_wide().collect::<Vec<_>>();
    wide.push(0);
    let result = unsafe {
        // SAFETY: The pointer is valid for the duration of the call and null-terminated.
        tunnel_service(wide.as_ptr())
    };
    Ok(if result == 0 { 1 } else { 0 })
}
