#!/usr/bin/env bash
set -euo pipefail

echo "=== ZeroNode VPN Suite — Ubuntu/Debian Builder Provisioning ==="

if [[ -r /etc/os-release ]]; then
  # shellcheck disable=SC1091
  . /etc/os-release
  echo "Detected: ${PRETTY_NAME:-unknown Linux}"
fi

sudo apt-get update

ALSA_PKG="libasound2-dev"
if apt-cache show libasound2t64-dev >/dev/null 2>&1; then
  ALSA_PKG="libasound2t64-dev"
fi

sudo apt-get install -y \
  build-essential \
  pkg-config \
  curl \
  git \
  libxkbcommon-dev \
  libwayland-dev \
  libx11-dev \
  libxi-dev \
  libgl1-mesa-dev \
  libfontconfig1-dev \
  libfreetype-dev \
  "$ALSA_PKG" \
  libudev-dev \
  nftables \
  wireguard-tools \
  xvfb \
  libxcb-render0-dev \
  libxcb-shape0-dev \
  libxcb-xfixes0-dev \
  libxcb-xkb-dev \
  libxrandr-dev \
  libxcursor-dev

# Client runtime tooling (also useful on builders for smoke tests).
sudo apt-get install -y \
  policykit-1 \
  iproute2 \
  kmod

# Optional protocol backends — the deb declares these as Recommends, but
# installing them here enables end-to-end protocol verification on this host.
if [[ "${ZERO_PROVISION_PROTOCOL_DEPS:-0}" == "1" ]]; then
  sudo apt-get install -y openvpn pptp-linux ppp fonts-dejavu-core
fi

# Phase-2 tray support (not yet linked into the client build).
if [[ "${ZERO_PROVISION_TRAY_DEPS:-0}" == "1" ]]; then
  sudo apt-get install -y libgtk-3-dev libxdo-dev libayatana-appindicator3-dev
fi

if ! command -v rustup >/dev/null 2>&1; then
  echo "Installing Rust via rustup..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
fi

RUSTUP="${HOME}/.cargo/bin/rustup"
CARGO="${HOME}/.cargo/bin/cargo"
if command -v rustup >/dev/null 2>&1; then
  RUSTUP="rustup"
  CARGO="cargo"
fi

$RUSTUP default stable
$RUSTUP update stable

if ! command -v cargo-deb >/dev/null 2>&1; then
  echo "Installing cargo-deb..."
  $CARGO install cargo-deb --locked
fi

echo ""
echo "=== Provisioning complete ==="
echo "  rustc: $(rustc --version)"
echo "  cargo: $(cargo --version)"
echo "  cargo-deb: $(cargo-deb --version 2>/dev/null || echo 'installed')"
echo ""
echo "Next: run ./tools/build-linux.sh to build release binaries and .deb packages."
