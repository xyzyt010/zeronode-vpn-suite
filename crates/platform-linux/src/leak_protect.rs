//! IPv6 anti-leak guard (ProtonVPN-style).
//!
//! While a system-wide tunnel is up, any traffic escaping over the physical
//! NIC breaks the VPN guarantee. Most consumer VPNs solve the "protocol is
//! IPv4-only" case by disabling IPv6 at the OS level for the duration of the
//! session and restoring it afterwards. This module implements exactly that
//! using `/proc/sys/net/ipv6/conf/<iface>/disable_ipv6`.
//!
//! * [`disable_all`] snapshots every interface's current value, then sets `1`.
//! * [`restore`] writes the snapshot back (safe even if interfaces changed).
//!
//! Pure filesystem implementation (OS-neutral so it compiles/tests anywhere);
//! on Linux the sysctl files are the live kernel knobs. Requires root when
//! targeting the real procfs — callers are the helper daemon / elevated flows.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Snapshot map: absolute sysctl path -> previous trimmed value.
pub type Guard = BTreeMap<String, String>;

fn conf_root() -> PathBuf {
    if let Ok(dir) = std::env::var("ZERONODE_IPV6_CONF_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    PathBuf::from("/proc/sys/net/ipv6/conf")
}

/// Disable IPv6 per-interface so v6 traffic cannot leak around the tunnel.
///
/// Critical kernel semantics: the `all` entry is combined with each interface
/// (effective value = max(all, iface)). Writing `all=1` therefore also kills
/// IPv6 on **loopback**, which breaks `::1` listeners — most importantly
/// systemd-resolved → total DNS blackout ("nothing works"). Real VPNs
/// (ProtonVPN/Mullvad) block v6 on physical links only.
///
/// So: skip `all` and `lo`; write every concrete interface plus `default`
/// (applies to interfaces created later, harmless to lo).
pub fn disable_all() -> Guard {
    let mut guard = Guard::new();
    let root = conf_root();
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!("ipv6 guard: cannot list {}: {error}", root.display());
            return guard;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // NEVER touch these two — see doc comment above.
        if name == "all" || name == "lo" {
            continue;
        }
        let knob = entry.path().join("disable_ipv6");
        if !knob.is_file() {
            continue;
        }
        let previous = std::fs::read_to_string(&knob)
            .unwrap_or_default()
            .trim()
            .to_string();
        match std::fs::write(&knob, b"1\n") {
            Ok(()) => {
                tracing::info!(
                    "ipv6 guard: disabled {} (was {previous})",
                    knob.display()
                );
                guard.insert(knob.display().to_string(), previous);
            }
            Err(error) => {
                tracing::warn!("ipv6 guard: could not disable {}: {error}", knob.display())
            }
        }
    }
    guard
}

/// Restore a previously captured guard. Missing knobs are skipped.
pub fn restore(guard: Guard) {
    for (knob, value) in guard {
        let payload = if value.is_empty() {
            String::from("0\n")
        } else {
            format!("{value}\n")
        };
        if let Err(error) = std::fs::write(&knob, payload.as_bytes()) {
            tracing::warn!("ipv6 guard: restore {} failed: {error}", knob);
        } else {
            tracing::info!("ipv6 guard: restored {} -> {}", knob, value.trim());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_conf_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("zn-ipv6-test-{tag}"));
        let _ = fs::remove_dir_all(&base);
        for iface in ["all", "default", "eth0", "wlan0", "lo"] {
            let dir = base.join(iface);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("disable_ipv6"), b"0\n").unwrap();
        }
        base
    }

    #[test]
    fn disable_then_restore_roundtrip() {
        let base = fake_conf_dir("roundtrip");
        std::env::set_var("ZERONODE_IPV6_CONF_DIR", &base);

        let guard = disable_all();
        // Only eth0 + wlan0 + default — `all` and `lo` MUST be untouched.
        assert_eq!(guard.len(), 3, "concrete ifaces + default only");
        for iface in ["eth0", "wlan0", "default"] {
            let v = fs::read_to_string(base.join(iface).join("disable_ipv6")).unwrap();
            assert_eq!(v.trim(), "1");
        }
        for safe in ["all", "lo"] {
            let v = fs::read_to_string(base.join(safe).join("disable_ipv6")).unwrap();
            assert_eq!(v.trim(), "0", "{safe} must never be disabled (loopback ::1)");
        }

        restore(guard);
        for iface in ["eth0", "wlan0", "default"] {
            let v = fs::read_to_string(base.join(iface).join("disable_ipv6")).unwrap();
            assert_eq!(v.trim(), "0");
        }

        std::env::remove_var("ZERONODE_IPV6_CONF_DIR");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_root_is_safe_noop() {
        std::env::set_var(
            "ZERONODE_IPV6_CONF_DIR",
            std::env::temp_dir().join("zn-ipv6-does-not-exist"),
        );
        let guard = disable_all();
        assert!(guard.is_empty());
        restore(guard);
        std::env::remove_var("ZERONODE_IPV6_CONF_DIR");
    }
}
