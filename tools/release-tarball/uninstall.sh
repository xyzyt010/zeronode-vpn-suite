#!/usr/bin/env bash
# ZeroNode VPN Suite — Linux uninstaller. Removes only ZeroNode files.
set -euo pipefail
[ "$(id -u)" -eq 0 ] || { echo "ERROR: run as root (sudo ./uninstall.sh)" >&2; exit 1; }
echo "--> stopping helper..."
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
    systemctl disable --now zeronode-vpn-helper.service 2>/dev/null || true
    rm -f /lib/systemd/system/zeronode-vpn-helper.service
    systemctl daemon-reload
elif command -v rc-service >/dev/null 2>&1; then
    rc-service zeronode-vpn-helper stop 2>/dev/null || true
    rc-update del zeronode-vpn-helper default 2>/dev/null || true
    rm -f /etc/init.d/zeronode-vpn-helper
fi
rm -f /usr/bin/vpn-client
rm -rf /usr/share/vpn-client
rm -f /usr/share/applications/io.zeronode.vpn.desktop
echo "Uninstalled. Per-user data left in ~/.local/share/vpnsuite (delete manually to wipe profiles)."
