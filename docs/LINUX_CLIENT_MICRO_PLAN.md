# ZeroNode VPN Client — Linux Micro Plan (Phase 1: Backend)

**Goal:** Port the Windows ZeroNode client experience to Linux Ubuntu (22.04/24.04+) with full protocol
integration — WireGuard, OpenVPN, Shadowsocks/Outline, PPTP, Tor — targeting **X11/XFCE4** and
**GNOME/Wayland**, reusing the exact egui GUI, globe artifact ("earth"), animations and interaction model.

**Scope lock (this phase):**
1. Full backend: every tunnel engine that platform-windows exposes, implemented in `crates/platform-linux`
   with the **same public API surface** so `apps/client` needs only thin `#[cfg]` dispatch changes.
2. Resource gathering: pinned external binaries/assets (Tor expert bundle, font strategy, GeoIP DBs,
   flags) plus apt dependency list.
3. Sufficient frontend work ONLY where required to drive/test the backend (protocol panels unblocked on
   Linux, fonts, elevation UX parity). Full tray/GNOME-shell polish = Phase 2.

**Output artifact:** `vpn-client` (egui app) + upgraded `libvpn_platform_linux`, packaged as `.deb`
(`zeronode-vpn-client`), verified on X11/XFCE4 and GNOME/Wayland sessions.

---

## Architecture (final)

```
┌───────────────────────────────────────────────────────────────────────────┐
│ apps/client  (egui UI — IDENTICAL look/animations on Win/Linux)           │
│  globe renderer (painter-based, portable) · protocol panels · dialogs     │
│  cfg(linux) → vpn_platform_linux::*   cfg(windows) → platform_windows::*  │
└──────────────┬────────────────────────────────────────────────────────────┘
               │ identical fn signatures (parity contract, §Parity)
┌──────────────▼────────────────────────────────────────────────────────────┐
│ crates/platform-linux                                                     │
│  elevation   : uid check + pkexec relaunch (UAC-parity UX)                │
│  wg_kernel   : kernel WireGuard via wireguard-control (full-tunnel mode)  │
│  wg_userspace: boringtun + vendored `tun` TUN pump (fallback, port of Win)│
│  socks_tun   : tun2proxy + vendored tproxy-config (shared engine)         │
│  outline     : embedded shadowsocks-service local SOCKS5 (+system route)  │
│  tor         : bundled tor lifecycle (app layer) + socks_tun system route │
│  openvpn     : binary discovery, runtime profile, spawn + log state mach. │
│  pptp        : pppd + pptp-linux (pon-equivalent direct invocation)       │
│  proc        : procfs scan/kill, silent-run equivalents, systemd probes   │
└──────────────┬────────────────────────────────────────────────────────────┘
               │
┌──────────────▼────────────────────────────────────────────────────────────┐
│ OS layer (Ubuntu)                                                         │
│  /dev/net/tun · kernel wireguard · iproute2 rtnetlink · nftables          │
│  /etc/resolv.conf bind-mount override (tproxy-config) · polkit/pkexec     │
│  openvpn (apt) · pptp-linux+ppp+mppe (apt) · tor (bundled | apt fallback) │
└───────────────────────────────────────────────────────────────────────────┘
```

