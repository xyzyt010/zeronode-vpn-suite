//! OpenVPN profile parsing, GeoIP enrichment, and binary resolution.
//!
//! Profiles are imported from `.ovpn` files (drag/drop or file picker). Remote
//! host is extracted from `remote` directives; when location metadata is
//! missing we resolve DNS and look up the public IP via online GeoIP APIs
//! (same stack as Tor / local IP cards).

use anyhow::{bail, Context, Result};
use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tracing::{info, warn};
use vpn_suite_core::model::TorExitInfo;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Parsed fields from a `.ovpn` client config (subset we surface in the UI).
#[derive(Clone, Debug, Default)]
pub struct ParsedOvpn {
    pub remote_host: String,
    pub remote_port: u16,
    pub proto: String,
    pub dev: String,
    pub cipher: String,
    pub auth: String,
    pub has_tls_auth: bool,
    pub has_inline_certs: bool,
    pub remotes: Vec<(String, u16)>,
}

/// Parse a standard OpenVPN client configuration text.
pub fn parse_ovpn(content: &str) -> ParsedOvpn {
    let mut out = ParsedOvpn {
        remote_port: 1194,
        proto: String::from("udp"),
        dev: String::from("tun"),
        ..Default::default()
    };
    let mut in_tag: Option<String> = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(tag) = line.strip_prefix('<') {
            if let Some(name) = tag.strip_suffix('>') {
                if !name.starts_with('/') {
                    in_tag = Some(name.to_ascii_lowercase());
                    if matches!(
                        name.to_ascii_lowercase().as_str(),
                        "ca" | "cert" | "key" | "tls-auth" | "tls-crypt" | "tls-crypt-v2"
                    ) {
                        out.has_inline_certs = true;
                    }
                    if matches!(
                        name.to_ascii_lowercase().as_str(),
                        "tls-auth" | "tls-crypt" | "tls-crypt-v2"
                    ) {
                        out.has_tls_auth = true;
                    }
                } else {
                    in_tag = None;
                }
            }
            continue;
        }
        if in_tag.is_some() {
            continue;
        }

        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else { continue };
        let key = key.to_ascii_lowercase();
        match key.as_str() {
            "remote" => {
                let host = parts.next().unwrap_or("").to_string();
                let port = parts
                    .next()
                    .and_then(|p| p.parse::<u16>().ok())
                    .unwrap_or(1194);
                if !host.is_empty() {
                    if out.remote_host.is_empty() {
                        out.remote_host = host.clone();
                        out.remote_port = port;
                    }
                    out.remotes.push((host, port));
                }
            }
            "proto" => {
                if let Some(p) = parts.next() {
                    out.proto = p.to_ascii_lowercase();
                }
            }
            "port" => {
                if let Some(p) = parts.next().and_then(|p| p.parse().ok()) {
                    out.remote_port = p;
                }
            }
            "dev" => {
                if let Some(d) = parts.next() {
                    out.dev = d.to_string();
                }
            }
            "cipher" => {
                if let Some(c) = parts.next() {
                    out.cipher = c.to_string();
                }
            }
            "auth" => {
                if let Some(a) = parts.next() {
                    out.auth = a.to_string();
                }
            }
            "tls-auth" | "tls-crypt" | "tls-crypt-v2" => {
                out.has_tls_auth = true;
            }
            "auth-user-pass" => {
                // Tracked via needs_auth_user_pass(); keep parse free of extra fields.
            }
            _ => {}
        }
    }
    out
}

/// True when the profile expects interactive (or file-based) username/password.
pub fn needs_auth_user_pass(content: &str) -> bool {
    let mut in_tag = false;
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('<') {
            in_tag = !line.starts_with("</");
            continue;
        }
        if in_tag {
            continue;
        }
        let mut parts = line.split_whitespace();
        if parts.next().map(|k| k.eq_ignore_ascii_case("auth-user-pass")) == Some(true) {
            return true;
        }
    }
    false
}

/// Write OpenVPN auth-user-pass file (username\\npassword\\n).
pub fn write_auth_file(path: &Path, username: &str, password: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // OpenVPN requires exactly two lines: user then pass.
    std::fs::write(path, format!("{username}\n{password}\n"))
        .with_context(|| format!("write auth file {}", path.display()))?;
    Ok(())
}

