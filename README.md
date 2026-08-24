# ZeroNode VPN Suite

[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Platform: Linux](https://img.shields.io/badge/platform-linux%20%7C%20windows%20%7C%20android-blue)](https://github.com/xyzyt010/zeronode-vpn-suite/releases)
[![Release](https://img.shields.io/badge/release-v0.2.0-brightgreen)](https://github.com/xyzyt010/zeronode-vpn-suite/releases/tag/v0.2.0)

Self-hosted VPN suite with zero-config control plane — **Rust workspace** shipping a UDP control daemon, **egui (glow)** desktop clients for **Linux (Debian · Ubuntu · Mint · Arch · Fedora — X11 + Wayland)** and **Windows**, an **Android VpnService APK**, and support for **WireGuard, OpenVPN, Shadowsocks/Outline, PPTP, Tor**.

> **Frontend parity:** The Linux client is a pixel-perfect replication of the Windows app — same `eframe 0.29 + egui + glow` stack, same cyber-green palette, same globe wireframe, same animations — running natively on **X11/XFCE4** and **GNOME/Wayland** from a **single glibc 2.31 binary** (Debian 11+ → Fedora 42).

---

## Features

- **Zero-config control plane** — single UDP port for discovery / auth / status / disconnect; Argon2id, per-IP cooldowns, lockdown, session leases, WireGuard peer rendering.
- **Multi-protocol tunnels** — kernel `wireguard-control` + `boringtun` fallback, `openvpn` binary, `shadowsocks-service` (Outline), `pppd` + `pptp`, Tor expert bundle + `tun2proxy`.
- **Desktop GUI** — `1160x720` egui app with interactive globe (`countries_50m.geojson` + centroids), protocol cards, drop zones (`.ovpn`/`.conf`/`ss://`), bootstrap progress, Tor exit geo, system-wide TUN via `tproxy-config` + `tun2proxy`.
- **Linux parity — 5 distros** — `winit 0.30` x11+wayland (`wayland-dlopen`), `ZERONODE_BACKEND=x11|wayland` override, `pkexec` (UAC parity), distro-aware diagnose (`/etc/os-release` → apt/pacman/dnf hints), `DejaVu→Liberation→Noto→Cantarell` font chain.
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

## Download — Latest Release `v0.2.0`

> Direct download links (GitHub Releases). No build required. One Linux binary runs on **Debian 11/12/13, Ubuntu 22.04/24.04, Mint 21/22, Arch rolling, Fedora 40/41/42**.

| Platform | File | Links |
|---|---|---|
| **Debian / Ubuntu / Mint** | `zeronode-vpn-client_0.2.0-1_amd64.deb` (19 MB) · `vpn-client-linux-amd64` (33 MB) · `zeronode-vpn-server_0.2.0-1_amd64.deb` | [deb](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/zeronode-vpn-client_0.2.0-1_amd64.deb) · [binary](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/vpn-client-linux-amd64) · [server deb](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/zeronode-vpn-server_0.2.0-1_amd64.deb) |
| **Arch Linux** | `zeronode-vpn-client-0.2.0-1-x86_64.pkg.tar.zst` + `PKGBUILD` | [pkg.tar.zst](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/zeronode-vpn-client-0.2.0-1-x86_64.pkg.tar.zst) · [PKGBUILD](https://github.com/xyzyt010/zeronode-vpn-suite/blob/main/tools/arch/PKGBUILD) |
| **Fedora 40/41/42** | `zeronode-vpn-client-0.2.0-1.x86_64.rpm` (+ fallback tarball) | [rpm](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/zeronode-vpn-client-0.2.0-1.x86_64.rpm) |
| **Portable (all)** | `zeronode-vpn-client-0.2.0-linux-x86_64.tar.gz` | [tar.gz](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/zeronode-vpn-client-0.2.0-linux-x86_64.tar.gz) |
| **Windows 10/11 x64** | `vpn-client.exe` (9.8 MB) + bundle | [vpn-client.exe](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/vpn-client-windows-x64.exe) · [bundle zip](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/zeronode-windows-bundle-v0.2.0.zip) |
| **Android 7+** | `zeronode-vpn-client-release.apk` | [apk](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/zeronode-vpn-client-release.apk) · [vpnservice apk](https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/zeronode-vpn-client-vpnservice-release.apk) |

**All releases:** https://github.com/xyzyt010/zeronode-vpn-suite/releases

SHA256: see `SHA256SUMS` in each release.

---

## Quick Start

### Debian / Ubuntu / Mint (X11 + Wayland) — apt

```bash
wget https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/zeronode-vpn-client_0.2.0-1_amd64.deb -O /tmp/zeronode.deb
sudo dpkg -i /tmp/zeronode.deb
sudo apt-get install -f   # pulls Recommends: openvpn wireguard-tools pptp-linux fonts-dejavu-core xdg-desktop-portal-gtk
vpn-client                # or ZeroNode VPN from menu (io.zeronode.vpn)
```

### Arch Linux — pacman / yay

```bash
# From Release (prebuilt)
wget https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/zeronode-vpn-client-0.2.0-1-x86_64.pkg.tar.zst
sudo pacman -U zeronode-vpn-client-0.2.0-1-x86_64.pkg.tar.zst
# or from PKGBUILD (build from source)
git clone https://github.com/xyzyt010/zeronode-vpn-suite.git && cd zeronode-vpn-suite/tools/arch && makepkg -si
vpn-client
```

Dependencies: `nftables iproute2 kmod polkit openvpn wireguard-tools pptpclient ppp` + `ttf-dejavu xdg-desktop-portal-gtk` (opt).

### Fedora — dnf

```bash
wget https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/zeronode-vpn-client-0.2.0-1.x86_64.rpm
sudo dnf install ./zeronode-vpn-client-0.2.0-1.x86_64.rpm
# Fallback tarball (if rpm not available):
# tar xzf zeronode-vpn-client-0.2.0-1.x86_64.fedora.tar.gz -C / && sudo dnf install nftables iproute kmod polkit
vpn-client
```

### Portable (any distro, no root install)

```bash
wget https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/vpn-client-linux-amd64
chmod +x vpn-client-linux-amd64
./vpn-client-linux-amd64
# Force backend if needed: ZERONODE_BACKEND=x11 ./vpn-client-linux-amd64
# Or tarball:
wget https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v0.2.0/zeronode-vpn-client-0.2.0-linux-x86_64.tar.gz && tar xzf *.tar.gz && ./vpn-client
```

Server:
```bash
sudo dpkg -i zeronode-vpn-server_0.2.0-1_amd64.deb  # Debian/Ubuntu/Mint; on Arch/Fedora build from source or tarball
sudo systemctl status zeronode-vpn-server
sudo vpn-server host-setup apply
vpn-server-gui   # dashboard
```

`XDG_SESSION_TYPE=wayland` → Wayland/EGL, else X11. Elevated Tor/OpenVPN/WireGuard uses `pkexec` → `XWayland` on GNOME (root cannot open Wayland socket) — handled automatically.

### Windows

```powershell
# From bundle zip (dist/windows/bin)
.\vpn-client.exe
```

Requires Administrator for system TUN (Wintun) — UAC dialog appears on connect.

### Android

```bash
adb install zeronode-vpn-client-release.apk
```

---

## Build from Source

**Prerequisites:** Rust stable via `rustup`, Ubuntu `22.04+` / Windows 10+ with VS Build Tools, Android SDK (for APK). For glibc 2.31 compat (Debian 11): `cargo install cargo-zigbuild --locked && pip install ziglang` or `brew install zig`.

```bash
# Linux builder deps
./tools/provision-ubuntu-builder.sh

# All-Linux (Debian .deb + Arch PKGBUILD + Fedora RPM + portable tarball) — glibc 2.31
./tools/build-linux-all.sh
# or single:
./tools/build-linux.sh          # → dist/linux/bin/  target/debian/*.deb  (zigbuild x86_64-unknown-linux-gnu.2.31)
./tools/arch/build.sh           # → dist/arch/*.pkg.tar.zst (makepkg or manual)
./tools/build-fedora.sh         # → dist/fedora/*.rpm (cargo-generate-rpm or rpmbuild)

# Windows (PowerShell)
powershell -ExecutionPolicy Bypass -File .\tools\build-windows.ps1 -Profile release
# → dist/windows/

# Android (PowerShell)
powershell -ExecutionPolicy Bypass -File .\tools\build-android-vpnservice.ps1 -Profile release
# → dist/android/*.apk
```

Cross-compiling Linux `amd64` on `arm64` host (aarch64 VM):
```bash
cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.31 -p vpn-client -p vpn-server
cargo deb --target x86_64-unknown-linux-gnu -p vpn-client --no-build
cargo generate-rpm --target x86_64-unknown-linux-gnu -p vpn-client
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
  platform-linux/  # elevation/pkexec, wg/tun, socks, tor, ovpn, pptp + Distro detection
  platform-windows/# wintun, win32, elevation UAC
vendor/
  tun/             # patched TUN (Linux + Windows)
  tproxy-config/   # nftables/DNS/route helpers
tools/
  build-linux.sh, build-linux-all.sh, provision-ubuntu-builder.sh, fetch-tor-linux.sh
  arch/PKGBUILD, arch/build.sh, build-fedora.sh, fedora/zeronode-vpn-client.spec
  build-windows.ps1, build-android-vpnservice.ps1
docs/
  LINUX_BACKEND_MULTI_DISTRO_PLAN.md, LINUX_FRONTEND_MULTI_DISTRO_PLAN.md
  LINUX_FRONTEND_REPLICATION_PLAN.md, LINUX_CLIENT.md
```

## Linux Desktop Notes

- **Backend:** `eframe 0.29 glow` + `winit 0.30` x11/wayland (`wayland-dlopen`), single glibc 2.31 binary for all distros.
- **Distro hints:** `crates/platform-linux/src/common.rs:detect_distro()` reads `/etc/os-release` → `apt`/`pacman`/`dnf` install hints, `/usr/sbin` pptp fallback.
- **Fonts:** `DejaVu Sans` → `Liberation` → `Noto` → `Cantarell` → `Segoe UI` via `egui::FontData`; per-distro Recommends `fonts-dejavu-core` / `ttf-dejavu` / `dejavu-sans-fonts`.
- **Pickers:** `rfd 0.15` xdg-portal; needs `xdg-desktop-portal-gtk` + `zenity`.
- **Wayland:** `centered` no-op, `with_app_id("io.zeronode.vpn")` for dock, `Wayland-dlopen` safe on X11-only.
- **Scaling:** fractional `1.25/1.5/1.75` via `wp-fractional-scale-v1`; globe hit math uses logical px.

## Releases

Semantic versioning. Artifacts are **not** tracked in git — download from [Releases](https://github.com/xyzyt010/zeronode-vpn-suite/releases) with `SHA256SUMS`.

- `vpn-client-linux-amd64` — portable (glibc 2.31)
- `zeronode-vpn-client_0.2.0-1_amd64.deb` — Debian/Ubuntu/Mint
- `zeronode-vpn-client-0.2.0-1-x86_64.pkg.tar.zst` — Arch
- `zeronode-vpn-client-0.2.0-1.x86_64.rpm` — Fedora
- `vpn-client-windows-x64.exe` — Windows
- `zeronode-vpn-client-release.apk` — Android

## Contributing

PRs welcome. Keep `cargo check -p vpn-client -p vpn-platform-linux` green on both `x86_64-unknown-linux-gnu` and Windows, and `cargo test -p vpn-platform-linux` passing.

## License

MIT — see [LICENSE](LICENSE).

---

Built with Rust · egui · boringtun · tun2proxy · shadowsocks-service · Tor
