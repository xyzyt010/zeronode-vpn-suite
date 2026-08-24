//! Helpers to spawn console tools without flashing a terminal window.
//!
//! On Windows, `Command::output()` still briefly shows a console for many
//! subsystem-console binaries (powershell.exe, taskkill.exe, netstat.exe)
//! unless `CREATE_NO_WINDOW` is set **and** stdio is redirected.
//! Prefer Win32 process termination over `taskkill.exe` when possible.

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

/// Hide the console window for the child process.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Build a command that never flashes a console window.
pub fn silent_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd
}

/// Run a program with args, discarding stderr, returning stdout as UTF-8 lossy text.
pub fn silent_output(program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Option<String> {
    let output = silent_command(program).args(args).output().ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Terminate every process whose image name matches (e.g. `"tor.exe"`)
/// using Win32 APIs only — **no** `taskkill.exe`, so no console flash.
///
/// Returns how many processes were terminated.
pub fn kill_process_image(image: &str) -> u32 {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_TERMINATE,
    };

    let target = image.to_ascii_lowercase();
    let target = target.trim_end_matches(".exe");
    let mut killed = 0u32;

    // SAFETY: standard ToolHelp snapshot of all processes.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE || snap.is_null() {
        // Last-resort silent taskkill (still CREATE_NO_WINDOW).
        let _ = silent_command("taskkill")
            .args(["/F", "/IM", image, "/T"])
            .stdout(Stdio::null())
            .output();
        return 0;
    }

    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        cntUsage: 0,
        th32ProcessID: 0,
        th32DefaultHeapID: 0,
        th32ModuleID: 0,
        cntThreads: 0,
        th32ParentProcessID: 0,
        pcPriClassBase: 0,
        dwFlags: 0,
        szExeFile: [0; 260],
    };

    let mut ok = unsafe { Process32FirstW(snap, &mut entry) };
    while ok != 0 {
        let name = {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            String::from_utf16_lossy(&entry.szExeFile[..len]).to_ascii_lowercase()
        };
        let stem = name.trim_end_matches(".exe");
        if stem == target || name == format!("{target}.exe") {
            let pid = entry.th32ProcessID;
            // Don't kill ourselves.
            if pid != std::process::id() {
                let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
                if !handle.is_null() {
                    let done = unsafe { TerminateProcess(handle, 1) };
                    unsafe { CloseHandle(handle) };
                    if done != 0 {
                        killed += 1;
                    }
                }
            }
        }
        ok = unsafe { Process32NextW(snap, &mut entry) };
    }
    unsafe { CloseHandle(snap) };
    killed
}

/// Clear leftover WinINet SOCKS proxy hints via the registry API only
/// (never `reg.exe` — that can flash a console on disconnect).
pub fn clear_stale_wininet_socks_hint() {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
        HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_SZ,
    };

    const KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\0";
    let key_path: Vec<u16> = KEY_PATH.encode_utf16().collect();
    let mut hkey = std::ptr::null_mut();
    let open = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key_path.as_ptr(),
            0,
            KEY_READ | KEY_SET_VALUE,
            &mut hkey,
        )
    };
    if open != ERROR_SUCCESS {
        return;
    }

    let name_enable: Vec<u16> = "ProxyEnable\0".encode_utf16().collect();
    let zero: u32 = 0;
    unsafe {
        let _ = RegSetValueExW(
            hkey,
            name_enable.as_ptr(),
            0,
            REG_DWORD,
            &zero as *const u32 as *const u8,
            4,
        );
    }

    let name_server: Vec<u16> = "ProxyServer\0".encode_utf16().collect();
    let mut typ = 0u32;
    let mut buf = [0u16; 512];
    let mut bytes = (buf.len() * 2) as u32;
    let q = unsafe {
        RegQueryValueExW(
            hkey,
            name_server.as_ptr(),
            std::ptr::null_mut(),
            &mut typ,
            buf.as_mut_ptr() as *mut u8,
            &mut bytes,
        )
    };
    if q == ERROR_SUCCESS && typ == REG_SZ {
        let nchars = (bytes as usize / 2).saturating_sub(1).min(buf.len());
        let text = String::from_utf16_lossy(&buf[..nchars]).to_ascii_lowercase();
        if text.contains("socks=127.0.0.1") || text.contains("socks://127.0.0.1") {
            unsafe {
                let _ = RegDeleteValueW(hkey, name_server.as_ptr());
            }
        }
    }
    unsafe {
        let _ = RegCloseKey(hkey);
    }
}
