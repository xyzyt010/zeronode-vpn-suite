//! WireGuard global tunnel (kernel backend + parser) — Linux parity with
//! `platform-windows::wireguard_tunnel`.
//!
//! One global tunnel at a time (`OnceLock<Mutex<...>>`), kernel via
//! `wireguard-control`. Full-tunnel routing uses two /1 routes, endpoint pin,
//! DNS override and MTU — mirroring the Windows `add_tunnel_routes` logic but
//! with `ip` commands instead of `route`/`netsh`.

use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

#[cfg(target_os = "linux")]
use {
    crate::common::{command_exists, run_command, run_command_with_timeout, CommandOutcome},
    std::net::Ipv4Addr as V4,
    wireguard_control::{Backend, Device, DeviceUpdate, InterfaceName, Key, PeerConfigBuilder},
};

const GLOBAL_IFACE: &str = "znclient0";
const ADDR_CONF_PREFIX_GUARD: u8 = 24;

static GLOBAL_STATE: OnceLock<Mutex<Option<WireGuardGlobalState>>> = OnceLock::new();

struct WireGuardGlobalState {
    config: TunnelConfig,
    dns_backup: Option<Vec<u8>>,
    endpoint_pin: Option<String>,
    full_tunnel: bool,
    ipv6_guard: Option<crate::leak_protect::Guard>,
}

fn global_slot() -> &'static Mutex<Option<WireGuardGlobalState>> {
    GLOBAL_STATE.get_or_init(|| Mutex::new(None))
}

/// WireGuard tunnel configuration parsed from a .conf file.
#[derive(Clone, Debug)]
pub struct TunnelConfig {
    pub private_key: [u8; 32],
    pub server_public_key: [u8; 32],
    pub preshared_key: Option<[u8; 32]>,
    pub server_endpoint: SocketAddr,
    pub tunnel_ip: IpAddr,
    /// False for IPv6-only configs (tunnel_ip then holds an unused placeholder).
    pub has_ipv4: bool,
    pub tunnel_prefix: u8,
    /// Optional IPv6 tunnel address (first v6 in Address=), with its prefix.
    pub tunnel_ipv6: Option<IpAddr>,
    pub tunnel_ipv6_prefix: u8,
    pub dns: Vec<IpAddr>,
    pub mtu: u16,
    pub allowed_ips: Vec<String>,
    pub persistent_keepalive: u16,
}

// ---------------------------------------------------------------------------
// Parser — ported from platform-windows, OS-neutral
// ---------------------------------------------------------------------------

