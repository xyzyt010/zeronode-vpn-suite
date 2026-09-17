//! Self-contained Windows runtime: extract bundled helpers and make them
//! loadable without a separate WireGuard / Wintun / OpenVPN install.
//!
//! `vpn-client.exe` is the only file the user needs. On first launch we write
//! `wintun.dll` (embedded) into `%LOCALAPPDATA%\ZeroNode\VpnSuite\client\bin`
//! and next to the exe, then prepend that directory to `PATH` / the DLL search
//! path so boringtun, tun2proxy, and OpenVPN all find Wintun.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::{env, fs};

/// Official amd64 Wintun 0.14.x, vendored from `apps/client/assets/tor/wintun.dll`.
const WINTUN_DLL: &[u8] = include_bytes!("../../../apps/client/assets/tor/wintun.dll");

/// `%LOCALAPPDATA%\ZeroNode\VpnSuite\client\bin`
pub fn runtime_bin_dir() -> Result<PathBuf> {
    let paths = vpn_suite_core::app_paths::client_paths()?;
    let dir = paths.base_dir.join("bin");
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

/// Extract embedded Wintun, pin the DLL search path, start RAS for PPTP.
pub fn ensure_runtime() -> Result<PathBuf> {
    let bin = runtime_bin_dir()?;
    let dest = bin.join("wintun.dll");
    write_if_same_len(&dest, WINTUN_DLL)
        .with_context(|| format!("write {}", dest.display()))?;

    if let Some(exe_dir) = current_exe_dir() {
        let beside = exe_dir.join("wintun.dll");
        let _ = write_if_same_len(&beside, WINTUN_DLL);
    }

    prepend_search_path(&bin);
    if let Some(exe_dir) = current_exe_dir() {
        prepend_search_path(&exe_dir);
    }
    set_dll_directory(&bin);

    Ok(bin)
}

pub fn wintun_dll_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(bin) = runtime_bin_dir() {
        candidates.push(bin.join("wintun.dll"));
    }
    if let Some(dir) = current_exe_dir() {
        candidates.push(dir.join("wintun.dll"));
        candidates.push(dir.join("assets").join("tor").join("wintun.dll"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Load Wintun from the extracted path (never requires a system WireGuard install).
pub fn load_wintun() -> Result<wintun::Wintun> {
    let _ = ensure_runtime();
    if let Some(path) = wintun_dll_path() {
        match unsafe { wintun::load_from_path(&path) } {
            Ok(lib) => return Ok(lib),
            Err(error) => tracing::warn!(
                "wintun load_from_path {} failed: {error} — trying default search",
                path.display()
            ),
        }
    }
    unsafe { wintun::load() }.map_err(|e| anyhow::anyhow!("failed to load wintun.dll: {e}"))
}

pub fn is_wintun_ready() -> bool {
    load_wintun().is_ok()
}

/// Copy `wintun.dll` into `dir` (OpenVPN looks next to `openvpn.exe`).
pub fn stage_wintun_in(dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let dest = dir.join("wintun.dll");
    write_if_same_len(&dest, WINTUN_DLL)?;
    Ok(dest)
}

/// Locate `wireguard.exe` / `wg.exe` / `openvpn.exe` shipped or extracted.
pub fn find_helper_exe(name: &str) -> Option<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(bin) = runtime_bin_dir() {
        dirs.push(bin.clone());
        dirs.push(bin.join("openvpn"));
        dirs.push(bin.join("tor"));
    }
    if let Some(dir) = current_exe_dir() {
        dirs.push(dir.clone());
        dirs.push(dir.join("openvpn"));
        dirs.push(dir.join("assets").join("tor"));
        dirs.push(dir.join("assets").join("openvpn"));
    }
    if let Ok(pf) = env::var("ProgramFiles") {
        dirs.push(PathBuf::from(&pf).join("WireGuard"));
        dirs.push(PathBuf::from(&pf).join("OpenVPN").join("bin"));
    }
    for dir in dirs {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Make sure the RAS service is up so PPTP `rasdial` / `Add-VpnConnection` work.
pub fn ensure_pptp_support() -> Result<()> {
    let _ = crate::silent_cmd::silent_output("sc.exe", &["config", "RasMan", "start=", "auto"]);
    let _ = crate::silent_cmd::silent_output("sc.exe", &["start", "RasMan"]);
    // Optional helpers that exist on client SKUs.
    let _ = crate::silent_cmd::silent_output("sc.exe", &["start", "SstpSvc"]);
    let _ = crate::silent_cmd::silent_output("sc.exe", &["start", "RasAuto"]);
    Ok(())
}

pub fn current_exe_dir() -> Option<PathBuf> {
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(ToOwned::to_owned))
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
    let tmp = path.with_extension("dll.tmp");
    fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path).or_else(|_| {
        fs::copy(&tmp, path)?;
        fs::remove_file(&tmp)?;
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

fn prepend_search_path(dir: &Path) {
    let extra = dir.to_string_lossy().into_owned();
    if extra.is_empty() {
        return;
    }
    let old = env::var("PATH").unwrap_or_default();
    let already = old
        .split(';')
        .any(|p| p.eq_ignore_ascii_case(&extra));
    if !already {
        env::set_var("PATH", format!("{extra};{old}"));
    }
}

fn set_dll_directory(dir: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = SetDllDirectoryW(wide.as_ptr());
    }
}
