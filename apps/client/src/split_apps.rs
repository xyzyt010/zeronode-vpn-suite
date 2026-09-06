//! Running-app discovery for split tunneling (read-only `/proc` scan, no root).
//!
//! Lists GUI-ish processes grouped into browsers vs other apps. The helper
//! matches on the raw exe `name` (that is what `find_pids_by_name` compares),
//! while `friendly` is only display text.

use std::collections::BTreeMap;

/// Well-known browser exe basenames (lowercase).
const BROWSERS: &[&str] = &[
    "firefox",
    "firefox-esr",
    "librewolf",
    "waterfox",
    "google-chrome",
    "google-chrome-stable",
    "chrome",
    "chromium",
    "chromium-browser",
    "brave",
    "brave-browser",
    "brave-browser-stable",
    "opera",
    "opera-stable",
    "vivaldi",
    "vivaldi-stable",
    "microsoft-edge",
    "microsoft-edge-stable",
    "edge",
    "falkon",
    "epiphany",
    "tor-browser",
];

/// Exe basenames we never offer (own binary, helpers,bantam desktop plumbing).
/// The client binary is also the helper-side name, so it must stay excluded —
/// routing the VPN client itself would be nonsense at best.
const HIDDEN: &[&str] = &[
    "vpn-client",
    "sd-pam",
    "dbus-daemon",
    "dbus-broker",
    "dbus-broker-launch",
    "at-spi-bus-launcher",
    "at-spi2-registryd",
    "gvfsd",
    "gvfsd-fuse",
    "gvfs-udisks2-volume",
    "gvfs-mtp-volume",
    "dconf-service",
    "evolution-source-registry",
    "evolution-calendar-factory",
    "goa-daemon",
    "goa-identity-service",
    "xdg-desktop-portal",
    "xdg-document-portal",
    "xdg-permission-store",
    "xdg-desktop-portal-gtk",
    "gnome-keyring-daemon",
    "pipewire",
    "pipewire-pulse",
    "wireplumber",
    "pulseaudio",
    "snapd",
    "snap",
    "ibus-daemon",
    "ibus-dconf",
    "ibus-extension-gtk3",
    "ibus-x11",
    "ibus-portal",
    "gpg-agent",
    "ssh-agent",
];

#[derive(Clone, Debug)]
pub struct RunningApp {
    /// Raw exe basename — the value sent to the helper for cgroup matching.
    pub name: String,
    /// Display name.
    pub friendly: String,
    pub pids: Vec<u32>,
    pub browser: bool,
}

impl RunningApp {
    pub fn instances(&self) -> usize {
        self.pids.len()
    }
}

fn friendly_name(exe: &str) -> String {
    match exe.to_lowercase().as_str() {
        "firefox" | "firefox-esr" => "Firefox".to_string(),
        "librewolf" => "LibreWolf".to_string(),
        "google-chrome" | "google-chrome-stable" | "chrome" => "Google Chrome".to_string(),
        "chromium" | "chromium-browser" => "Chromium".to_string(),
        "brave" | "brave-browser" | "brave-browser-stable" => "Brave".to_string(),
        "opera" | "opera-stable" => "Opera".to_string(),
        "vivaldi" | "vivaldi-stable" => "Vivaldi".to_string(),
        "microsoft-edge" | "microsoft-edge-stable" | "edge" => "Edge".to_string(),
        "thunderbird" => "Thunderbird".to_string(),
        "discord" => "Discord".to_string(),
        "spotify" => "Spotify".to_string(),
        "code" => "VS Code".to_string(),
        "steam" => "Steam".to_string(),
        "telegram" | "telegram-desktop" => "Telegram".to_string(),
        "slack" => "Slack".to_string(),
        "vlc" => "VLC".to_string(),
        "qbittorrent" => "qBittorrent".to_string(),
        "transmission-gtk" | "transmission-qt" => "Transmission".to_string(),
        "zoom" => "Zoom".to_string(),
        "skypeforlinux" => "Skype".to_string(),
        "obs" => "OBS Studio".to_string(),
        other => {
            let mut c = other.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => other.to_string(),
            }
        }
    }
}

/// Exe basename for a pid: `/proc/<pid>/exe` link, else argv[0] basename
/// (covers scripts / snaps whose exe link is unreadable).
fn exe_of(pid: u32) -> Option<String> {
    if let Ok(link) = std::fs::read_link(format!("/proc/{pid}/exe")) {
        if let Some(n) = link.file_name().and_then(|s| s.to_str()) {
            if !n.is_empty() {
                return Some(n.to_string());
            }
        }
    }
    let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let first = cmd.split(|b| *b == 0).next()?.to_vec();
    let s = String::from_utf8_lossy(&first).into_owned();
    let base = s.rsplit('/').next()?.trim().to_string();
    if base.is_empty() {
        return None;
    }
    Some(base)
}

/// Scan `/proc` and return running apps, browsers first, then others —
/// each alphabetical by display name. Kernel threads (no exe/cmdline) and
/// plumbing daemons are skipped.
pub fn scan_running_apps() -> Vec<RunningApp> {
    let self_pid = std::process::id();
    let mut by_name: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let Some(exe) = exe_of(pid) else {
            continue;
        };
        let lower = exe.to_lowercase();
        if HIDDEN.contains(&lower.as_str()) {
            continue;
        }
        // Skip obvious kernel threads (`[kworker/0:1]` style shows up when
        // exe is unreadable and cmdline is empty — exe_of already None then,
        // but belt-and-braces for bracketed argv[0]).
        if exe.starts_with('[') && exe.ends_with(']') {
            continue;
        }
        by_name.entry(lower).or_default().push(pid);
    }
    let mut browsers = Vec::new();
    let mut others = Vec::new();
    for (lower, mut pids) in by_name {
        pids.sort_unstable();
        pids.dedup();
        let is_browser = BROWSERS.contains(&lower.as_str());
        let app = RunningApp {
            friendly: friendly_name(&lower),
            name: lower,
            pids,
            browser: is_browser,
        };
        if is_browser {
            browsers.push(app);
        } else {
            others.push(app);
        }
    }
    browsers.sort_by(|a, b| a.friendly.cmp(&b.friendly));
    others.sort_by(|a, b| a.friendly.cmp(&b.friendly));
    // Cap the "other" tail so a build server with 400 processes stays usable.
    const OTHER_CAP: usize = 80;
    if others.len() > OTHER_CAP {
        others.truncate(OTHER_CAP);
    }
    browsers.into_iter().chain(others).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_finds_processes_and_groups_browsers_first() {
        let apps = scan_running_apps();
        assert!(!apps.is_empty(), "expected at least one visible process");
        for a in &apps {
            assert!(!a.name.is_empty());
            assert_eq!(a.name, a.name.to_lowercase());
            assert!(!a.pids.is_empty());
            assert!(!a.friendly.is_empty());
        }
        // Browsers (if any) all precede non-browsers.
        let first_other = apps.iter().position(|a| !a.browser);
        if let Some(idx) = first_other {
            assert!(apps[idx..].iter().all(|a| !a.browser));
        }
        // vpn-client itself must never be offered.
        assert!(apps.iter().all(|a| a.name != "vpn-client"));
    }

    #[test]
    fn browser_table_spots_known_browsers() {
        for b in ["firefox", "chrome", "chromium", "brave", "opera", "vivaldi", "edge"] {
            assert!(
                BROWSERS.contains(&b),
                "{b} should classify as a browser"
            );
        }
    }
}
