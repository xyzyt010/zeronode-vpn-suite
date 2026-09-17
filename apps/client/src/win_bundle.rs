//! Windows one-binary runtime: extract Tor + Wintun, provision OpenVPN and
//! optional official WireGuard helpers into AppData so the user never installs
//! those products separately.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::{fs, time::Duration};
use tracing::{info, warn};

const TOR_EXE: &[u8] = include_bytes!("../assets/tor/tor.exe");
const TOR_GEOIP: &[u8] = include_bytes!("../assets/tor/geoip");
const TORRC_DEFAULTS: &[u8] = include_bytes!("../assets/tor/torrc-defaults");

/// Extract Wintun + Tor and pin DLL search paths. Cheap after the first run.
pub fn ensure_windows_runtime() {
    match vpn_platform_windows::ensure_runtime() {
        Ok(bin) => info!("Windows runtime bin {}", bin.display()),
        Err(error) => warn!("Windows runtime extract failed: {error:#}"),
    }
    match extract_tor_bundle() {
        Ok(tor) => info!("Tor ready at {}", tor.display()),
        Err(error) => warn!("Tor extract failed: {error:#}"),
    }
    let _ = vpn_platform_windows::ensure_pptp_support();
}

pub fn resolve_tor_exe() -> Option<PathBuf> {
    let _ = extract_tor_bundle();
    let mut candidates = Vec::new();
    if let Ok(bin) = vpn_platform_windows::runtime_bin_dir() {
        candidates.push(bin.join("tor").join("tor.exe"));
    }
    if let Some(dir) = vpn_platform_windows::current_exe_dir() {
        candidates.push(dir.join("assets").join("tor").join("tor.exe"));
        candidates.push(dir.join("tor").join("tor.exe"));
        candidates.push(dir.join("tor.exe"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("apps/client/assets/tor/tor.exe"));
        candidates.push(cwd.join("assets/tor/tor.exe"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

pub fn extract_tor_bundle() -> Result<PathBuf> {
    let dir = vpn_platform_windows::runtime_bin_dir()?.join("tor");
    fs::create_dir_all(&dir)?;
    write_if_same_len(&dir.join("tor.exe"), TOR_EXE)?;
    write_if_same_len(&dir.join("geoip"), TOR_GEOIP)?;
    write_if_same_len(&dir.join("torrc-defaults"), TORRC_DEFAULTS)?;

    // Optional extras from a staged folder (dev tree / dist) — not embedded
    // because geoip6 is 16 MB and lyrebird is 18 MB.
    for name in ["geoip6", "torrc", "tun2socks.exe", "wintun.dll"] {
        copy_first_existing(name, &dir);
    }
    let pt = dir.join("pluggable_transports");
    if let Some(lyrebird) = first_existing(&[
        "apps/client/assets/tor/pluggable_transports/lyrebird.exe",
        "tor-expert-bundle-windows-x86_64-15.0.17/tor/pluggable_transports/lyrebird.exe",
    ]) {
        let _ = fs::create_dir_all(&pt);
        let dest = pt.join("lyrebird.exe");
        if !dest.is_file() {
            let _ = fs::copy(lyrebird, dest);
        }
    }

    let exe = dir.join("tor.exe");
    if !exe.is_file() {
        anyhow::bail!("tor.exe missing after extract at {}", exe.display());
    }
    Ok(exe)
}

/// Download official WireGuard helpers into AppData when missing.
pub async fn ensure_wireguard_helpers() -> Result<()> {
    if vpn_platform_windows::find_helper_exe("wireguard.exe").is_some()
        && vpn_platform_windows::find_helper_exe("wg.exe").is_some()
    {
        return Ok(());
    }
    let bin = vpn_platform_windows::runtime_bin_dir()?;
    // Prefer already-staged copies next to this source tree.
    for name in ["wireguard.exe", "wg.exe"] {
        if vpn_platform_windows::find_helper_exe(name).is_some() {
            continue;
        }
        let staged = PathBuf::from("dist/windows/bin").join(name);
        if staged.is_file() {
            let _ = fs::copy(&staged, bin.join(name));
        }
    }
    if vpn_platform_windows::find_helper_exe("wireguard.exe").is_some() {
        return Ok(());
    }

    info!("Provisioning official WireGuard helpers (no system install)");
    let msi = bin.join("wireguard-setup.msi");
    let url = "https://download.wireguard.com/windows-client/wireguard-amd64-1.1.msi";
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .user_agent("ZeroNodeVPN/0.2")
        .build()
        .context("http client")?;
    let bytes = client
        .get(url)
        .send()
        .await
        .context("download WireGuard MSI")?
        .error_for_status()
        .context("WireGuard MSI HTTP")?
        .bytes()
        .await
        .context("read WireGuard MSI")?;
    fs::write(&msi, &bytes).context("write WireGuard MSI")?;

    let extract = bin.join("wg-msi-extract");
    let _ = fs::remove_dir_all(&extract);
    fs::create_dir_all(&extract)?;
    let status = {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("msiexec")
            .args([
                "/a",
                msi.to_str().unwrap_or_default(),
                &format!("TARGETDIR={}", extract.display()),
                "/qn",
            ])
            .creation_flags(0x0800_0000)
            .status()
            .context("msiexec WireGuard extract")?
    };
    if !status.success() {
        anyhow::bail!("msiexec WireGuard extract failed: {status}");
    }
    for name in ["wireguard.exe", "wg.exe"] {
        if let Ok(found) = find_file_named(&extract, name) {
            let _ = fs::copy(&found, bin.join(name));
        }
    }
    let _ = fs::remove_file(&msi);
    let _ = fs::remove_dir_all(&extract);
    Ok(())
}

pub fn stage_wintun_beside_openvpn() {
    if let Some(ovpn) = crate::ovpn::find_openvpn_exe() {
        if let Some(dir) = ovpn.parent() {
            if let Err(error) = vpn_platform_windows::stage_wintun_in(dir) {
                warn!("could not copy wintun.dll next to OpenVPN: {error:#}");
            }
        }
    }
}

fn write_if_same_len(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Ok(meta) = fs::metadata(path) {
        if meta.len() == bytes.len() as u64 {
            return Ok(());
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn copy_first_existing(name: &str, dest_dir: &Path) {
    let dest = dest_dir.join(name);
    if dest.is_file() {
        return;
    }
    let origins = [
        format!("apps/client/assets/tor/{name}"),
        format!("assets/tor/{name}"),
    ];
    if let Some(src) = first_existing(&origins.iter().map(|s| s.as_str()).collect::<Vec<_>>()) {
        let _ = fs::copy(src, dest);
    }
    if let Some(dir) = vpn_platform_windows::current_exe_dir() {
        let src = dir.join("assets").join("tor").join(name);
        if src.is_file() && !dest_dir.join(name).is_file() {
            let _ = fs::copy(src, dest_dir.join(name));
        }
    }
}

fn first_existing(rel: &[&str]) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok();
    for r in rel {
        let p = PathBuf::from(r);
        if p.is_file() {
            return Some(p);
        }
        if let Some(c) = cwd.as_ref() {
            let p = c.join(r);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn find_file_named(root: &Path, name: &str) -> Result<PathBuf> {
    fn walk(dir: &Path, name: &str, out: &mut Option<PathBuf>) -> std::io::Result<()> {
        if out.is_some() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
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
