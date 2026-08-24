#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT_DIR/dist/linux"

echo "=== ZeroNode VPN Suite — Linux Release Build ==="

if [[ -r /etc/os-release ]]; then
  # shellcheck disable=SC1091
  . /etc/os-release
  echo "Building on: ${PRETTY_NAME:-unknown Linux}"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust toolchain not found."
  echo "Run ./tools/provision-ubuntu-builder.sh first."
  exit 1
fi

cd "$ROOT_DIR"

# Fetch Tor bundle if missing (for client deb)
if [[ ! -f "$ROOT_DIR/apps/client/assets/tor-linux/tor" ]]; then
  echo "Tor bundle not found, fetching..."
  if [[ -x "$ROOT_DIR/tools/fetch-tor-linux.sh" ]]; then
    "$ROOT_DIR/tools/fetch-tor-linux.sh" || echo "warning: tor fetch failed"
  fi
fi

echo ""
echo "[1/4] Building all release binaries..."
cargo build --release \
  -p vpn-server \
  -p vpn-client

echo ""
echo "[2/4] Staging binaries to dist/linux/..."
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR/bin"

for binary in vpn-server vpn-client; do
  if [[ -f "$ROOT_DIR/target/release/$binary" ]]; then
    cp "$ROOT_DIR/target/release/$binary" "$DIST_DIR/bin/"
    echo "  staged $binary ($(du -h "$DIST_DIR/bin/$binary" | cut -f1))"
  else
    echo "  warning: $binary not found in target/release/"
  fi
done

echo ""
echo "[3/4] Staging support files..."
mkdir -p "$DIST_DIR/systemd"
mkdir -p "$DIST_DIR/applications"
mkdir -p "$DIST_DIR/debian"

cp "$ROOT_DIR/apps/server/assets/debian/zeronode-vpn-server.service" "$DIST_DIR/systemd/"
cp "$ROOT_DIR/apps/client/assets/debian/io.zeronode.vpn.desktop" "$DIST_DIR/applications/"

for script in postinst prerm postrm; do
  if [[ -f "$ROOT_DIR/apps/server/assets/debian/maintainer/$script" ]]; then
    cp "$ROOT_DIR/apps/server/assets/debian/maintainer/$script" "$DIST_DIR/debian/"
  fi
done

echo ""
echo "[4/4] Building .deb packages..."
if command -v cargo-deb >/dev/null 2>&1; then
  cargo deb -p vpn-server --no-build
  cargo deb -p vpn-client --no-build
  echo ""
  echo "Debian packages:"
  ls -lh "$ROOT_DIR/target/debian/"*.deb 2>/dev/null
else
  echo "  cargo-deb not installed; skipped .deb packaging."
  echo "  Install with: cargo install cargo-deb --locked"
  echo "  Raw binaries are available under dist/linux/bin/"
fi

echo ""
echo "=== Linux build complete ==="
echo ""
echo "Release binaries: $DIST_DIR/bin/"
echo ""
echo "Quick install (without .deb):"
echo "  sudo ./tools/install-linux.sh"
echo ""
echo "Install via .deb (recommended):"
echo "  sudo dpkg -i target/debian/*.deb"
