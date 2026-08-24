#!/usr/bin/env bash
set -euo pipefail

echo "=== ZeroNode VPN Suite — Uninstall ==="

if [[ "${EUID}" -ne 0 ]]; then
  echo "This script must be run as root (sudo)."
  exit 1
fi

if command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
  systemctl stop zeronode-vpn-server.service 2>/dev/null || true
  systemctl disable zeronode-vpn-server.service 2>/dev/null || true
  echo "  stopped and disabled systemd service"
fi

ip link delete znwg0 2>/dev/null || true
ip link delete znclient0 2>/dev/null || true
echo "  removed WireGuard interfaces"

nft delete table inet zeronode 2>/dev/null || true
echo "  removed nftables rules"

for binary in vpn-server vpn-client; do
  rm -f "/usr/bin/$binary"
done
echo "  removed binaries from /usr/bin/"

rm -f /lib/systemd/system/zeronode-vpn-server.service
rm -f /usr/share/applications/io.zeronode.vpn.desktop
echo "  removed systemd unit and .desktop files"

if command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
  systemctl daemon-reload
fi

echo ""
echo "=== Uninstall complete ==="
echo ""
echo "Data directories were NOT removed:"
echo "  /var/lib/zeronode-vpn-server (server state)"
echo "  ~/.local/share/ZeroNode/     (user config and keys)"
echo ""
echo "To purge all data: sudo rm -rf /var/lib/zeronode-vpn-server && rm -rf ~/.local/share/ZeroNode"
