# ZeroNode VPN Suite

[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Platform: Linux](https://img.shields.io/badge/platform-linux%20%7C%20windows%20%7C%20android-blue)](https://github.com/xyzyt010/zeronode-vpn-suite/releases)
[![Release](https://img.shields.io/badge/release-v0.1.0-brightgreen)](https://github.com/xyzyt010/zeronode-vpn-suite/releases/tag/v0.1.0)

Self-hosted VPN suite with zero-config control plane — **Rust workspace** shipping a UDP control daemon, **egui (glow)** desktop clients for **Linux (Ubuntu X11/Wayland + Mint)** and **Windows**, an **Android VpnService APK**, and support for **WireGuard, OpenVPN, Shadowsocks/Outline, PPTP, Tor**.

> **Frontend parity:** The Linux client is a pixel-perfect replication of the Windows app — same `eframe 0.29 + egui + glow` stack, same cyber-green palette, same globe wireframe, same animations — running natively on both **X11/XFCE4** and **GNOME/Wayland** from a single binary.

---

## Features

- **Zero-config control plane** — single UDP port for discovery / auth / status / disconnect; Argon2id, per-IP cooldowns, lockdown, session leases, WireGuard peer rendering.
- **Multi-protocol tunnels** — kernel `wireguard-control` + `boringtun` fallback, `openvpn` binary, `shadowsocks-service` (Outline), `pppd` + `pptp-linux`, Tor expert bundle + `tun2proxy`.
- **Desktop GUI** — `1160x720` egui app with interactive globe (`countries_50m.geojson` + centroids), protocol cards, drop zones (`.ovpn`/`.conf`/`ss://`), bootstrap progress, Tor exit geo, system-wide TUN via `tproxy-config` + `tun2proxy`.
- **Linux parity** — `winit 0.30` x11+wayland, `ZERONODE_BACKEND=x11|wayland` override, `pkexec` (UAC parity), `DejaVu` font chain, `rfd` portal pickers, fractional scaling, `app_id=io.zeronode.vpn`.
- **Windows** — `wintun.dll` + `tap-windows`, `ShellExecuteW runas` UAC, `winres` manifest.
- **Android** — `VpnService` + JNI + `boringtun`, `aarch64`/`armv7`/`x86_64` via `cargo-apk` without Gradle.

## Architecture

```
apps/client (egui) ──► platform-linux / platform-windows (same API) ──► OS (tun, nftables, pkexec, openvpn, pppd)
apps/server  (UDP daemon) ──► vpn-suite-core (config, crypto, wireguard rendering)
apps/server-gui (dashboard)      vendor/tun + vendor/tproxy-config
vpnctl (CLI)                     crates/core
```

Pure `egui::Painter` globe — no OpenGL mesh, no textures — portable across platforms.

---

## Download — Latest Release `v0.1.0`

> Direct download links (GitHub Releases). No build required.

| Platform | File | Links |
|---|---|---|
| **Linux (Ubuntu 22.04+ / Mint 21/22)** | `zeronode-vpn-client_0.1.0-1_amd64.deb` (19 MB) + `vpn-client` binary (33 MB) | [deb](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.1.0/zeronode-vpn-client_0.1.0-1_amd64.deb) · [binary](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.1.0/vpn-client-linux-amd64) · [server deb](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.1.0/zeronode-vpn-server_0.1.0-1_amd64.deb) |
| **Windows 10/11 x64** | `vpn-client.exe` (9.8 MB) + `vpn-server.exe`, `vpnctl.exe`, `wireguard.exe` | [vpn-client.exe](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.1.0/vpn-client-windows-x64.exe) · [bundle zip](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.1.0/zeronode-windows-bundle-v0.1.0.zip) |
| **Android 7+** | `zeronode-vpn-client-release.apk` | [apk](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.1.0/zeronode-vpn-client-release.apk) · [vpnservice apk](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.1.0/zeronode-vpn-client-vpnservice-release.apk) |

**All releases:** https://github.com/xyzyt010/zeronode-vpn-suite/releases

SHA256: see `SHA256SUMS` in each release.

---

## Quick Start

### Linux — Ubuntu / Mint (X11 + Wayland)

**Install .deb (recommended):**
```bash
sudo dpkg -i zeronode-vpn-client_0.1.0-1_amd64.deb
sudo apt-get install -f   # pulls Recommends: openvpn wireguard-tools pptp-linux fonts-dejavu-core
vpn-client                # or launch ZeroNode VPN from menu (io.zeronode.vpn)
```

**Raw binary:**
```bash
chmod +x vpn-client-linux-amd64
./vpn-client-linux-amd64
# Force backend if needed: ZERONODE_BACKEND=x11 ./vpn-client-linux-amd64
```

Server:
```bash
sudo dpkg -i zeronode-vpn-server_0.1.0-1_amd64.deb
sudo systemctl status zeronode-vpn-server
sudo vpn-server host-setup apply
vpn-server-gui   # dashboard
```

`XDG_SESSION_TYPE=wayland` → Wayland/EGL, else X11. Elevated Tor/OpenVPN/WireGuard uses `pkexec` → `XWayland` on GNOME (root cannot open Wayland socket) — handled automatically.

### Windows

```powershell
# From bundle zip (dist/windows/bin)
.\vpn-client.exe
# Or installer: double-click zeronode-windows-bundle-v0.1.0.zip → run vpn-client.exe
```

Requires Administrator for system TUN (Wintun) — UAC dialog appears on connect.

### Android

```bash
adb install zeronode-vpn-client-release.apk
# Or copy APK to device and install
```

Grant VPN permission, select protocol, import `.ovpn`/`.conf`/`ss://`.

---

## Build from Source

**Prerequisites:** Rust stable via `rustup`, Ubuntu `22.04+` / Windows 10+ with VS Build Tools, Android SDK (for APK).

```bash
# Linux builder deps
./tools/provision-ubuntu-builder.sh

# Release + debs (fetches Tor 15.0.17 bundle)
./tools/build-linux.sh
# → dist/linux/bin/  target/debian/*.deb

# Windows (PowerShell)
powershell -ExecutionPolicy Bypass -File .\tools\build-windows.ps1 -Profile release
# → dist/windows/

# Android (PowerShell)
powershell -ExecutionPolicy Bypass -File .\tools\build-android-vpnservice.ps1 -Profile release
# → dist/android/*.apk
```

Cross-compiling Linux `amd64` on `arm64` host:
```bash
cargo zigbuild --release --target x86_64-unknown-linux-gnu -p vpn-client -p vpn-server
cargo deb --target x86_64-unknown-linux-gnu -p vpn-client --no-build
```

---

## Project Structure

```
apps/
  client/          # egui desktop (globe, protocols, Tor)
  client-cli/      # vpnctl
  server/          # UDP control daemon
  server-gui/      # dashboard
crates/
  core/            # config, packets, leases, wireguard rendering
  platform-linux/  # elevation/pkexec, wg/tun, socks, tor, ovpn, pptp
  platform-windows/# wintun, win32, elevation UAC
vendor/
  tun/             # patched TUN (Linux + Windows)
  tproxy-config/   # nftables/DNS/route helpers
tools/
  build-linux.sh, provision-ubuntu-builder.sh, fetch-tor-linux.sh
  build-windows.ps1, build-android-vpnservice.ps1
docs/
  LINUX_CLIENT_MICRO_PLAN.md, LINUX_FRONTEND_REPLICATION_PLAN.md, LINUX_CLIENT.md
```

## Linux Desktop Notes

- **Backend:** `eframe 0.29 glow` + `winit 0.30` x11/wayland (`wayland-dlopen`), single binary.
- **Fonts:** `DejaVu Sans` → `Liberation` → `Noto` → `Segoe UI` fallback; `fonts-dejavu-core` Recommends.
- **Pickers:** `rfd 0.15` xdg-portal; needs `xdg-desktop-portal-gtk` + `zenity` (Recommends).
- **Wayland:** `centered` no-op, drag-drop disabled (use picker/paste), `with_app_id("io.zeronode.vpn")` for dock.
- **Scaling:** fractional `1.25/1.5/1.75` via `wp-fractional-scale-v1`; globe hit math uses logical px.
- **Tracer:** logs to `~/.local/share/ZeroNode/vpn-client.log` + `vpn-client.panic.log`.

## Releases

Semantic versioning. Artifacts are **not** tracked in git — download from [Releases](https://github.com/xyzyt010/zeronode-vpn-suite/releases) with `SHA256SUMS`.

- `vpn-client-linux-amd64` — portable, no install
- `zeronode-vpn-client_0.1.0-1_amd64.deb` — full install (tor bundle, flags, desktop, icon)
- `vpn-client-windows-x64.exe` — portable Windows
- `zeronode-windows-bundle-v0.1.0.zip` — full Windows dist
- `zeronode-vpn-client-release.apk` — Android

## Contributing

PRs welcome. Keep `cargo check -p vpn-client -p vpn-platform-linux` green on both `x86_64-unknown-linux-gnu` and Windows, and `cargo test -p vpn-platform-linux` (11 tests) passing.

## License

MIT — see [LICENSE](LICENSE).

---

Built with Rust · egui · boringtun · tun2proxy · shadowsocks-service · Tor