pub fn auth_file_path(profiles_dir: &Path, id: i64) -> PathBuf {
    profiles_dir.join(format!("ovpn_{id}.auth"))
}

pub fn log_file_path(profiles_dir: &Path, id: i64) -> PathBuf {
    profiles_dir.join(format!("ovpn_{id}.log"))
}

pub fn runtime_profile_path(profiles_dir: &Path, id: i64) -> PathBuf {
    profiles_dir.join(format!("ovpn_{id}.runtime.ovpn"))
}

/// OpenVPN treats `\` in config values as shell-escapes. Windows paths must use
/// forward slashes (`C:/Users/...`) or doubled backslashes.
pub fn openvpn_config_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Prefer a Windows driver only when we know a compatible DLL is present.
///
/// OpenVPN **2.7** defaults to `ovpn-dco` and has **removed Wintun**. Forcing
/// `windows-driver wintun` / `tap-windows6` often causes warnings or worse.
/// When unsure, return `None` and let OpenVPN choose (DCO → TAP fallback).
fn preferred_windows_driver(openvpn_exe: Option<&Path>) -> Option<&'static str> {
    let Some(exe) = openvpn_exe else {
        return None;
    };
    let dir = exe.parent()?;
    // Only inject Wintun when the DLL is actually next to openvpn.exe (2.5/2.6).
    if dir.join("wintun.dll").is_file() {
        return Some("wintun");
    }
    // OpenVPN 2.7+: leave driver unset so ovpn-dco / tap-windows6 auto-select works.
    let _ = dir;
    None
}

/// Build a runtime .ovpn that works headless on Windows:
/// - points `auth-user-pass` at a file (no console prompt)
/// - normalizes `dev tun1` → `dev tun` (named adapters often missing)
/// - picks a real Windows driver (TAP/Wintun) when known
/// - ensures `redirect-gateway def1` for system-wide default route
pub fn prepare_runtime_profile(
    content: &str,
    profiles_dir: &Path,
    id: i64,
    auth_path: Option<&Path>,
) -> Result<PathBuf> {
    prepare_runtime_profile_with_driver(content, profiles_dir, id, auth_path, None)
}

pub fn prepare_runtime_profile_with_driver(
    content: &str,
    profiles_dir: &Path,
    id: i64,
    auth_path: Option<&Path>,
    openvpn_exe: Option<&Path>,
) -> Result<PathBuf> {
    std::fs::create_dir_all(profiles_dir)?;
    let mut out = String::with_capacity(content.len() + 256);
    let mut in_tag = false;
    let mut saw_auth = false;
    let mut saw_redirect = false;
    let mut saw_windows_driver = false;
    let mut saw_dev = false;

    for raw in content.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with('<') {
            in_tag = !trimmed.starts_with("</");
            out.push_str(raw);
            out.push('\n');
            continue;
        }
        if in_tag {
            out.push_str(raw);
            out.push('\n');
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            out.push_str(raw);
            out.push('\n');
            continue;
        }

        let lower = trimmed.to_ascii_lowercase();
        let mut parts = trimmed.split_whitespace();
        let key = parts.next().unwrap_or("").to_ascii_lowercase();

        match key.as_str() {
            "auth-user-pass" => {
                saw_auth = true;
                if let Some(auth) = auth_path {
                    // Force file-based auth so openvpn never blocks on stdin.
                    // Paths MUST use / not \ or OpenVPN treats \ as escapes and aborts.
                    out.push_str("auth-user-pass ");
                    out.push_str(&openvpn_config_path(auth));
                    out.push('\n');
                } else if let Some(existing) = parts.next() {
                    // Re-write any existing path with safe separators.
                    let fixed = existing.replace('\\', "/");
                    out.push_str("auth-user-pass ");
                    out.push_str(&fixed);
                    out.push('\n');
                } else {
                    // Interactive with no credentials → leave marker; caller must supply auth.
                    out.push_str(raw);
                    out.push('\n');
                }
            }
            "dev" => {
                saw_dev = true;
                // Windows: avoid device names like tun1 that require a pre-created adapter.
                out.push_str("dev tun\n");
            }
            "windows-driver" => {
                saw_windows_driver = true;
                out.push_str(raw);
                out.push('\n');
            }
            "redirect-gateway" => {
                saw_redirect = true;
                // def1 installs 0.0.0.0/1 + 128.0.0.0/1 instead of replacing the default route
                // in a fragile way on Windows.
                if lower.contains("def1") {
                    out.push_str(raw);
                    out.push('\n');
                } else {
                    out.push_str("redirect-gateway def1 bypass-dhcp\n");
                }
            }
            _ => {
                out.push_str(raw);
                out.push('\n');
            }
        }
    }

    if !saw_dev {
        out.push_str("dev tun\n");
    }
    if !saw_windows_driver {
        if let Some(driver) = preferred_windows_driver(openvpn_exe) {
            out.push_str("windows-driver ");
            out.push_str(driver);
            out.push('\n');
        }
        // If unknown, omit — OpenVPN 2.7 will pick DCO/TAP itself.
    }
    if !saw_redirect {
        out.push_str("redirect-gateway def1 bypass-dhcp\n");
    }
    if let Some(auth) = auth_path {
        if !saw_auth {
            out.push_str("auth-user-pass ");
            out.push_str(&openvpn_config_path(auth));
            out.push('\n');
        }
    }

    // Disable interactive prompts / management console surprises.
    if !out.to_ascii_lowercase().contains("auth-nocache") {
        out.push_str("auth-nocache\n");
    }
    if !out.to_ascii_lowercase().contains("script-security") {
        out.push_str("script-security 2\n");
    }

    let path = runtime_profile_path(profiles_dir, id);
    std::fs::write(&path, out).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

