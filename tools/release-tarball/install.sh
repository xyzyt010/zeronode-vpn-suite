#!/usr/bin/env bash
# ZeroNode VPN Suite — Linux installer (v0.3.0, x86_64 + aarch64).
# Supports: apt (Debian/Ubuntu/Mint), dnf (Fedora), pacman (Arch),
# emerge (Gentoo). Init: systemd preferred, OpenRC fallback.
# Installs: /usr/bin/vpn-client, root helper service, bundled Tor
# expert files, desktop entry. Installs only base deps itself;
# per-protocol extras are printed at the end.
set -euo pipefail

VERSION="0.3.0"
PREFIX_BIN="/usr/bin/vpn-client"
SHARE_DIR="/usr/share/vpn-client/tor-linux"
SERVICE_SRC="zeronode-vpn-helper.service"
OPENRC_SRC="zeronode-vpn-helper.openrc"
DESKTOP_SRC="io.zeronode.vpn.desktop"

die() { echo "ERROR: $*" >&2; exit 1; }
info() { echo "--> $*"; }

[ "$(id -u)" -eq 0 ] || die "run as root (sudo ./install.sh)"
[ -f ./vpn-client ] || die "run from the extracted release directory (vpn-client missing)"
case "$(uname -m)" in
    x86_64|amd64) EXPECT_ARCH="x86_64" ;;
    aarch64|arm64) EXPECT_ARCH="aarch64" ;;
    *) die "unsupported CPU: $(uname -m) (need x86_64 or aarch64)" ;;
esac
if ! file ./vpn-client 2>/dev/null | grep -qi "$EXPECT_ARCH"; then
    die "wrong tarball for this machine (need $EXPECT_ARCH, see release assets)"
fi

# --- integrity -------------------------------------------------------------
if [ -f ./SHA256SUMS ]; then
    info "verifying checksums..."
    sha256sum -c ./SHA256SUMS --status 2>/dev/null \
        || die "checksum mismatch — re-download the release"
fi

# --- dependencies ----------------------------------------------------------
PM=""
if command -v apt-get >/dev/null; then PM=apt
elif command -v dnf >/dev/null; then PM=dnf
elif command -v pacman >/dev/null; then PM=pacman
elif command -v emerge >/dev/null; then PM=emerge
fi
info "package manager: ${PM:-none detected}"
missing=""
for t in ip iptables; do command -v "$t" >/dev/null || missing="$missing $t"; done
if [ -n "$missing" ]; then
    info "installing base deps:$missing"
    case "$PM" in
        apt) apt-get update -qq && apt-get install -y -qq iproute2 iptables nftables kmod policykit-1 ;;
        dnf) dnf install -y -q iproute iptables nftables kmod polkit ;;
        pacman) pacman -Sy --noconfirm --needed iproute2 iptables nftables kmod polkit ;;
        emerge) emerge --quiet net-misc/iproute2 net-firewall/iptables sys-apps/kmod sys-auth/polkit ;;
        *) die "missing:$missing and no supported package manager — install them manually" ;;
    esac
fi

# --- files -----------------------------------------------------------------
info "installing $PREFIX_BIN"
install -m 0755 ./vpn-client "$PREFIX_BIN"
info "installing bundled Tor -> $SHARE_DIR"
mkdir -p "$SHARE_DIR"
install -m 0755 ./tor-linux/tor "$SHARE_DIR/tor"
install -m 0644 ./tor-linux/geoip "$SHARE_DIR/geoip"
install -m 0644 ./tor-linux/geoip6 "$SHARE_DIR/geoip6"
info "installing desktop entry"
install -m 0644 "./$DESKTOP_SRC" /usr/share/applications/io.zeronode.vpn.desktop
command -v update-desktop-database >/dev/null && update-desktop-database -q /usr/share/applications || true

# --- service (systemd preferred, OpenRC fallback) ---------------------------
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
    info "installing systemd helper service"
    install -m 0644 "./$SERVICE_SRC" /lib/systemd/system/zeronode-vpn-helper.service
    info "enabling + starting zeronode-vpn-helper"
    systemctl daemon-reload
    systemctl enable -q zeronode-vpn-helper.service
    systemctl restart zeronode-vpn-helper.service
    sleep 2
    systemctl is-active -q zeronode-vpn-helper.service \
        || die "helper failed to start — see: journalctl -u zeronode-vpn-helper -e"
elif command -v rc-service >/dev/null 2>&1; then
    info "installing OpenRC helper service"
    install -m 0755 "./$OPENRC_SRC" /etc/init.d/zeronode-vpn-helper
    rc-update add zeronode-vpn-helper default
    rc-service zeronode-vpn-helper restart
    sleep 2
    rc-service zeronode-vpn-helper status \
        || die "helper failed to start — see: /var/log/zeronode-vpn-helper.log"
else
    die "neither systemd nor OpenRC found — install the service manually (see $SERVICE_SRC)"
fi

echo
echo "ZeroNode VPN $VERSION installed ($(uname -m))."
echo "  Launch : vpn-client   (or ZeroNode VPN in your app menu)"
echo "  Helper : systemctl status zeronode-vpn-helper  (or rc-service zeronode-vpn-helper status)"
echo "  Config : ~/.local/share/vpnsuite/client/"
echo "  Display: one binary covers X11 and Wayland (auto-detected; ZERONODE_BACKEND=x11|wayland to force)"
echo
echo "Optional per-protocol packages (install only what you use):"
case "$PM" in
    apt) echo "  sudo apt install openvpn wireguard-tools pptp-linux ppp tor obfs4proxy" ;;
    dnf) echo "  sudo dnf install openvpn wireguard-tools pptp ppp tor obfs4" ;;
    pacman) echo "  sudo pacman -S openvpn wireguard-tools pptpclient ppp tor obfs4proxy" ;;
    emerge) echo "  sudo emerge net-vpn/openvpn net-vpn/wireguard-tools net-dialup/ppp net-vpn/tor net-proxy/obfs4proxy" ;;
esac
