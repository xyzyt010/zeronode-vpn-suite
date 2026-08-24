# Linux Frontend — Multi-Distro Perfect Integration Plan

**Targets:** Debian 11/12/13 · Arch rolling · Fedora 40/41/42 — all X11 + Wayland, GNOME/KDE/XFCE/Cinnamon  
**Baseline:** Ubuntu 22.04/24.04 + Mint 21/22 already pixel-perfect (`eframe 0.29.1 glow`, `winit 0.30 x11+wayland`, `ZERONODE_BACKEND` env, `fonts-dejavu` fallback, `io.zeronode.vpn.desktop` `Icon`/`StartupWMClass`)  
**Date:** 2026-08-24  
**Version:** v0.2.0 frontend

---

## 1. Objective

Make the **same `vpn-client` binary** (`egui 0.29.1`, 1160×720 glow, earth wireframe + beacon, 5 protocol cards) run pixel-identical on Debian/Arch/Fedora X11 and Wayland, with no per-distro forks. Only packaging and distro-aware hints change.

Reuse backend plan's **glibc 2.31** binary + packaging matrix. Frontend deltas are almost zero — this plan proves why.

---

## 2. Current Frontend (Ubuntu/Mint) — audit

* **Viewport** (`apps/client/src/app.rs:138`): `ViewportBuilder 1160×720 min 820×580  title APP_NAME  app_id io.zeronode.vpn  IconData` + `NativeOptions.event_loop_builder` switching `with_x11`/`with_wayland` via `ZERONODE_BACKEND` env or `WAYLAND_DISPLAY`/`DISPLAY` auto. `elevation.rs` `pkexec` child forces `ZERONODE_BACKEND=x11` (root cannot open Wayland socket).
* **Theme** (`install_theme`): `VPN_GREEN 0,255,127` `VPN_CARD_BG 13,13,13` side `8,8,8 stroke 32,32,32` header 24 Strong WHITE; `feathering_size 0.6`; globe `globe/renderer.rs` pure `egui::Painter` (body 18,20,22 halo 0,255,127,18 borders 1.002 server dots 3/4px beacon 1.6 rad/s ripples 2.4s 26) — **OS-neutral**, already verified on X11/Wayland via `wayland-dlopen`.
* **Fonts** (`install_crisp_fonts`): probes `[DejaVuSans, LiberationSans, NotoSans, Ubuntu-R]` and mono equivalents before Segoe via `OnceLock` `ctx.set_fonts`; `Cargo.toml:62` `Recommends fonts-dejavu-core` + `xdg-desktop-portal-gtk|kde, zenity`. On Arch `ttf-dejavu`, Fedora `dejavu-sans-fonts` — same probe list works.
* **Dialogs/tray** (`rfd 0.15` `xdg-portal`, `tray-icon 0.24`): Wayland uses `libwayland-client.so.0` dlopen, xdg portal for file pickers, `tray-icon` GTK fallback. Already feature-gated.
* **Elevation UX** (`app.rs:1960` Tor label, system VPN `ACTIVE (0.0.0.0/0 via TUN)` vs `Wintun`, `apply_tor_system_route`/`remove_tor_system_route` `pkexec` need-Admin messages): pkexec message already distro-aware via backend `install_hint`.
* **Icons/systemd**: `Cargo.toml:65` `assets = [icon.png → hicolor/512x512]` `io.zeronode.vpn.desktop` `Icon=io.zeronode.vpn` `StartupWMClass=io.zeronode.vpn`; `egui viewport icon` from `IconData` — GNOME dock class already correct on all distros.
* **Build**: `eflate2`+`image 0.25.5 png`+`geojson 0.24.1`+`maxminddb` pure Rust — no glibc-sensitive UI native deps.

**Verdict:** No code change needed for Debian/Arch/Fedora Wayland/X11 rendering. Same binary, same `winit` backends, same fonts. Only `Recommends`/`Suggests` per-family strings and docs differ.

---

## 3. Gap Analysis

| Area | Debian 11/12/13 | Arch | Fedora | Frontend impact |
|---|---|---|---|---|
| **Windowing** | Xorg default on 11, Wayland default on 12/13 GNOME | Wayland default (GNOME/KDE) + X11 | Wayland default (GNOS W) + X11 on KDE | **None** — `winit 0.30 wayland+x11` dlopen already covers both; `ZERONODE_BACKEND` override stays |
| **Scale/HiDPI** | GNOME 100/200% fractional | KDE 125/150% | GNOME 200% | **None** — `egui`/`winit` scale factor is compositor-native; wireframe uses `Painter` logical pixels; tested on 1x/2x |
| **Tray** | `ayatana-appindicator` (GNOME extension) | `libayatana-appindicator` AUR | `libayatana-appindicator-gtk3` + GNOME appindicator extension | **No code** — `tray-icon 0.24` uses `libayatana` if present, else falls back to GTK; `Suggests` per family already correct |
| **File dialogs** | `xdg-desktop-portal-gtk` | `xdg-desktop-portal-gtk`/`xdg-desktop-portal-kde` + `xdg-desktop-portal-gnome` | `xdg-desktop-portal-gtk` | **No code** — `rfd xdg-portal` union already `Recommends` portal+zenity |
| **Fonts** | `fonts-dejavu-core` | `ttf-dejavu` + `noto-fonts` | `dejavu-sans-fonts` + `google-noto-sans-fonts` | **No code** — probe list `DejaVu→Liberation→Noto→Ubuntu` resolves on all; Arch `ttf-dejavu` provides `DejaVuSans.ttf` same path `/usr/share/fonts/TTF/DejaVuSans.ttf` vs Debian `/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf` but `egui` `SystemSource` enumerates via fontconfig, not path |
| **Icon class** | `io.zeronode.vpn` WM_CLASS | same | same | **None** — `with_app_id` + desktop `StartupWMClass` already perfect |
| **Polkit dialog** | `polkit-1` `pkexec` GTK | `polkit` `pkexec` | `polkit` `pkexec` | **None** — backend `pkexec_available()` now checks `/usr/sbin` too |
| **NVIDIA/Wayland** | old 470 driver → XWayland fallback | latest → Wayland native | latest → Wayland | Already handled: `ZERONODE_BACKEND=x11` fallback note in docs |

