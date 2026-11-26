# ZeroNode Android — static PSS accounting (no device, code-only)

Source: measured file sizes on 2026-11-26 build + line-level allocation sites.
APK 0.4.25: dist/android/zeronode-vpn-client-vpnservice-release.apk 16,292,785 B (15.54 MB) — +12,356 B vs 0.4.24 (new Java classes LockIconView/OfflineIpDb)
APK 0.4.23: 16,280,429 B (15.53 MB) — before/after vs 0.4.22: -6,619,272 B (-28.9%, compressed); uncompressed -17,860,000 B (libTor 8,379,672 + geoip 9,481,354)
libmain.so: target/aarch64-linux-android/release/libmain.so 8,603,024 B (8.20 MB; original 24.11 MB, -66%)
tor-bundle-arm64.zip: 6,620,591 B (6.31 MB) — hosted at releases/download/tor-bundle-arm64-v1/tor-bundle-arm64.zip, contains libTor.so + geoip only
libTor.so: lib/arm64-v8a/libTor.so 8,379,672 B (8.00 MB, in bundle zip only — NOT in base APK), liblyrebird.so 17,098,544 B (16.31 MB, still bundled), geoip 9,481,354 B (9.04 MB, bundle only), conjure-client 13,012,656 B (12.41 MB)
Offline IP DB (maintained, tracked): apps/android-client/assets/ipdb/ipv4-country.csv 1,239 B (16 rows, starter) + README.txt 219 B — .gitignore un-ignored so `https://raw.githubusercontent.com/xyzyt010/zeronode-vpn-suite/main/apps/android-client/assets/ipdb/ipv4-country.csv` resolves (OfflineIpDb.java:39-40); Settings Download/Update replaces it, Delete removes it, `OfflineIpDb.java:176-205` binary-searches it with zero network/background cost

## Component breakdown (resident, not disk)

### 1. Native mapping (libmain.so)
- File 8.60 MB, compressed 3.45 MB in APK (see zip listing).
- Android pages in only referenced .text: JNI natives (discover/connect/status/progress) + boringtun + shadowsocks + tun2proxy + reqwest/rustls. Estimated resident text+rodata ~6-7 MB.
- Dead in 0.4.21: eframe/egui/winit desktop GUI — removed via `apps/client/Cargo.toml:35-39` cfg(not(android)), `apps/client/src/lib.rs:58-64` android_main stub. Previously 24.11 MB, now ~8-13 MB. Saves ~10 MB RSS pages.

### 2. Java heap — UI views
- `MainActivity.java` view tree: ~2-3 MB (LinearLayout/TextView/ImageView objects, GradientDrawable per card).
- `GlobeView.java:799-869` staticCache: 0.5x RGB_565. 1080x1920 => 1080*1920*2*0.25 = 1.04 MB; 1440x3200 => 2.30 MB. Previously full-res 4-9 MB, now quarter. No LAYER_TYPE_HARDWARE (`GlobeView.java:216-217` LAYER_TYPE_NONE) — no duplicate GPU texture (~9 MB saved on flagships).
- `AppCatalog.java:18-35` ICON_CACHE_LIMIT 24: 24 * ~80KB = ~1.9 MB cap vs previously unbounded 100+ icons ~8-15 MB.
- TextViews cache? None.

### 3. Threads / stacks
- `socks_tun.rs:91-104` new_current_thread: 1 thread, ~64KB stack resident (virtual 8MB).
- `outline.rs:122-125` worker_threads(2): 2 threads, ~128KB.
- `wireguard.rs:92-101` pump thread: 1 thread + 3*2048 buffers.
- Main thread + JVM: ~10 MB.
Total thread stack RSS: <2 MB (was ~6-10 MB with multi-thread pools).

### 4. Packet buffers
- `wireguard.rs:32` PKT_BUF 2048 *3 = 6KB, plus socket buffers.
- tun2proxy per-flow: ~18KB + stream state, 5-10 concurrent => <1 MB.

### 5. GeoIP / borders
- `GlobeView` rings: borders.bin 0.80 MB on disk, parsed float[][] ~2-4 MB heap. Freed on pauseForBackground (`GlobeView.java:761-788`).
- Arm geoip: `assets/tor/data/geoip` 9.48 MB compressed 2.66 MB. mmap pages count as RSS only when touched (Tor data dir). On Android UI uses live API (`IpGeoApi.java`), not MaxMind mmap, so maxminddb mmap (17 MB source) is NOT resident on Android path — only in desktop.
- Offline IP DB (0.4.25): `OfflineIpDb.java:88-205` — CSV ipv4-country.csv (1,239 B starter, grows as rows are added), sorted Entry[] in heap (~100 B/row: 1k rows ≈ 100 KB), binary search `lookupEntry` O(log N) with no network/background thread. Update is one-shot `downloadOrUpdate` (15 s/30 s timeouts), `deleteDatabase` frees it. Feature loss if DB absent: falls back to live APIs exactly as before.

