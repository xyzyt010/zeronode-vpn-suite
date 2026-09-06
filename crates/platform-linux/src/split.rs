//! Per-app split tunneling for Linux (cgroup2 + fwmark policy routing).
//!
//! Applied ON TOP of an already-established system-wide tunnel (any protocol:
//! OpenVPN / WireGuard / PPTP / Outline / Tor — all of them end in TUN routes
//! in the main table, which is all this module looks at). Two modes:
//!
//! - [`SplitMode::OnlyThese`]: the tunnel routes are *moved* out of the main
//!   table into private table 77. Only processes placed in the `split-vpn`
//!   cgroup (fwmark `0x51`, rule prio 480) use them; everything else follows
//!   the untouched physical default route. Tearing down restores the exact
//!   saved route specs, so disconnect/clear is lossless.
//! - [`SplitMode::AllExceptThese`]: the tunnel stays exactly as-is; processes
//!   in the `split-by` cgroup (fwmark `0x52`) are detoured to private table 78
//!   which mirrors the physical routes.
//!
//! Process selection is by executable basename (`firefox`, `chrome`, …).
//! A monitor thread re-scans every 4s so apps launched *after* enabling are
//! picked up too. Notes / honest limits:
//! - Matching is per-packet on the socket's cgroup: long-lived connections
//!   established *before* the process was moved keep their old path until
//!   they reconnect. Restarting the app moves it instantly.
//! - IPv4 only (the client disables IPv6 while a tunnel is up anyway).
//! - DNS still follows the tunnel's resolver while connected.
//! - `OnlyThese` needs a discoverable physical default (main-table default,
//!   an endpoint `/32` pin, or Tor's bypass table 100). If none exists the
//!   apply fails with a plain-language error instead of blackholing traffic.

use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::Duration;

const CGROUP_ROOT: &str = "/sys/fs/cgroup/zeronode-vpn";
const CGROUP_VPN: &str = "split-vpn";
const CGROUP_BY: &str = "split-by";
const TABLE_VPN: &str = "77";
const TABLE_PHYS: &str = "78";
const MARK_VPN: &str = "0x51";
const MARK_BY: &str = "0x52";
const RULE_PRIO: &str = "480";

/// Tunnel interface name shapes we manage. A name only counts when it is UP
/// *and* carries a tunnel route (default / 0/1 / 128/1) in the main table —
/// never on name alone.
fn is_tun_name(name: &str) -> bool {
    name == "znclient0"
        || name.starts_with("ZeroNode")
        || name.starts_with("tun")
        || name.starts_with("ppp")
        || name.starts_with("outline")
        || name.starts_with("wg")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitMode {
    OnlyThese,
    AllExceptThese,
}

impl SplitMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "only" | "onlythese" | "only_these" => Some(Self::OnlyThese),
            "except" | "allexcept" | "all_except" | "allexceptthese" => {
                Some(Self::AllExceptThese)
            }
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OnlyThese => "only",
            Self::AllExceptThese => "except",
        }
    }

    fn cgroup(self) -> &'static str {
        match self {
            Self::OnlyThese => CGROUP_VPN,
            Self::AllExceptThese => CGROUP_BY,
        }
    }

    fn mark(self) -> &'static str {
        match self {
            Self::OnlyThese => MARK_VPN,
            Self::AllExceptThese => MARK_BY,
        }
    }

    fn table(self) -> &'static str {
        match self {
            Self::OnlyThese => TABLE_VPN,
            Self::AllExceptThese => TABLE_PHYS,
        }
    }
}

struct AppliedSplit {
    mode: SplitMode,
    apps: Vec<String>,
    /// Exact main-table tunnel route specs moved to table 77 (OnlyThese).
    moved_routes: Vec<String>,
}

static SPLIT_STATE: Mutex<Option<AppliedSplit>> = Mutex::new(None);
static SPLIT_MON_STOP: Mutex<Option<std::sync::Arc<AtomicBool>>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// tiny command helpers
// ---------------------------------------------------------------------------