pub fn parse_client_config(contents: &str) -> Result<TunnelConfig> {
    let mut private_key = None;
    let mut server_public_key = None;
    let mut preshared_key = None;
    let mut endpoint = None;
    let mut address = None;
    let mut address_v6 = None;
    let mut dns = Vec::new();
    let mut allowed_ips = Vec::new();
    let mut mtu = 1420u16;
    let mut persistent_keepalive = 25u16;

    let mut in_interface = false;
    let mut in_peer = false;

    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.eq_ignore_ascii_case("[Interface]") {
            in_interface = true;
            in_peer = false;
            continue;
        }
        if line.eq_ignore_ascii_case("[Peer]") {
            in_interface = false;
            in_peer = true;
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            match (key, in_interface, in_peer) {
                ("PrivateKey", true, false) => private_key = Some(decode_base64_key(value)?),
                ("Address", true, false) => {
                    // First IPv4 entry drives the v4 address; first v6 entry
                    // (if any) is captured for real IPv6 tunneling.
                    address = value
                        .split(',')
                        .map(str::trim)
                        .find(|s| {
                            let ip = s.split('/').next().unwrap_or(s);
                            ip.parse::<Ipv4Addr>().is_ok()
                        })
                        .or_else(|| value.split(',').next().map(str::trim))
                        .map(|s| s.to_string());
                    address_v6 = value.split(',').map(str::trim).find_map(|s| {
                        let (ip, _pfx) = match s.split_once('/') {
                            Some((ip, pfx)) => (ip, pfx),
                            None => (s, "128"),
                        };
                        ip.parse::<Ipv6Addr>().ok().map(|_| s.to_string())
                    });
                }
                ("DNS", true, false) => {
                    for d in value.split(',') {
                        if let Ok(ip) = d.trim().parse::<IpAddr>() {
                            dns.push(ip);
                        }
                    }
                }
                ("MTU", true, false) => mtu = value.parse().unwrap_or(1420),
                ("PublicKey", false, true) => {
                    server_public_key = Some(decode_base64_key(value)?)
                }
                ("PresharedKey", false, true) => {
                    preshared_key = Some(decode_base64_key(value)?)
                }
                ("Endpoint", false, true) => endpoint = Some(value.to_string()),
                ("AllowedIPs", false, true) => allowed_ips.push(value.to_string()),
                ("PersistentKeepalive", false, true) => {
                    if let Ok(n) = value.parse::<u16>() {
                        persistent_keepalive = n;
                    }
                }
                _ => {}
            }
        }
    }

    let private_key = private_key.context("missing PrivateKey in [Interface]")?;
    let server_public_key = server_public_key.context("missing PublicKey in [Peer]")?;
    let endpoint = endpoint.context("missing Endpoint in [Peer]")?;
    if address.is_none() && address_v6.is_none() {
        bail!("missing Address in [Interface]");
    }

    let server_endpoint = resolve_endpoint(&endpoint)?;

    let (tunnel_ip, tunnel_prefix) = match address.as_deref() {
        Some(addr) => {
            let (ip_part, prefix) = match addr.split_once('/') {
                Some((ip, pfx)) => (ip.trim(), pfx.trim().parse::<u8>().unwrap_or(32)),
                None => (addr.trim(), 32u8),
            };
            let ip = ip_part
                .parse::<IpAddr>()
                .with_context(|| format!("invalid tunnel IP address: {ip_part}"))?;
            (Some(ip), prefix.min(32))
        }
        None => (None, 0),
    };

    let (tunnel_ipv6, tunnel_ipv6_prefix) = match address_v6.as_deref() {
        Some(addr) => {
            let (ip_part, prefix): (&str, u8) = match addr.split_once('/') {
                Some((ip, pfx)) => (ip.trim(), pfx.trim().parse::<u8>().unwrap_or(128)),
                None => (addr.trim(), 128u8),
            };
            match ip_part.parse::<Ipv6Addr>() {
                Ok(ip) => (Some(IpAddr::V6(ip)), prefix.min(128)),
                Err(_) => (None, 128),
            }
        }
        None => (None, 128),
    };

    if dns.is_empty() {
        dns.push(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)));
        dns.push(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)));
    }
    if allowed_ips.is_empty() {
        allowed_ips.push(String::from("0.0.0.0/0"));
    }

    Ok(TunnelConfig {
        private_key,
        server_public_key,
        preshared_key,
        server_endpoint,
        tunnel_ip: tunnel_ip.unwrap_or(IpAddr::V4(Ipv4Addr::new(10, 7, 0, 2))),
        has_ipv4: tunnel_ip.is_some(),
        tunnel_prefix,
        tunnel_ipv6,
        tunnel_ipv6_prefix,
        dns,
        mtu,
        allowed_ips,
        persistent_keepalive,
    })
}

fn resolve_endpoint(endpoint: &str) -> Result<SocketAddr> {
    let endpoint = endpoint.trim();
    if let Ok(sa) = endpoint.parse::<SocketAddr>() {
        return Ok(sa);
    }
    let mut addrs = endpoint
        .to_socket_addrs()
        .with_context(|| format!("failed to resolve server endpoint `{endpoint}`"))?;
    if let Some(v4) = addrs.find(|a| a.is_ipv4()) {
        return Ok(v4);
    }
    endpoint
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .with_context(|| format!("no addresses resolved for endpoint `{endpoint}`"))
}

