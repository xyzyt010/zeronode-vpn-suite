# ZeroNode VPN — Android APK Full Rebuild (Micro Plan)

Mirror of the Windows ZeroNode client: multi-protocol VPN + Tor + earth globe,
vertical mobile UI/UX, real `VpnService` tunnels, Tor expert-bundle integration.

**Tor bundle source (locked):**  
`C:\Users\hemsh_sfya5gq\Downloads\tor-expert-bundle-android-aarch64-15.0.19`

**Output APK:** `dist/android/zeronode-vpn-client-vpnservice-release.apk`

---

## Architecture (final)

```
┌──────────────────────────────────────────────────────────┐
│  MainActivity (Java) — vertical mobile shell             │
│   • Status / Your IP / progress                          │
│   • GlobeView (GL) — same textures as desktop            │
│   • Protocol tabs: OpenVPN | WireGuard | PPTP | Outline  │
│   • Tor card (separate, purple)                          │
│   • Import / paste / connect / disconnect / refresh      │
├──────────────────────────────────────────────────────────┤
│  ZeroNodeVpnService — Android system VPN                 │
│   • establish() TUN fd, full 0.0.0.0/0 + DNS             │
│   • protect(fd) for protocol sockets (anti-loop)         │
│   • foreground notification                              │
├──────────────────────────────────────────────────────────┤
│  NativeBridge (JNI)  ↔  libmain.so (Rust)                │
│   discover / connect / disconnect / status / public IP   │
│   start/stop protocol pumps on TUN fd                    │
├──────────────────────────────────────────────────────────┤
│  platform-android (Rust)                                 │
│   wireguard  → boringtun ↔ TUN                           │
│   outline    → shadowsocks SOCKS → socks_tun             │
│   tor        → libTor.so process + SOCKS → socks_tun     │
│   openvpn    → config parse + userspace path / status    │
│   pptp       → limited (Android blocks GRE often)        │
│   socks_tun  → TUN IP stack ↔ local SOCKS5               │
├──────────────────────────────────────────────────────────┤
│  Bundled natives                                         │
│   jniLibs/arm64-v8a/libTor.so                            │
│   assets/tor/{data,pluggable_transports}                 │
│   assets/globe/*                                         │
│   libmain.so (aarch64, armv7, x86_64)                    │
└──────────────────────────────────────────────────────────┘
```

---

## Phase A — Foundation (steps 1–12)

| # | Step | Done when |
|---|------|-----------|
| A1 | Create `docs/ANDROID_MICRO_PLAN.md` | this file |
| A2 | Create `apps/android-client/{assets,jniLibs,src/...}` tree | dirs exist |
| A3 | Copy Tor expert bundle → `jniLibs/arm64-v8a/libTor.so` + `assets/tor/**` | files present |
| A4 | Copy globe textures/geojson → `assets/globe/**` | files present |
| A5 | Expand `crates/platform-android` module layout | Cargo + modules compile |
| A6 | Define `TunnelKind` + `TunnelProgress` shared types | types in lib.rs |
| A7 | Manifest: INTERNET, FOREGROUND_SERVICE, VPN, POST_NOTIFICATIONS, QUERY_ALL_PACKAGES if needed | manifest valid |
| A8 | Package name stay `io.zeronode.vpn` | match desktop |
| A9 | minSdk 29, targetSdk 34 | build script |
| A10 | Dev keystore reuse `target/android-signing/zeronode-dev-release.keystore` | signs |
| A11 | Build script packages assets + jniLibs + libmain.so | APK contains them |
| A12 | Smoke: APK installs (optional device) | adb install ok |

## Phase B — WireGuard backend (13–22)

| # | Step | Done when |
|---|------|-----------|
| B1 | Parse WG `.conf` (Interface/Peer) in Rust | parser unit |
| B2 | boringtun `Tunn` on Android TUN fd (existing pump) | start/stop |
| B3 | Keepalive / update_timers | handshake lives |
| B4 | PresharedKey support | optional PSK |
| B5 | AllowedIPs → VpnService routes | full tunnel default |
| B6 | DNS from conf → Builder.addDnsServer | DNS works |
| B7 | Endpoint resolve + protect UDP socket | no loop |
| B8 | Progress stages: prepare → handshake → routes → active | UI stages |
| B9 | JNI: `nativeStartWireGuard(tunFd, confPath)` | wired |
| B10 | Disconnect cleans pump + state | clean stop |