**Conclusion:** Frontend **zero code diff** for Debian/Arch/Fedora. Only packaging `depends`/`recommends` already in backend plan, plus docs.

---

## 4. Design Decisions

1. **Keep single binary** `vpn-client` built with `winit 0.30` `wayland`+`x11` + `wayland-dlopen`. Do **not** add `x11-only` or `wayland-only` features. Debian 11 X11-only still runs because `wayland` feature dlopens at runtime — missing `libwayland-client.so.0` is not fatal when `DISPLAY` set and `WAYLAND_DISPLAY` unset (tested v0.1.0).
2. **No per-distro font map** — keep `install_crisp_fonts` probe order. Add Fedora `Cantarell` to probe tail (optional) for GNOME native feel, but not required for perfect replication.
3. **Keep `ViewportBuilder` unchanged** — `with_inner_size [1160,720]` + `with_min_inner_size [820,580]` + `with_app_id("io.zeronode.vpn")` works on all WMs. `centered` is Wayland no-op, fine.
4. **Keep `rfd` + `tray-icon` as is** — no Arch/Fedora special case. Ensure `Suggests`/`OptDepends` lists cover `libayatana` so tray appears when extension installed; absence = silent no-tray (acceptable).
5. **Docs only**: Update `README.md` distro matrix (Debian/Arch/Fedora install commands) and `docs/LINUX_CLIENT_MICRO_PLAN.md` to mention glibc 2.31 single binary.

---

## 5. File Map (frontend changes)

* `apps/client/src/app.rs` — **no change** (optional: add `Cantarell` to tail of font probe for Fedora polish; not required)
* `apps/client/assets/debian/io.zeronode.vpn.desktop` — **no change** (`Icon`/`StartupWMClass` already perfect)
* `apps/client/Cargo.toml` — **already done** in backend plan `generate-rpm` assets include icon → hicolor, same for arch/fedora
* `docs/LINUX_FRONTEND_MULTI_DISTRO_PLAN.md` — **this file** (Step 3)
* `README.md` — Step 5 matrix update: add Debian `sudo apt install ./zeronode...deb`, Arch `sudo pacman -U ...pkg.tar.zst` / `yay -S zeronode-vpn-client`, Fedora `sudo dnf install ./...rpm`
* No new Rust files.

---

## 6. Execution Checklist (Step 4)

1. **Audit only** — verify `app.rs` viewport/theme/globe/fonts/elevation strings already handle all three families (done above — zero diff)
2. **Optional polish** — if Fedora `Cantarell` desired, add to tail of `install_crisp_fonts` chain (one-line)
3. **Regenerate docs** — update `README.md` install matrix to include Debian/Arch/Fedora alongside Mint/Ubuntu
4. **Build smoke** — reuse backend's `cargo zigbuild --target x86_64-unknown-linux-gnu.2.31 -p vpn-client` binary; launch headless `vpn-client --help` and `cargo check -p vpn-client` on VM; verify `WAYLAND_DISPLAY`/`DISPLAY` auto-switch logs
5. **Container smoke** (where available) — `docker run debian:11|debian:12|archlinux:latest|fedora:41` `ldd` glibc 2.31, `gtk-launch io.zeronode.vpn` desktop validates, `pactl` no-op
6. **Packaging reuse** — Arch `PKGBUILD` and Fedora `spec` already stage same icon/desktop; no frontend packaging delta

---

## 7. Packaging Matrix (frontend-relevant rows)

| Family | Frontend deps (from backend plan) | Icon/desktop | Tray |
|---|---|---|---|
| Debian | `fonts-dejavu-core` `xdg-desktop-portal-gtk` | `hicolor/512x512 io.zeronode.vpn.png` + `io.zeronode.vpn.desktop` | `libayatana-appindicator3-1` Suggests |
| Arch | `ttf-dejavu` `xdg-desktop-portal-gtk|kde` | same | `libayatana-appindicator` optdepends |
| Fedora | `dejavu-sans-fonts` `xdg-desktop-portal-gtk` | same | `libayatana-appindicator-gtk3` Suggests |

---

## 8. Verification (perfect)

* `cargo check -p vpn-client` clean on VM + Windows, `WAYLAND_DISPLAY` unset → X11 path, set → Wayland path (log `winit backend: wayland|x11 forced x11 (pkexec root)`).
* Visual: 1160×720 glow, cyber-green `#00FF7F` cards, earth wireframe 18+20+22 circles, server dots 3/4px — screenshot diff vs Ubuntu reference <1% (font AA only).
* `desktop-file-validate io.zeronode.vpn.desktop` passes on all three; `gtk-launch` shows ZeroNode icon in dock (Wayland `app_id` correct).
* `rfd` file picker opens via portal on Wayland, via GTK on X11; no crash when `libayatana` missing (graceful no-tray).

---

## 9. Risks

* Arch `ttf-dejavu` path differs but `fontconfig` resolves via `SystemSource` — no risk; add `Cantarell` tail only if GNOME default preferred.
* Fedora `firewalld` not frontend — covered in backend plan.
* Missing `libwayland-client.so.0` on minimal Debian 11 X11-only → `wayland-dlopen` safe, no crash.
