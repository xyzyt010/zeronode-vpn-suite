#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PKG_DIR="$ROOT_DIR/dist/arch"
echo "=== Arch PKGBUILD — zeronode-vpn-client ==="
echo "Root: $ROOT_DIR"

# Ensure binary exists (prefers glibc 2.31 zigbuild)
if [[ ! -f "$ROOT_DIR/target/x86_64-unknown-linux-gnu/release/vpn-client" && ! -f "$ROOT_DIR/target/release/vpn-client" ]]; then
  echo "No release binary found — building via cargo..."
  if command -v cargo-zigbuild >/dev/null 2>&1 && command -v zig >/dev/null 2>&1; then
    cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.31 -p vpn-client || cargo build --release -p vpn-client
  else
    cargo build --release -p vpn-client
  fi
fi

# Fetch tor if missing (optional for package)
if [[ ! -f "$ROOT_DIR/apps/client/assets/tor-linux/tor" && -x "$ROOT_DIR/tools/fetch-tor-linux.sh" ]]; then
  "$ROOT_DIR/tools/fetch-tor-linux.sh" || echo "warn: tor fetch failed"
fi

mkdir -p "$PKG_DIR"
if command -v makepkg >/dev/null 2>&1; then
  cd "$ROOT_DIR/tools/arch"
  makepkg -f --noconfirm || makepkg -s --noconfirm
  cp ./*.pkg.tar.zst "$PKG_DIR/" 2>/dev/null || true
  echo "Arch packages:"
  ls -lh "$PKG_DIR"/*.pkg.tar.zst 2>/dev/null
else
  echo "makepkg not found — staging Arch layout manually (for CI/container)..."
  # Manual layout that makepkg would produce — install tree → tar.zst
  STAGE="$ROOT_DIR/target/arch-pkg"
  rm -rf "$STAGE"; mkdir -p "$STAGE"
  BIN_SRC="$ROOT_DIR/target/x86_64-unknown-linux-gnu/release/vpn-client"
  [[ -f "$BIN_SRC" ]] || BIN_SRC="$ROOT_DIR/target/release/vpn-client"
  install -Dm755 "$BIN_SRC" "$STAGE/usr/bin/vpn-client"
  install -Dm644 "$ROOT_DIR/apps/client/assets/debian/io.zeronode.vpn.desktop" "$STAGE/usr/share/applications/io.zeronode.vpn.desktop"
  install -Dm644 "$ROOT_DIR/assets/icon.png" "$STAGE/usr/share/icons/hicolor/512x512/apps/io.zeronode.vpn.png"
  if [[ -f "$ROOT_DIR/apps/client/assets/tor-linux/tor" ]]; then
    install -Dm755 "$ROOT_DIR/apps/client/assets/tor-linux/tor" "$STAGE/usr/share/vpn-client/tor-linux/tor"
    install -Dm644 "$ROOT_DIR/apps/client/assets/tor-linux/geoip" "$STAGE/usr/share/vpn-client/tor-linux/geoip"
    install -Dm644 "$ROOT_DIR/apps/client/assets/tor-linux/geoip6" "$STAGE/usr/share/vpn-client/tor-linux/geoip6"
  fi
  mkdir -p "$STAGE/usr/share/vpn-client/flags"; cp -a "$ROOT_DIR/apps/client/assets/flags/"* "$STAGE/usr/share/vpn-client/flags/" 2>/dev/null || true
  # Create .PKGINFO and tarball (simplified, not signed)
  PKG="zeronode-vpn-client-0.2.0-1-x86_64.pkg.tar.zst"
  tar -I zstd -cf "$PKG_DIR/$PKG" -C "$STAGE" .
  echo "Staged manual Arch tarball: $PKG_DIR/$PKG ($(du -h "$PKG_DIR/$PKG" | cut -f1))"
fi
echo "=== Arch build done ==="