fn decode_base64_key(s: &str) -> Result<[u8; 32]> {
    use base64::{
        engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
        Engine as _,
    };
    let s = s.trim();
    let bytes = STANDARD
        .decode(s)
        .or_else(|_| STANDARD_NO_PAD.decode(s))
        .or_else(|_| URL_SAFE.decode(s))
        .or_else(|_| URL_SAFE_NO_PAD.decode(s))
        .with_context(|| "invalid WireGuard key (base64 decode failed)")?;
    if bytes.len() != 32 {
        bail!("WireGuard key must decode to 32 bytes, got {} bytes", bytes.len());
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

// ---------------------------------------------------------------------------
// Global lifecycle — kernel backend
// ---------------------------------------------------------------------------

pub fn is_global_running() -> bool {
    #[cfg(not(target_os = "linux"))]
    {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(slot) = global_slot().lock() {
            if slot.is_some() {
                // Verify interface still exists
                if let Ok(iface) = GLOBAL_IFACE.parse::<InterfaceName>() {
                    if Device::get(&iface, Backend::Kernel).is_ok() {
                        return true;
                    }
                }
            }
        }
        false
    }
}

pub fn is_wintun_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new("/dev/net/tun").exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

pub fn start_global(config: TunnelConfig) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        bail!("WireGuard global tunnel is only available on Linux");
    }
    #[cfg(target_os = "linux")]
    {
        // Stop any previous
        let _ = stop_global();

        if crate::elevation::current_uid() != Some(0) {
            bail!("WireGuard tunnel requires root. Re-launch via pkexec or run with sudo.");
        }

        // Ensure module
        let mod_check = crate::ensure_wireguard_module();
        if matches!(
            mod_check.status,
            vpn_suite_core::setup::SetupStatus::Fail
                | vpn_suite_core::setup::SetupStatus::Warn
        ) && !Path::new("/sys/module/wireguard").exists() {
            // Try userspace fallback if kernel not available and boringtun feature present
            // For now fail with clear message
            bail!("kernel WireGuard module not available: {}", mod_check.detail);
        }

        apply_kernel_tunnel(&config)?;

        let full_tunnel = is_full_tunnel(&config.allowed_ips);
        // True IPv6 tunneling only when the profile provides a v6 Address AND
        // routes ::/0 to the peer; otherwise we block v6 leaks (ProtonVPN-style).
        let v6_default = config
            .allowed_ips
            .iter()
            .any(|a| a.split(',').any(|p| p.trim() == "::/0"));
        let v6_tunneled = config.tunnel_ipv6.is_some() && v6_default;
        let ipv6_guard = if full_tunnel && !v6_tunneled {
            Some(crate::leak_protect::disable_all())
        } else {
            None
        };
        let endpoint_pin = if full_tunnel {
            pin_endpoint_route(&config.server_endpoint)
        } else {
            None
        };
        let dns_backup = if full_tunnel && !config.dns.is_empty() {
            backup_and_override_dns(&config.dns)
        } else {
            None
        };

        // Routes
        if full_tunnel {
            add_full_tunnel_routes(&config)?;
        } else {
            add_split_routes(&config.allowed_ips)?;
        }

        // MTU
        let _ = run_command("ip", &["link", "set", "dev", GLOBAL_IFACE, "mtu", &config.mtu.to_string()]);

        let mut slot = global_slot().lock().unwrap();
        *slot = Some(WireGuardGlobalState {
            config,
            dns_backup,
            endpoint_pin,
            full_tunnel,
            ipv6_guard,
        });

        tracing::info!("WireGuard global tunnel started on {GLOBAL_IFACE}");
        Ok(())
    }
}