fn ip(args: &[&str]) -> bool {
    Command::new("ip")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ip_out(args: &[&str]) -> String {
    Command::new("ip")
        .args(args)
        .stdin(Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn iptables(args: &[&str]) -> bool {
    Command::new("iptables")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn main_routes() -> Vec<String> {
    ip_out(&["-4", "route", "show", "table", "main"])
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// (destination, dev) of a main-table route line, e.g.
/// `0.0.0.0/1 dev znclient0 scope link` → ("0.0.0.0/1", "znclient0").
fn route_dst_dev(line: &str) -> (String, String) {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let dst = toks.first().unwrap_or(&"").to_string();
    let mut dev = String::new();
    let mut i = 0;
    while i + 1 < toks.len() {
        if toks[i] == "dev" {
            dev = toks[i + 1].to_string();
            break;
        }
        i += 1;
    }
    (dst, dev)
}

fn iface_up(name: &str) -> bool {
    std::fs::read_to_string(format!("/sys/class/net/{name}/operstate"))
        .map(|s| s.trim() == "up" || s.trim() == "unknown")
        .unwrap_or(false)
}

/// Tunnel routes currently in the main table: (full line, dev) pairs whose
/// destination is a full-range catch (`default`, `0.0.0.0/1`, `128.0.0.0/1`)
/// on an UP tunnel-shaped interface.
fn tunnel_routes() -> Vec<(String, String)> {
    main_routes()
        .into_iter()
        .filter_map(|line| {
            let (dst, dev) = route_dst_dev(&line);
            let catch_all =
                dst == "default" || dst == "0.0.0.0/1" || dst == "128.0.0.0/1";
            if catch_all && !dev.is_empty() && is_tun_name(&dev) && iface_up(&dev) {
                Some((line, dev))
            } else {
                None
            }
        })
        .collect()
}

/// Physical default route discovery, best effort:
/// 1. a main-table `default … dev <phys>` that is NOT on a tunnel dev,
/// 2. an endpoint `/32 via <gw> dev <phys>` pin → reconstructed default,
/// 3. Tor's bypass table 100 (a snapshot of the pre-VPN physical default).
/// Returns `(gateway, phys_dev)`.
fn phys_default() -> Option<(String, String)> {
    let tun_devs: HashSet<String> =
        tunnel_routes().into_iter().map(|(_, d)| d).collect();
    let mut pin: Option<(String, String)> = None;
    for line in main_routes() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.first() == Some(&"default") {
            let mut gw = String::new();
            let mut dev = String::new();
            let mut i = 0;
            while i + 1 < toks.len() {
                match toks[i] {
                    "via" => gw = toks[i + 1].to_string(),
                    "dev" => dev = toks[i + 1].to_string(),
                    _ => {}
                }
                i += 1;
            }
            if !gw.is_empty() && !dev.is_empty() && !tun_devs.contains(&dev) {
                return Some((gw, dev));
            }
        }
        // `<a.b.c.d>/32 via <gw> dev <phys> …` endpoint pin.
        if pin.is_none()
            && toks.first().is_some_and(|d| {
                d.ends_with("/32") && !d.starts_with("127.") && !d.starts_with("10.")
            })
        {
            let mut gw = String::new();
            let mut dev = String::new();
            let mut i = 0;
            while i + 1 < toks.len() {
                match toks[i] {
                    "via" => gw = toks[i + 1].to_string(),
                    "dev" => dev = toks[i + 1].to_string(),
                    _ => {}
                }
                i += 1;
            }
            if !gw.is_empty() && !dev.is_empty() && !tun_devs.contains(&dev) {
                pin = Some((gw, dev));
            }
        }
    }
    if let Some(p) = pin {
        return Some(p);
    }
    // Tor's bypass table mirrors the pre-VPN physical default.
    for line in ip_out(&["-4", "route", "show", "table", "100"]).lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.first() == Some(&"default") {
            let mut gw = String::new();
            let mut dev = String::new();
            let mut i = 0;
            while i + 1 < t.len() {
                match t[i] {
                    "via" => gw = t[i + 1].to_string(),
                    "dev" => dev = t[i + 1].to_string(),
                    _ => {}
                }
                i += 1;
            }
            if !gw.is_empty() && !dev.is_empty() {
                return Some((gw, dev));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// cgroup membership
// ---------------------------------------------------------------------------

fn ensure_cgroup(name: &str) -> Option<String> {
    let dir = format!("{CGROUP_ROOT}/{name}");
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    Some(dir)
}

fn cgroup_rel(name: &str) -> String {
    format!("zeronode-vpn/{name}")
}

/// Move every running process whose exe basename matches `app` into `dir`.
/// Returns how many pids were moved.
fn move_app_pids(app: &str, dir: &str) -> usize {
    let mut moved = 0;
    for pid in crate::procfs::find_pids_by_name(app) {
        if std::fs::write(format!("{dir}/cgroup.procs"), pid.to_string()).is_ok() {
            moved += 1;
        }
    }
    moved
}

fn sync_membership(apps: &[String], dir: &str) -> usize {
    apps.iter().map(|a| move_app_pids(a, dir)).sum()
}

/// Move every member of `dir` into `dest` (`cgroup.procs` path).
fn migrate_all(dir: &str, dest_procs: &str) {
    if let Ok(members) = std::fs::read_to_string(format!("{dir}/cgroup.procs")) {
        for pid in members.split_whitespace() {
            let _ = std::fs::write(dest_procs, pid);
        }
    }
}

/// Best-effort cgroup removal. Members go back to the toplevel cgroup (NOT
/// our root: cgroup2 refuses rmdir on a cgroup that still holds processes,
/// so parking them in our root would make the root itself unremovable).
fn remove_cgroup(name: &str) {
    let dir = format!("{CGROUP_ROOT}/{name}");
    migrate_all(&dir, &format!("{CGROUP_ROOT}/cgroup.procs"));
    let _ = std::fs::remove_dir(&dir);
    // Our root only ever holds processes we parked there: hand them back to
    // toplevel too, then drop the root so `clear` leaves zero residue.
    migrate_all(CGROUP_ROOT, "/sys/fs/cgroup/cgroup.procs");
    let _ = std::fs::remove_dir(CGROUP_ROOT);
}

// ---------------------------------------------------------------------------
// iptables + policy rules
// ---------------------------------------------------------------------------

fn mark_rule_present(rel: &str, mark: &str) -> bool {
    iptables(&[
        "-t", "mangle", "-C", "OUTPUT", "-m", "cgroup", "--path", rel, "-j", "MARK",
        "--set-mark", mark,
    ])
}

fn ensure_mark_rule(rel: &str, mark: &str) {
    if !mark_rule_present(rel, mark) {
        iptables(&[
            "-t", "mangle", "-A", "OUTPUT", "-m", "cgroup", "--path", rel, "-j", "MARK",
            "--set-mark", mark,
        ]);
    }
}

fn delete_mark_rule(rel: &str, mark: &str) {
    let _ = iptables(&[
        "-t", "mangle", "-D", "OUTPUT", "-m", "cgroup", "--path", rel, "-j", "MARK",
        "--set-mark", mark,
    ]);
}

fn ensure_fwmark_rule(mark: &str, table: &str) {
    // Idempotent: drop ours first, then add.
    let _ = ip(&["rule", "del", "fwmark", mark, "lookup", table]);
    let _ = ip(&["rule", "add", "prio", RULE_PRIO, "fwmark", mark, "lookup", table]);
}

fn delete_fwmark_rule(mark: &str, table: &str) {
    let _ = ip(&["rule", "del", "fwmark", mark, "lookup", table]);
}

fn add_route_to_table(line: &str, table: &str) {
    let mut args: Vec<&str> = line.split_whitespace().collect();
    if args.is_empty() {
        return;
    }
    // `ip route add <spec> table <T>` — use replace so re-applies converge.
    let mut cmd = vec!["-4", "route", "replace"];
    cmd.extend(args.drain(..));
    cmd.extend(["table", table]);
    let _ = ip(&cmd);
}

// ---------------------------------------------------------------------------
// public API
// ---------------------------------------------------------------------------

/// Snapshot for status replies: (active, mode, app count).
pub fn split_status() -> (bool, String, usize) {
    let guard = SPLIT_STATE.lock().ok();
    match guard.as_ref().and_then(|s| s.as_ref()) {
        Some(a) => (true, a.mode.as_str().to_string(), a.apps.len()),
        None => (false, String::new(), 0),
    }
}

/// Apply split tunneling for `apps` (exe basenames). Idempotent: re-applying
/// only re-syncs cgroup membership unless the mode changed. Must be called
/// while a system-wide tunnel is up (the GUI sends it right after connect).
pub fn apply_split_tunnel(mode_str: &str, apps: Vec<String>) -> Result<String, String> {
    let mode = SplitMode::parse(mode_str)
        .ok_or_else(|| format!("unknown split mode '{mode_str}' (want 'only' or 'except')"))?;
    let apps: Vec<String> = apps
        .into_iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect();
    if apps.is_empty() {
        return Err("no apps selected for split tunneling".to_string());
    }

    // Fast path: same mode already live → just re-sync membership (cheap,
    // keeps OnlyThese route surgery untouched).
    if let Ok(state) = SPLIT_STATE.lock() {
        if let Some(cur) = state.as_ref() {
            if cur.mode == mode {
                let dir = format!("{CGROUP_ROOT}/{}", mode.cgroup());
                let n = sync_membership(&apps, &dir);
                drop(state);
                if let Ok(mut s) = SPLIT_STATE.lock() {
                    if let Some(cur) = s.as_mut() {
                        cur.apps = apps.clone();
                    }
                }
                return Ok(format!(
                    "split tunneling re-synced ({} app{}, {} process{} moved)",
                    apps.len(),
                    if apps.len() == 1 { "" } else { "s" },
                    n,
                    if n == 1 { "" } else { "es" },
                ));
            }
        }
    }

    // Mode (re)build: tear down previous state first (restores main table).
    let _ = clear_split_tunnel();

    let tun = tunnel_routes();
    if tun.is_empty() {
        return Err(
            "no active VPN tunnel detected — connect first, then enable split tunneling"
                .to_string(),
        );
    }
    let (gw, phys) = phys_default().ok_or_else(|| {
        "could not find the physical default route — split tunneling needs it; \
         'All except these' may work once a normal (non-PPTP) connection is up"
            .to_string()
    })?;

    let mut saved_moved: Vec<String> = Vec::new();
    if mode == SplitMode::OnlyThese {
        // Move every tunnel catch-all out of main into table 77.
        let mut moved = Vec::new();
        for (line, _) in &tun {
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks.is_empty() {
                continue;
            }
            // `ip route del <spec>` with the exact stored tokens.
            let mut del = vec!["-4", "route", "del"];
            del.extend(toks.iter().copied());
            if ip(&del) {
                add_route_to_table(line, TABLE_VPN);
                moved.push(line.clone());
            }
        }
        if moved.is_empty() {
            return Err("could not move tunnel routes (they may already be gone)".to_string());
        }
        // LAN stays direct for tunneled apps (mirrors the Tor bypass table).
        for line in main_routes() {
            let (dst, dev) = route_dst_dev(&line);
            if dev == phys && (line.contains("scope link") || dst.contains('/')) {
                add_route_to_table(&line, TABLE_VPN);
            }
        }
        // Remember the exact specs so clear/disconnect can restore main.
        saved_moved = moved;
    }

    // Physical mirror table 78 (used by AllExceptThese; harmless otherwise).
    let _ = ip(&["-4", "route", "flush", "table", TABLE_PHYS]);
    for line in main_routes() {
        let (_, dev) = route_dst_dev(&line);
        if dev == phys {
            add_route_to_table(&line, TABLE_PHYS);
        }
    }
    // pppd-style setups replace the phys default: reconstruct it from the pin.
    let has_def = ip_out(&["-4", "route", "show", "table", TABLE_PHYS])
        .lines()
        .any(|l| l.trim_start().starts_with("default"));
    if !has_def {
        let _ = ip(&[
            "-4", "route", "replace", "default", "via", &gw, "dev", &phys, "table",
            TABLE_PHYS,
        ]);
    }

    let dir = ensure_cgroup(mode.cgroup())
        .ok_or_else(|| "cannot create split-tunnel cgroup (need cgroup2 + root)".to_string())?;
    let rel = cgroup_rel(mode.cgroup());
    let n = sync_membership(&apps, &dir);
    ensure_mark_rule(&rel, mode.mark());
    ensure_fwmark_rule(mode.mark(), mode.table());

    {
        let mut state = SPLIT_STATE.lock().map_err(|e| format!("split lock: {e}"))?;
        *state = Some(AppliedSplit { mode, apps: apps.clone(), moved_routes: saved_moved });
    }

    start_monitor(apps.clone(), dir);
    let what = match mode {
        SplitMode::OnlyThese => "only the selected apps use the VPN",
        SplitMode::AllExceptThese => "everything except the selected apps uses the VPN",
    };
    Ok(format!(
        "split tunneling on ({what}): {} app{}, {} running process{} placed",
        apps.len(),
        if apps.len() == 1 { "" } else { "s" },
        n,
        if n == 1 { "" } else { "es" },
    ))
}

/// Tear everything down and — for OnlyThese — restore the exact tunnel routes
/// saved at apply time, but only while the tunnel device still exists (after
/// a disconnect the tunnel's own teardown owns the routes; restoring then
/// would plant dead routes).
pub fn clear_split_tunnel() -> Result<String, String> {
    stop_monitor();
    for mode in [SplitMode::OnlyThese, SplitMode::AllExceptThese] {
        delete_fwmark_rule(mode.mark(), mode.table());
        delete_mark_rule(&cgroup_rel(mode.cgroup()), mode.mark());
        remove_cgroup(mode.cgroup());
    }
    let _ = ip(&["-4", "route", "flush", "table", TABLE_VPN]);
    let _ = ip(&["-4", "route", "flush", "table", TABLE_PHYS]);

    let prev = SPLIT_STATE.lock().map_err(|e| format!("split lock: {e}"))?.take();
    if let Some(applied) = prev {
        if applied.mode == SplitMode::OnlyThese {
            for line in &applied.moved_routes {
                let (dst, dev) = route_dst_dev(line);
                if dev.is_empty() || !iface_up(&dev) {
                    continue; // tunnel already gone — its teardown owns this.
                }
                // Don't duplicate: skip if an identical catch-all is back.
                let already = main_routes().iter().any(|l| {
                    let (d, dv) = route_dst_dev(l);
                    d == dst && dv == dev
                });
                if already {
                    continue;
                }
                let toks: Vec<&str> = line.split_whitespace().collect();
                let mut add = vec!["-4", "route", "add"];
                add.extend(toks.iter().copied());
                let _ = ip(&add);
            }
        }
        return Ok("split tunneling off".to_string());
    }
    Ok("split tunneling already off".to_string())
}

// ---------------------------------------------------------------------------
// monitor: pick up apps launched after enabling
// ---------------------------------------------------------------------------

fn start_monitor(apps: Vec<String>, dir: String) {
    stop_monitor();
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    *SPLIT_MON_STOP.lock().unwrap() = Some(stop.clone());
    let _ = std::thread::Builder::new()
        .name("zn-split-monitor".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(4));
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                // Only while our state is still live (a clear stops us anyway).
                if SPLIT_STATE.lock().ok().map(|s| s.is_some()).unwrap_or(false) {
                    let _ = sync_membership(&apps, &dir);
                } else {
                    break;
                }
            }
        });
}

fn stop_monitor() {
    if let Ok(mut slot) = SPLIT_MON_STOP.lock() {
        if let Some(stop) = slot.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }
}
