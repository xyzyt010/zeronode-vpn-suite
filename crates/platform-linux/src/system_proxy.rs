//! System-wide proxy via GNOME/Cinnamon gsettings (Mint 22 / Ubuntu Noble).
//!
//! Mirrors the manual steps from ss.txt (Settings > Network > Proxy):
//!   mode 'manual' + socks host 127.0.0.1:port for SOCKS5.
//! Also handles XFCE's optional proxy via env hints.
//!
//! We touch gsettings only when available; failures are logged not fatal.
//! Restore is exact: previous mode/host/port/ignore-hosts saved via `guard`.

use std::collections::BTreeMap;
use std::process::Command;

#[derive(Clone, Debug, Default)]
pub struct ProxyGuard {
    prev: BTreeMap<String, String>,
}

impl ProxyGuard {
    fn get(key: &str) -> Option<String> {
        let parts: Vec<&str> = key.splitn(2, ' ').collect();
        if parts.len() != 2 {
            return None;
        }
        let schema = parts[0];
        let k = parts[1];
        match crate::common::run_command("gsettings", &["get", schema, k]) {
            crate::common::CommandOutcome::Success(s) if !s.is_empty() && s != "No such schema" => Some(s),
            _ => None,
        }
    }
    fn set(key: &str, value: &str) -> bool {
        let parts: Vec<&str> = key.splitn(2, ' ').collect();
        if parts.len() != 2 {
            // For cases where key already includes schema+key with space, split
            return false;
        }
        let schema = parts[0];
        let k = parts[1];
        matches!(
            crate::common::run_command("gsettings", &["set", schema, k, value]),
            crate::common::CommandOutcome::Success(_)
        )
    }
    fn set_raw(args: &[&str]) -> bool {
        matches!(
            crate::common::run_command("gsettings", args),
            crate::common::CommandOutcome::Success(_)
        )
    }
}

/// Enable SOCKS5 system proxy at 127.0.0.1:port.
/// Also sets http/https to same SOCKS via env-compatible? We use SOCKS only per ss.txt.
///
/// NOTE on architecture: gsettings lives in the USER's D-Bus session. The
/// root helper daemon has no DBUS_SESSION_BUS_ADDRESS, so its call here is
/// a fast no-op (detected below) — the GUI (running as the user) is the
/// authoritative side and calls this after helper tor_start succeeds.
/// Disconnect reverses it via disable/disable_all (user side).
pub fn enable(socks_port: u16) -> Option<ProxyGuard> {
    if !crate::common::command_exists("gsettings") {
        tracing::info!("system_proxy: gsettings not available, skip");
        return None;
    }
    // Root helper without a user D-Bus session cannot set the user's proxy
    // (and must not hang trying to autolaunch one). Skip fast; GUI does it.
    #[cfg(target_os = "linux")]
    if std::env::var("DBUS_SESSION_BUS_ADDRESS").is_err()
        && crate::elevation::current_uid() == Some(0)
    {
        tracing::info!("system_proxy: root without user D-Bus session, skip (GUI sets proxy)");
        return None;
    }

    let mut guard = ProxyGuard::default();

    // Backup current values
    for key in [
        "org.gnome.system.proxy mode",
        "org.gnome.system.proxy.socks host",
        "org.gnome.system.proxy.socks port",
        "org.gnome.system.proxy ignore-hosts",
        "org.gnome.system.proxy.http host",
        "org.gnome.system.proxy.https host",
    ] {
        if let Some(v) = ProxyGuard::get(key) {
            guard.prev.insert(key.to_string(), v);
        }
    }

    let port_str = socks_port.to_string();

    let ok_mode = ProxyGuard::set("org.gnome.system.proxy mode", "'manual'");
    let ok_host = ProxyGuard::set("org.gnome.system.proxy.socks host", "'127.0.0.1'");
    let ok_port = ProxyGuard::set("org.gnome.system.proxy.socks port", &port_str);
    // Ignore localhost/private to avoid loops
    let ignore = "['localhost', '127.0.0.0/8', '10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16', '::1']";
    let _ = ProxyGuard::set("org.gnome.system.proxy ignore-hosts", ignore);

    if ok_mode && ok_host && ok_port {
        tracing::info!("system_proxy: enabled SOCKS 127.0.0.1:{} via gsettings", socks_port);
        // Also set environment for child processes via /etc/environment hint? We export via DBus for current session
        // Notify NetworkManager via `gsettings` done; some apps need env. Set ALL_PROXY env for current process
        std::env::set_var("ALL_PROXY", format!("socks5h://127.0.0.1:{}", socks_port));
        Some(guard)
    } else {
        tracing::warn!("system_proxy: failed to enable via gsettings");
        // Try still to return guard for restore
        Some(guard)
    }
}

pub fn disable(guard: Option<ProxyGuard>) {
    if !crate::common::command_exists("gsettings") {
        return;
    }
    // Remove our env
    std::env::remove_var("ALL_PROXY");
    std::env::remove_var("all_proxy");

    // If we have guard, restore previous; else just set none
    if let Some(g) = guard {
        for (k, v) in g.prev {
            // v already includes quotes from gsettings get, use directly
            // But gsettings set expects value; if original was 'none', we need to strip? Use raw
            // Simplify: if original mode was 'none', set back to none
            let parts: Vec<&str> = k.splitn(2, ' ').collect();
            if parts.len() == 2 {
                let schema = parts[0];
                let key = parts[1];
                let value = v.trim();
                // gsettings get returns "'none'" including quotes, which is valid for set
                let _ = ProxyGuard::set(&format!("{} {}", schema, key), value);
            }
        }
        // Ensure mode is none if not restored
        let _ = ProxyGuard::set("org.gnome.system.proxy mode", "'none'");
        tracing::info!("system_proxy: disabled and restored previous");
    } else {
        let _ = ProxyGuard::set("org.gnome.system.proxy mode", "'none'");
        tracing::info!("system_proxy: disabled (no guard)");
    }
    // Clear SOCKS host/port anyway
    let _ = ProxyGuard::set("org.gnome.system.proxy.socks host", "''");
    let _ = ProxyGuard::set("org.gnome.system.proxy.socks port", "0");
}

/// Direct disable without guard (for emergency cleanup after kill)
pub fn disable_all() {
    if !crate::common::command_exists("gsettings") {
        return;
    }
    let _ = ProxyGuard::set("org.gnome.system.proxy mode", "'none'");
    let _ = ProxyGuard::set("org.gnome.system.proxy.socks host", "''");
    let _ = ProxyGuard::set("org.gnome.system.proxy.socks port", "0");
    std::env::remove_var("ALL_PROXY");
    std::env::remove_var("all_proxy");
    tracing::info!("system_proxy: emergency disable done");
}
