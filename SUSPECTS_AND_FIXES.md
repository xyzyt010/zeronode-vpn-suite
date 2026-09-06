# ZeroNode VPN Suite — What we fixed and what still suspects (for Mint agent)

> **Goal:** Make Tor system-wide VPN work on Linux Mint 22.3 (and Ubuntu/Debian/Fedora/Arch) exactly like Windows: `Connect` → whole PC via Tor, `Refresh` → Tor exit, browsing via Tor, `Disconnect` → restore. No second window, no pkexec dialog (helper daemon), no "no internet" after connect.

## Environment
- **Mint host:** `hs01@192.168.1.4/24` `wlp58s0` `Lenovo YOGA 730` `DISPLAY=:0` Xorg `cinnamon` `systemd-resolved` (`/run/systemd/resolve/stub-resolv.conf` 127.0.0.53) `wmctrl` `X11` not Wayland. `libxdo3` `libayatana-appindicator3` `gtk 0.18` `tray-icon 0.24`.
- **Builder VM:** `ubuntu@129.151.239.19` `~/.cargo/bin` `zig 0.13` `cargo-zigbuild 0.23` `glibc 2.31` target `x86_64-unknown-linux-gnu.2.31` `PKG_CONFIG_ALLOW_CROSS=1`.
- **Helper:** `vpn-client --daemon` root `zeronode-vpn-helper.service` `/run/zeronode-vpn.sock` 0666 JSON `tor_start/tor_stop wg_start/wg_stop ovpn_start/ovpn_stop pptp_start/pptp_stop ss_start/ss_stop status ping`. GUI is `hs01` unprivileged, talks via helper, never relaunches.
- **Tor bundles:** Windows `apps/client/assets/tor/tor.exe` `wintun.dll` `torrc` `geoip` (15.0.17). Linux `tools/fetch-tor-linux.sh` → `apps/client/assets/tor-linux/{tor,geoip,geoip6,torrc-defaults,pluggable_transports}` → installed `/usr/share/vpn-client/tor-linux/tor` (3598168) + geoip. Linux resolver `client_setup.rs:21 resolve_tor_binary()` checks `current_exe_dir/assets/tor-linux/tor`, `/usr/share/...`, `cwd`, fallback `system tor`, with `is_tor_binary_usable` (checks `libevent-2.1.so.7` `libssl.so.3`).
- **TUN:** `tun2proxy 0.8.2` `tproxy-config 7.0.7` `vendor/tun` `tun 10.0.0.33/24 GW 10.0.0.1 DNS 10.0.0.1` `MTU 1500` `0.0.0.0/1 via ZeroNodeTor` + `128.0.0.0/1`. `leak_protect.rs` disables IPv6 per-iface (`disable_ipv6` 1) restoring on stop (never `all`/`lo`).

## What we already fixed (Hotfixes 1-6, tag v0.2.0 a07aa1f, branch main)
- **Hotfix 1:** IPv6 guard `leak_protect.rs` snapshot per-iface, not `all`.
- **Hotfix 2:** Tor 18% deadlock — `socks_tun.rs: tor_guard_bypass` wait for ESTABLISHED guards before installing TUN, fix IPv4/IPv6 parsing.
- **Hotfix 3:** Tray panic `app.rs:8707` `gtk::init()` before `Menu::new()`.
- **Hotfix 4 (e8f0859):** `tproxy_args.rs:92` `ipv4_default_route` setter bug, `socks_tun` empty bypass after 15s not bail, `/proc/net/tcp` hex parse fix, `app.rs:7642` `refresh_local_ip` via `tor_proxy_client` when `tor_system_route_active`, disconnect progress 60fps.
- **Hotfix 5 (d109704):** `socks_tun.rs:155` + `tor_tunnel.rs:217` `new_current_thread → new_multi_thread(2)` (ipstack `block_in_place` panic on first UDP `239.255.255.250:1900`), `ss -tnp` fallback, `vendor/tproxy-config` `resolvectl` DNS (`dns ZeroNodeTor 10.0.0.1 domain ~. default-route yes flush-caches` + `revert`), `app.rs:2342` remove red `Disable` button (now `ACTIVE … Disconnect to restore`), helper pre-clean `tproxy_remove`.
- **Hotfix 6 (a07aa1f, current):** **THE BIG ONE for "no internet"**
  - `socks_tun.rs:128` `ZeroNodeTor` → `ArgDns::Virtual` (not `OverTcp`). `OverTcp` tried `TCP 10.0.0.33 → 8.8.8.8:53` via Tor → most exits block 53 → `Connection refused` → DNS blackhole. `Virtual` (`198.18.0.0/15`) gives fake IP per hostname (`google.com → 198.18.0.53`) and `tun2proxy` does `SOCKS5h` with domain — Tor resolves.
  - `tor_guard_bypass_strings()` rewrote to primary `/proc/net/tcp` + `tor_socket_inodes` (`/proc/<tor>/fd` `socket:[ino]` → `st 01` → `hex_be_to_ipv4/6` → `/32`/`/128`), fallback `ss -tnp` only if 0. Empty now **fails fast** `bail!("no guard… retry")` instead of proceeding empty (which caused `0.0.0.0/1` loop → Tor loses guards → SOCKS `all GeoIP exhausted` → Refresh dead). GUI retry `tor_route_attempts <24` already schedules `ApplyTorSystemRoute` after 5s.

