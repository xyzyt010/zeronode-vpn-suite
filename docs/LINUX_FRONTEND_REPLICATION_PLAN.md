# ZeroNode VPN Client — Linux Frontend Replication Plan (Phase 2: Pixel-Perfect)

**Goal:** Exact replication of the Windows ZeroNode VPN desktop app on **Ubuntu 22.04/24.04+ and Linux Mint 21/22 (Cinnamon/XFCE/MATE)**, running **identically** on **X11/XFCE4** and **GNOME/Wayland** (and Mint Cinnamon X11 + Wayland sessions), reusing the *same* frameworks as Windows where compatible (`eframe 0.29.1` + `egui 0.29.1` + `glow` + `glutin 0.32` + `winit 0.30`), with zero visual/behavioral divergence.

**Principle:** Single binary, no per-DE recompilation. `cargo build --release` on a stock Ubuntu builder produces a `.deb` and a portable `vpn-client` that the reviewer can `dpkg -i` on Linux Mint and see the *same* window, colors, fonts, buttons, cards, globe, animations, dialogs and interactions as on Windows — pixel-for-pixel where the compositor allows, documented where it does not (Wayland).

---

## 1. Windows Forensic — What We Must Replicate (Ground Truth: `apps/client/src/app.rs` 7522 lines)

### 1.1 Dependency baseline (do NOT change)
| Crate | Version | Why locked |
|---|---|---|
| `eframe` | `0.29.1` `default-features=false` `features=["default_fonts","glow"]` workspace | Windows uses **glow (OpenGL)**, not wgpu. Globe is pure `egui::Painter` — no GPU path to swap. |
| `egui` / `egui_extras` | `0.29.1` (`image`,`svg`) | All frames/buttons/combos/ScrollAreas via egui. |
| `winit` | `0.30.13` (via eframe) | 0.30 refactored Wayland on `sctk 0.9`; fractional scaling, app_id, decorations semantics changed — must be handled. |
| `glutin` | `0.32` + `glutin-winit 0.5` + `glow 0.14` | EGL on Wayland, GLX/EGL on X11 — auto. |
| `image` | `0.25.5` png only | Icon + flag Lanczos resize. |
| `rfd` | `0.15.4` (`xdg-portal`/`ashpd 0.11`) | File picker — Wayland uses portal; no direct GTK linkage. |
| No `tray-icon` | Phase 1 binary without tray | `tray-icon 0.24` needs `libgtk-3-dev` + `libayatana-appindicator3` + SNI extension — defer to feature flag. |

### 1.2 Visual design system (must be byte-identical on Linux)
* **Palette** — `VPN_GREEN 0,255,127`, `VPN_GREEN_DIM 0,180,120`, `VPN_CARD_BG 13,13,13`, side panel `8,8,8` + `stroke 32,32,32`, header `24pt Strong WHITE`, section accent `4×16 rect rounding2`. Tor purple `168,85,247`, warn `255,180,60`. All RGBs copied verbatim from `protocols/mod.rs:5-9` + `app.rs:3608 install_theme`.
* **Typography** — Windows hunts `Segoe UI → Calibri → Consolas → CascadiaMono`; egui overrides: Body 15, Heading 22, Button 14, Small 12.5, Monospace 13.5, header 24 Strong, Tor title 16, small rows 10/11, flag badges 10 Mono. Linux must *insert* `DejaVu Sans → Liberation Sans → Noto Sans → Ubuntu` **before** the Windows chain so Segoe miss is cheap; keep unmodified size table.
* **Geometry** — Window `1160×720` default, `820×580` min (`ViewportBuilder` in `app.rs:128-132`). Side panel `240–560` default `560` `exact_width` + manual grip `Rect left-2..left+6` `CursorIcon::ResizeHorizontal` line `0,255,127` hover else `48,48,48`. Paddings `left14 right18 top12 bottom12`, inner margins `10,8` (or `8` narrow), card `rounding8 stroke1.0` (`1.5` when selected).
* **Assets** — `assets/icon.png` → `IconData` via `image` (`app.rs:173`); `assets/flags/*.png|svg` staged by `build.rs` → `target/{profile}/assets/flags`; at runtime `resolve_flag_path` probes 9 dirs incl. `/usr/share/vpn-client/flags` (deb) + centroid fallback; globe `assets/globe/countries_50m.geojson` (3.08 MB, `include_str!`) + `country_centroids.json` only; `mesh.rs`/`2k_earth*.jpg` are dead code — untouched.
* **Font rendering** — `tessellation feathering_size 0.6`, flag Lanczos3 resize to `size*ppp*3.5` clamped, cache `OnceLock<Mutex<HashMap>>` `"{cc}:{tw}x{th}"` uri `bytes://zeronode/flag/crisp/...` `Filtering Linear / Wrap Clamp / mipmap Linear fit_to_exact_size`, fallback badge mono green. Identical on Linux.