#[derive(Debug, Clone)]
pub enum OvpnTunnelState {
    Connecting,
    Up,
    AuthFailed,
    Fatal(String),
    Exited,
}

/// Scan OpenVPN log for success / hard failure markers.
pub fn inspect_openvpn_log(log: &str) -> OvpnTunnelState {
    let lower = log.to_ascii_lowercase();
    if lower.contains("initialization sequence completed") {
        return OvpnTunnelState::Up;
    }
    if lower.contains("auth_failed")
        || lower.contains("auth failed")
        || lower.contains("received control message: auth_failed")
    {
        return OvpnTunnelState::AuthFailed;
    }
    // Common Windows / driver failures.
    for needle in [
        "there are no tap-windows adapters",
        "cannot open tun/tap",
        "all tap-windows adapters",
        "failed to open tun",
        "createfile failed on",
        "exiting due to fatal error",
        "options error:",
        "bad backslash",
        "error: windows route add",
        "route: waiting for tun/tap",
        "use --help for more information",
    ] {
        if lower.contains(needle) {
            // Prefer the last matching line as detail.
            let detail = log
                .lines()
                .rev()
                .find(|l| l.to_ascii_lowercase().contains(needle))
                .unwrap_or(needle)
                .to_string();
            return OvpnTunnelState::Fatal(detail);
        }
    }
    if lower.contains("process exiting") || lower.contains("sigterm") {
        return OvpnTunnelState::Exited;
    }
    OvpnTunnelState::Connecting
}