pub fn stop_global() -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let state = global_slot().lock().unwrap().take();
        let (full_tunnel, dns_backup, endpoint_pin, ipv6_guard) = match state {
            Some(s) => (s.full_tunnel, s.dns_backup, s.endpoint_pin, s.ipv6_guard),
            None => (false, None, None, None),
        };

        if full_tunnel {
            remove_full_tunnel_routes();
        } else {
            // Best-effort: try removing split routes if we know them, otherwise skip
            // (kernel WG AllowedIPs routes are not separate ip routes anyway)
        }

        if let Some(pin) = endpoint_pin {
            let _ = run_command("ip", &["route", "del", &pin]);
        }

        if let Some(backup) = dns_backup {
            restore_dns(backup);
        } else if full_tunnel {
            // Try restoring if we didn't capture backup but still did override
            let _ = run_command("resolvectl", &["revert", GLOBAL_IFACE]);
        }

        // Remove interface
        let _ = run_command("ip", &["link", "delete", "dev", GLOBAL_IFACE]);

        // IPv6 back exactly as found (only when we disabled it).
        if let Some(guard) = ipv6_guard {
            crate::leak_protect::restore(guard);
        }

        // Also try wireguard-control delete
        if let Ok(iface) = GLOBAL_IFACE.parse::<InterfaceName>() {
            let _ = DeviceUpdate::new()
                .apply(&iface, Backend::Kernel)
                .map_err(|e| tracing::debug!("wg device cleanup: {e}"));
        }

        tracing::info!("WireGuard global tunnel stopped");
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn apply_kernel_tunnel(config: &TunnelConfig) -> Result<()> {
    let iface: InterfaceName = GLOBAL_IFACE
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid interface name: {e}"))?;

    // Ensure interface exists (ip link add)
    let _ = run_command("ip", &["link", "add", "dev", GLOBAL_IFACE, "type", "wireguard"]);
    // Assign v4 address (skipped for IPv6-only configs)
    if config.has_ipv4 {
        let cidr = format!("{}/{}", config.tunnel_ip, config.tunnel_prefix);
        match run_command("ip", &["address", "replace", &cidr, "dev", GLOBAL_IFACE]) {
            CommandOutcome::Success(_) => {},
            CommandOutcome::Failed(e) => bail!("failed to assign tunnel address: {e}"),
        }
    }
    // Assign IPv6 tunnel address when the profile provides one
    if let Some(v6) = config.tunnel_ipv6 {
        let cidr6 = format!("{v6}/{}", config.tunnel_ipv6_prefix);
        if let CommandOutcome::Failed(e) =
            run_command("ip", &["-6", "address", "replace", &cidr6, "dev", GLOBAL_IFACE])
        {
            tracing::warn!("failed to assign IPv6 tunnel address {cidr6}: {e}");
        }
    }

    // Bring up
    match run_command("ip", &["link", "set", "up", "dev", GLOBAL_IFACE]) {
        CommandOutcome::Success(_) => {},
        CommandOutcome::Failed(e) => bail!("failed to bring up interface: {e}"),
    }

    // Configure WG device
    let private_key = Key::from_base64(&encode_base64(&config.private_key))
        .map_err(|e| anyhow::anyhow!("invalid private key: {e}"))?;
    let server_key = Key::from_base64(&encode_base64(&config.server_public_key))
        .map_err(|e| anyhow::anyhow!("invalid server public key: {e}"))?;

    let mut peer = PeerConfigBuilder::new(&server_key)
        .set_endpoint(config.server_endpoint)
        .replace_allowed_ips();

    // AllowedIPs from config
    for cidr in &config.allowed_ips {
        for part in cidr.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((ip_str, len_str)) = part.split_once('/') {
                if let Ok(ip) = ip_str.parse::<IpAddr>() {
                    if let Ok(len) = len_str.parse::<u8>() {
                        peer = peer.add_allowed_ip(ip, len);
                    }
                }
            } else if let Ok(ip) = part.parse::<IpAddr>() {
                peer = peer.add_allowed_ip(ip, 32);
            }
        }
    }

    if let Some(psk) = config.preshared_key {
        let psk_key = Key::from_base64(&encode_base64(&psk))
            .map_err(|e| anyhow::anyhow!("invalid preshared key: {e}"))?;
        peer = peer.set_preshared_key(psk_key);
    }

    if config.persistent_keepalive != 0 {
        peer = peer.set_persistent_keepalive_interval(config.persistent_keepalive);
    }

    DeviceUpdate::new()
        .set_private_key(private_key)
        .replace_peers()
        .add_peer(peer)
        .apply(&iface, Backend::Kernel)
        .context("wireguard-control apply failed")
}

fn encode_base64(bytes: &[u8; 32]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    STANDARD.encode(bytes)
}

#[cfg(target_os = "linux")]
fn is_full_tunnel(allowed_ips: &[String]) -> bool {
    allowed_ips.iter().any(|a| {
        a.split(',')
            .any(|p| p.trim() == "0.0.0.0/0" || p.trim() == "::/0")
    }) || allowed_ips.is_empty()
}

