use crate::unix_now;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
#[cfg(target_os = "windows")]
use std::process::Command;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct HostNetInfo {
    #[serde(default)]
    pub global_ipv4: Vec<String>,
    #[serde(default)]
    pub local_ipv4: Vec<String>,
    #[serde(default)]
    pub global_ipv6: Vec<String>,
    #[serde(default)]
    pub local_ipv6: Vec<String>,
}

impl HostNetInfo {
    pub fn query() -> Self {
        let mut info = Self::default();

        if let Ok(interfaces) = if_addrs::get_if_addrs() {
            for interface in interfaces {
                if interface.is_loopback() {
                    continue;
                }
                classify_ip(&mut info, interface.ip());
            }
        }

        #[cfg(target_os = "windows")]
        if info.global_ipv4.is_empty() && info.global_ipv6.is_empty() {
            merge_windows_ip_fallback(&mut info);
        }

        sort_dedup(&mut info.global_ipv4);
        sort_dedup(&mut info.local_ipv4);
        sort_dedup(&mut info.global_ipv6);
        sort_dedup(&mut info.local_ipv6);
        info
    }

    pub fn has_global_connectivity(&self) -> bool {
        !self.global_ipv4.is_empty() || !self.global_ipv6.is_empty()
    }

    pub fn effective_selected_global_ipv4(&self, configured: &[String]) -> Vec<String> {
        effective_selected(&self.global_ipv4, configured)
    }

    pub fn effective_selected_global_ipv6(&self, configured: &[String]) -> Vec<String> {
        effective_selected(&self.global_ipv6, configured)
    }
}

fn classify_ip(info: &mut HostNetInfo, ip: IpAddr) {
    match ip {
        IpAddr::V4(ipv4) => {
            if is_private_ipv4(ipv4) {
                info.local_ipv4.push(ipv4.to_string());
            } else {
                info.global_ipv4.push(ipv4.to_string());
            }
        }
        IpAddr::V6(ipv6) => {
            if is_private_ipv6(ipv6) {
                info.local_ipv6.push(ipv6.to_string());
            } else {
                info.global_ipv6.push(ipv6.to_string());
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn merge_windows_ip_fallback(info: &mut HostNetInfo) {
    let output = command_no_window("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            "Get-NetIPAddress -AddressFamily IPv4,IPv6 | Select-Object -ExpandProperty IPAddress",
        ])
        .output();
    let Ok(output) = output else {
        return;
    };
    if !output.status.success() {
        return;
    }

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(ip) = trimmed.parse::<IpAddr>() {
            classify_ip(info, ip);
        }
    }
}

#[cfg(target_os = "windows")]
fn command_no_window(program: &str) -> Command {
    let mut command = Command::new(program);
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn effective_selected(available: &[String], configured: &[String]) -> Vec<String> {
    let mut selected = configured
        .iter()
        .filter(|candidate| available.iter().any(|ip| ip == *candidate))
        .cloned()
        .collect::<Vec<_>>();

    if selected.is_empty() {
        if let Some(default) = choose_default(available) {
            selected.push(default.clone());
        }
    }

    sort_dedup(&mut selected);
    selected
}

fn choose_default<'a>(available: &'a [String]) -> Option<&'a String> {
    if available.is_empty() {
        return None;
    }
    let index = (unix_now() as usize) % available.len();
    available.get(index)
}

fn sort_dedup(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

fn is_private_ipv4(addr: Ipv4Addr) -> bool {
    let octets = addr.octets();
    addr.is_private()
        || addr.is_loopback()
        || addr.is_link_local()
        || addr.is_multicast()
        || addr.is_unspecified()
        || (octets[0] == 100 && octets[1] >= 64 && octets[1] <= 127)
        || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
}

fn is_private_ipv6(addr: Ipv6Addr) -> bool {
    let segments = addr.segments();
    addr.is_loopback()
        || addr.is_multicast()
        || addr.is_unspecified()
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfec0
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
}
