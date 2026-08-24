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
echo "[1/4] Building all release binaries (multi-distro glibc 2.31)..."
# Multi-distro compat: pin glibc 2.31 so one ELF runs on Debian 11/12/13, Ubuntu/Mint, Arch, Fedora.
# Uses cargo-zigbuild when available (zig cc), else plain cargo build.
TARGETS=("x86_64-unknown-linux-gnu.2.31" "aarch64-unknown-linux-gnu.2.31")
BUILD_CMD="cargo"
BUILD_ARGS=(build --release -p vpn-server -p vpn-client)
if command -v cargo-zigbuild >/dev/null 2>&1 && command -v zig >/dev/null 2>&1; then
  # Translate .2.31 suffixed target to real target for zigbuild + set glibc suffix via env
  echo "  zigbuild available: building for glibc 2.31 (x86_64+aarch64)..."
  # zigbuild understands --target with glibc suffix; ensure targets added
  rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu 2>/dev/null || true
  # Try x86_64 glibc 2.31; fallback to plain if zig fails
  if cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.31 -p vpn-server -p vpn-client 2>&1 | tail -20; then
    echo "  zigbuild x86_64-unknown-linux-gnu.2.31 ok"
  else
    echo "  zigbuild failed, falling back to plain cargo build"
    cargo build --release -p vpn-server -p vpn-client
  fi
  # Also build aarch64 if host is aarch64 cross or native
  if cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.31 -p vpn-server -p vpn-client 2>&1 | tail -5 || true; then
    echo "  aarch64 glibc 2.31 build attempted"
  fi
else
  echo "  building with plain cargo (no zig) — for full Debian 11 compat install cargo-zigbuild + zig"
  cargo build --release \
    -p vpn-server \
    -p vpn-client
fi

echo ""
echo "[2/4] Staging binaries to dist/linux/..."
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR/bin"
mkdir -p "$DIST_DIR/bin/aarch64"

for binary in vpn-server vpn-client; do
  # Prefer cross-built glibc 2.31 binary, then host release
  SRC=""
  for cand in \
    "$ROOT_DIR/target/x86_64-unknown-linux-gnu/release/$binary" \
    "$ROOT_DIR/target/aarch64-unknown-linux-gnu/release/$binary" \
    "$ROOT_DIR/target/release/$binary"; do
    if [[ -f "$cand" ]]; then SRC="$cand"; break; fi
  done
  if [[ -n "$SRC" ]]; then
    cp "$SRC" "$DIST_DIR/bin/"
    # Also try to stage aarch64 cross if exists (for multi-arch tarball)
    if [[ -f "$ROOT_DIR/target/aarch64-unknown-linux-gnu/release/$binary" ]]; then
      cp "$ROOT_DIR/target/aarch64-unknown-linux-gnu/release/$binary" "$DIST_DIR/bin/aarch64/$binary"
    fi
    # Normalize name for release-assets (vpn-client-linux-amd64)
    if [[ "$binary" == "vpn-client" && -f "$DIST_DIR/bin/$binary" ]]; then
      cp "$DIST_DIR/bin/$binary" "$DIST_DIR/bin/vpn-client-linux-amd64" 2>/dev/null || true
    fi
    echo "  staged $binary from $SRC ($(du -h "$DIST_DIR/bin/$binary" | cut -f1))"
  else
    echo "  warning: $binary not found in target/{x86_64,aarch64,}/release/"
  fi
done
# Backward compat link for portable users
if [[ -f "$DIST_DIR/bin/vpn-client" && ! -f "$DIST_DIR/bin/vpn-client-linux-amd64" ]]; then
  cp "$DIST_DIR/bin/vpn-client" "$DIST_DIR/bin/vpn-client-linux-amd64"
fi

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
echo "[4/4] Building .deb packages (Debian/Ubuntu/Mint compatible)..."
if command -v cargo-deb >/dev/null 2>&1; then
  # Prefer glibc 2.31 cross-built artifacts when present (ensures Debian 11 compat)
  if [[ -f "$ROOT_DIR/target/x86_64-unknown-linux-gnu/release/vpn-client" ]]; then
    cargo deb --target x86_64-unknown-linux-gnu -p vpn-server --no-build || cargo deb -p vpn-server --no-build || true
    cargo deb --target x86_64-unknown-linux-gnu -p vpn-client --no-build || cargo deb -p vpn-client --no-build || true
  else
    cargo deb -p vpn-server --no-build
    cargo deb -p vpn-client --no-build
  fi
  echo ""
  echo "Debian packages:"
  ls -lh "$ROOT_DIR/target/debian/"*.deb 2>/dev/null || ls -lh "$ROOT_DIR/target/x86_64-unknown-linux-gnu/debian/"*.deb 2>/dev/null || true
  # Stage debs for dist
  mkdir -p "$DIST_DIR/debian"
  cp "$ROOT_DIR/target/debian/"*.deb "$DIST_DIR/debian/" 2>/dev/null || true
  cp "$ROOT_DIR/target/x86_64-unknown-linux-gnu/debian/"*.deb "$DIST_DIR/debian/" 2>/dev/null || true
else
  echo "  cargo-deb not installed; skipped .deb packaging."
  echo "  Install with: cargo install cargo-deb --locked"
  echo "  Raw binaries are available under dist/linux/bin/"
fi

echo ""
echo "[5/5] Portable tarball (all distros)..."
mkdir -p "$DIST_DIR/tarball"
if [[ -f "$DIST_DIR/bin/vpn-client" ]]; then
  tar czf "$DIST_DIR/tarball/zeronode-vpn-client-0.2.0-linux-x86_64.tar.gz" -C "$DIST_DIR/bin" vpn-client 2>/dev/null || true
  echo "  tarball: $DIST_DIR/tarball/zeronode-vpn-client-0.2.0-linux-x86_64.tar.gz"
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