## Current state after Hotfix 6 install on Mint (21:45 IST, PID 3856)
- `sha256sum /usr/bin/vpn-client` `432a01b26443619b11959d047732aecbe1282d0d6cc3e80e3f61ca5900bbafee`, `strings` shows `Virtual` + `failing so GUI can retry`, deb `f1338181…` `1b892609…` `a9ca2f04…` (19M/35M/15M), helper `3834` active, `ip route` clean (`default via 192.168.1.1`), `wmctrl 0x04e00003 ZeroNode VPN Suite`, `~/.local/share/vpnsuite/client/vpn-client.log` `Local IP 103.86.19.5 IN` refreshed, `journalctl` clean (no `8.8.8.8:53` spam after fix). Window is up, ready for user to click **Tor Connect**.

## 4 suspects you listed (plus our best guesses) — what to check/fix next on Mint

### 1. Authentication pop-up required for VPN proxy if any required
- **Current:** Helper is `0666` Unix socket, `allow_active=yes` style, no polkit/pkexec dialog. GUI checks `helper::available()` first, falls back to `pkexec` only if helper not running. On Mint `systemctl status zeronode-vpn-helper` should be `active`, `ls -l /run/zeronode-vpn.sock` `srw-rw-rw-` and `python3 -c "import socket,json; s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM); s.connect('/run/zeronode-vpn.sock'); s.sendall((json.dumps({'cmd':'ping','params':{}})+'\n').encode()); print(s.recv(4096).decode())"` → `{"ok":true}`. If not, `sudo systemctl enable --now zeronode-vpn-helper` (installed via `apps/client/assets/debian/postinst` `enable --now`, `PrivateTmp=yes`).
- **Suspect:** If helper not running, GUI falls back to `platform::is_elevated()` `0` → `start_socks_system_tunnel` bails `requires root. Re-launch via pkexec.` That path shows `Enable System-Wide Routing` button and needs `pkexec` which on Mint may not have `polkit` rule. Check `journalctl -u zeronode-vpn-helper -n 50` for `cannot bind /run/zeronode-vpn.sock: Permission denied` or `vpn-client --daemon` not running. Also check `XDG_RUNTIME_DIR` `DBUS_SESSION_BUS_ADDRESS` for GUI → `nohup env DISPLAY=:0 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus XDG_RUNTIME_DIR=/run/user/1000 /usr/bin/vpn-client`.
- **Fix:** Ensure `zeronode-vpn-helper.service` `User=root` `ExecStart=/usr/bin/vpn-client --daemon` `RuntimeDirectory=zeronode-vpn` `PermissionsStartOnly`. No auth pop-up should be needed; if `systemd` fails, add `polkit` rule or `pkexec` fallback `relaunch_elevated_with_args` (but it spawns second window — we removed that for Tor).

### 2. IPv6 being forced and hence breaking Tor to connect
- **Current:** `socks_tun.rs:187` `ipv6_guard = Some(leak_protect::disable_all())` + `args.ipv6_enabled=false`. `leak_protect.rs` snapshots `/proc/sys/net/ipv6/conf/<iface>/disable_ipv6` for each concrete iface + `default`, writes `1`, restores on `stop_socks_system_tunnel`. `tproxy-config` `ipv6_default_route=false` → deletes `::/0` if exists, otherwise no `::/1`+`8000::/1`. `ip -6 route` should show no `::/1` via `ZeroNodeTor`, only `2402:a00:.../64 dev wlp58s0`.
- **Suspect:** If `disable_ipv6` wrote `1` to `all` (we fixed to not), it would break `lo` and Tor's `::1`? We fixed to not touch `all`/`lo`, but check `cat /proc/sys/net/ipv6/conf/*/disable_ipv6` before/after Connect: should be `1` for `wlp58s0`+`default` while TUN up, `0` after Disconnect. If Tor is built with IPv6 support and `torrc` has `GeoIPv6File`, but we set `ipv6_enabled=false`, Tor may try IPv6 guards and fail? The `hex_be_to_ipv6` handles `v6/128` bypass, but we bypass only if guard is IPv6; if IPv6 disabled, those guards would be unreachable anyway. Check `notice.log` `Bootstrapped 45% need more microdescriptors` — may indicate IPv6 guards needed but blocked.
- **Fix:** Test with `ipv6_enabled=true` and `ipv6_guard` not disabling, or test with `echo 0 > /proc/sys/net/ipv6/conf/wlp58s0/disable_ipv6` after Connect. Also check `torrc` `SocksPort` `NoIsolateDestAddr` etc., not forcing IPv6. The helper's `is_private_ip` filters link-local, but not ULA.