/// Poll log file until tunnel is up, fails, or timeout elapses.
pub async fn wait_for_openvpn_up(log_path: &Path, timeout: Duration) -> Result<OvpnTunnelState> {
    let start = std::time::Instant::now();
    let mut last = String::new();
    let stderr_path = log_path.with_extension("stderr.log");
    loop {
        // Merge main log + early stderr so fatals before --log opens still surface.
        let mut combined = String::new();
        if log_path.is_file() {
            if let Ok(text) = std::fs::read_to_string(log_path) {
                combined.push_str(&text);
            }
        }
        if stderr_path.is_file() {
            if let Ok(text) = std::fs::read_to_string(&stderr_path) {
                if !text.is_empty() {
                    if !combined.is_empty() {
                        combined.push('\n');
                    }
                    combined.push_str(&text);
                }
            }
        }
        if !combined.is_empty() {
            last = combined;
            match inspect_openvpn_log(&last) {
                OvpnTunnelState::Connecting => {}
                other => return Ok(other),
            }
        }

        // Process died without a decisive log line.
        if start.elapsed() > Duration::from_secs(4) && !is_openvpn_running() {
            if last.is_empty() {
                bail!(
                    "openvpn.exe exited within seconds with no log. \
                     Run ZeroNode elevated (UAC), confirm OpenVPN DCO/TAP is installed, \
                     and check {}",
                    stderr_path.display()
                );
            }
            return Ok(OvpnTunnelState::Exited);
        }

        if start.elapsed() >= timeout {
            let tail: String = last
                .lines()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            if tail.is_empty() {
                bail!(
                    "OpenVPN did not report success within {}s (no log output). \
                     Usually means missing Admin rights, missing TAP/DCO, or blocked remote. \
                     stderr: {}",
                    timeout.as_secs(),
                    stderr_path.display()
                );
            }
            bail!(
                "OpenVPN did not finish within {}s. Last log:\n{tail}",
                timeout.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

/// Resolve hostname → first IPv4/IPv6, or return the host if it is already an IP.
pub fn resolve_remote_ip(host: &str) -> Option<String> {
    if host.is_empty() {
        return None;
    }
    if host.parse::<IpAddr>().is_ok() {
        return Some(host.to_string());
    }
    let with_port = format!("{host}:0");
    match with_port.to_socket_addrs() {
        Ok(mut addrs) => addrs.next().map(|a| a.ip().to_string()),
        Err(error) => {
            warn!("DNS resolve failed for OpenVPN remote {host}: {error}");
            None
        }
    }
}

/// Look up GeoIP for a remote host (DNS first, then online APIs for that IP).
pub async fn enrich_remote_location(host: &str) -> Option<TorExitInfo> {
    let ip = resolve_remote_ip(host)?;
    resolve_ip_geo(&ip).await
}

async fn resolve_ip_geo(ip: &str) -> Option<TorExitInfo> {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) ZeroNodeVPN/0.1")
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(_) => return None,
    };

    // Prefer query-by-IP so we describe the *server* location, not our own.
    if let Some(info) = fetch_ip_api_for(&client, ip).await {
        return Some(info);
    }
    if let Some(info) = fetch_ipwho_for(&client, ip).await {
        return Some(info);
    }
    Some(TorExitInfo {
        ip: ip.to_string(),
        country: String::from("Unknown"),
        ..Default::default()
    })
}

async fn fetch_ip_api_for(client: &reqwest::Client, ip: &str) -> Option<TorExitInfo> {
    let url = format!(
        "http://ip-api.com/json/{ip}?fields=status,message,query,country,countryCode,region,regionName,city,zip,lat,lon,timezone,isp,org,as"
    );
    let json: serde_json::Value = client.get(&url).send().await.ok()?.json().await.ok()?;
    if json["status"].as_str() != Some("success") {
        return None;
    }
    Some(TorExitInfo {
        ip: json["query"].as_str().unwrap_or(ip).to_string(),
        ipv6: String::new(),
        country_code: json["countryCode"].as_str().unwrap_or("").to_string(),
        country: json["country"].as_str().unwrap_or("").to_string(),
        region_code: json["region"].as_str().unwrap_or("").to_string(),
        region: json["regionName"].as_str().unwrap_or("").to_string(),
        city: json["city"].as_str().unwrap_or("").to_string(),
        zip: json["zip"].as_str().unwrap_or("").to_string(),
        lat: json["lat"].as_f64().unwrap_or(0.0),
        lon: json["lon"].as_f64().unwrap_or(0.0),
        timezone: json["timezone"].as_str().unwrap_or("").to_string(),
        isp: json["isp"].as_str().unwrap_or("").to_string(),
        org: json["org"].as_str().unwrap_or("").to_string(),
        as_name: json["as"].as_str().unwrap_or("").to_string(),
    })
}

async fn fetch_ipwho_for(client: &reqwest::Client, ip: &str) -> Option<TorExitInfo> {
    let url = format!("https://ipwho.is/{ip}");
    let json: serde_json::Value = client.get(&url).send().await.ok()?.json().await.ok()?;
    if json["success"].as_bool() == Some(false) {
        return None;
    }
    let connection = json.get("connection");
    let isp = connection
        .and_then(|c| c.get("isp"))
        .and_then(|v| v.as_str())
        .or_else(|| json["isp"].as_str())
        .unwrap_or("")
        .to_string();
    let org = connection
        .and_then(|c| c.get("org"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let asn = connection
        .and_then(|c| c.get("asn"))
        .map(|v| match v {
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => s.clone(),
            _ => String::new(),
        })
        .unwrap_or_default();
    let as_name = if asn.is_empty() {
        String::new()
    } else if asn.starts_with("AS") {
        format!("{asn} {org}").trim().to_string()
    } else {
        format!("AS{asn} {org}").trim().to_string()
    };
    Some(TorExitInfo {
        ip: json["ip"].as_str().unwrap_or(ip).to_string(),
        ipv6: String::new(),
        country_code: json["country_code"].as_str().unwrap_or("").to_string(),
        country: json["country"].as_str().unwrap_or("").to_string(),
        region_code: json["region_code"].as_str().unwrap_or("").to_string(),
        region: json["region"].as_str().unwrap_or("").to_string(),
        city: json["city"].as_str().unwrap_or("").to_string(),
        zip: json["postal"].as_str().unwrap_or("").to_string(),
        lat: json["latitude"].as_f64().unwrap_or(0.0),
        lon: json["longitude"].as_f64().unwrap_or(0.0),
        timezone: json
            .pointer("/timezone/id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        isp,
        org,
        as_name,
    })
}

/// Locate an OpenVPN binary: system install → PATH → app-local strip-down.
pub fn find_openvpn_exe() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("ZERONODE_OPENVPN") {
        let p = PathBuf::from(custom);
        if p.is_file() {
            return Some(p);
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("openvpn.exe"));
            candidates.push(dir.join("openvpn").join("openvpn.exe"));
            candidates.push(dir.join("assets").join("openvpn").join("openvpn.exe"));
            candidates.push(dir.join("bin").join("openvpn").join("openvpn.exe"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("apps/client/assets/openvpn/openvpn.exe"));
        candidates.push(cwd.join("assets/openvpn/openvpn.exe"));
        candidates.push(cwd.join("target/debug/openvpn/openvpn.exe"));
        candidates.push(cwd.join("target/release/openvpn/openvpn.exe"));
    }
    if let Ok(paths) = vpn_suite_core::app_paths::client_paths() {
        candidates.push(paths.base_dir.join("bin").join("openvpn").join("openvpn.exe"));
        candidates.push(paths.base_dir.join("openvpn").join("openvpn.exe"));
    }

    #[cfg(target_os = "windows")]
    {
        let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| String::from(r"C:\Program Files"));
        let pf86 = std::env::var("ProgramFiles(x86)")
            .unwrap_or_else(|_| String::from(r"C:\Program Files (x86)"));
        candidates.push(PathBuf::from(&pf).join("OpenVPN").join("bin").join("openvpn.exe"));
        candidates.push(PathBuf::from(&pf86).join("OpenVPN").join("bin").join("openvpn.exe"));
        candidates.push(
            PathBuf::from(&pf)
                .join("OpenVPN Connect")
                .join("resources")
                .join("openvpn")
                .join("bin")
                .join("openvpn.exe"),
        );
    }
    #[cfg(not(target_os = "windows"))]
    {
        candidates.push(PathBuf::from("/usr/sbin/openvpn"));
        candidates.push(PathBuf::from("/usr/bin/openvpn"));
        candidates.push(PathBuf::from("/usr/local/sbin/openvpn"));
        candidates.push(PathBuf::from("/usr/local/bin/openvpn"));
    }

    for c in &candidates {
        if c.is_file() {
            info!("OpenVPN found at {}", c.display());
            return Some(c.clone());
        }
    }

    // PATH search
    #[cfg(target_os = "windows")]
    let which = Command::new("where").arg("openvpn").output();
    #[cfg(not(target_os = "windows"))]
    let which = Command::new("which").arg("openvpn").output();
    if let Ok(out) = which {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = text.lines().next() {
                let p = PathBuf::from(line.trim());
                if p.is_file() {
                    info!("OpenVPN found on PATH: {}", p.display());
                    return Some(p);
                }
            }
        }
    }

    None
}

/// Directory used for the stripped-down OpenVPN we manage ourselves.
pub fn managed_openvpn_dir() -> Result<PathBuf> {
    let paths = vpn_suite_core::app_paths::client_paths()?;
    let dir = paths.base_dir.join("bin").join("openvpn");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Ensure an OpenVPN binary is available. Uses an existing install when present;
/// otherwise installs a minimal managed copy (via winget when possible, or a
/// direct community release download).
pub async fn ensure_openvpn_exe() -> Result<PathBuf> {
    if let Some(existing) = find_openvpn_exe() {
        return Ok(existing);
    }

    info!("OpenVPN not found — provisioning a managed strip-down copy");
    let dir = managed_openvpn_dir()?;
    let target = dir.join("openvpn.exe");
    if target.is_file() {
        return Ok(target);
    }

    // 1) Prefer winget (already on modern Windows) so we do not re-download
    //    if the package cache / install completes into Program Files.
    #[cfg(target_os = "windows")]
    {
        if try_winget_install_openvpn().await {
            if let Some(found) = find_openvpn_exe() {
                return Ok(found);
            }
        }
    }

    // 2) Download a portable community zip when available; fall back to a clear error.
    #[cfg(target_os = "windows")]
    {
        match download_portable_openvpn(&dir).await {
            Ok(path) => return Ok(path),
            Err(error) => {
                warn!("portable OpenVPN download failed: {error:#}");
            }
        }
    }

    bail!(
        "OpenVPN is not installed. Install OpenVPN Community \
         (https://openvpn.net/community-downloads/) or place openvpn.exe under \
         {} — then retry Connect.",
        dir.display()
    )
}

#[cfg(target_os = "windows")]
async fn try_winget_install_openvpn() -> bool {
    // Non-interactive; ignore failure if winget is missing or user declines.
    let status = tokio::process::Command::new("winget")
        .args([
            "install",
            "--id",
            "OpenVPNTechnologies.OpenVPNCommunity",
            "-e",
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--disable-interactivity",
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .status()
        .await;
    matches!(status, Ok(s) if s.success())
}

#[cfg(target_os = "windows")]
async fn download_portable_openvpn(dir: &Path) -> Result<PathBuf> {
    // Official community releases are MSI/EXE installers. We stage openvpn.exe
    // by downloading a known-good build artifact mirror used for CI/portable
    // deployments. If the URL 404s, callers surface a helpful install message.
    //
    // Primary: extract from OpenVPN MSI via msiexec administrative install
    // into our managed directory (no permanent system install).
    // Prefer a current community MSI; fall back through a short list so a
    // renamed release does not brick provisioning.
    let msi_urls = [
        "https://swupdate.openvpn.org/community/releases/OpenVPN-2.7.5-I001-amd64.msi",
        "https://swupdate.openvpn.org/community/releases/OpenVPN-2.6.14-I004-amd64.msi",
        "https://build.openvpn.net/downloads/releases/OpenVPN-2.6.14-I004-amd64.msi",
    ];
    let msi_path = dir.join("openvpn-setup.msi");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .user_agent("ZeroNodeVPN/0.1")
        .build()
        .context("http client")?;

    let mut last_err = String::from("no MSI URL tried");
    let mut downloaded = false;
    for msi_url in msi_urls {
        info!("Downloading OpenVPN community MSI from {msi_url}");
        match client.get(msi_url).send().await {
            Ok(resp) => match resp.error_for_status() {
                Ok(ok) => match ok.bytes().await {
                    Ok(bytes) => {
                        if let Err(error) = std::fs::write(&msi_path, &bytes) {
                            last_err = format!("write MSI: {error}");
                            continue;
                        }
                        downloaded = true;
                        break;
                    }
                    Err(error) => last_err = format!("read body {msi_url}: {error}"),
                },
                Err(error) => last_err = format!("HTTP {msi_url}: {error}"),
            },
            Err(error) => last_err = format!("download {msi_url}: {error}"),
        }
    }
    if !downloaded {
        bail!("could not download OpenVPN MSI ({last_err})");
    }

    let extract_dir = dir.join("msi-extract");
    let _ = std::fs::remove_dir_all(&extract_dir);
    std::fs::create_dir_all(&extract_dir)?;

    // Administrative install: copies files without registering product.
    let status = Command::new("msiexec")
        .args([
            "/a",
            msi_path.to_str().unwrap_or_default(),
            &format!("TARGETDIR={}", extract_dir.display()),
            "/qn",
        ])
        .status()
        .context("msiexec administrative extract")?;
    if !status.success() {
        bail!("msiexec extract failed with {status}");
    }

    // Walk for openvpn.exe under the extract tree.
    let found = find_file_named(&extract_dir, "openvpn.exe")?;
    let dest = dir.join("openvpn.exe");
    std::fs::copy(&found, &dest).context("copy openvpn.exe")?;

    // Copy sibling DLLs next to openvpn.exe (libcrypto, libssl, …).
    if let Some(parent) = found.parent() {
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("dll") {
                    if let Some(name) = p.file_name() {
                        let _ = std::fs::copy(&p, dir.join(name));
                    }
                }
            }
        }
    }

    // Cleanup bulky MSI + extract tree to keep the managed install lean.
    let _ = std::fs::remove_file(&msi_path);
    let _ = std::fs::remove_dir_all(&extract_dir);

    if dest.is_file() {
        info!("Managed OpenVPN ready at {}", dest.display());
        Ok(dest)
    } else {
        bail!("openvpn.exe missing after MSI extract");
    }
}

fn find_file_named(root: &Path, name: &str) -> Result<PathBuf> {
    fn walk(dir: &Path, name: &str, out: &mut Option<PathBuf>) -> std::io::Result<()> {
        if out.is_some() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                walk(&path, name, out)?;
            } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
                *out = Some(path);
                return Ok(());
            }
        }
        Ok(())
    }
    let mut found = None;
    walk(root, name, &mut found).context("walk extract dir")?;
    found.ok_or_else(|| anyhow::anyhow!("file {name} not found under {}", root.display()))
}

/// Spawn OpenVPN against a profile path with a dedicated log file.
/// Returns the child process handle.
pub fn spawn_openvpn(openvpn_exe: &Path, profile: &Path) -> Result<std::process::Child> {
    spawn_openvpn_with_log(openvpn_exe, profile, None)
}

/// Spawn OpenVPN writing verbose log to `log_path` (overwritten each connect).
///
/// Important Windows notes:
/// - Do **not** hold an open write handle on the same path we pass to `--log`
///   (exclusive locking made OpenVPN exit with an empty log — that was the
///   silent "system VPN not working" failure).
/// - Prefer forward-slash config paths (OpenVPN treats `\` as escapes in some
///   option contexts).
pub fn spawn_openvpn_with_log(
    openvpn_exe: &Path,
    profile: &Path,
    log_path: Option<&Path>,
) -> Result<std::process::Child> {
    // Run from the OpenVPN bin directory so DCO/TAP helpers + DLLs resolve.
    let workdir = openvpn_exe.parent().map(|p| p.to_path_buf());
    let config_arg = openvpn_config_path(profile);

    let mut cmd = Command::new(openvpn_exe);
    cmd.arg("--config").arg(&config_arg);
    cmd.arg("--verb").arg("4");
    // Never pull interactive management console.
    cmd.arg("--suppress-timestamps");

    if let Some(log) = log_path {
        // Fresh log owned solely by OpenVPN after spawn. Marker line goes to a
        // sidecar so we never lock the OpenVPN log path ourselves.
        let marker = log.with_extension("spawn.txt");
        let _ = std::fs::write(
            &marker,
            format!(
                "=== ZeroNode OpenVPN spawn ===\nexe={}\nconfig={}\nlog={}\n",
                openvpn_exe.display(),
                config_arg,
                log.display()
            ),
        );
        // Drop any stale content so wait_for_openvpn_up does not see an old
        // "Initialization Sequence Completed" from a previous session.
        let _ = std::fs::remove_file(log);
        let log_arg = openvpn_config_path(log);
        cmd.arg("--log").arg(&log_arg);

        // Early parser/fatal errors before --log opens → separate stderr file.
        let err_path = log.with_extension("stderr.log");
        let _ = std::fs::remove_file(&err_path);
        if let Ok(err_file) = std::fs::File::create(&err_path) {
            cmd.stderr(std::process::Stdio::from(err_file));
        } else {
            cmd.stderr(std::process::Stdio::null());
        }
        cmd.stdout(std::process::Stdio::null());
    }

    if let Some(dir) = workdir {
        cmd.current_dir(dir);
    }

    #[cfg(target_os = "windows")]
    {
        // CREATE_NO_WINDOW — keep console off; elevated token still applies.
        cmd.creation_flags(0x08000000);
    }

    cmd.spawn()
        .with_context(|| format!("spawn {}", openvpn_exe.display()))
}

/// True if at least one `openvpn.exe` process is running (Windows/Linux best-effort).
pub fn is_openvpn_running() -> bool {
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq openvpn.exe", "/NH"])
            .creation_flags(0x08000000)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
            return stdout.contains("openvpn.exe");
        }
        false
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new("pgrep")
            .args(["-f", "openvpn"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

/// Kill any running openvpn.exe processes (best-effort).
pub fn kill_openvpn_processes() {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "openvpn.exe", "/T"])
            .creation_flags(0x08000000)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = Command::new("pkill").args(["-f", "openvpn"]).output();
    }
}