**Locked asset sources**
- Tor Expert Bundle Linux x86_64: `https://archive.torproject.org/tor-package-archive/torbrowser/15.0.17/tor-expert-bundle-linux-x86_64-15.0.17.tar.gz` (31 MB, same version as repo's Windows bundle 15.0.17). Contains `tor/tor`, `tor/geoip*`, pluggable transports.
- Fonts: DejaVu Sans / DejaVu Sans Mono (`fonts-dejavu-core`, always present on Ubuntu) with Noto fallback probing; final fallback = egui default fonts (never hard-fail).
- GeoIP: unchanged core pipeline (`dbip-country-lite`/`dbip-asn-lite` mmdb download) — already cross-platform.
- Flags/globe assets: already compile-time/runtime staged by `apps/client/build.rs` (verify Linux staging path).

**Research verdicts (why these choices)**
| Topic | Decision | Rationale |
|---|---|---|
| GUI toolkit | Keep eframe/egui 0.29 (glow) | Window app is already egui; winit 0.30 runs native Wayland **and** X11; glow→EGL/GLX automatic |
| Globe | Port unchanged | Pure `egui::Painter` wireframe (renderer.rs) — zero GL, zero OS coupling; mesh.rs/textures are dead code we ignore |
| Backend selection | `event_loop_builder` hook → detect `WAYLAND_DISPLAY`/`DISPLAY`; explicit `with_wayland()`/`with_x11()`; env overrides `ZERONODE_BACKEND=x11\|wayland` | Deterministic across DEs; avoids winit auto-detect edge cases on mixed sessions |
| Elevation | Parity with Windows UAC flow: `pkexec --disable-internal-agent` relaunch of self with preserved args + auto-connect flags | Same UX as Windows (`is_elevated → relaunch_elevated_with_args → exit_after_relaunch`); polkit agent dialog exists on GNOME & XFCE; no setuid helper needed in Phase 1 |
| WireGuard client | Primary: kernel WG (`wireguard-control`) upgraded to full-tunnel (two-/1 routes via tproxy-config helpers), endpoint-pin host route (anti-loop), DNS override, MTU. Secondary: boringtun+vendored `tun` userspace pump (direct port of `platform-windows/wireguard_tunnel.rs`) for kernels without module / non-root-friendly split | Kernel path is already half-built in platform-linux; userspace path gives byte-for-byte behavioral parity with Windows and a tested fallback |
| System-wide SOCKS (Tor & Outline) | `tun2proxy` crate + vendored `tproxy-config` (its own dep) | Vendor dir already contains complete Linux impls (rtnetlink two-/1 takeover, fwmark policy routing, resolv.conf read-only bind-mount, rollback). Identical engine to Windows/Android — proven pattern in this repo |
| Shadowsocks | Embedded `shadowsocks-service` (same crate/version as Windows) | In-process sslocal is OS-neutral; only system-route step differs |
| Tor | Bundle expert-bundle tar.gz into deb assets; resolution order: exe-dir `/usr/share/vpn-client/tor/tor` → `$PATH` (`tor`) → data dir; generated torrc identical to Windows (SocksPort NoIsolateDestAddr/Port, GeoIPFile, AvoidDiskWrites, notice log) | Mirrors Windows resolver chain; apt `tor` fallback keeps app functional without bundled payload |
| OpenVPN | Spawn distro `openvpn` binary (apt dep), same log-state machine (`wait_for_openvpn_up` markers), auth-user-pass `.auth` file, runtime profile rewrite minus `windows-driver` line | Windows app deliberately avoids management console; log parsing ports cleanly; `--dev tun` + `/dev/net/tun` needs root → runs under elevated relaunch, same as Windows |
| PPTP | `pppd` + `pptp-linux`: `pppd pty "pptp HOST --nolaunchpppd" call zeronode-pptp` with managed `/etc/ppp/chap-secrets` entry + peers file; status = pppd pid + `ppp0` presence | Direct pppd invocation (not pon) for controllability; MS-CHAPv2+MPPE parity with Windows `rasdial`; core `pptp.rs` docs already anticipate "Linux pppd" |
| DNS handling | tproxy-config bind-mount override during system tunnels; kernel-WG path writes via `resolvectl` (systemd-resolved detected) else direct resolv.conf write with backup/restore | Matches vendor impl; NM-safe |
| Tray icon | **Phase 2** (tray-icon crate: GTK+libappindicator; works GNOME-SNI-extension & XFCE 4.18 panel) | Backend-first mandate; tray needs gtk3 dev linkage decisions isolated from backend risk |

---

## Parity contract — platform-linux public API (must match platform-windows names/signatures)

```
// elevation
is_elevated() -> bool
relaunch_elevated_with_args(extra_args: &[&str]) -> Result<()>          // pkexec
exit_after_relaunch() -> !
// wireguard (global single-tunnel slot)
parse_wireguard_config(&str) -> Result<TunnelConfig>
start_wireguard_global(TunnelConfig) -> Result<()>                      // userspace boringtun OR kernel bridge
stop_wireguard_global() -> Result<()>
is_wireguard_running() -> bool
// outline/shadowsocks
start_outline(method:&str,password:&str,server_host:&str,server_port:u16,system_wide:bool) -> Result<u16>
stop_outline() -> Result<()>
is_outline_running() -> bool
outline_socks_port() -> Option<u16>
// tor system routing
start_tor_system_tunnel(socks_port: u16) -> Result<()>
stop_tor_system_tunnel() -> Result<()>
is_tor_tunnel_running() -> bool
start_socks_system_tunnel(socks_port: u16, tun_name: &str, extra_bypass: &[String]) -> Result<()>   // shared engine
stop_socks_system_tunnel() -> Result<()>
// pptp
start_pptp(server:&str, username:&str, password:&str) -> Result<()>
stop_pptp() -> Result<()>
is_pptp_running() -> bool
// control-plane WG client (existing, upgraded)
apply_client_tunnel(...) / remove_client_tunnel() / client_tunnel_status()
// process infra
kill_process_by_name(name:&str) -> u32                                  // procfs+kill(2) ≙ kill_process_image
process_exists(image/name) / find_pid(...)
// setup vocabulary
client_setup_checks() -> Vec<SetupCheck>                                // /dev/net/tun, wg module, pkexec, deps
```

Windows-side call sites in `apps/client/src/app.rs` get mechanical mapping:
`app.rs:5762-5773,6050-6060,6307-6317,6469-6476,7016-7025` (elevation),
`app.rs:6000-6155` (WG), `5724-5943` (OVPN), `6157-6230` (PPTP), `6259-6398` (Outline),
`6460-6756` (Tor), `6758-6853` (disconnect teardown), `4019-4027` (wininet hint → no-op on Linux).

---

## Phases / Blocks

### Phase A — Foundation & resource gathering
| # | Step | Done when |
|---|---|---|
| A1 | Create `docs/LINUX_CLIENT.md` resource sheet (asset URLs, sha256 placeholders, apt list, version pins) | Doc lists every external resource + source URL |
| A2 | Download `tor-expert-bundle-linux-x86_64-15.0.17.tar.gz`, extract layout inventory (binaries/geoip/PTs), stage under `apps/client/assets/tor-linux/` (gitignored payload fetcher script `tools/fetch-tor-linux.sh` mirrors Windows flow) | Script reproducibly populates assets dir; layout documented |
| A3 | Extend `crates/platform-linux/Cargo.toml`: tokio, boringtun 0.7.x, tun (workspace patch applies), tun2proxy 0.8.x, shadowsocks-service 1.24, libc, rand, flate2 already via core | `cargo check -p vpn-platform-linux` green on linux target |
| A4 | Split platform-linux into modules: `lib.rs` (re-exports + server code untouched), `elevation.rs`, `proc.rs`, `wg_kernel.rs`, `wg_userspace.rs`, `socks_tun.rs`, `outline.rs`, `pptp.rs`, `openvpn.rs`, `setup.rs` | Module tree compiles; server fns byte-identical behavior |
| A5 | Add `client_setup_checks()` reporting: /dev/net/tun, `modprobe wireguard` availability, pkexec presence, openvpn/pptp-linux/tor presence, cap_net_admin capability note | Function returns SetupCheck vec consumed by app diagnostics |
| A6 | Update `tools/provision-ubuntu-builder.sh`: add `libgtk-3-dev libxdo-dev libayatana-appindicator3-dev` (future tray), ensure `openvpn pptp-linux ppp nftables kmod iproute2 policykit-1` listed as runtime deps section | Builder script provisions everything to compile all blocks |

### Phase B — Process & elevation infrastructure
| # | Step | Done when |
|---|---|---|
| B1 | `proc.rs`: scan /proc for cmdline-exe match, `kill_process_by_name`, graceful TERM→KILL ladder, self-skip | Unit test kills a spawned sleep process by name |
| B2 | `elevation.rs`: `is_elevated()` (geteuid==0, hardened via /proc/self/status read like current code), `pkexec_args()` builder preserving argv + appending auto-connect extras, `relaunch_elevated_with_args()` spawning `pkexec --disable-internal-agent <exe> <args>` , `exit_after_relaunch()` | Manual test: clicking a protected action pops polkit dialog on both DEs; decline returns friendly error (map polkit cancel) |
| B3 | Env sanitization for elevated child (drop WAYLAND_DISPLAY/DISPLAY only where child doesn't need them; keep HOME remap to target user's for config paths) — document chosen semantics | Elevated relaunch reaches GUI on Wayland session without crash; config paths stay in invoking user's home |
| B4 | Command runner: `run(cmd,args)` capturing output with timeouts (≙ silent_output), used everywhere instead of ad-hoc Command | Helper exists + used by subsequent blocks |

### Phase C — WireGuard client (kernel, full-tunnel)
| # | Step | Done when |
|---|---|---|
| C1 | Port pure parser: copy `parse_client_config` + `resolve_endpoint` + key/cidr helpers from platform-windows into shared location (either `vpn-suite-core::wireguard` new `parse_client_config` or platform-linux copy — pick core to dedupe) | Same fixture .conf parses identically on both platforms (unit test) |
| C2 | `wg_kernel.rs`: apply TunnelConfig to `znclient0` (exists today) **extended**: endpoint pin route via detected physical default gw (`ip route get 1.1.1.1` parse), AllowedIPs-driven route install (full-tunnel ⇒ two /1 replaces, split ⇒ per-CIDR), MTU set (`ip link set mtu`), DNS override (resolvectl detect → else resolv.conf backup/restore), metric pinning | Netns test: full-tunnel bring-up inside namespace routes ICMP through wg iface; DNS file shows tunnel DNS |
| C3 | Teardown/rollback: remove pin route, /1 routes, restore DNS bytes exactly (pre-read snapshot), delete iface; idempotent double-stop | Repeated stop/start cycles leave `ip rule/route/resolv.conf` pristine |
| C4 | Status probe: handshake/latest-handshake via Device::get + rx/tx counters surfaced into SetupCheck/status string | Status visible in CLI before GUI wiring |
| C5 | Kill-switch option (block non-tunnel egress while connected): optional `nft insert` forward/out drop rules tagged `zeronode-clientswitch`, removed on stop | Toggle test passes; default OFF (parity with Windows behavior) |

### Phase D — WireGuard userspace fallback (boringtun + TUN pump)
| # | Step | Done when |
|---|---|---|
| D1 | Port `WireGuardTunnel` struct + packet pump thread from `platform-windows/wireguard_tunnel.rs` swapping wintun adapter for vendored `tun` create_dev (`znwg0u`), ioctl address/MTU config | Compiles against linux tun API |
| D2 | Routing for userspace mode reuses tproxy-config linux helpers (`ip_route_add` two-/1, bypass for server endpoint, DNS bind-mount) instead of netsh equivalents | Full-tunnel through userspace pump verified in netns (ICMP + TCP) |
| D3 | `start_wireguard_global(TunnelConfig)` strategy switch: prefer kernel (C2) when module present AND config feasible; else userspace; expose which engine ran via status | Both engines reachable behind one API; forced-engine env var for tests (`ZERONODE_WG_ENGINE=kernel\|userspace`) |
| D4 | Global slot + liveness probe + stop parity (`OnceLock<Mutex<Option<…>>>` idiom copied) | is_wireguard_running reflects real state after crash-kill of pump thread |

### Phase E — Shared SOCKS→TUN system tunnel engine
| # | Step | Done when |
|---|---|---|
| E1 | Port `tor_tunnel.rs` generic core → `socks_tun.rs`: Args construction (ArgProxy::Socks5 127.0.0.1:P, setup=true, dns OverTcp, ipv6 off), bypass CIDR builder (loopback/RFC1918/link-local + caller extras), worker-thread + CancellationToken lifecycle, startup liveness probe, log-file sink | Engine brings up TUN on linux in netns-less manual test (root), curl through SOCKS exits remotely |
| E2 | Teardown parity: token cancel, join ≤1200 ms, verify tproxy-config rollback restored routes/rules/resolv.conf | Post-stop diff of `ip route/rule/resolv.conf` clean |
| E3 | Anti-loop bypass resolution helper: resolve hostname → /32,/128 CIDRs (port `resolve_server_bypass_cidrs`) | Unit test resolves example host to expected CIDR strings |

### Phase F — Outline / Shadowsocks
| # | Step | Done when |
|---|---|---|
| F1 | Port `outline_tunnel.rs` (embedded `shadowsocks_service::run_local`) nearly verbatim — pick free port, wait_for_port readiness, worker+oneshot cancel | Local SOCKS5 up; curl --socks5 through it fetches remote IP |
| F2 | `system_wide=true` → call E1 engine with tun name `ZeroNodeOutline` + server bypass CIDRs | System traffic routed via SS; stop restores |
| F3 | `start_outline(...)->Result<u16>` / stop / is_running / socks_port exported per contract | Signature-level parity compile-checked from app side |

### Phase G — Tor
| # | Step | Done when |
|---|---|---|
| G1 | Binary resolver: `exe_dir/assets/tor/tor` → `/usr/share/vpn-client/tor/tor` → `$PATH` → error with remedy text; chmod +x enforcement post-install | Resolver unit test with fake tree |
| G2 | Port app-layer tor launcher into reusable fn (still owned by app, calling platform proc helpers): free SOCKS port pick, generated torrc (identical fields to Windows: SocksPort NoIsolateDestAddr NoIsolateDestPort, GeoIPFile(s), AvoidDiskWrites, DataDirectory, Log notice file), spawn detached | tor boots headless on Ubuntu; SOCKS answers; exit resolved ≤180 s on clean network |
| G3 | `start_tor_system_tunnel(socks_port)` → E1 engine with tor-guard bypass collection: parse `ss -tnp`/procfs sockets for tor-owned ESTABLISHED peers → /32 bypasses (port of `tor_or_peer_bypass_strings`) | Feedback loop impossible: guard IPs present as bypass routes before TUN default takeover |
| G4 | Exit-location GeoIP flow unchanged (`tor_geo.rs` reqwest socks5h pinned auth) — verify crate builds on linux, no win32 paths | Exit country/IP card populates on Linux UI |
| G5 | Isolation mode "apps": port `launch_tor_isolated_app` (env ALL_PROXY/HTTP(S)_PROXY + chromium `--proxy-server`, terminal detach) using `/usr/bin/env` spawn semantics; works for firefox/chromium flatpak caveat documented | Launching firefox via Tor shows Tor exit IP in browser test |

### Phase H — OpenVPN
| # | Step | Done when |
|---|---|---|
| H1 | Discovery: `$ZERONODE_OPENVPN` → exe-dir sibling `openvpn` → `/usr/sbin/openvpn`,`/usr/bin/openvpn` → PATH; absent ⇒ SetupCheck Fail with `sudo apt install openvpn` remedy (no MSI-style downloader on Linux) | Resolver tested; missing-binary message actionable |
| H2 | Runtime profile prep: port `prepare_runtime_profile_with_driver` minus `windows-driver` injection; force `dev tun`, file-based auth forward-slash paths, redirect-gateway def1, auth-nocache; keep script-security minimal | Generated .runtime.ovpn validated by `openvpn --config --test-crypto`-style dry parse (or `--show-ciphers` sanity) |
| H3 | Spawn + log state machine port: `spawn_openvpn_with_log`, marker scanner (`OvpnTunnelState` Connecting/Up/AuthFailed/Fatal/Exited), `wait_for_openvpn_up` 400 ms×60 s, stderr merge | Against public test server (or local dummy ovpn server in netns) reaches Up; wrong creds ⇒ AuthFailed mapped to UI error |
| H4 | Process mgmt: terminate by recorded PID first (pidfile), fallback `kill_process_by_name("openvpn")`; stale-log cleanup | Disconnect leaves no openvpn processes; logs rotated per-profile |
| H5 | Credentials dialog flow (auth-user-pass) reused as-is; `.auth` written 0600 | File perms correct |

### Phase I — PPTP
| # | Step | Done when |
|---|---|---|
| I1 | Dep checks: `pppd`, `pptp` binaries + kernel `ppp_mppe` module probe (`modprobe ppp_mppe` attempt), SetupChecks otherwise | Missing-dep remedy strings accurate |
| I2 | Profile writer: `/etc/ppp/peers/zeronode-pptp` (pty pptp line, name entry, require-mppe-128 optional, defaultroute toggle, nodeflate, noauth-not) + managed `chap-secrets` block delimited by comment markers (idempotent rewrite) | Files generated correctly; repeated connects don't duplicate secrets entries |
| I3 | `start_pptp(server,user,pass)`: write profile → spawn pppd detached → poll `ppp0` presence + pppd alive 30×200 ms (parity with rasdial poll) → return; store pidfile | Connect to test PPTP server yields ppp0 with assigned IP |
| I4 | `stop_pptp`: TERM pidfile pid → `poff`-equivalent fallback → sweep by name; `is_pptp_running` = pidfile alive ∨ any pppd with our peer file | Clean teardown; routes restored |
| I5 | Security warning banner from `core::pptp::SECURITY_WARNING` shown in panel (already wired on Windows — verify Linux path shows it too) | UI parity confirmed |

### Phase J — Control-plane client path + host-setup upgrades
| # | Step | Done when |
|---|---|---|
| J1 | Upgrade `apply_client_tunnel` (ZeroNode-server flow) from server-/32-split to honor lease AllowedIPs/full-tunnel via C2 machinery | E2E ping-through-tunnel test still green + full-tunnel variant added |
| J2 | Server NAT completion (bonus, unblocks real internet egress over ZeroNode servers): nft masquerade rule in existing `inet zeronode` table + forward accepts, applied with host-setup, removable | `apply_server_host_setup` then client can reach internet via server |
| J3 | Disconnect teardown matrix: per-prefix stop calls mirrored from Windows `disconnect` (`app.rs:6758-6853`) with linux fns; restore real IP refresh | Every protocol connect→disconnect cycle leaves zero residue (routes, DNS, processes, interfaces) |

### Phase K — App-layer integration (minimum viable frontend for backend)
| # | Step | Done when |
|---|---|---|
| K1 | Replace every `"Windows-first"` guard (`app.rs:6139-6151,6218-6226,6384-6394,7061-7066`) with cfg-dispatch to platform-linux fns; Windows paths untouched | `cargo check` both targets; protocol buttons live on Linux |
| K2 | Elevation UX swap at 5 sites: `is_elevated/relaunch_elevated_with_args/exit_after_relaunch` now resolve per-OS (import shim module in app) | Polkit flow drives auto-connect flags exactly like UAC did |
| K3 | Font loader: probe DejaVu/Noto/Liberation families (proportional+mono) → fallback defaults; sizes/theme untouched | Text renders crisp on XFCE & GNOME; no panic if fonts missing |
| K4 | Fix `resolve_flag_path` hazards (hardcoded Downloads/user paths) → exe-dir assets + installed share path + embedded centroid fallback | Flags render from installed deb location |
| K5 | Panic logger + tracing paths verified under `~/.local/share/ZeroNode/` ProjectDirs (core app_paths already handles); log file writable pre-elevation | Logs appear in expected dirs both normal & elevated runs |
| K6 | `main.rs`: gate `run_wireguard_tunnel_service_if_requested` to Windows; add Linux `--service-marker` stub returning None | CLI parses cleanly on Linux |
| K7 | Auto-connect flag consumption parity (`--auto-connect-{tor,ovpn,wg,outline}`) exercised end-to-end after pkexec relaunch | Each protocol auto-connects post-elevation once |

### Phase L — Desktop environment adaptation (X11/XFCE4 + GNOME/Wayland)
| # | Step | Done when |
|---|---|---|
| L1 | Backend selector: detect `WAYLAND_DISPLAY` vs `DISPLAY`; `event_loop_builder` hook forcing winit backend; `ZERONODE_BACKEND` override; log chosen combo | Forced runs on both stacks boot correctly; wrong-stack clear error |
| L2 | Wayland identity: `ViewportBuilder::with_app_id("io.zeronode.vpn")` (matches desktop file) so GNOME maps icon/taskbar; X11: set WM_CLASS via app_id path too | gnome-shell dash shows proper icon+name; xprop WM_CLASS correct |
| L3 | Icon pipeline: PNG→RGBA IconData already portable; verify no winres-only assumptions; multi-size icon for GNOME | Window icon visible on both DEs titlebar/dock |
| L4 | Scaling QA: fractional scaling on GNOME Wayland (egui pixels_per_point auto) and XFCE HiDPI; globe hit-testing under scale factors (hover radius math uses physical px today — verify/adjust) | No blurry text; globe hover/click accurate at 1×/1.5×/2× |
| L5 | `centered=true` no-op on Wayland noted; acceptable deviation logged in doc | Known-issue entry written |
| L6 | Drag-drop import paths (`.ovpn/.conf`) via winit file-drop — confirm event delivery on both backends | Dropping profiles onto window imports on X11 & Wayland |
| L7 | Clipboard (copy IP etc.) — arboard/egui clipboard on Wayland may need paste-approval; verify each copy site | Copy works (or graceful degradation documented) |
| L8 | (Phase-2 placeholder) tray-icon spike behind feature flag `tray`: GTK main-thread constraints + EventLoopProxy forwarding documented, NOT merged this phase | Spike notes committed |

### Phase M — Packaging & resources (.deb)
| # | Step | Done when |
|---|---|---|
| M1 | cargo-deb metadata expansion for `zeronode-vpn-client`: Depends add `nftables, kmod, iproute2, policykit-1, libayatana-appindicator3-1 (opt), openvpn | pptp-linux | ppp` as Recommends per-protocol soft deps; tor NOT in Depends (bundled) | dpkg -i succeeds on clean Ubuntu 24.04 with only base + Recommends choices honored |
| M2 | Assets into deb: `usr/share/vpn-client/tor/{tor,geoip,geoip6,…}` from fetched expert bundle (M1 keeps deb ~35 MB; document size), `assets/flags/**`, `globe/*.geojson|json` already embedded (verify), icon hicolor sizes, desktop file Exec/Icon/StartupWMClass | Installed tree complete; desktop launches fromActivities & XFCE menu |
| M3 | Maintainer scripts: postinst chmod +x tor, ldconfig not needed; prerm best-effort teardown (stop tunnels, restore DNS) guarded; postrm preserves user data | Upgrade/remove cycles safe even while connected |
| M4 | `tools/build-linux.sh` extended: fetch-tor step, `-p vpn-client` deb with assets, checksums manifest | One-command release build from clean clone |
| M5 | `tools/install-linux.sh`/`uninstall-linux.sh` parity updates (client assets, DNS restore note) | Manual install path equals deb result |

### Phase N — Verification & E2E
| # | Step | Done when |
|---|---|---|
| N1 | Extend `tools/verify-linux-deb-e2e.sh` OR new `tools/verify-linux-client.sh`: netns-based WG full-tunnel ICMP+TCP check, userspace-engine variant | Automated proof both WG engines pass |
| N2 | Protocol smoke matrix script (manual-assist): for each of WG/OVPN/SS/PPTP/TOR: import/connect/verify-egress-IP/disconnect/no-residue assertions | Matrix table filled with PASS on test VMs |
| N3 | Leak checks: DNS leak (resolv.conf + DoH bypass note), IPv6 posture during v4-only tunnels (tproxy ipv6_enabled=false ⇒ document/optionally block v6) | Documented results; no v4 leaks |
| N4 | Session QA: XFCE4 (X11) full flow + GNOME (Wayland) full flow incl. elevation dialogs, drag-drop, globe interactions, resize, fractional scale | Checklist signed off both DEs |
| N5 | Crash/failure drills: kill tor mid-system-route, kill openvpn mid-up, revoke elevation mid-flight, double-click connect spam | App recovers or fails gracefully with actionable errors; no orphaned routes |
| N6 | Performance sanity: idle CPU (repaint policy), memory footprint vs Windows baseline, globe 60 fps orbit on llvmpipe software GL (XFCE VM case) | Numbers recorded in LINUX_CLIENT.md |

---

## Implementation order (across sessions)
1. **Session 1:** A1–A6, B1–B4 (foundation + elevation) — compiles green, pkexec demo works.
2. **Session 2:** C1–C5, D1–D4 (WireGuard both engines) — netns E2E green.
3. **Session 3:** E1–E3, F1–F3 (SOCKS engine + Outline) — system-route demo.
4. **Session 4:** G1–G5 (Tor end-to-end incl. system tunnel + isolation apps).
5. **Session 5:** H1–H5, I1–I5 (OpenVPN, PPTP).
6. **Session 6:** J1–J3, K1–K7 (control-plane + app wiring; protocols live in GUI).
7. **Session 7:** L1–L7 (DE adaptation), M1–M5 (deb), N1–N6 (verification).

## Non-goals (explicit, this phase)
- No tray icon merge (spike only, L8). No GNOME Shell extension authoring. No NetworkManager/dbus profile import (future: nmcli export button idea). No macOS. No mobile. No server-GUI changes beyond J2 NAT necessity. No redesign of UI visuals — pixel parity with Windows build. No removal of Windows codepaths.

## Risk register
| Risk | Impact | Mitigation |
|---|---|---|
| pkexec refuses GUI env vars / runs app as true root breaking config paths | High | B3 env strategy + config-path pinning to invoking SUDO_USER/home; fallback sudo prompt path |
| Wayland + EGL unavailable in odd VMs (llvmpipe) | Med | L1 forces x11 fallback automatically when EGL init fails; document `LIBGL_ALWAYS_SOFTWARE` |
| Kernel WG absent (custom kernels) | Low | Userspace engine D covers; setup-check advertises |
| tproxy-config resolv.conf bind-mount vs systemd-resolved stub conflicts | Med | resolvectl branch first; bind-mount fallback; N3 verifies |
| PPTP deprecated/MPPE module missing on stock kernels | Med | Probe I1 + honest security banner; degrade to informative failure not crash |
| Deb size blowup from tor bundle (~31 MB src → larger installed) | Low | Compress in deb; document; optional `--without-bundled-tor` build flavor later |
| egui hit-test panic seen in Windows crash.log resurfacing under scale changes | Med | L4 explicitly stress-tests scaling; keep eframe 0.29 pinned; upstream issue referenced |
| shadowsocks-service/tun2proxy version drift between platforms | Low | Pin identical versions as platform-windows Cargo.toml (single workspace dep versions) |

## Resource sheet (fill during A1)
| Item | Source | Pinned |
|---|---|---|
| Tor expert bundle linux x86_64 | archive.torproject.org/tor-package-archive/torbrowser/15.0.17/ | 15.0.17 |
| openvpn | ubuntu archive (apt) | distro default |
| pptp-linux, ppp | ubuntu archive (apt) | distro default |
| Fonts | fonts-dejavu-core (preinstalled) | distro |
| GeoIP mmdb | download.db-ip.com/free (existing core URLs) | monthly lite |
| Flags/globe | repo assets (existing) | current |
| Rust toolchain | rustup stable (existing provisioner) | stable |
