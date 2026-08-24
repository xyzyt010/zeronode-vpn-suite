#!/usr/bin/env bash
# Fetch and stage the Tor Expert Bundle for Linux into apps/client/assets/tor-linux/.
#
# Mirrors the Windows flow (tor-expert-bundle-windows-x86_64-15.0.17 committed in-repo):
# the Linux payload is downloaded at build time to keep the repo lean, then flattened
# to the layout the client expects:
#
#   tor                      <- tar:tor/tor
#   geoip                    <- tar:data/geoip
#   geoip6                   <- tar:data/geoip6
#   torrc-defaults           <- tar:data/torrc-defaults
#   pluggable_transports/    <- tar:tor/pluggable_transports/
#
# Usage:  ./tools/fetch-tor-linux.sh [version]
# Env:    TOR_VERSION (overridden by argv[1])
set -euo pipefail

VERSION="${1:-${TOR_VERSION:-15.0.17}}"
ARCH="x86_64"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${ROOT}/apps/client/assets/tor-linux"
URL="https://archive.torproject.org/tor-package-archive/torbrowser/${VERSION}/tor-expert-bundle-linux-${ARCH}-${VERSION}.tar.gz"

echo "=== ZeroNode: fetching Tor Expert Bundle linux-${ARCH} ${VERSION} ==="
echo "URL: ${URL}"

command -v curl >/dev/null 2>&1 || { echo "error: curl is required" >&2; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "error: tar is required" >&2; exit 1; }

TMPDIR_TOR="$(mktemp -d)"
trap 'rm -rf "${TMPDIR_TOR}"' EXIT

ARCHIVE="${TMPDIR_TOR}/tor-expert-bundle.tar.gz"
echo "Downloading..."
curl --proto '=https' --tlsv1.2 -fL --retry 3 -o "${ARCHIVE}" "${URL}"

SHA256="$(sha256sum "${ARCHIVE}" | awk '{print $1}')"
echo "sha256(${ARCHIVE##*/}) = ${SHA256}"
echo "Record this value in docs/LINUX_CLIENT.md."

echo "Extracting..."
mkdir -p "${TMPDIR_TOR}/extracted"
tar -xzf "${ARCHIVE}" -C "${TMPDIR_TOR}/extracted"

SRC="${TMPDIR_TOR}/extracted"
[[ -f "${SRC}/tor/tor" ]] || { echo "error: unexpected archive layout (missing tor/tor)" >&2; exit 1; }

rm -rf "${DEST}"
mkdir -p "${DEST}/pluggable_transports"

install -m 0755 "${SRC}/tor/tor" "${DEST}/tor"
install -m 0644 "${SRC}/data/geoip" "${DEST}/geoip" 2>/dev/null || true
install -m 0644 "${SRC}/data/geoip6" "${DEST}/geoip6" 2>/dev/null || true
install -m 0644 "${SRC}/data/torrc-defaults" "${DEST}/torrc-defaults" 2>/dev/null || true
if [[ -d "${SRC}/tor/pluggable_transports" ]]; then
  cp -a "${SRC}/tor/pluggable_transports/." "${DEST}/pluggable_transports/"
fi

echo "Staged layout:"
find "${DEST}" -maxdepth 2 -type f -exec ls -la {} \;

echo ""
echo "=== Done: ${DEST} ==="
