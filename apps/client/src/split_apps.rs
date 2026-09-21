//! Running-app discovery for split tunneling (read-only `/proc` scan, no root).
//!
//! Lists GUI-ish processes grouped into browsers vs other apps. The helper
//! matches on the raw exe `name` (that is what `find_pids_by_name` compares),
//! while `friendly` is only display text.

use std::collections::BTreeMap;

/// Well-known browser exe basenames (lowercase). Covers distro package
/// names, snap/flatpak wrappers, and newer Chromium forks.
const BROWSERS: &[&str] = &[
    "firefox",
    "firefox-esr",
    "firefox-bin",
    "librewolf",
    "waterfox",
    "floorp",
    "zen",
    "zen-browser",
    "google-chrome",
    "google-chrome-stable",
    "google-chrome-beta",
    "chrome",
    "chromium",
    "chromium-browser",
    "chromium-bin",
    "brave",
    "brave-browser",
    "brave-browser-stable",
    "brave-browser-beta",
    "opera",
    "opera-stable",
    "opera-beta",
    "vivaldi",
    "vivaldi-stable",
    "vivaldi-snapshot",
    "microsoft-edge",
    "microsoft-edge-stable",
    "microsoft-edge-beta",
    "edge",
    "falkon",
    "epiphany",
    "gnome-web",
    "konqueror",
    "qutebrowser",
    "nyxt",
    "pale-moon",
    "palemoon",
    "seamonkey",
    "tor-browser",
    "torbrowser-launcher",
    "ungoogled-chromium",
    "chromium-freeworld",
    "arc",
    "sigmaos",
    "min",
    "badwolf",
    "surf",
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
        "firefox" | "firefox-esr" | "firefox-bin" => "Firefox".to_string(),
        "librewolf" => "LibreWolf".to_string(),
        "floorp" => "Floorp".to_string(),
        "zen" | "zen-browser" => "Zen Browser".to_string(),
        "google-chrome" | "google-chrome-stable" | "google-chrome-beta" | "chrome" => "Google Chrome".to_string(),
        "chromium" | "chromium-browser" | "chromium-bin" | "ungoogled-chromium" => "Chromium".to_string(),
        "brave" | "brave-browser" | "brave-browser-stable" | "brave-browser-beta" => "Brave".to_string(),
        "opera" | "opera-stable" | "opera-beta" => "Opera".to_string(),
        "vivaldi" | "vivaldi-stable" | "vivaldi-snapshot" => "Vivaldi".to_string(),
        "microsoft-edge" | "microsoft-edge-stable" | "microsoft-edge-beta" | "edge" => "Edge".to_string(),
        "falkon" => "Falkon".to_string(),
        "epiphany" | "gnome-web" => "GNOME Web".to_string(),
        "qutebrowser" => "qutebrowser".to_string(),
        "nyxt" => "Nyxt".to_string(),
        "thunderbird" => "Thunderbird".to_string(),
        "discord" => "Discord".to_string(),
        "spotify" => "Spotify".to_string(),
        "code" | "vscode" | "vscodium" => "VS Code".to_string(),
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

/// Scan `/proc` plus installed `.desktop` entries and return apps,
/// browsers first, then others — each alphabetical by display name.
/// Kernel threads and plumbing daemons are skipped. Installed-but-not-running
/// apps get empty `pids` so they can still be selected for isolation.
pub fn scan_running_apps() -> Vec<RunningApp> {
    let self_pid = std::process::id();
    let mut by_name: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
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
            if exe.starts_with('[') && exe.ends_with(']') {
                continue;
            }
            by_name.entry(lower).or_default().push(pid);
        }
    }
    // Merge installed GUI apps so the picker shows everything on the device,
    // not just currently-running processes. Cheap: two dirs, skip NoDisplay.
    let installed = scan_installed_desktop_exes();
    let desktop_browsers = installed
        .iter()
        .filter(|(_, b)| *b)
        .map(|(e, _)| e.to_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    for (exe, _) in &installed {
        let lower = exe.to_lowercase();
        if lower.is_empty() || HIDDEN.contains(&lower.as_str()) {
            continue;
        }
        by_name.entry(lower).or_default();
    }
    let mut browsers = Vec::new();
    let mut others = Vec::new();
    for (lower, mut pids) in by_name {
        pids.sort_unstable();
        pids.dedup();
        let is_browser =
            BROWSERS.contains(&lower.as_str()) || desktop_browsers.contains(&lower);
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
    const OTHER_CAP: usize = 300;
    if others.len() > OTHER_CAP {
        others.truncate(OTHER_CAP);
    }
    browsers.into_iter().chain(others).collect()
}

/// Installed GUI apps from XDG `.desktop` files.
/// Returns (exe basename, is_browser_category). Cached per call — caller
/// throttles to every few seconds, dirs are small.
fn scan_installed_desktop_exes() -> Vec<(String, bool)> {
    use std::collections::BTreeSet;
    let mut out: BTreeSet<(String, bool)> = BTreeSet::new();
    let mut dirs: Vec<std::path::PathBuf> = vec![
        std::path::PathBuf::from("/usr/share/applications"),
        std::path::PathBuf::from("/usr/local/share/applications"),
        std::path::PathBuf::from("/var/lib/snapd/desktop/applications"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(std::path::PathBuf::from(format!(
            "{home}/.local/share/applications"
        )));
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            if content.contains("NoDisplay=true") {
                continue;
            }
            let mut exec_bin: Option<String> = None;
            let mut is_browser = false;
            for line in content.lines() {
                let line = line.trim();
                if let Some(v) = line.strip_prefix("Exec=") {
                    // First token of Exec, strip path + args + % codes.
                    let token = v.split_whitespace().next().unwrap_or("").trim_matches('"');
                    let base = token.rsplit('/').next().unwrap_or(token);
                    let base = base.trim();
                    if !base.is_empty() && !base.starts_with('%') {
                        exec_bin = Some(base.to_string());
                    }
                } else if let Some(v) = line.strip_prefix("Categories=") {
                    if v.contains("WebBrowser") {
                        is_browser = true;
                    }
                }
                if exec_bin.is_some() && is_browser {
                    break;
                }
            }
            if let Some(bin) = exec_bin {
                // Strip snap wrapper prefixes like snap.run.
                let bin = bin.trim().to_string();
                if !bin.is_empty() {
                    // Merge: if same exe appears twice, browser wins.
                    out.remove(&(bin.clone(), false));
                    out.insert((bin, is_browser));
                }
            }
        }
        if out.len() > 600 {
            break;
        }
    }
    out.into_iter().collect()
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
