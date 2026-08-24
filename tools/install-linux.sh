#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT_DIR/dist/linux"

echo "=== ZeroNode VPN Suite — Local Install ==="

if [[ "${EUID}" -ne 0 ]]; then
  echo "This script must be run as root (sudo)."
  exit 1
fi

BINARIES=(vpn-server vpn-client)

for binary in "${BINARIES[@]}"; do
  src="$DIST_DIR/bin/$binary"
  if [[ ! -f "$src" ]]; then
    src="$ROOT_DIR/target/release/$binary"
  fi
  if [[ -f "$src" ]]; then
    install -m 0755 "$src" "/usr/bin/$binary"
    echo "  installed /usr/bin/$binary"
  else
    echo "  warning: $binary not found; run ./tools/build-linux.sh first"
  fi
done

SERVICE_SRC="$ROOT_DIR/apps/server/assets/debian/zeronode-vpn-server.service"
SERVICE_DST="/lib/systemd/system/zeronode-vpn-server.service"
if [[ -f "$SERVICE_SRC" ]]; then
  install -m 0644 "$SERVICE_SRC" "$SERVICE_DST"
  echo "  installed $SERVICE_DST"
fi

install -d -m 0750 /var/lib/zeronode-vpn-server

CLIENT_DESKTOP="$ROOT_DIR/apps/client/assets/debian/io.zeronode.vpn.desktop"
if [[ -f "$CLIENT_DESKTOP" ]]; then
  install -m 0644 "$CLIENT_DESKTOP" /usr/share/applications/io.zeronode.vpn.desktop
  echo "  installed /usr/share/applications/io.zeronode.vpn.desktop"
fi

if command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
  systemctl daemon-reload
  systemctl enable zeronode-vpn-server.service || true
  echo "  systemd: service enabled (start with: sudo systemctl start zeronode-vpn-server)"
fi

echo ""
echo "=== Install complete ==="
echo ""
echo "Start the server daemon:"
echo "  sudo systemctl start zeronode-vpn-server"
echo "  sudo systemctl status zeronode-vpn-server"
echo ""
echo "Open the client GUI:"
echo "  vpn-client"
echo ""
echo "Or use the client CLI:"
echo "  vpn-client discover"
echo "  vpn-client connect --host <server-ip>"
echo ""
echo "Server admin dashboard:"
echo "  vpn-server gui"
echo ""
echo "Uninstall:"
echo "  sudo ./tools/uninstall-linux.sh"