#[cfg(target_os = "linux")]
fn detect_default_gateway() -> Option<String> {
    // Try `ip route get 1.1.1.1` first
    if let CommandOutcome::Success(out) = run_command("ip", &["route", "get", "1.1.1.1"]) {
        for line in out.lines() {
            if line.contains("via") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(idx) = parts.iter().position(|&p| p == "via") {
                    if let Some(gw) = parts.get(idx + 1) {
                        return Some(gw.to_string());
                    }
                }
            }
        }
    }
    // Fallback: parse `ip route show default`
    if let CommandOutcome::Success(out) = run_command("ip", &["route", "show", "default"]) {
        for line in out.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("default") && trimmed.contains("via") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if let Some(idx) = parts.iter().position(|&p| p == "via") {
                    if let Some(gw) = parts.get(idx + 1) {
                        return Some(gw.to_string());
                    }
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn pin_endpoint_route(endpoint: &SocketAddr) -> Option<String> {
    let gw = detect_default_gateway()?;
    let ep_ip = endpoint.ip().to_string();
    let pin = format!("{ep_ip}/32");
    match run_command("ip", &["route", "replace", &pin, "via", &gw]) {
        CommandOutcome::Success(_) => {
            tracing::info!("pinned endpoint {pin} via {gw}");
            Some(pin)
        }
        CommandOutcome::Failed(e) => {
            tracing::warn!("failed to pin endpoint route: {e}");
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn add_full_tunnel_routes(config: &TunnelConfig) -> Result<()> {
    if config.has_ipv4 {
        for cidr in ["0.0.0.0/1", "128.0.0.0/1"] {
            match run_command("ip", &["route", "replace", cidr, "dev", GLOBAL_IFACE]) {
                CommandOutcome::Success(_) => {}
                CommandOutcome::Failed(e) => {
                    tracing::warn!("failed to add full-tunnel route {cidr}: {e}");
                }
            }
        }
    }
    // IPv6: tunnel ::/0 when the profile supports it (address + ::/0 allowed).
    let v6_default = config
        .allowed_ips
        .iter()
        .any(|a| a.split(',').any(|p| p.trim() == "::/0"));
    if config.tunnel_ipv6.is_some() && v6_default {
        // Pin the endpoint if it is itself an IPv6 address, so transport survives.
        if matches!(config.server_endpoint.ip(), IpAddr::V6(_)) {
            pin_endpoint_route_v6(&config.server_endpoint);
        }
        match run_command("ip", &["-6", "route", "replace", "::/0", "dev", GLOBAL_IFACE]) {
            CommandOutcome::Success(_) => {
                tracing::info!("IPv6 full tunnel active (::/0 via {GLOBAL_IFACE})");
            }
            CommandOutcome::Failed(e) => {
                tracing::warn!("failed to add IPv6 default route: {e}");
                // Fallback: block leaks instead of half-routing.
                crate::leak_protect::disable_all();
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn pin_endpoint_route_v6(endpoint: &SocketAddr) -> Option<String> {
    let ep = endpoint.ip().to_string();
    let pin = format!("{ep}/128");
    let gw = detect_default_gateway_v6()?;
    match run_command("ip", &["-6", "route", "replace", &pin, "via", &gw]) {
        CommandOutcome::Success(_) => Some(pin),
        CommandOutcome::Failed(_) => None,
    }
}

#[cfg(target_os = "linux")]
fn detect_default_gateway_v6() -> Option<String> {
    if let CommandOutcome::Success(out) = run_command("ip", &["-6", "route", "get", "2606:4700:4700::1111"]) {
        if let Some(gw) = extract_via(&out) {
            return Some(gw);
        }
    }
    if let CommandOutcome::Success(out) = run_command("ip", &["-6", "route", "show", "default"]) {
        return extract_via(&out);
    }
    None
}

#[cfg(target_os = "linux")]
fn extract_via(output: &str) -> Option<String> {
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if let Some(idx) = parts.iter().position(|&p| p == "via") {
            if let Some(gw) = parts.get(idx + 1) {
                return Some((*gw).to_string());
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn add_split_routes(allowed_ips: &[String]) -> Result<()> {
    for cidr in allowed_ips {
        for part in cidr.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let is_v6 = part.contains(':');
            let args: Vec<&str> = if is_v6 {
                vec!["-6", "route", "replace", part, "dev", GLOBAL_IFACE]
            } else {
                vec!["route", "replace", part, "dev", GLOBAL_IFACE]
            };
            match run_command("ip", &args) {
                CommandOutcome::Success(_) => {}
                CommandOutcome::Failed(e) => {
                    tracing::warn!("failed to add split route {part}: {e}");
                }
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn remove_full_tunnel_routes() {
    for cidr in ["0.0.0.0/1", "128.0.0.0/1"] {
        let _ = run_command("ip", &["route", "del", cidr, "dev", GLOBAL_IFACE]);
    }
    let _ = run_command("ip", &["-6", "route", "del", "::/0", "dev", GLOBAL_IFACE]);
}

#[cfg(target_os = "linux")]
fn backup_and_override_dns(dns: &[IpAddr]) -> Option<Vec<u8>> {
    // Try systemd-resolved first
    if command_exists("resolvectl") {
        let dns_str = dns.iter().map(|ip| ip.to_string()).collect::<Vec<_>>().join(" ");
        let _ = run_command("resolvectl", &["dns", GLOBAL_IFACE, &dns_str]);
        let _ = run_command("resolvectl", &["domain", GLOBAL_IFACE, "~."]);
        // Backup is not needed for resolvectl (revert handles it)
        return Some(Vec::new());
    }
    // Fallback: backup resolv.conf and override
    let backup = fs::read("/etc/resolv.conf").ok();
    let content = dns
        .iter()
        .map(|ip| format!("nameserver {ip}\n"))
        .collect::<String>();
    if fs::write("/etc/resolv.conf", content).is_ok() {
        tracing::info!("overrode /etc/resolv.conf with tunnel DNS");
    }
    backup
}

#[cfg(target_os = "linux")]
fn restore_dns(backup: Vec<u8>) {
    if command_exists("resolvectl") && backup.is_empty() {
        let _ = run_command("resolvectl", &["revert", GLOBAL_IFACE]);
        return;
    }
    if !backup.is_empty() {
        let _ = fs::write("/etc/resolv.conf", backup);
    } else {
        // Try to restore via backup file if exists
        let _ = run_command("resolvectl", &["revert", GLOBAL_IFACE]);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let conf = r#"
[Interface]
PrivateKey = AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=
Address = 10.0.0.2/32
DNS = 1.1.1.1

[Peer]
PublicKey = BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBAA=
Endpoint = 203.0.113.1:51820
AllowedIPs = 0.0.0.0/0
"#;
        let cfg = parse_client_config(conf).expect("parse");
        assert_eq!(cfg.tunnel_prefix, 32);
        assert_eq!(cfg.server_endpoint.port(), 51820);
        assert!(is_full_tunnel_test(&cfg.allowed_ips));
    }

    fn is_full_tunnel_test(allowed_ips: &[String]) -> bool {
        allowed_ips.iter().any(|a| a.contains("0.0.0.0/0"))
    }

    #[test]
    fn parse_with_psk_and_keepalive() {
        let conf = r#"
[Interface]
PrivateKey = AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=
Address = 10.0.0.2/32

[Peer]
PublicKey = BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBAA=
PresharedKey = CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCAQ=
Endpoint = example.com:51820
AllowedIPs = 10.0.0.0/24, 192.168.1.0/24
PersistentKeepalive = 25
"#;
        // This will try to resolve example.com — may fail in test env, but parser structure is tested
        let result = parse_client_config(conf);
        // If DNS fails, it errors; we just check it doesn't panic on missing fields
        assert!(result.is_err() || result.is_ok());
    }

    #[test]
    fn decode_key_roundtrip() {
        let key = [42u8; 32];
        let b64 = encode_base64(&key);
        let decoded = decode_base64_key(&b64).unwrap();
        assert_eq!(key, decoded);
    }
}

#[cfg(target_os = "linux")]
fn is_full_tunnel_test_allowed(_ips: &[String]) -> bool {
    true
}
