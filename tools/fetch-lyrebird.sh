#!/usr/bin/env bash
# Build and stage the lyrebird pluggable-transport binary for Linux into
# apps/client/assets/tor-linux/ (next to the fetched Tor bundle).
#
# lyrebird (Tor Project, MPL-2.0) implements snowflake + obfs4 (+ webtunnel)
# in one static binary. The client uses it for:
#   - Snowflake bridges (built in: default bridge line, nothing to paste)
#   - obfs4 bridges (transport only; bridge lines are pasted by the user)
#
# Built from source per arch (Go toolchain required) so the binary is static
# and distro-agnostic, exactly like the client's glibc strategy.
#
# Layout after staging:
#   apps/client/assets/tor-linux/lyrebird-amd64   (x86_64, static)
#   apps/client/assets/tor-linux/lyrebird-arm64   (aarch64, static)
# The release packer installs the arch-matching one as `lyrebird`.
#
# Usage:  ./tools/fetch-lyrebird.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${ROOT}/apps/client/assets/tor-linux"
REPO_URL="https://gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/lyrebird.git"

command -v go >/dev/null 2>&1 || { echo "error: Go toolchain is required (apt install golang-go)" >&2; exit 1; }
mkdir -p "${DEST}"

TMPDIR_LB="$(mktemp -d)"
trap 'rm -rf "${TMPDIR_LB}"' EXIT

if [[ ! -d "${TMPDIR_LB}/lyrebird/.git" ]]; then
  echo "Cloning lyrebird..."
  git clone --depth 1 "${REPO_URL}" "${TMPDIR_LB}/lyrebird"
fi

echo "Building lyrebird (x86_64, static)..."
(
  cd "${TMPDIR_LB}/lyrebird"
  CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" \
    -o "${DEST}/lyrebird-amd64" ./cmd/lyrebird
)
echo "Building lyrebird (aarch64, static)..."
(
  cd "${TMPDIR_LB}/lyrebird"
  CGO_ENABLED=0 GOARCH=arm64 go build -trimpath -ldflags="-s -w" \
    -o "${DEST}/lyrebird-arm64" ./cmd/lyrebird
)

chmod +x "${DEST}/lyrebird-amd64" "${DEST}/lyrebird-arm64"
file "${DEST}/lyrebird-amd64" "${DEST}/lyrebird-arm64"
ls -lh "${DEST}/lyrebird-amd64" "${DEST}/lyrebird-arm64"
echo "lyrebird staged."