### 1.3 Full UI component inventory (every pixel has a spec)
1. **Header** `TopBottomPanel::top("header")` — `10px` pad, `RichText "ZeroNode Client"` 24 Strong WHITE, `stroke 40,40,40` separator.
2. **SidePanel right `"details"`** — `Fill 8,8,8 stroke 1.0 32,32,32`, not resizable by egui (manual grip), `ScrollArea::vertical id="details_scroll"`, `panel_w = (available_width-16).max(120)`, `narrow = panel_w < 300` (stacks button rows vertically at <300).
   - *Session* `render_session_section:4396` — "Session" title 17pt + neon accent, `pane_card`, online badge `refreshed {elapsed}`, active tunnel `server_name / Client IP green / Server IP / Session trunc28`.
   - *Your IP* `render_ip_details_card:4458` — "Your IP" neon, card `stroke green*0.3` rounding8, Refresh 70×24 green, flag 52×36 + country name, IP mono `0,255,127`, IPv6 second row, Location/ISP/Coords rows, Updated age.
   - *Tor* `1941` — purple `168,85,247`, Windows elevated hint (Linux: hide/admin note via `#[cfg]`), inner card `fill 12,10,16 stroke purple`, title "Tor VPN" 16pt, Connect purple / Disconnect `42,42,48`, progress purple `paint_connect_progress_bar`, state "Tor circuit established", flag 48×34 + "Tor Exit: {country}" + Exit IP green + "Local SOCKS5: 127.0.0.1:{port}" purple + `TorExitInfo` rows (City/Region, ISP skip-dup Org, AS, Coords 4-dec, TZ, ZIP), System-Wide Route separator + toggle (Linux: same enable/disable via pkexec parity), Isolation mode radios "Whole PC (system VPN)" vs "Selected apps (SOCKS5)" + per-app row `Launch via Tor` purple + trash.
   - *VPN Protocol chooser* `2547` — title neon + hint "Choose a protocol, then import…", cross-protocol busy banner amber `"{active} is connected — disconnect first…"`, combo `vpn_protocol_select` width `(panel_w-90).clamp(120,280)` enumerating `VpnUiProtocol::ALL` (OpenVPN/WireGuard/PPTP/Outline), then per-protocol bodies:
     - **OpenVPN 2599** — card `VPN_CARD_BG stroke VPN_GREEN_DIM`, Connect green / Disconnect grey 48,48,48, progress green, `Combo ovpn_server_select` `(avail-4).clamp(80,panel_w)` label `name · country`, flag 28×20 + Endpoint green + Resolved IP, flag+country, Location/ISP/Cipher, Drop zone `fill18,22,20 stroke1.5 0,180,120 minH52 "Drop .ovpn here · or click to browse" 0,220,150 strong12`, `rfd` filter `ovpn`, profile cards `stroke1.5 0,200,120` selected + `●` green, endpoint 10pt.
     - **WireGuard 697** — same card, `server_id starts_with "wg_"`, Paste card `Name + multiline 5 rows hint [Interface]…` Save green, Drop zone `Drop .conf here…`, `rfd` filter `conf`, list same style.
     - **PPTP 1085** — `SECURITY_WARNING` amber 10pt head, form `Name/Host/User/Pass/Domain` Save green, list `host:port · username`.
     - **Outline 1350** — hint `Paste Outline access key (ss://…)`, Connect `system_wide:true` hover, key textarea, "Embedded Shadowsocks…" note, list `host:port · method`.