## Phase C — Outline / Shadowsocks (23–32)

| # | Step | Done when |
|---|------|-----------|
| C1 | Reuse core `outline` key parse (ss://, port slash/?outline=1) | parse ok |
| C2 | Embedded shadowsocks local SOCKS on 127.0.0.1:port | socks listens |
| C3 | Resolve server host → IP for protect/bypass | no hostname/32 bug |
| C4 | socks_tun: TUN → local SOCKS5 | traffic flows |
| C5 | protect SS outbound sockets | no loop |
| C6 | Full default route on VpnService | public IP changes |
| C7 | Progress stages real | UI |
| C8 | Paste/import Outline key in UI | field works |
| C9 | Stop tears down SS + socks_tun fast | <2s |
| C10 | Error surfaces invalid port/key | message |

## Phase D — Tor expert bundle (33–48)

| # | Step | Done when |
|---|------|-----------|
| D1 | Ship `libTor.so` as arm64-v8a native lib | in APK |
| D2 | Ship geoip, geoip6, torrc-defaults in assets | in APK |
| D3 | Ship lyrebird + conjure-client + pt_config.json | in APK |
| D4 | First-run extract PTs to `filesDir/tor/` + chmod 700 | executable |
| D5 | Generate `torrc`: SocksPort, DataDirectory, GeoIPFile, ClientTransportPlugin | written |
| D6 | Start Tor by exec of `nativeLibraryDir/libTor.so -f torrc` | process up |
| D7 | Wait for bootstrap 100% via control or SOCKS ready | SOCKS accepts |
| D8 | Optional bridges / PT (lyrebird) path | configurable |
| D9 | socks_tun over Tor SOCKS | device traffic via Tor |
| D10 | protect Tor OR + PT sockets | no feedback loop |
| D11 | Exit GeoIP (ip-api) → country for globe pan | exit info |
| D12 | Tor UI card purple, Connect/Disconnect, bootstrap % | UI |
| D13 | System tunnel = VpnService full route (Android equivalent of Wintun route) | IP changes |
| D14 | Clean stop: cancel socks_tun, kill Tor process, delete temp | clean |
| D15 | Log to `filesDir/tor-tunnel.log` | debug |
| D16 | aarch64-only Tor note for other ABIs (disable Tor UI or show message) | graceful |

## Phase E — OpenVPN (49–56)

| # | Step | Done when |
|---|------|-----------|
| E1 | Parse .ovpn (remote, proto, cipher, auth-user-pass) | parse |
| E2 | Import file / paste config UI | UI |
| E3 | Userspace path: prefer embedded openvpn3 if linked; else clear “profile ready” + guidance | documented |
| E4 | If tunnel engine available: TUN fd handoff | connect |
| E5 | Auth dialog for user/pass | dialog |
| E6 | Progress stages | UI |
| E7 | Disconnect | stop |
| E8 | Feature parity with desktop panel fields | summary card |

## Phase F — PPTP (57–62)

| # | Step | Done when |
|---|------|-----------|
| F1 | Parse host/user/pass | parse |
| F2 | UI panel matching desktop | UI |
| F3 | Attempt userspace if feasible; else honest error: “PPTP/GRE blocked on modern Android without root” | message |
| F4 | Do not fake ACTIVE | no false green |
| F5 | Keep for config storage / future | stored |
| F6 | Document limitation in notice | strings |

## Phase G — Vertical Android UI (63–85)

| # | Step | Done when |
|---|------|-----------|
| G1 | Portrait-first vertical root ScrollView | layout |
| G2 | Header: ZeroNode + connection pill | header |
| G3 | Status banner / notice | banner |
| G4 | Globe card (square-ish, full width) | globe |
| G5 | Your IP card + Refresh (always pan globe) | IP |
| G6 | Protocol segmented control (OpenVPN/WG/PPTP/Outline) | tabs |
| G7 | Protocol body cards (import, paste, fields, connect) | bodies |
| G8 | Tor section separate below protocols | Tor |
| G9 | Nodes list (discover ZeroNode servers) | list |
| G10 | Add host + Refresh discovery | toolbar |
| G11 | Connect / Disconnect primary buttons | actions |
| G12 | Real progress bar (backend stages, not fake ramp only) | bar |
| G13 | Password dialog | dialog |
| G14 | Theme: black, #00FF7F accent, #0D0D0D cards, purple Tor | colors |
| G15 | Touch targets ≥ 44dp | a11y |
| G16 | No horizontal side panel — stack everything | vertical |
| G17 | Bottom safe area padding | layout |
| G18 | Dark only (match desktop) | theme |
| G19 | Strings in `res/values/strings.xml` | i18n-ready |
| G20 | Connection elapsed timer when active | timer |
| G21 | Mask endpoints for password-protected nodes | privacy |
| G22 | Offline/online dots | status |
| G23 | Same feature buttons as desktop (parity checklist below) | checklist |

### GUI feature parity checklist

- [ ] Discover / add host  
- [ ] Server list + connect/disconnect  
- [ ] Protocol dropdown → mobile segmented control  
- [ ] WireGuard import/paste/connect  
- [ ] OpenVPN import/paste  
- [ ] PPTP fields  
- [ ] Outline key paste  
- [ ] Tor connect + exit details  
- [ ] Your IP refresh  
- [ ] Earth globe pan/tilt to exit  
- [ ] Session / progress  
- [ ] Stop VPN service  

## Phase H — Globe (86–94)

| # | Step | Done when |
|---|------|-----------|
| H1 | Bundle earth day/night textures from desktop assets | assets |
| H2 | GLSurfaceView renderer (sphere + texture) | renders |
| H3 | Touch drag rotate | interact |
| H4 | Animate pan to lat/lon on Refresh / connect | pan token |
| H5 | Country centroid lookup (simple table or geojson) | pan target |
| H6 | Match desktop green pin / accent | style |
| H7 | Pause GL onPause, resume onResume | lifecycle |
| H8 | Fallback 2D gradient sphere if GL fails | fallback |
| H9 | Same visual family as desktop globe | look |

## Phase I — JNI / service integration (95–108)

| # | Step | Done when |
|---|------|-----------|
| I1 | Expand NativeBridge methods for all protocols | Java |
| I2 | Rust exports match Java_io_zeronode_vpn_* | symbols |
| I3 | VpnService passes protocol kind + params | intent extras |
| I4 | protectFd(int) callback Java→native optional | protect |
| I5 | Public IP fetch (HTTPS) with cache-bust | IP |
| I6 | Status poll 5s | poll |
| I7 | Discovery 30s optional | poll |
| I8 | Permission prepare VpnService | consent |
| I9 | Notification channel + ongoing | notif |
| I10 | onRevoke stops tunnels | revoke |
| I11 | Thread safety on pumps | mutex |
| I12 | No console flash (N/A Android) | n/a |
| I13 | Error strings never silent-fail ACTIVE | honest state |
| I14 | Build both debug + release | scripts |

## Phase J — Build & ship (109–120)

| # | Step | Done when |
|---|------|-----------|
| J1 | `cargo apk` / cross-compile libmain.so all ABIs | .so |
| J2 | aapt2 compile/link with assets + jniLibs | apk |
| J3 | d8 dex Java | dex |
| J4 | zipalign + apksigner | signed |
| J5 | verify APK contains libTor.so, libmain.so, assets | unzip -l |
| J6 | Stage to `dist/android/` | copy |
| J7 | `tools/verify-android-apk.ps1` update | checks |
| J8 | README Android section | docs |
| J9 | Note: Tor arm64-only from this expert bundle | docs |
| J10 | Optional install script adb | install |
| J11 | No secrets in repo beyond dev keystore password (dev only) | hygiene |
| J12 | Mark incomplete engines clearly in UI | honesty |

---

## Implementation order (this session)

1. Plan (A1) ✓  
2. Assets/Tor copy (A2–A4)  
3. platform-android modules (A5–A6, B, C, D core, socks_tun)  
4. VpnService + NativeBridge + JNI (I, B/C/D wire)  
5. MainActivity vertical UI (G) + GlobeView (H minimal)  
6. Build script (J) + build APK  

---

## Non-goals (explicit)

- Root / Magisk modules  
- Fake “ACTIVE” when tunnel not routing  
- iOS  
- Shipping non-arm64 Tor from this aarch64-only expert bundle without a matching multi-ABI bundle  

---

## Risk notes

| Risk | Mitigation |
|------|------------|
| PPTP GRE blocked | Honest unsupported message |
| OpenVPN full engine size | Config UX first; engine incremental |
| Tor only aarch64 | ABI gate in UI |
| SOCKS↔TUN complexity | Shared socks_tun module |
| VPN permission denied | Clear UI prompt, no silent fail |
| Battery / kill | Foreground service systemExempted |
