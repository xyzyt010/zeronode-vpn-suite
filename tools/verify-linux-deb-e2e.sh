#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVER_DATA=/tmp/zeronode-e2e-server
CLIENT_DATA=/tmp/zeronode-e2e-client
NETNS=znclienttest
SERVER_LOG=/tmp/zeronode-e2e-server.log
SERVER_PID=

echo "=== ZeroNode VPN Suite — Linux .deb E2E Verification ==="

cleanup() {
  set +e
  if [[ -n "${SERVER_PID:-}" ]]; then
    kill "$SERVER_PID" 2>/dev/null
    wait "$SERVER_PID" 2>/dev/null
  fi
  ip netns exec "$NETNS" env XDG_DATA_HOME="$CLIENT_DATA" /usr/bin/vpn-client tunnel-remove >/dev/null 2>&1
  ip netns del "$NETNS" >/dev/null 2>&1
  ip link del vethsrv >/dev/null 2>&1
  ip link del znwg0 >/dev/null 2>&1
}
trap cleanup EXIT

if [[ "${EUID}" -ne 0 ]]; then
  echo "Run as root; this test creates WireGuard interfaces and a network namespace." >&2
  exit 1
fi

dpkg -i \
  "$ROOT_DIR"/target/debian/zeronode-vpn-server_*_amd64.deb \
  "$ROOT_DIR"/target/debian/zeronode-vpn-client_*_amd64.deb >/dev/null

for binary in /usr/bin/vpn-server /usr/bin/vpn-client; do
  test -x "$binary"
done

test -f /usr/share/applications/io.zeronode.vpn.desktop

systemctl stop zeronode-vpn-server.service >/dev/null 2>&1 || true
pkill -x vpn-server >/dev/null 2>&1 || true
cleanup

rm -rf "$SERVER_DATA" "$CLIENT_DATA" "$SERVER_LOG"
mkdir -p "$SERVER_DATA" "$CLIENT_DATA"

env XDG_DATA_HOME="$SERVER_DATA" /usr/bin/vpn-server \
  --name "E2E Node" \
  --country-code IN \
  --country-name India \
  run >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 40); do
  if ss -lun | grep -q ':51820 '; then
    break
  fi
  sleep 0.25
done

if ! kill -0 "$SERVER_PID" 2>/dev/null; then
  cat "$SERVER_LOG" >&2
  echo "vpn-server exited before E2E test could run" >&2
  exit 1
fi

ip netns add "$NETNS"
ip link add vethsrv type veth peer name vethcli
ip addr add 192.0.2.1/24 dev vethsrv
ip link set vethsrv up
ip link set vethcli netns "$NETNS"
ip netns exec "$NETNS" ip addr add 192.0.2.2/24 dev vethcli
ip netns exec "$NETNS" ip link set vethcli up
ip netns exec "$NETNS" ip link set lo up

ip netns exec "$NETNS" env XDG_DATA_HOME="$CLIENT_DATA" /usr/bin/vpn-client add-host 192.0.2.1 >/dev/null
ip netns exec "$NETNS" env XDG_DATA_HOME="$CLIENT_DATA" /usr/bin/vpn-client connect --host 192.0.2.1
ip netns exec "$NETNS" env XDG_DATA_HOME="$CLIENT_DATA" /usr/bin/vpn-client tunnel-apply
ip netns exec "$NETNS" ping -c 1 -W 3 10.44.0.1
ip netns exec "$NETNS" env XDG_DATA_HOME="$CLIENT_DATA" /usr/bin/vpn-client status
ip netns exec "$NETNS" env XDG_DATA_HOME="$CLIENT_DATA" /usr/bin/vpn-client disconnect
ip netns exec "$NETNS" env XDG_DATA_HOME="$CLIENT_DATA" /usr/bin/vpn-client tunnel-remove

echo "Linux .deb E2E passed: packaged server/client CLI exchanged traffic over znwg0 <-> znclient0."
