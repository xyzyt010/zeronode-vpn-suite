#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
echo "=== ZeroNode — All-Linux build (Debian .deb + Arch PKGBUILD + Fedora RPM + portable) ==="
if [[ -r /etc/os-release ]]; then . /etc/os-release; echo "Host: ${PRETTY_NAME:-unknown}"; fi

FOUND=0
for script in "$ROOT_DIR/tools/build-linux.sh" "$ROOT_DIR/tools/arch/build.sh" "$ROOT_DIR/tools/build-fedora.sh"; do
  echo ""
  echo "--- Running: $script ---"
  if [[ -x "$script" ]]; then
    bash "$script" || echo "warn: $script exited non-zero"
    FOUND=1
  elif [[ -f "$script" ]]; then
    bash "$script" || echo "warn: $script exited non-zero"
    FOUND=1
  else
    echo "missing: $script"
  fi
done

echo ""
echo "=== Summary ==="
echo "Debian (.deb):"
ls -lh "$ROOT_DIR/target/debian/"*.deb 2>/dev/null || ls -lh "$ROOT_DIR/target/x86_64-unknown-linux-gnu/debian/"*.deb 2>/dev/null || echo "  (none — need cargo-deb)"
echo ""
echo "Arch (.pkg.tar.zst):"
ls -lh "$ROOT_DIR/dist/arch/"*.pkg.tar.zst 2>/dev/null || echo "  (manual fallback .tar.zst if makepkg missing)"
ls -lh "$ROOT_DIR/dist/arch/"* 2>/dev/null || true
echo ""
echo "Fedora (.rpm):"
ls -lh "$ROOT_DIR/dist/fedora/"*.rpm 2>/dev/null || ls -lh "$ROOT_DIR/dist/fedora/"*.tar.gz 2>/dev/null || echo "  (need cargo-generate-rpm or rpmbuild)"
echo ""
echo "Portable (bin + tarball):"
ls -lh "$ROOT_DIR/dist/linux/bin/"* 2>/dev/null
ls -lh "$ROOT_DIR/dist/linux/tarball/"* 2>/dev/null || true
echo ""
echo "=== All-Linux build complete ==="