3. **CentralPanel** `3133` — `rect = available_rect_before_wrap`, `center = rect.center()+Vec2(0,h*0.045)`, `radius = min(w,h)*0.42*zoom`, `interact_radius = radius+36`, counts *every* globe interaction.
4. **Globe** `globe/renderer.rs:590` — pure `egui::Painter` wireframe:
   - Body `circle_filled 18,20,22` + `stroke1.2 0,255,127*0.25` + halo `radius+3 stroke6 rgba0,255,127,18`
   - Borders `Vec<(Vec3,Vec3)>` from `countries_50m.geojson` raised `1.002`, matrix `m00=cosY m10=sinY*sinX …`, skip `z<0 both`, screen `center+Vec2(x_t,-y_t)*radius`, lines `stroke1.0 0,255,127*0.55`
   - Server dots: centroid `lat_lng_to_vec3(c.lat,c.lng,1.01)` if `z_t>=0` → halo `size*2.5*0.3` + dot `4 (protected grey 200,200,200) or 3 (open green)` → click returns `server_id`; freestanding beacon for local_ip/exit when not in server list.
   - `paint_beacon` — 2 ripples `period2.4 max26 core4.5 phase=(elapsed/period+i*0.5)%1 alpha=(1-phase)*intensity*0.55 stroke1.35`, core `pulse=0.5+0.5*sin(1.6*elapsed) size=4.2+0.55*pulse` halo `1.85*alpha` + solid + center `rgb220,255,235`.
5. **Bottom toolbar** (under globe) — `TextEdit "Server IP or host" 200×32 white + Add 80×32 green + Refresh grey 70×32 + Tunnel Apply light-grey 70×32 + Disconnect 42,45,53 80×32 + Remove Tunnel 26,26,26 90×32` → `ClientCommand` sends.
6. **Dialogs** — Password `401` `"Protected Node"` 13,13,13 stroke31,31,31 300×36 + Connect 120×34; OvpnAuth `409` `"OpenVPN Credentials"` stroke0,180,120 320×32 + 140×34; ServerSettings `416` `"Server Address Selection"` 520×420 resizable two-column `render_ip_selection_group` → saves `SaveLocalServerSelection`.

### 1.4 Animations & interactions (must feel identical)
| Animation | Params (`app.rs` / `renderer.rs`) | Linux note |
|---|---|---|
| Pan to country/exit | `1.75s ease_in_out_cubic` (cubic 4t³ / 1-(-2t+2)³/2), zoom lag `0.08` same ease, `COUNTRY_FOCUS_ZOOM 2.55`, shortest-yaw wrap `while dy>PI target-=TAU` | Identical — pure f64 lerp, no OS coupling |
| Inertia/orbit | `dt clamp 1/240..1/20 else 0.016`, `rot+=vel*dt decay=exp(-4.2*dt)` half-life 0.18s snap `<0.02→0 clamp ±1.35 radX`, drag `delta*0.0065 → apply_orbit_delta blend0.35 cap8.0` | Wayland may deliver scroll in `lpx` already scaled — verify scaling factor parity |
| Scroll zoom | `smooth_scroll_delta*0.0035`, `zoom_delta^0.85 clamp0.55..4.5` | On fractional scale compositor, wheel delta is physical — egui already converts |
| Beacon pulse | `1.6 rad/s`, ripples `2.4s 2 waves max26 core4.5 alpha0.55→0` | Repaint-gated, keep `request_repaint()` |
| Progress bars | `paint_connect_progress_bar height10 track18,22,20 fill rounding5 + glow 3.0 alpha70` | 60 fps via `real_op_progress(kind,elapsed)` + `ctx.request_repaint()` |
| Hover tooltips | Manual `hover_pos` → `show_tooltip_at_pointer` flag 36×24 + name/country/coords/ CONNECTED | Wayland cursor grab fixes in `winit 0.30.8` — no change needed |
| Repaint policy | `is_animating()‖is_dragging‖side_panel_resizing‖connecting → request_repaint()` else `request_repaint_after(250ms)` | Keep — idle 0% CPU, 60Hz during pan; single viewport avoids vsync contention `#5836` |
| Globe Y offset | `GLOBE_CENTER_Y_OFFSET_FRAC 0.045` | Keep — ensures bottom clip first |

### 1.5 Server/persistence layer (unchanged on Linux)
`db.rs` SQLite `client_db.sqlite` (`bundled`), tables `ovpn_configs` + prefs + `wg_configs/pptp_configs/outline_configs` + `tor_isolated_apps`; `lib.rs` exposes identical `ClientCommand` enum + `run_desktop_with_auto_ex(DesktopAutoConnect{tor,ovpn,wg,outline})`; `main.rs` clap `Cli` with `--auto-connect-*`, `install_panic_logger` to `vpn-client.panic.log` beside exe.