### 3. Proxy not working as system-wide proxy due to architecturally problem not allowing it to function at all whatever it is fix it
- **Current:** System-wide is `Wintun`/`TUN` `tun2proxy` `general_run_async` `0.0.0.0/1`+`128.0.0.0/1` host routes for bypass, not `WinINet` proxy. `tproxy-config` `setup=true` does `ip link set up`, `do_bypass_ip`, `ip route add`, `setup_resolv_conf`, `detect_routing_loop`. `general_run_async` does TUN→SOCKS (TCP via `ipstack` `block_in_place` needs `new_multi_thread`). `is_tor_tunnel_running()` checks `slot.thread.is_finished()`.
- **Suspect:** `tproxy-config` `detect_routing_loop` checks `proxy_cidr` (127.0.0.1) via TUN — should not be via TUN (we bypass `127.0.0.0/8`). If bypass missing for `127.0.0.1:34333`, it would detect loop and bail, leaving TUN half-setup. Check `journalctl` for `routing loop detected`. Also `ip rule` `fwmark` table 100 not used for Tor (we don't set `socket_fwmark`), but `ip route get 8.8.8.8` after Connect should show `dev ZeroNodeTor src 10.0.0.33` if not bypassed, but `ping 8.8.8.8` via TUN would be `Virtual`? With `Virtual`, `8.8.8.8` is not used for DNS, but `ping` to `8.8.8.8` would still go via TUN → SOCKS → Tor exit to `8.8.8.8` (should be allowed, but Tor may block ICMP). `tun2proxy` logs `unhandled transport - Ip Protocol 1 (ICMP)` — that's normal, `ping` won't work via Tor (Tor only TCP/UDP). So `ping 1.1.1.1` after Connect will fail, but `curl https://1.1.1.1` should succeed via `Virtual` (fake IP). Check `curl -v https://api.ipify.org --connect-timeout 5` after Connect vs `curl --socks5-hostname 127.0.0.1:34333 https://api.ipify.org`.
- **Fix:** Ensure `bypass` includes `127.0.0.0/8` `10.0.0.0/8` etc. (we do), guard bypass correct (we fixed), `ArgDns::Virtual` (we fixed), `DEFAULT_MTU 1500` correct for `wlp58s0` `1500`. Check `ip link show ZeroNodeTor` `POINTOPOINT,MULTICAST,NOARP,UP` `mtu 1500` `qdisc fq_codel`. Check `nft list ruleset` — should be empty for `tun2proxy` (unlike `tproxy`), not required. The real test is `resolvectl status` `ZeroNodeTor DNS 10.0.0.1 Domain ~.` `Current Scopes: DNS` and `getent hosts google.com` → `198.18.x.y` (Virtual) not `NXDOMAIN`. If not, `Virtual` pool `198.18.0.0/15` may conflict with `10.0.0.0/8`? No, distinct.
- **Also:** The GUI's `disconnect()` progress `0.08→0.55→0.8→1.0` was stuck because `refresh_local_ip` hung on DNS blackhole. With `Virtual` it should complete. If still stuck, add timeout to `helper::send` (90s) and fallback `platform::stop_tor_system_tunnel()` direct (even though not root, it will fail, but helper already did). Check `cargo` `is_elevated` vs helper `available()`.

### 4. Tor folder embedded used in the app is not the one for Ubuntu Linux distros for x86_64 ISA
- **Current:** `apps/client/assets/tor` (Windows) `tor.exe` 15.0.17, `apps/client/assets/tor-linux` not in Windows git (fetched via `tools/fetch-tor-linux.sh` `tor-expert-bundle-linux-x86_64-15.0.17.tar.gz` → `tor` 3598168 `geoip` 9481354 `geoip6` 15991320). On Mint after install, `ls -l /usr/share/vpn-client/tor-linux/tor` `rwxr-xr-x 3598168` exists, `ldd /usr/share/vpn-client/tor-linux/tor` → `libevent-2.1.so.7 => /lib/x86_64-linux-gnu/libevent-2.1.so.7`, `libssl.so.3`, `libcrypto.so.3`, `zlib`, etc. `tor --version` `0.4.9.11` `OpenSSL 3.0.13` (headers 3.5.7 mismatch warning but runs, `notice.log` `Bootstrapped 100%` proves it works). `client_setup.rs: resolve_tor_binary()` prefers `/usr/share/vpn-client/tor-linux/tor` then `current_exe_dir/assets/tor-linux/tor` then `system tor`.
- **Suspect:** If `apps/client/assets/tor-linux` missing on your Mint transfer (Windows folder doesn't have it), the agent on Mint must run `tools/fetch-tor-linux.sh` (needs `curl` `tar`) and rebuild (`cargo deb` or `tools/build-linux.sh`). The script downloads `https://dist.torproject.org/torbrowser/15.0.17/tor-expert-bundle-linux-x86_64-15.0.17.tar.gz` (check URL, may be `https://archive.torproject.org/...` if 404, fallback to `https://dist.torproject.org/...`). The bundle's `tor` may be linked against `libevent-2.1.so.7` which on Mint 22.3 is `libevent-2.1-7` (exists), but if missing, `is_tor_binary_usable` will fail and fallback to `system tor` (`apt install tor` `0.4.7`?). System tor may be older but still bootstrap. Check `apt-cache policy tor` `tor --version`.
- **Fix:** On Mint, run `tools/fetch-tor-linux.sh --force` (it `mkdir -p apps/client/assets/tor-linux && curl -L ... | tar -xz --strip-components=1 -C apps/client/assets/tor-linux`). Verify `sha256sum apps/client/assets/tor-linux/tor` and `file apps/client/assets/tor-linux/tor` `ELF 64-bit LSB executable, x86-64`. If `ldd` missing `libevent-2.1.so.7`, `sudo apt install libevent-2.1-7 libssl3`. The `Cargo.toml` `tproxy-config` `tun2proxy` etc. need `pkg-config` `libgtk-3-dev` `libayatana-appindicator3-dev` `libxdo-dev` `libssl-dev`. The `tools/build-linux.sh` does `cargo zigbuild --target x86_64-unknown-linux-gnu.2.31` with `PKG_CONFIG_ALLOW_CROSS`.

## How to build on Mint (from this transferred folder)
```bash
# 1. Unpack (you received vpn-suite-transfer.tar.gz)
tar -xzf vpn-suite-transfer.tar.gz -C ~/Documents
cd ~/Documents/vpn-suite
# 2. Fetch Tor Linux bundle (if missing)
tools/fetch-tor-linux.sh
ls -lh apps/client/assets/tor-linux/tor  # → 3598168
# 3. Install deps (Mint 22.3 / Ubuntu 22.04+)
sudo apt update
sudo apt install -y curl build-essential pkg-config libgtk-3-dev libayatana-appindicator3-dev libxdo-dev libssl-dev libsqlite3-dev libevent-dev
cargo --version; zig --version || cargo install cargo-zigbuild; rustup target add x86_64-unknown-linux-gnu
# 4. Build & install (glibc 2.31 for max compat, or native)
export PATH=$HOME/.cargo/bin:$PATH
PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_LIBDIR=/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/lib/pkgconfig:/usr/share/pkgconfig PKG_CONFIG_SYSROOT_DIR=/ \
  cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.31 -p vpn-client --bin vpn-client
cargo deb -p vpn-client --target x86_64-unknown-linux-gnu --no-build  # debs in target/debian/
sudo dpkg -i target/debian/zeronode-vpn-client_0.2.0-1_amd64.deb && sudo apt-get install -f -y
sudo systemctl daemon-reload; sudo systemctl restart zeronode-vpn-helper; systemctl status zeronode-vpn-helper
# Or native build for local test:
cargo build -p vpn-client --release && sudo ./target/release/vpn-client --daemon &  # or use helper
# 5. Run GUI
pkill -u hs01 -f vpn-client; DISPLAY=:0 nohup /usr/bin/vpn-client > /tmp/vpn-client.log 2>&1 &  # or cargo run -p vpn-client
DISPLAY=:0 wmctrl -l | grep ZeroNode
journalctl -u zeronode-vpn-helper -f &
tail -f ~/.local/share/vpnsuite/client/vpn-client.log &
tail -f /tmp/vpn_suite_tor_data_*/notice.log &
# Click Tor Connect in GUI, then:
ip route; resolvectl status; ss -tnp state established; cat /proc/net/tcp | head; ps aux | grep tor
python3 - << 'PY'
import socket,json
s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);s.connect("/run/zeronode-vpn.sock")
s.sendall((json.dumps({"cmd":"status","params":{}})+"\n").encode());print(s.recv(4096).decode())
PY
# Should be {"ok":true,"tor_tunnel":true} when ACTIVE, false after Disconnect
```

## Logs to watch on Mint
- `~/.local/share/vpnsuite/client/vpn-client.log` — `Tor exit resolved`, `helper tor_start failed`, `Local IP refreshed`, `close requested: hid to tray`
- `journalctl -u zeronode-vpn-helper --no-pager -n 100` — `tor bypass: /proc found N guards`, `socks tun2proxy OK`, `Virtual DNS query`, `stop_socks_system_tunnel: cancelling worker`, `ipv6 restored`, *not* `TCP 10.0.0.33 -> 8.8.8.8:53 error Connection refused` (fixed by Virtual)
- `/tmp/vpn_suite_tor_data_*/notice.log` — `Bootstrapped 0%…100% (done)`, `Parsing GEOIP`, `Opening Socks listener on 127.0.0.1:XXXX`, `Catching signal TERM` on Disconnect
- `/tmp/vpn-client.log` (nohup) and `DISPLAY=:0 wmctrl -l`

## Known good manual checks (when Tor ACTIVE)
```bash
# Via helper socket (no socat needed):
python3 - << 'PY'
import socket,json
for cmd in ["ping","status"]:
    s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);s.connect("/run/zeronode-vpn.sock")
    s.sendall((json.dumps({"cmd":cmd,"params":{}})+"\n").encode());print(cmd, s.recv(8192).decode())
PY
# Via TUN (Virtual DNS):
getent hosts google.com  # → 198.18.x.y
curl -m 10 https://api.ipify.org  # via TUN → Tor exit IP
curl --socks5-hostname 127.0.0.1:$(grep SocksPort /tmp/vpn_suite_tor_data_*/torrc | awk '{print $2}' | cut -d: -f2) https://api.ipify.org -m 10  # direct SOCKS → same Tor IP
curl https://check.torproject.org/api/ip -m 10  # via TUN should be Tor
ip route get 8.8.8.8  # dev ZeroNodeTor src 10.0.0.33
resolvectl status | grep -A2 ZeroNodeTor
# After Disconnect:
ip route  # no 0.0.0.0/1, only default via 192.168.1.1
resolvectl status | grep ZeroNodeTor  # should be gone
curl -m 5 https://api.ipify.org  # ISP IP 103.86.19.5
```

## Your 4 suspects — short answer
1. **Auth pop-up** — *Not needed* with helper 0666, but check `helper available` and `systemctl`. If helper dead, GUI needs `pkexec` which we removed for Tor.
2. **IPv6 forced** — *We do force* `disable_ipv6=1` on concrete ifaces + TUN `ipv6_enabled=false`. Could be suspect if Tor needs IPv6 guards. Test with `ipv6_enabled=true` or not disabling.
3. **Proxy system-wide architecturally broken** — *Was broken* by `OverTcp` DNS + empty guard loop. Now fixed by `Virtual` + `/proc` guard. Still check `detect_routing_loop` and `nft`/`ip rule` if helper `general_run_async` fails (missing `wintun.dll` on Linux not needed, but `/dev/net/tun` needs `modprobe tun`).
4. **Tor folder not Ubuntu x86_64** — *Bundle is* `tor-expert-bundle-linux-x86_64-15.0.17` built on glibc 2.31, runs on Mint 22.3 glibc 2.39 (log proves `Bootstrapped 100%`). If `ldd` fails, run `fetch-tor-linux.sh` or `apt install tor`.

## What to do on Mint now
- The window `0x04e00003` is up (Hotfix 6). Click **Tor Connect** → watch helper log for `Virtual` and guard count. If still `no internet`, immediately `python socket tor_stop` should restore `ip route` in <2s (we verified). If Disconnect button still dead, check `~/.local/share/vpnsuite/client/vpn-client.log` for `disconnect` progress and `journalctl` for `stop_socks...`. If GUI hidden to tray, `DISPLAY=:0 wmctrl -l` and `ps aux | grep vpn-client` to find it. The full `vpn-suite` tar is being sent to `~/Documents/vpn-suite` (or `/tmp`), build from there.

*This file was auto-generated for the Mint agent on 2026-09-01 21:50 IST from Windows builder `C:\Users\hemsh_sfya5gq\Documents\New project\vpn-suite` at commit `a07aa1f` tag `v0.2.0` Hotfix 6.*
