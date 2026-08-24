use crate::{
    app_paths::AppPaths,
    config::{ClientConfig, ServerConfig},
};
use std::{fmt::Write as _, fs, net::UdpSocket};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Clone, Debug)]
pub struct SetupCheck {
    pub name: String,
    pub status: SetupStatus,
    pub detail: String,
    pub remedy: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SetupReport {
    pub role: String,
    pub os: String,
    pub checks: Vec<SetupCheck>,
}

impl SetupReport {
    pub fn is_ready(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.status != SetupStatus::Fail)
    }
}

pub fn server_setup_report(paths: &AppPaths, config: &ServerConfig) -> SetupReport {
    let mut checks = common_checks(paths, config.wireguard_keys.is_complete());
    checks.push(subnet_check(&config.vpn_subnet));
    checks.push(server_port_check("Control UDP port", config.listen_port));
    checks.push(server_port_check(
        "WireGuard UDP port",
        config.wireguard_port,
    ));
    checks.push(SetupCheck {
        name: String::from("Password policy"),
        status: SetupStatus::Pass,
        detail: if config.password_hash.is_some() {
            String::from("password protection is enabled")
        } else {
            String::from("password protection is disabled for this node")
        },
        remedy: None,
    });

    SetupReport {
        role: String::from("server"),
        os: std::env::consts::OS.to_owned(),
        checks,
    }
}

pub fn client_setup_report(paths: &AppPaths, config: &ClientConfig) -> SetupReport {
    let keys_ready = config
        .wireguard_keys
        .as_ref()
        .map(|keys| keys.is_complete())
        .unwrap_or(false);
    let mut checks = common_checks(paths, keys_ready);
    checks.push(SetupCheck {
        name: String::from("Known hosts"),
        status: if config.known_hosts.is_empty() {
            SetupStatus::Warn
        } else {
            SetupStatus::Pass
        },
        detail: if config.known_hosts.is_empty() {
            String::from("no manual hosts saved; LAN broadcast discovery is still available")
        } else {
            format!("{} manual host(s) saved", config.known_hosts.len())
        },
        remedy: Some(String::from(
            "add a host with vpn-client add-host <ip-or-hostname>",
        )),
    });

    SetupReport {
        role: String::from("client"),
        os: std::env::consts::OS.to_owned(),
        checks,
    }
}

pub fn format_setup_report(report: &SetupReport) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "Role: {}", report.role);
    let _ = writeln!(output, "OS: {}", report.os);
    let _ = writeln!(
        output,
        "Ready: {}",
        if report.is_ready() { "yes" } else { "no" }
    );
    for check in &report.checks {
        let _ = writeln!(
            output,
            "[{}] {} - {}",
            status_label(&check.status),
            check.name,
            check.detail
        );
        if let Some(remedy) = check.remedy.as_deref() {
            let _ = writeln!(output, "      remedy: {remedy}");
        }
    }
    output
}

fn common_checks(paths: &AppPaths, keys_ready: bool) -> Vec<SetupCheck> {
    vec![
        writable_dir_check("App data directory", &paths.base_dir),
        readable_file_check("Config file", &paths.config_file),
        writable_dir_check("Profiles directory", &paths.profiles_dir),
        SetupCheck {
            name: String::from("WireGuard key material"),
            status: if keys_ready {
                SetupStatus::Pass
            } else {
                SetupStatus::Fail
            },
            detail: if keys_ready {
                String::from("public and private keys are present")
            } else {
                String::from("public or private key is missing")
            },
            remedy: Some(String::from(
                "delete the config only if you intend to regenerate identity",
            )),
        },
    ]
}

fn writable_dir_check(name: &str, path: &std::path::Path) -> SetupCheck {
    let probe = path.join(".zeronode-write-test");
    match fs::write(&probe, b"ok").and_then(|_| fs::remove_file(&probe)) {
        Ok(()) => SetupCheck {
            name: name.to_owned(),
            status: SetupStatus::Pass,
            detail: format!("{} is writable", path.display()),
            remedy: None,
        },
        Err(error) => SetupCheck {
            name: name.to_owned(),
            status: SetupStatus::Fail,
            detail: format!("{} is not writable: {error}", path.display()),
            remedy: Some(String::from(
                "check filesystem permissions for the app data path",
            )),
        },
    }
}

fn readable_file_check(name: &str, path: &std::path::Path) -> SetupCheck {
    match fs::read(path) {
        Ok(_) => SetupCheck {
            name: name.to_owned(),
            status: SetupStatus::Pass,
            detail: format!("{} is readable", path.display()),
            remedy: None,
        },
        Err(error) => SetupCheck {
            name: name.to_owned(),
            status: SetupStatus::Fail,
            detail: format!("{} is not readable: {error}", path.display()),
            remedy: Some(String::from(
                "run the matching show-config command to recreate config",
            )),
        },
    }
}

fn subnet_check(subnet: &str) -> SetupCheck {
    let valid = subnet
        .split_once('/')
        .map(|(base, mask)| mask == "24" && base.parse::<std::net::Ipv4Addr>().is_ok())
        .unwrap_or(false);

    SetupCheck {
        name: String::from("VPN subnet"),
        status: if valid {
            SetupStatus::Pass
        } else {
            SetupStatus::Fail
        },
        detail: if valid {
            format!("{subnet} is valid for the current /24 allocator")
        } else {
            format!("{subnet} is not supported by the current allocator")
        },
        remedy: Some(String::from(
            "use a private IPv4 /24 subnet such as 10.44.0.0/24",
        )),
    }
}

fn server_port_check(name: &str, port: u16) -> SetupCheck {
    match UdpSocket::bind(("0.0.0.0", port)) {
        Ok(_) => SetupCheck {
            name: String::from(name),
            status: SetupStatus::Pass,
            detail: format!("UDP {port} is available"),
            remedy: None,
        },
        Err(error) => SetupCheck {
            name: String::from(name),
            status: SetupStatus::Warn,
            detail: format!("UDP {port} could not be bound: {error}"),
            remedy: Some(String::from(
                "if the daemon is already running this is expected",
            )),
        },
    }
}

fn status_label(status: &SetupStatus) -> &'static str {
    match status {
        SetupStatus::Pass => "PASS",
        SetupStatus::Warn => "WARN",
        SetupStatus::Fail => "FAIL",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::generate_key_material;

    #[test]
    fn subnet_validation_accepts_private_24() {
        assert_eq!(subnet_check("10.44.0.0/24").status, SetupStatus::Pass);
    }

    #[test]
    fn client_report_warns_without_known_hosts() {
        let root = std::env::temp_dir().join(format!("zeronode-test-{}", crate::unix_now()));
        fs::create_dir_all(root.join("profiles")).unwrap();
        fs::write(root.join("config.toml"), b"ok").unwrap();
        let paths = AppPaths {
            base_dir: root.clone(),
            config_file: root.join("config.toml"),
            state_file: root.join("state.toml"),
            runtime_file: root.join("runtime.toml"),
            log_file: root.join("events.log"),
            profiles_dir: root.join("profiles"),
        };
        let config = ClientConfig {
            client_id: String::from("client"),
            display_name: String::from("Client"),
            known_hosts: Vec::new(),
            reduced_motion: false,
            wireguard_keys: Some(generate_key_material()),
        };

        let report = client_setup_report(&paths, &config);
        assert!(report
            .checks
            .iter()
            .any(|check| check.name == "Known hosts" && check.status == SetupStatus::Warn));
        let _ = fs::remove_dir_all(root);
    }
}