---

## 2. X11/Wayland Research Verdicts → Concrete Decisions

| Topic | Finding | Decision (this plan) |
|---|---|---|
| **Toolkit fitness** | `glow` + `glutin 0.32` creates EGL on Wayland, GLX/EGL on X11 automatically. `wgpu 22.1` resize lag on Wayland — not wanted. Workspace `eframe features ["default_fonts","glow"]` currently **drops `wayland`+`x11`** (verified in Cargo docs) → Linux would build with no backend. | **A1:** Fix workspace `Cargo.toml` to `features=["default_fonts","glow","wayland","x11"]` (or per-crate add). `wayland-dlopen` stays default — binary dlopens `libwayland-client.so.0`, so runs on X-only hosts. |
| **Backend selection** | `winit 0.30` removed `WINIT_UNIX_BACKEND` (`PR #3011 WontFix #4327`). Selection: `WAYLAND_DISPLAY`/`WAYLAND_SOCKET` with successful connect → Wayland else `DISPLAY` → X11 (`linux/mod.rs`). Stale `WAYLAND_DISPLAY=wayland-0` on X session crashes `WaylandError(NoCompositor)`. `XDG_SESSION_TYPE` **not read by winit**. | **A2:** Implement `ZERONODE_BACKEND=x11|wayland` override via `EventLoopBuilderExtX11::with_x11()` / `EventLoopBuilderExtWayland::with_wayland()` before `eframe::run_native` + clear stale `WAYLAND_DISPLAY` in pkexec elevated child (helper does `env ... ZERONODE_BACKEND=x11`, host keeps auto-detect). `app.rs:138-168` today only logs — upgrade to builder. |
| **Viewport parity** | `with_inner_size 1160×720 / min 820×580`, `with_app_id("io.zeronode.vpn")`, `with_icon(image)` | Keep verbatim. Wayland `centered()` ignored (compositor decides) — already not called, document. `app_id` **must** match `io.zeronode.vpn.desktop` filename for GNOME dock icon mapping. X11 `WM_CLASS` same string. Wayland icon ignored by many compositors — `.desktop Icon=` covers. |
| **Decorations** | Wayland CSD via `sctk-adwaita` Adwaita headerbar (slightly thicker); X11 SSD native. `with_decorations(false)` works both but not needed. | Keep default decorations; accept thicker headerbar on Wayland as only acknowledged deviation. |
| **Scaling** | `winit 0.30` + `sctk 0.9` implements `wp-fractional-scale-v1`; `Window::scale_factor()` reports fractional `1.25/1.5/1.75`, `MonitorHandle::scale_factor()` intentionally integer fallback — use window factor. egui handles `ScaleFactorChanged`. | **F3:** QA at 100/125/150/200% GNOME + XFCE HiDPI; verify `radius=min(w,h)*0.42*zoom` math uses logical px (already does — egui is scaled). No blur if buffer follows window factor. Keep vsync on (Wayland always vsync). |
| **File dialogs** | `rfd 0.15` default `xdg-portal`/`ashpd` → `org.freedesktop.portal.FileChooser` → `xdg-desktop-portal-gtk`, fallback `zenity`. Build needs no `libgtk-3-dev`. Same portal on X11. | Reuse `rfd::FileDialog` already in `render_wireguard_panel`/`render_openvpn` filters `conf`/`ovpn`; verify `xdg-desktop-portal* + zenity` present on test images. Already `rfd` default — no change. |
| **Drag-drop** | X11: XDnD works (`.ovpn/.conf` drop zone). **Wayland: `winit 0.30/0.31` never fires `FileDropped/FileHovered` (`issue #1881`, PR #4504 closed 2026 without merge).** | Document as Wayland limitation; keep drop zone UI but rely on picker + paste fallback (`wg_draft_paste` multiline, `outline_draft_key` textarea, `rfd` browse) — already coded for Android portability. |
| **Clipboard** | `egui-winit 0.29` bundles `arboard 3.6` (X11 `CLIPBOARD+PRIMARY`) + `smithay-clipboard 0.7.2` (`ext-data-control-v1`/`wlr-data-control-unstable-v1`). Wayland clipboard vanishes on exit unless clipboard manager (GNOME yes via mutter). | Transparent; copy IP/coords buttons work both. Note lifecycle difference in test checklist. |
| **Tray** | `tray-icon 0.24` Linux is entirely GTK3 (`libgtk-3-dev` build + `libayatana-appindicator3-1` runtime, SNI host: XFCE panel `StatusNotifierItem` works, GNOME needs `gnome-shell-extension-appindicator` not default). | **Phase 2** behind `features=["tray"]` optional. Package `Suggests: libayatana-appindicator3-1 libgtk-3-0` (already `apps/client/Cargo.toml:63`). Phase 1 binary **without** tray — not a regression vs Windows (tray is optional helper there too). |
| **Fonts** | egui `default_fonts` embeds Ubuntu-Light/Hack/emoji but not full coverage. `Segoe UI` miss on Linux is cheap if probed after DejaVu. | **B1:** DejaVu `fonts-dejavu-core` (already `Recommends`) → `Liberation Sans` → `Noto Sans` → `Ubuntu` → Segoe/Calibri fallback → default_fonts; mono `DejaVu Sans Mono → Liberation Mono → Hack`. File probes `/usr/share/fonts/...` once via `OnceLock`, `ctx.set_fonts` replaceProportional order. |
| **Elevation** | Wayland root cannot open `WAYLAND_DISPLAY=/run/user/1000/wayland-0`; XWayland is fallback. pkexec spawns as root, env must switch to X11. | **F2:** Helper already does `pkexec --disable-internal-agent env HOME=$HOME ZERONODE_BACKEND=x11 $exe $args`; elevate path forces builder to `with_x11()`. Non-elevated keeps auto-detect. Document `wayland → XWayland` transition redisplay. |
| **VSync/anim** | Wayland compositor caps present (`swap_interval(1)` forced), immediate present ignored. Multi-viewport vsync contention `#5836` when hidden viewports wait serially — single viewport `ROOT` safe. | Keep single viewport, `request_repaint()` during anim else `after(250ms)`; cache is unnecessary. Keep desired frame latency 1. |
| **Build link** | `DT_NEEDED libX11.so.6 libxkbcommon.so.0`, not `libwayland-client` (dlopen). | Single binary works on X-only VMs. `Depends: $auto` auto-pulls `libxcb1 libxkbcommon0`; `Recommends: xdg-desktop-portal-gtk|kde, zenity, fonts-dejavu-core`. |

---

## 3. Parity Contract (Frontend)

**Pixel-identical on X11/XFCE4 vs Windows** (allowable delta: window decoration thickness + HDR gamma).
**Wayland/GNOME** identical content geometry; acknowledged deviations logged: centered no-op, drop disabled, icon from `.desktop`, slightly thicker Adwaita CSD, `1.25×` fractional scaling handled.

All acceptance gates are checked with the **same** release binary on a persistent Linux VM/UBoxes (Ubuntu 24.04 LTS XFCE+X11 + GNOME/Wayland) and on the consumer device (Linux Mint 21/22 Cinnamon X11).

---

## 4. Detailed Execution Blocks (Phase 2 Frontend)

### Block A — Build & Runtime Foundation
| # | Step | Done when | Ref |
|---|---|---|---|
| A1 | Fix `Cargo.toml` workspace `eframe` features → add `wayland`+`x11`; verify `cargo check -p vpn-client` green on `x86_64-unknown-linux-gnu` and `cargo check` on Windows untouched | Building on clean Ubuntu image succeeds without GTK build deps (unless tray feature) | `Cargo.toml` |
| A2 | Upgrade `app.rs:55-171` to real `EventLoopBuilder` override: `match ZERONODE_BACKEND { x11→with_x11(), wayland→with_wayland(), _→auto }` before `NativeOptions { event_loop_builder: Some(Box::new(\|b\| *b = builder)) }`; keep `app_id("io.zeronode.vpn")` + icon + sizing. Ensure `elevation.rs` pkexec env already sets `ZERONODE_BACKEND=x11` for elevated child | `WAYLAND_DISPLAY=wayland-0` + `ZERONODE_BACKEND=x11 ./vpn-client` boots on X11 even when Wayland socket stale; gnome wayland non-elevated still chooses Wayland; elevated Tor/OVPN system route relaunches correctly on GNOME| `app.rs`, `crates/platform-linux/src/elevation.rs:72` |
| A3 | Verify `.desktop` `Exec/StartupWMClass/Icon` + `cargo-deb` asset `usr/share/…` + icon `hicolor`; ensure Wayland dock match | `io.zeronode.vpn.desktop` launches from GNOME Activities + XFCE menu with correct icon | `apps/client/Cargo.toml:metadata.deb` |
| A4 | Provison builder: `tools/provision-ubuntu-builder.sh` sections stable (policykit, iproute, nft, openvpn/pptp optional, tray dev libs optional) | Script provisions headless VM for `build-linux.sh` | `tools/provision-ubuntu-builder.sh` |

### Block B — Visual System (colors / fonts / theme / flags / globe)
| B1 | Replace `install_crisp_fonts` shim: probe Linux `[/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf, DejaVuSansMono, LiberationSans-Regular, NotoSans-Regular, Ubuntu-R]` and `/…/mono/…` before Windows Segoe chain; load via `include_bytes` or file-read + `egui::FontData::from_static` then `ctx.set_fonts(definitions)` with `families[Proportional][0]` ordering DejaVu→Liberation→Noto→Segoe→default; same for Mono; keep `feathering_size 0.6`, sizes unchanged. Guard `OnceLock` / `fonts_installed` bool | Text renders crisp on XFCE + GNOME at 1×/1.5×/2×; no fallback tofu; Windows still finds Segoe unchanged | `app.rs:3608` |
| B2 | Keep `install_theme` verbatim: palette `VPN_GREEN … 0,255,127`, panels, strokes, spacing `8,6 button 12,7`, window fill `0,0,0`, selection visuals; verify `egui::Visuals` dark branches on Linux | Side-by-side Windows screenshot vs `wgpu?` not needed — glow palette identical | `app.rs:3620` |
| B3 | Keep `resolve_flag_path` + flag pipeline untouched (already deb-aware); warm `flag_png_cache` + Lanczos path identical; verify `build.rs` staged w2560+flags under `target/{debug,release}/assets/flags` + deb `usr/share/vpn-client/flags` | Flags render at 52×36, 48×34, 28×20 from either tree; Mint Cinnamon shows same ISO badges on miss | `apps/client/src/tor_geo.rs`, `apps/client/build.rs` |
| B4 | Keep `GlobeRenderer::new` + `countries_50m.geojson` included; no `three_d`/`mesh` resurrection; `paint()` same matrix/halo/beacon constants | Globe wireframe + halo + server dots + freestanding beacon identical | `apps/client/src/globe/renderer.rs` |

### Block C — Layout & Panels (header/side/central/toolbar)
| C1 | Side panel `exact_width` + manual grip `Rect left-2..left+6` `Sense::click_and_drag` `ResizeHorizontal` width clamp `240..560` already in `app.rs:1868`; verify grip hit at HiDPI (uses logical px) | Drag grip works on both compositors even while scrolling; width drives `narrow` flag | `app.rs:1878` |
| C2 | Session + Your IP + Tor cards verbatim; Tor System-Wide Route toggle wired to `platform::start_tor_system_tunnel`/`stop` via pkexec extra dispatches (reuse `ClientCommand::ApplyTorSystemRoute` from backend phase) | Cards layout matches Windows screenshots; narrow <300 stacks buttons vertically | `app.rs:1941,4396,4458` |
| C3 | Protocol chooser + per-panel bodies (OVPN/WG/PPTP/Outline) shell styling `VPN_CARD_BG stroke VPN_GREEN_DIM` verified; combos `width (avail-4).clamp(80,panel_w)` vs `clamp(120,280)` as coded | Switching protocols, selecting profile, showing details (endpoint/cipher/peer) identical | `protocols/mod.rs:34`, `app.rs:2547,2599,697,1085,1350` |
| C4 | CentralPanel + bottom toolbar commissioned identically (Add/Refresh/Tunnel/Disconnect/Remove) → `ClientCommand` | Toolbar spacing/button sizes 200×32/80×32/70×32 etc. identical | `app.rs:3133` |
| C5 | Responsiveness: `PANE_NARROW_BREAK 300` verified at `820` min width drag extreme | No overflow, no truncated progress bars | `app.rs:399` |

### Block D — Protocol Panels & Dialogs
| D1 | Drop zones `fill18,22,20 stroke1.5 0,180,120 rounding8 minH52` + `rfd::FileDialog` filters; WireGuard `conf`, OpenVPN `ovpn`, Outline `ss://` textarea | On X11 drops import; on Wayland picker still imports (documented drop no-op) | `app.rs:978,1005` |
| D2 | Dialogs: Password / OvpnAuth / ServerSettings windows centered, strokes, sizes, `TextEdit` password flag, auth 0600 write, IPv4/IPv6 column selection preserving global-routable filter | Dialog centering on X11 exact, on Wayland compositor-centered (acceptable); inputs/validation match Windows | `app.rs:401,409,416` |
| D3 | Cross-protocol busy banner amber `"… is connected — disconnect first …"` | Banner appears whenever non-selected protocol has active connection | `protocols/mod.rs:54` |

### Block E — Animations & Interactions
| E1 | Globe orbit/pan/inertia/zoom params preserved byte-identical (`1.75s cubic`, `COUNTRY_FOCUS_ZOOM 2.55`, `decay exp(-4.2*dt)`, `drag 0.0065`, `scroll 0.0035`) | Orbit on llvmpipe software GL @60fps without tearing | `renderer.rs:88` |
| E2 | Beacon `pulse 1.6 rad/s + ripples 2.4s` + halo | Beacon pulsing visible, two rings expand/fade at same speed as Windows | `renderer.rs:482` |
| E3 | Progress `real_op_progress(kind,elapsed)` + `paint_connect_progress_bar height10` + `request_repaint()` gating | Connect/disconnect 0..1 fills uniformly | `app.rs:789,3608` |
| E4 | Repaint policy `is_animating‖dragging‖resizing‖op_progress 0..1 → repaint else after 250ms`; single viewport | Idle 0% CPU (verified `top`), anim 60Hz under vsync | `app.rs:1765` |
| E5 | Hover tooltips `show_tooltip_at_pointer` 28/12 radius | Tooltips appear at pointer with correct flag + name | `renderer.rs:340` |

### Block F — OS Integration (Wayland/X11 divergences resolved)
| F1 | `rfd` portal verified on `xdg-desktop-portal + xdg-desktop-portal-gtk/gtk-{debian: libportal} + zenity` (Ubuntu GNOME pulls `portal-gnome`); add `Recommends: xdg-desktop-portal-gtk | xdg-desktop-portal-kde, zenity` | File picker appears on both DEs | `Cargo.toml` |
| F2 | Elevated XWayland switch verified: non-elevated GNOME→Wayland EGL, pkexec elevated child → X11 (`XWAYLAND` window). `elevation.rs` already sets `env ZERONODE_BACKEND=x11` — A2 builder makes it effective | After clicking Enable System-Wide Route on GNOME, pkexec dialog → elevated window reappears (maybe position shift) and `tproxy` TUN comes up | `elevation.rs` + `app.rs` |
| F3 | Fractional scaling QA 100/125/150/200% GNOME + XFCE `Settings→Display` → measure globe hover radius, text crisp, no double-scale | Checklist signed | `renderer.rs` hit math |
| F4 | Clipboard copy-IP verified (`arboard`/`smithay-clipboard`): copy then switch app → paste succeeds; Wayland exit-clear documented | Works; on Wayland clipboard clearing on exit if manager missing is noted as limitation | egui `ctx.copy_text` |
| F5 | Document non-goals: Wayland drop disabled, centered no-op, thicker CSD, `set_cursor_grab` no-op | `docs/LINUX_CLIENT.md` Wayland section updated | docs |

### Block G — Packaging, Build & Mint E2E
| G1 | `tools/fetch-tor-linux.sh` idempotent (already assets/tor-linux), `tools/build-linux.sh` fetch-step + `-p vpn-client` `cargo deb` with assets (tor 755, geoip 644, flags) | `target/debian/*.deb` ~35 MB built from clean clone | `tools/build-linux.sh:22` |
| G2 | Cargo deb metadata: `Package zeronode-vpn-client`, `Depends $auto nftables,ipr.. policykit-1`, `Recommends openvpn,pptp-linux,ppp,wireguard-tools,fonts-dejavu-core,xdg-desktop-portal-gtk,zenity`, `Suggests libayatana-appindicator3-1` | `dpkg -I` shows correct control fields | `apps/client/Cargo.toml:53` |
| G3 | Maintainer scripts `postinst` chmod +x tor, `prerm` best-effort stop + DNS restore, safe while connected | Upgrade/remove safe | `debian/` |
| G4 | Release binaries staged: `target/release/vpn-client` (portable) + `target/debian/zeronode-vpn-client_*_amd64.deb` + `SHA256SUMS` | Artifacts copied to `dist/` for Mint transfer | — |
| G5 | Mint Cinnamon smoke (consumer E2E): install `.deb` on Mint, launch from menu + terminal, test: header/panel/globe/files/flags/panels/dialogs/Tor/WG/OVPN/PPTP/Outline connect→egress-IP→disconnect→no-residue, resize 820→1160, fractional scale 125%, clipboard copy, drag-drop X11 case | All PASS; Wayland deviations logged; no crash on elevation decline, kill-mid-route, double-click spam | `tools/verify-linux-client.sh` extended |

---

## 5. Implementation order (this session — do not stop until binaries staged)

1. **09-11**  A1–A4 (build/runtime) — cargo green both hosts, builder provisions — **gates all**
2. **11-14**  B1–B4 visual system — fonts + theme + flags + globe — **pixel gate**
3. **14-16**  C1–C5 layout — panel grip + cards + central + toolbar — **geometry gate**
4. **16-18**  D1–D3 dialogs — drop/paste/rfd + dialogs + busy banner — **functional gate**
5. **18-19**  E1–E5 animations — globe physics + beacon + progress + repaint — **motion gate**
6. **19-20**  F1–F5 OS integration — portals + XWayland + scale + clipboard — **DE gate**
7. **20-21**  G1–G5 packaging + `cargo build --release -p vpn-client` + `cargo deb` + stage `dist/` — **artifact gate**

Each gate enforces `cargo check -p vpn-client -p vpn-platform-linux -p vpn-suite-core` on VM + Win + `cargo test -p vpn-platform-linux` 11 green.

## 6. Non-goals (explicit)
No tray merge (Phase 2 behind `tray` flag — GTK dep isolation), no GNOME extension, no NetworkManager import (future `nmcli`), no macOS/mobile/server-GUI changes beyond NAT, no palette redesign, no Windows codepath removal, no wgpu migration.

## 7. Risk register
| Risk | Impact | Mitigation |
|---|---|---|
| eframe wayland/x11 feature omission breaks clean Linux clone builds | High | A1 pins both; CI `cargo check --target x86_64-unknown-linux-gnu` gates |
| pkexec clears WAYLAND_DISPLAY and elevated GUI never appears | High | F2 env `ZERONODE_BACKEND=x11` + builder forces x11 — manual GNOMe elevated flow test mandatory |
| Wayland EGL unavailable in headless llvmpipe VM | Med | Auto fallback to x11 via A2 stale DISPLAY handling; document `LIBGL_ALWAYS_SOFTWARE=1` |
| Flag/port stale `WAYLAND_DISPLAY=wayland-0` crash | Med | `ZERONODE_BACKEND` override path covered; detect empty `WAYLAND_DISPLAY` empty string since winit 0.30.8 |
| Fractional scale blur/hit-test drift | Med | F3 QA matrix + logical-px math audit |
| Drag-drop expectation on Wayland | Low | F5 docs + picker fallback already in UI; not a bug |
| Deb size from tor bundle (~31 MB src → ~45 MB installed) | Low | Compress + document; optional `--without-bundled-tor` future |
| `egui` hit-test panic (#egui 5836 scale) | Med | F3 stress-test + eframe pinned 0.29; upstream pinned |

## 8. Resource sheet
| Item | Source | Pinned |
|---|---|---|
| Tor expert bundle linux x86_64 | `archive.torproject.org/tor-package-archive/torbrowser/15.0.17/tor-expert-bundle-linux-x86_64-15.0.17.tar.gz` | 15.0.17 |
| openvpn / pptp-linux / ppp / wireguard-tools | Ubuntu archive apt | distro default |
| Fonts | `fonts-dejavu-core` (Recommends) + Liberation/Noto fallbacks | distro |
| GeoIP mmdb | `download.db-ip.com/free` (core pipeline) | monthly lite |
| Flags/globe | repo `assets/flags`, `assets/globe` | current |
| Rust | `rustup stable` | stable |

— End of plan. Execution starts at §4 Block A.