### 6. Tor child process (separate PID, counts toward total) — tradeoff variant
- `libTor.so` 8.38 MB file, text ~6 MB resident, heap for consensus/circuits: Tor alone 60-120 MB (consensus ~70 MB gz decompressed + circuits). torrc caps in `crates/platform-android/src/tor.rs:127-170`: NumCPUs 1, MaxOnionsPending 1, MaxClientCircuitsPending 2, NumEntryGuards 2, ClientOnly 1, ORPort/DirPort/HTTPTunnelPort 0, FetchDirInfoEarly 0, DownloadExtraInfo 0, ConnectionPadding 0 => trims ~20 MB vs default, ~5 MB vs 0.4.23 caps.
- `geoip6` removed from torrc (no GeoIPv6File) => saves ~10-15 MB mmap pages. Before: geoip6 15.25 MB file; after: 0 B shipped, torrc has no GeoIPv6File line.
- `TorBundle.java:45-47 downloadTorBundleUrl()` + `49-113 downloadTorBundleIfNeeded()` (HttpURLConnection + ZipInputStream, 15s/30s timeouts) + `186-212 ensureExtracted()` lazy: data not loaded until first Tor Connect, so idle RSS excludes consensus entirely. Base APK 0.4.24 ships WITHOUT libTor.so/geoip (zip entries absent); first Tor tap downloads 6.31 MB zip once into filesDir/tor/ (marker `.tor-downloaded`). This caps *non-Tor* resident <150 MB; Tor-connected total is still app+Tor child.
- Combined app+Tor: app ~70-110 + Tor 60-120 = 130-230 MB. Floor is Tor itself; caps keep it at low end (~130-180 estimated, ~110-160 with lazy consensus). Base APK now guarantees WG/Outline <150 MB; Tor connected is the explicit tradeoff variant (opt-in, consensus floor cannot be coded away).

### 7. Idle vs background vs connected (static estimate, NOT measured — no device/adb)
- Idle foreground: 6-7 (native) + 2.3 (globe 0.5x) + 1.9 (icons) + 2 (threads) + 3 (views) + framework ~20 = 33-43 MB (report 38-48 with margin).
- Background (onStop recycle): globe 0 + rings 0 + icons maybe trimmed => ~25-30 MB.
- WG connected: + pump 1 MB + socket buffers 2 MB => 70-110 (includes VpnService + protect JNI).
- Outline: + 2 workers + shadowsocks buffers 10-20 MB => 85-125.
- Tor: Tor child dominates; trimmed variant Tor rate ~130-180, still above 150 floor.

### 8. Decision (explicit, per 2026-11-26 review)
- <50 MB idle/background and <150 MB WireGuard/Outline: **claimed via accounting only, not profiler-proven**.
- **Tor is the accepted >150 MB tradeoff variant** (its consensus floor cannot be coded away; downloadable bundle keeps it out of the base resident set until first Tor Connect). The <150 MB "working VPN" claim is therefore scoped to WireGuard/Outline; Tor-connected total exceeds it by design. No budget is claimed as measured.

## Trimmed variants recorded (quantified)
- Removed: WebView SVG (15-30 MB Chromium) `IconFactory.java:1-12` rejects SVG. **Loss:** SVG custom icons not supported; PNG/JPEG/WebP only (Settings tip updated). Saves ~20 MB peak RSS during icon pick.
- Removed: `geoip6` asset (15.25 MB) + `countries_50m.geojson` fallback 2.94 MB (`tools/build-android-vpnservice.ps1:62-79` skips, `tools/verify-android-apk.ps1:122-145` drops). **Loss:** Tor IPv6 geo lookup and full-res geojson fallback (borders.bin 0.80 MB remains primary, no border loss). Saves ~18 MB APK, ~10-15 MB Tor mmap.
- Removed: dup `lyrebird` asset (16.31 MB x2 -> x1) `tools/build-android-vpnservice.ps1:280-292`. **Loss:** none (symlink to jniLibs copy).
- Removed: `eframe/egui/winit` desktop GUI from Android lib (`apps/client/Cargo.toml`, `apps/client/src/lib.rs`). **Loss:** none on Android (Java MainActivity is the shell); desktop builds keep GUI via `cfg(not(android))`. Saves ~15.9 MB .so (24.11->8.20 MB).
- Removed (v0.4.23): `libTor.so` 8.38 MB + `geoip` 9.48 MB from base APK -> downloadable `tor-bundle-arm64.zip` 6.31 MB (`tools/build-android-vpnservice.ps1` skips `libTor.so` + `geoip` in pack loops, `TorBundle.java:49-113` fetches/unzips on first Tor Connect). **Loss:** Tor needs one-time 6.31 MB download; offline Tor connect fails until cached. Saves 6.62 MB compressed APK (21.84->15.53 MB) and keeps WG/Outline resident <150 MB.
- Globe tradeoff: 3D -> 2D fallback `GlobeView.setUse2dFallback` (`GlobeView.java:871-904`) draws static `2k_earth_nightmap.jpg` when `isLowMemory()` (`MainActivity.java:5815-5827`, `onTrimMemory` RUNNING_CRITICAL). **Loss:** no orbit/pin 3D under memory pressure; map is static. Saves 1-2 MB cache + 2-4 MB rings under pressure.

## Verification (static only, per "no black box")
- `cargo check -p vpn-client --lib --target aarch64-linux-android` EXIT=0 (2026-11-25)
- `tools/build-android-vpnservice.ps1 -Profile release` EXIT=0
- `tools/verify-android-apk.ps1` EXIT=0 (apksigner v3, versionCode 421 example)
- APK/lib sizes measured via System.IO.Compression.ZipFile.

No emulator/device used. PSS numbers above are address-space accounting, not dumpsys.
