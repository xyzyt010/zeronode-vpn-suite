# Linux Backend — Multi-Distro Perfect Integration Plan

**Targets:** Debian 11/12/13 (X11 + Wayland, GNOME/KDE/XFCE) · Arch Linux rolling (X11/Wayland) · Fedora 40/41/42 (Workstation, X11/Wayland)  
**Baseline:** Ubuntu 22.04/24.04 + Mint 21/22 already perfect (zigbuild x86_64, `vpn-platform-linux`, `cargo-deb`, `pkexec`+TUN)  
**Date:** 2026-08-24  
**Version:** v0.2.0 backend

---

## 1. Objective

Extend `crates/platform-linux` from Ubuntu/Mint-only to **distro-agnostic** Linux backend: one Rust crate, one `vpn-client` binary (`x86_64-unknown-linux-gnu` built on oldest glibc, plus `aarch64`), runs flawlessly on Debian/Arch/Fedora X11 and Wayland with:

* identical 5 protocols (WG kernel+boringtun, OpenVPN, SS/Outline, PPTP, Tor expert bundle)
* identical TUN (`vendor/tun` `vendor/tproxy-config`) nft/iptables, DNS, `pkexec` elevation
* **native packaging per family:** `.deb` (Debian/Ubuntu/Mint), `.pkg.tar.zst` / `PKGBUILD` (Arch), `.rpm` (Fedora), plus portable `tar.gz`/`bin`

No per-distro forks. All detection at **runtime** + packaging at build time only.

---

## 2. Current State (Ubuntu/Mint)

* `crates/platform-linux`: `lib.rs` kernel WG, `wireguard.rs` (global/boringtun `0.0.0.0/1` `128.0.0.0/1`, `ip route get` endpoint pin, `resolvectl`+`resolv.conf` DNS), `openvpn.rs` (`/usr/bin/openvpn` probe), `pptp.rs` (`pppd pty "pptp"` + `/etc/ppp/peers/zeronode-pptp` `# BEGIN/END`), `socks_tun.rs` (`tun2proxy`+`tproxy`), `outline.rs` (`sslocal`), `elevation.rs` (`pkexec --disable-internal-agent env HOME=$HOME ZERONODE_BACKEND=x11`), `common.rs` `command_exists` PATH, `client_setup.rs` diagnostics
* `apps/client/Cargo.toml:57` `cargo-deb` `Depends $auto nftables iproute2 kmod policykit-1` `Recommends openvpn pptp-linux ppp wireguard-tools fonts-dejavu-core xdg-desktop-portal-gtk|kde zenity`
* Build: `cargo zigbuild --target x86_64` on Ubuntu 24.04 aarch64 host → ELF glibc 2.39 (too new for Debian 11 glibc 2.31)
* DNS: `resolvectl` primary, `resolv.conf` bind-mount fallback, MTU probe — tested on systemd-resolved
* `vendor/tun` `vendor/tproxy-config` rtnetlink nft

**Works on:** Ubuntu 22.04/24.04, Mint 21/22, *and* Debian 12/13 unchanged (same apt names). Fails packaging/distro: Arch (`pacman` names `polkit` `pptpclient`), Fedora (`dnf` `polkit` `iprange` etc), and glibc forward-compat.

---

## 3. Gap Analysis (per distro)

| Area | Debian 11/12/13 | Arch rolling | Fedora 40/41/42 | Fix |
|---|---|---|---|---|
| **Package manager** | `apt dpkg` same as Ubuntu | `pacman` (`nftables iproute2 kmod polkit openvpn wireguard-tools pptpclient ppp ttf-dejavu xdg-desktop-portal`) | `dnf rpm` (`nftables iproute kmod polkit openvpn wireguard-tools pptp ppp dejavu-sans-fonts xdg-desktop-portal`) | Add `PKGBUILD` + `cargo-generate-rpm`/`rpm` spec, keep `.deb` for Debian |
| **Glibc compat** | 11=2.31 12=2.36 13=2.39 | rolling 2.39-2.40 | 40/41=2.39 42=2.40 | Build on **Debian 11 container** or `zigbuild -target x86_64-unknown-linux-gnu.2.31` + musl fallback |
| **binary paths** | `/usr/bin/{openvpn,pppd,pptp,nft,ip,systemctl,pkexec,resolvectl}` same | same | `pptp` is `/usr/sbin/pptp` on Fedora, `nft`/`ip` same | Resolve via `which`/`command_exists` PATH sweep, add `/usr/sbin` to probe |
| **pptp peer name** | `pptp-linux` package `pptp` bin | `pptpclient` package `pptp` bin | `pptp` package `pptp` bin | `command_exists("pptp")` already; doc `Suggests` per family |
| **DNS** | `systemd-resolved`+`resolvconf`+`NetworkManager` | `systemd-resolved` + `openresolv` (`/etc/resolv.conf` symlink) | `systemd-resolved` dominant | Keep dual-stack: `resolvectl` if exists else `resolv.conf` backup/restore + `NetworkManager` unmanaged device note |
| **polkit** | `policykit-1` | `polkit` | `polkit` | `cargo-deb Depends policykit-1` stays; `PKGBUILD depends=(polkit)` `rpm Requires: polkit` |
| **nft vs iptables** | nft default, iptables fallback | same | `nft` default on F42, `firewalld` may hold `iptables` | Already dual: `nft` table `inet zeronode` → fallback `iptables -t nat`; add `firewalld` detection warn |
| **Desktop portal** | `xdg-desktop-portal-gtk` | `xdg-desktop-portal-gtk|kde` + `xdg-desktop-portal-gnome` | `xdg-desktop-portal-gtk` | Keep `Recommends` union |
| **Tray** | `libayatana-appindicator3-1` | `libayatana-appindicator` | `libayatana-appindicator-gtk3` | Already `Suggests`; Arch/Fedora `optdepends`/`Recommends` |
| **systemd unit path** | `/lib/systemd/system` | `/usr/lib/systemd/system` | `/usr/lib/systemd/system` | `lib.rs:enable_systemd_service` already checks both, keep |
| **Flags/assets** | `assets/flags` ok | same | same | No change |

