#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_RPM="$ROOT_DIR/dist/fedora"
echo "=== Fedora RPM — zeronode-vpn-client ==="

if [[ ! -f "$ROOT_DIR/target/x86_64-unknown-linux-gnu/release/vpn-client" && ! -f "$ROOT_DIR/target/release/vpn-client" ]]; then
  echo "No release binary — building..."
  if command -v cargo-zigbuild >/dev/null 2>&1 && command -v zig >/dev/null 2>&1; then
    cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.31 -p vpn-client || cargo build --release -p vpn-client
  else
    cargo build --release -p vpn-client
  fi
fi
if [[ ! -f "$ROOT_DIR/apps/client/assets/tor-linux/tor" && -x "$ROOT_DIR/tools/fetch-tor-linux.sh" ]]; then
  "$ROOT_DIR/tools/fetch-tor-linux.sh" || true
fi

mkdir -p "$DIST_RPM"

# Prefer cargo-generate-rpm (adds [package.metadata.generate-rpm] requires)
if command -v cargo-generate-rpm >/dev/null 2>&1 2>&1; then
  echo "Building RPM via cargo-generate-rpm..."
  if [[ -f "$ROOT_DIR/target/x86_64-unknown-linux-gnu/release/vpn-client" ]]; then
    cargo generate-rpm --target x86_64-unknown-linux-gnu -p vpn-client 2>&1 | tail -20 || true
    cp "$ROOT_DIR/target/x86_64-unknown-linux-gnu/generate-rpm"/*.rpm "$DIST_RPM/" 2>/dev/null || true
  fi
  cargo generate-rpm -p vpn-client 2>&1 | tail -20 || true
  cp "$ROOT_DIR/target/generate-rpm"/*.rpm "$DIST_RPM/" 2>/dev/null || true
fi

# Fallback: rpmbuild from spec
if ! ls "$DIST_RPM"/*.rpm >/dev/null 2>&1; then
  if command -v rpmbuild >/dev/null 2>&1; then
    echo "Building RPM via rpmbuild..."
    TMP="$(mktemp -d)"
    mkdir -p "$TMP"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
    tar czf "$TMP/SOURCES/zeronode-vpn-client-0.2.0.tar.gz" --exclude='.git' --exclude='target' --exclude='dist' -C "$ROOT_DIR/.." "vpn-suite" 2>/dev/null || tar czf "$TMP/SOURCES/zeronode-vpn-client-0.2.0.tar.gz" -C "$ROOT_DIR" .
    cp "$ROOT_DIR/tools/fedora/zeronode-vpn-client.spec" "$TMP/SPECS/"
    rpmbuild --define "_topdir $TMP" -bb "$TMP/SPECS/zeronode-vpn-client.spec" 2>&1 | tail -30 || true
    cp "$TMP/RPMS"/**/*.rpm "$DIST_RPM/" 2>/dev/null || cp "$TMP/RPMS"/*.rpm "$DIST_RPM/" 2>/dev/null || true
    rm -rf "$TMP"
  fi
fi

# Last fallback: manual rpm-like staging tarball (for CI without rpmbuild) — still upload as .rpm placeholder? Use tar.gz
if ! ls "$DIST_RPM"/*.rpm >/dev/null 2>&1; then
  echo "rpmbuild/cargo-generate-rpm not available — staging manual Fedora layout tarball..."
  STAGE="$ROOT_DIR/target/fedora-pkg"
  rm -rf "$STAGE"; mkdir -p "$STAGE"
  BIN_SRC="$ROOT_DIR/target/x86_64-unknown-linux-gnu/release/vpn-client"; [[ -f "$BIN_SRC" ]] || BIN_SRC="$ROOT_DIR/target/release/vpn-client"
  install -Dm755 "$BIN_SRC" "$STAGE/usr/bin/vpn-client"
  install -Dm644 "$ROOT_DIR/apps/client/assets/debian/io.zeronode.vpn.desktop" "$STAGE/usr/share/applications/io.zeronode.vpn.desktop"
  install -Dm644 "$ROOT_DIR/assets/icon.png" "$STAGE/usr/share/icons/hicolor/512x512/apps/io.zeronode.vpn.png"
  if [[ -f "$ROOT_DIR/apps/client/assets/tor-linux/tor" ]]; then
    install -Dm755 "$ROOT_DIR/apps/client/assets/tor-linux/tor" "$STAGE/usr/share/vpn-client/tor-linux/tor"
    install -Dm644 "$ROOT_DIR/apps/client/assets/tor-linux/geoip" "$STAGE/usr/share/vpn-client/tor-linux/geoip"
    install -Dm644 "$ROOT_DIR/apps/client/assets/tor-linux/geoip6" "$STAGE/usr/share/vpn-client/tor-linux/geoip6"
  fi
  mkdir -p "$STAGE/usr/share/vpn-client/flags"; cp -a "$ROOT_DIR/apps/client/assets/flags/"* "$STAGE/usr/share/vpn-client/flags/" 2>/dev/null || true
  # Produce a tarball and fake .rpm path for release script (will be renamed to .tar.gz if needed)
  tar czf "$DIST_RPM/zeronode-vpn-client-0.2.0-1.x86_64.fedora.tar.gz" -C "$STAGE" . 2>/dev/null || true
  echo "Staged Fedora fallback tarball (install: tar xzf -C / && dnf install deps manually)"
fi

if ls "$DIST_RPM"/*.rpm >/dev/null 2>&1; then
  echo "Fedora RPMs:"; ls -lh "$DIST_RPM"/*.rpm
elif ls "$DIST_RPM"/*.tar.gz >/dev/null 2>&1; then
  echo "Fedora fallback tarballs:"; ls -lh "$DIST_RPM"/*.tar.gz
else
  echo "No Fedora artifact produced."
fi
echo "=== Fedora build done ==="
