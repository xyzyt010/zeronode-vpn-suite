# ZeroNode VPN Client — Linux Edition Resource Sheet

Companion to `docs/LINUX_CLIENT_MICRO_PLAN.md`. Every external resource, package and
pinned version needed to build/run the Linux desktop client.

## Pinned external assets

| Item | Version | Source URL | Staged at | Consumed by |
|---|---|---|---|---|
| Tor Expert Bundle (linux x86_64) | 15.0.17 | `https://archive.torproject.org/tor-package-archive/torbrowser/15.0.17/tor-expert-bundle-linux-x86_64-15.0.17.tar.gz` | `apps/client/assets/tor-linux/` via `tools/fetch-tor-linux.sh` | Tor launcher (`app.rs` connect_to_tor) |
| GeoIP mmdb (country/ASN) | monthly lite | download.db-ip.com/free (existing core pipeline) | runtime cache dir | `core::geoip`, exit-node lookup |
| Flags + globe data | repo current | in-tree (`apps/client/assets/flags`, `assets/globe`) | deb asset stage | UI |

### tor-linux staging layout (flattened from expert bundle)

```
apps/client/assets/tor-linux/
├── tor                      # from tar: tor/tor          (chmod 755)
├── geoip                    # from tar: data/geoip
├── geoip6                   # from tar: data/geoip6
├── torrc-defaults           # from tar: data/torrc-defaults
└── pluggable_transports/    # from tar: tor/pluggable_transports/*
```

The app resolves the binary in this order (parity with Windows resolver chain):

1. `<exe_dir>/assets/tor-linux/tor`
2. `/usr/share/vpn-client/tor-linux/tor` (deb-installed payload)
3. `$PATH` (`tor`) — apt fallback
4. dev checkout fallbacks: `./apps/client/assets/tor-linux/tor`, `<exe_dir>/../../apps/client/assets/tor-linux/tor`

### Archive integrity

Record SHA-256 of each fetched archive here after first successful fetch:

```
tor-expert-bundle-linux-x86_64-15.0.17.tar.gz  sha256:<fill-after-fetch>
```

## Ubuntu packages

### Build-time (builder host) — `tools/provision-ubuntu-builder.sh`

Already covered by existing script plus client additions:
`build-essential pkg-config curl git libxkbcommon-dev libwayland-dev libx11-dev libxi-dev
libgl1-mesa-dev libfontconfig1-dev libfreetype-dev libasound2(-t64)-dev libudev-dev
libxcb-*-dev libxrandr-dev libxcursor-dev xvfb nftables wireguard-tools cargo-deb`

Client additions added this phase:
`policykit-1 iproute2 kmod` (runtime tooling present on builders too).

### Runtime (end-user) — declared in deb metadata

| Package | Needed for | Deb relation |
|---|---|---|
| `iproute2` | all routing ops (`ip link/route/address`) | Depends |
| `kmod` | `modprobe wireguard|tun|ppp_mppe` | Depends |
| `nftables` | firewall/kill-switch, server NAT | Depends (already) |
| `policykit-1` | pkexec elevation prompts | Recommends (present on stock desktops) |
| `wireguard-tools` | optional CLI diagnostics | Recommends |
| `openvpn` | OpenVPN protocol | Recommends |
| `pptp-linux`, `ppp` | PPTP protocol | Recommends |
| `fonts-dejavu-core` | crisp text rendering | Depends (always installed on desktop) |
| `libayatana-appindicator3-1`, `libgtk-3-0` | Phase-2 tray only | Suggests |

Build-time GUI dev libs (needed only when Phase-2 tray merges):
`libgtk-3-dev libxdo-dev libayatana-appindicator3-dev`.

## Desktop environments verified matrix

| DE | Session | Notes |
|---|---|---|
| XFCE 4.18 | X11 | primary target; polkit agent = lxpolkit/xfce polkit; panel supports StatusNotifier (Phase 2) |
| GNOME 42+ | Wayland | pkexec works; elevated instance forced to X11 backend (root cannot attach to Wayland compositor socket on mutter); window identity via app_id `io.zeronode.vpn` |
| GNOME | X11 | standard path |

## Known platform deviations vs Windows client

1. `centered(true)` is ignored under Wayland (winit limitation).
2. Elevated (pkexec) instances run with backend hint `ZERONODE_BACKEND=x11`; native Wayland
   rendering requires running unprivileged.
3. Files created while elevated inside `$HOME` config dirs become root-owned; a later
   non-elevated launch may hit permission errors — documented footgun (same class as
   Windows ProgramData ACL differences); mitigation tracked in plan Block K5.
4. No WinINet proxy-hint cleanup equivalent exists on Linux — nothing to clean (no-op).

## Verification commands (from repo root, Ubuntu)

```bash
./tools/provision-ubuntu-builder.sh        # once per builder VM
./tools/fetch-tor-linux.sh                 # stage Tor payload (network)
cargo test -p vpn-platform-linux --target x86_64-unknown-linux-gnu   # or native on Linux
cargo check --workspace
cargo deb -p vpn-client
```