No Rust code fork needed for Debian; small runtime probes for Arch/Fedora paths/DNS, plus packaging.

---

## 4. Design Decisions (perfect + minimal)

1. **Single binary stays** `x86_64-unknown-linux-gnu` built against **glibc 2.31** via `cargo zigbuild --target x86_64-unknown-linux-gnu.2.31` on VM, so one ELF runs on all three families (tested `ldd` floor). Keep `aarch64-unknown-linux-gnu.2.31` as well. Alternative `musl` kept as fallback for portable `*-musl` bin.
2. **Runtime distro detection** only for diagnostics & messages: `crates/platform-linux/src/common.rs` adds `detect_distro()` reading `/etc/os-release` `ID`/`ID_LIKE` → `Debian|Arch|Fedora|Ubuntu|Mint|Unknown`. Used for `client_setup.rs` advice and `resolve_tor_binary` paths, **not** for branching TUN (except pptp `/usr/sbin` sweep and DNS `openresolv` path).
3. **PATH probe extended**: `command_exists()` already sweeps `$PATH`; add explicit `/usr/sbin` (`pptp` on Fedora, `nft` sometimes) fallback `Path::new("/usr/sbin/<bin>").exists()` before fail. Add `which` helper that checks `/usr/bin` then `/usr/sbin`.
4. **DNS**: keep `wireguard.rs` current `resolvectl status` → `resolvectl dns` + `resolvconf` fallback. Add `openresolv` detection (`/sbin/resolvconf` or `/usr/sbin/resolvconf` symlink) for Arch: same bind-mount `resolv.conf` restore already covers it.
5. **Packaging only** per family, no code split:
   * **Debian**: reuse existing `cargo-deb` (no new file) — ensure `glibc 2.31` deb builds; add CI `debian/control` check `ldd --version`.
   * **Arch**: new `tools/arch/PKGBUILD` (maintainer ZeroNode, `arch=(x86_64 aarch64)`, `depends=(nftables iproute2 kmod polkit openvpn wireguard-tools pptpclient ppp)`, `optdepends=('ttf-dejavu: crisp fonts' 'xdg-desktop-portal-gtk: file dialogs')`, `source=(vpn-client)` + `package()` installs `usr/bin/vpn-client`, `usr/share/applications/io.zeronode.vpn.desktop`, `usr/share/icons/hicolor`, `usr/share/vpn-client/{tor-linux,flags}`. Provide `tools/build-arch.sh` that does `cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.31` then `makepkg` in container or plain `tar czf` fallback.
   * **Fedora**: new `tools/fedora/zeronode-vpn-client.spec` or `cargo-generate-rpm` config in `Cargo.toml` `[package.metadata.generate-rpm]` `requires = nftables iproute kmod polkit openvpn wireguard-tools pptp ppp` , `tools/build-fedora.sh` via `cargo generate-rpm --target x86_64-unknown-linux-gnu.2.31` → `.rpm`.
   * Portable: `tools/build-linux.sh` already produces `dist/linux/bin/*` + `dist/linux/tarball/*.tar.gz` usable on all three.
6. **Tor bundle**: same `assets/tor-linux/{tor,geoip,geoip6}` for all; no per-distro geoip. Fetch once, package thrice.

---

## 5. File Map (changes)

* `crates/platform-linux/src/common.rs` — add `detect_distro() -> Distro`, `which_binary(name) -> Option<PathBuf>` that checks `PATH` + `/usr/sbin`, helper `command_exists` extended
* `crates/platform-linux/src/client_setup.rs` — distro-aware `client_setup_checks()` hint (if `pptp` missing suggests `pptpclient` on Arch, `pptp` on Fedora, `pptp-linux` on Debian; DNS hint `openresolv` vs `systemd-resolved`)
* `crates/platform-linux/src/wireguard.rs` — DNS comment + `/usr/sbin/resolvconf` probe (no logic change, just extended path list)
* `crates/platform-linux/src/openvpn.rs` — resolve `openvpn` via `which_binary` (covers `/usr/sbin/openvpn` rare)
* `crates/platform-linux/src/pptp.rs` — resolve `pptp`/`pppd` via `which_binary`
* `apps/client/Cargo.toml` — keep `cargo-deb`; add `[package.metadata.generate-rpm]` for Fedora (kept optional, not required for deb build)
* `tools/arch/PKGBUILD` **new**, `tools/arch/build.sh` **new**, `tools/fedora/zeronode-vpn-client.spec` **new**, `tools/build-fedora.sh` **new**, `tools/build-linux-all.sh` **new** orchestrator (deb+arch+fedora+tarball)
* `dist/` — new `dist/arch/*.pkg.tar.zst`, `dist/fedora/*.rpm`, `dist/linux/tarball/*.tar.gz` (gitignored, released)

No frontend changes in this backend plan.

---

## 6. Execution Checklist (Step 2)

1. **Probe helpers** — `common.rs` add `Distro` enum + `detect_distro()` + `find_binary()` + extend `command_exists()` for `/usr/sbin`; unit test `/etc/os-release` parse
2. **Client setup hints** — `client_setup.rs` distro-aware remedy strings (test on `ID=arch`, `ID=fedora`, `ID=debian`)
3. **Binary resolvers** — `wireguard/openvpn/pptp.rs` use new `find_binary` (verify `cargo check -p vpn-platform-linux`)
4. **Packaging manifests** — write `tools/arch/PKGBUILD`, `tools/fedora/zeronode-vpn-client.spec`, add `generate-rpm` metadata, `tools/build-*.sh` wrappers
5. **Glibc pin** — switch `tools/build-linux.sh` to `zigbuild --target x86_64-unknown-linux-gnu.2.31` (and `aarch64-unknown-linux-gnu.2.31`); verify `readelf --version-info` floor
6. **Local build smoke** — VM `cargo check -p vpn-platform-linux -p vpn-client` + `tools/build-linux-all.sh` produces `deb`/`tar.gz` (arch/fedora dry-run if no `makepkg`/`rpmbuild`)
7. **Distro smoke** (containers where available) — `docker run debian:11|debian:12|archlinux:latest|fedora:41 -- dpkg -i / pacman -U / dnf install` dry, `vpn-client --help`, `vpn-client tunnel-apply` dry, `nft list ruleset` mock
8. **Tests** — `cargo test -p vpn-platform-linux` 11 prior + new distro tests pass on VM + Windows host

---

## 7. Packaging Matrix (final)

| Family | Artifact | Builder | Install | Deps |
|---|---|---|---|---|
| Debian 11/12/13 | `zeronode-vpn-client_0.2.0-1_amd64.deb` | `cargo deb --target x86_64-unknown-linux-gnu.2.31 -p vpn-client` | `sudo dpkg -i *.deb && sudo apt-get install -f` | `nftables iproute2 kmod policykit-1` + Recommends `openvpn wireguard-tools pptp-linux ppp fonts-dejavu-core xdg-desktop-portal` |
| Ubuntu/Mint | same `.deb` | same | same | same |
| Arch | `zeronode-vpn-client-0.2.0-1-x86_64.pkg.tar.zst` + `PKGBUILD` | `makepkg` (or `tar czf` fallback) | `sudo pacman -U *.pkg.tar.zst` | `depends nftables iproute2 kmod polkit openvpn wireguard-tools pptpclient ppp` |
| Fedora | `zeronode-vpn-client-0.2.0-1.x86_64.rpm` | `cargo generate-rpm` | `sudo dnf install *.rpm` | `Requires nftables iproute kmod polkit openvpn wireguard-tools pptp ppp` |
| Portable | `vpn-client-linux-amd64` `zeronode-vpn-client-0.2.0-linux-x86_64.tar.gz` | `zigbuild` | `tar xzf`/`./vpn-client` | none (static assets bundled) |

---

## 8. Verification Matrix (perfect)

* `cargo check -p vpn-client` clean on VM + Windows, `cargo test -p vpn-platform-linux` 11+ new pass
* `ldd target/x86_64-unknown-linux-gnu/release/vpn-client | grep GLIBC` shows `2.31` floor, runs on `debian:11` container
* `dpkg -I dist/debian/*.deb` `Depends: $auto, nftables...` ; `makepkg --printsrcinfo` ok ; `rpm -qip dist/fedora/*.rpm` Requires ok
* Manual: each distro VM/container — `vpn-client` launches (glow) X11+Wayland, `pkexec` prompt, WG `znclient0` up, DNS switch/restore, OpenVPN/PPTP/Outline/Tor smoke, `nft`/`iptables` table present

---

## 9. Risks & Mitigations

* `glibc 2.31` zigbuild may pull `GLIBC_2.33` via `wireguard-control` — pin `zig cc` with `-target x86_64-linux-gnu.2.31`; fallback `musl` portable binary
* `makepkg`/`rpmbuild` not on Ubuntu VM — dry-run package layout + `cargo generate-rpm --dry-run`, final `.pkg/.rpm` built in `archlinux`/`fedora` containers or GitHub Actions; portable `.deb` already covers fast path
* Fedora `firewalld` conflict — document `firewalld` stop or `nft` table precedence, add `Warn` check
