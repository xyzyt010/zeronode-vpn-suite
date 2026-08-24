#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "=== ZeroNode VPN Suite — Debian Package Builder ==="

if [[ -r /etc/os-release ]]; then
  # shellcheck disable=SC1091
  . /etc/os-release
  echo "Building on: ${PRETTY_NAME:-unknown Linux}"
  case "${ID:-}" in
    ubuntu|debian|linuxmint|pop)
      ;;
    *)
      echo "warning: building .deb artifacts on a non-Debian-family distribution."
      echo "warning: first-iteration packages target Ubuntu 22.04+ and Debian 12+."
      ;;
  esac
fi

if ! command -v cargo-deb >/dev/null 2>&1; then
  echo "cargo-deb is required. Install it with: cargo install cargo-deb"
  echo "Or run: ./tools/provision-ubuntu-builder.sh"
  exit 1
fi

cd "$ROOT_DIR"

echo ""
echo "[1/3] Building release binaries..."
cargo build --release -p vpn-server -p vpn-client

echo ""
echo "[2/3] Packaging vpn-server .deb..."
cargo deb -p vpn-server --no-build

echo ""
echo "[3/3] Packaging vpn-client .deb..."
cargo deb -p vpn-client --no-build

echo ""
echo "=== Build complete ==="
echo ""
echo "Debian packages:"
ls -lh "$ROOT_DIR/target/debian/"*.deb 2>/dev/null || echo "  (none found)"
echo ""
echo "Install all packages:"
echo "  sudo dpkg -i target/debian/zeronode-vpn-server_*.deb"
echo "  sudo dpkg -i target/debian/zeronode-vpn-client_*.deb"
echo ""
echo "Or run: sudo dpkg -i target/debian/*.deb"
