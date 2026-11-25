# ZeroNode Android — static PSS accounting (no device, code-only)

Source: measured file sizes on 2026-11-25 build + line-level allocation sites.
APK 0.4.21: dist/android/zeronode-vpn-client-vpnservice-release.apk 22,899,701 B (21.84 MB)
libmain.so: target/aarch64-linux-android/release/libmain.so 8,603,176 B (8.20 MB in 0.4.20; 12.93 MB after opt-level=s rebuild — still <50% of original 24.11 MB)

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

### 6. Tor child process (separate PID, counts toward total)
- `libTor.so` 8.38 MB file, text ~6 MB resident, heap for consensus/circuits: Tor alone 60-120 MB (consensus ~70 MB gz decompressed + circuits). torrc caps in `crates/platform-android/src/tor.rs:119-170`: NumCPUs 2, MaxOnionsPending 4, NumEntryGuards 1, ClientOnly 1, ORPort 0 => trims ~10-20 MB vs default.
- `geoip6` removed from torrc (no GeoIPv6File) => saves ~10-15 MB mmap pages.
- `TorBundle.java:107-128` lazy extract: data not loaded until first Tor Connect, so idle RSS excludes consensus entirely.
- Combined app+Tor: app ~70-110 + Tor 60-120 = 130-230 MB. Floor is Tor itself; caps keep it at low end (~130-180 estimated). Cannot go under 150 without removing consensus feature or making Tor downloadable split (future).

### 7. Idle vs background vs connected (static estimate)
- Idle foreground: 6-7 (native) + 2.3 (globe 0.5x) + 1.9 (icons) + 2 (threads) + 3 (views) + framework ~20 = 33-43 MB (report 38-48 with margin).
- Background (onStop recycle): globe 0 + rings 0 + icons maybe trimmed => ~25-30 MB.
- WG connected: + pump 1 MB + socket buffers 2 MB => 70-110 (includes VpnService + protect JNI).
- Outline: + 2 workers + shadowsocks buffers 10-20 MB => 85-125.
- Tor: Tor child dominates; trimmed variant Tor rate ~130-180, still above 150 floor.

## Trimmed variants recorded
- Removed: WebView SVG (15-30 MB Chromium) `IconFactory.java:1-12` rejects SVG.
- Removed: `geoip6` asset + `countries_50m.geojson` fallback (`tools/build-android-vpnservice.ps1:62-79`, `tools/verify-android-apk.ps1:122-145`).
- Removed: dup `lyrebird` asset (16.31 MB x2 -> x1) `tools/build-android-vpnservice.ps1:280-292`.
- Removed: `eframe/egui/winit` desktop GUI from Android lib (`apps/client/Cargo.toml`, `apps/client/src/lib.rs`).

## Verification (static only, per "no black box")
- `cargo check -p vpn-client --lib --target aarch64-linux-android` EXIT=0 (2026-11-25)
- `tools/build-android-vpnservice.ps1 -Profile release` EXIT=0
- `tools/verify-android-apk.ps1` EXIT=0 (apksigner v3, versionCode 421 example)
- APK/lib sizes measured via System.IO.Compression.ZipFile.

No emulator/device used. PSS numbers above are address-space accounting, not dumpsys.
