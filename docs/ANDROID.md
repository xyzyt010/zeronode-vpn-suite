# ZeroNode VPN — Android (VpnService APK)

## Install

```text
dist/android/zeronode-vpn-client-vpnservice-release.apk
```

- **ABI:** `arm64-v8a` only (matches Tor expert bundle aarch64)
- **minSdk:** 29 · **targetSdk:** 34
- Signed with dev keystore `target/android-signing/zeronode-dev-release.keystore`  
  (alias `zeronode`, password `zeronode-dev`)

```powershell
adb install -r "dist\android\zeronode-vpn-client-vpnservice-release.apk"
```

Allow **VPN connection** when Android prompts. Use an arm64 device/emulator.

## What’s in the app

| Feature | Status |
|--------|--------|
| Compact UI: protocol tabs, saved-connection dropdown, + / pencil edit popups | Yes |
| Earth globe (zoom-scaled drag, max zoom 7.2, ~1.75× border stroke) | Yes |
| Reverse GeoIP (confirm exit IP, then look up that address) | Yes |
| Profile location tags + persist; WireGuard also shows IPv4/IPv6 | Yes |
| Per-app VPN (full-page picker: Browsers / Other apps; All / Some / None) | Yes |
| Progress bar auto-hides 5s after 100% / success | Yes |
| WireGuard (boringtun; Java-protected UDP + excludeRoute; pre-resolve Endpoint) | Yes |
| Outline / Shadowsocks (embedded + tun2proxy) | Yes |
| OpenVPN | Removed from Android |
| Tor (libTor.so expert bundle + SOCKS + system route) | Yes (arm64) |
| ZeroNode server discover / connect (WireGuard lease) | Yes |
| PPTP | Removed from Android |
| Real progress stages | Yes |

## Tor expert bundle

Bundled from:

```text
C:\Users\hemsh_sfya5gq\Downloads\tor-expert-bundle-android-aarch64-15.0.19
```

- `lib/arm64-v8a/libTor.so`
- `assets/tor/data/geoip`, `geoip6`, `torrc-defaults`
- `assets/tor/pluggable_transports/lyrebird`, `conjure-client`, …

On first Tor connect the app extracts PTs into `filesDir/tor/` and runs `libTor.so -f torrc`.

## Rebuild (arm64 only, reuse native lib)

```powershell
cd "Documents\New project\vpn-suite"
.\tools\build-android-vpnservice.ps1 -Profile release -SkipNative -Abis "arm64-v8a"
```

Rebuild native `libmain.so` (aarch64 only):

```powershell
$sdk = "$env:USERPROFILE\AppData\Local\Android\Sdk"
$ndk = (Get-ChildItem "$sdk\ndk" | Sort-Object Name -Descending | Select-Object -First 1).FullName
$bin = "$ndk\toolchains\llvm\prebuilt\windows-x86_64\bin"
$env:PATH = "$bin;$env:PATH"
$env:ANDROID_NDK_ROOT = $ndk
$env:CC_aarch64_linux_android = "$bin\aarch64-linux-android29-clang.cmd"
$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = "$bin\aarch64-linux-android29-clang.cmd"
cargo build -p vpn-client --lib --target aarch64-linux-android --release
.\tools\build-android-vpnservice.ps1 -Profile release -SkipNative -Abis "arm64-v8a"
```

## Architecture

```text
MainActivity (vertical Java UI + GlobeView)
    → NativeBridge (JNI) → libmain.so
    → ZeroNodeVpnService (full 0.0.0.0/0 TUN)
         → WireGuard: boringtun
         → Outline/Tor: local SOCKS + tun2proxy(--tun-fd)
         → Tor process: libTor.so expert bundle
```

Full micro-step plan: `docs/ANDROID_MICRO_PLAN.md`.
