param(
    [string]$ApkPath = ".\dist\android\zeronode-vpn-client-vpnservice-release.apk"
)

$ErrorActionPreference = "Stop"

$sdkRoot = $env:ANDROID_HOME
if ($sdkRoot -and (Split-Path -Leaf $sdkRoot) -eq "platform-tools") {
    $sdkRoot = Split-Path -Parent $sdkRoot
}
if (-not $sdkRoot) {
    $sdkRoot = Join-Path $env:USERPROFILE "AppData\Local\Android\Sdk"
}

$buildTools = Get-ChildItem (Join-Path $sdkRoot "build-tools") -Directory |
    Where-Object {
        (Test-Path (Join-Path $_.FullName "aapt.exe")) -and
        (Test-Path (Join-Path $_.FullName "apksigner.bat"))
    } |
    Sort-Object Name -Descending |
    Select-Object -First 1
if (-not $buildTools) {
    throw "Android build-tools with aapt and apksigner were not found under $sdkRoot"
}

if (-not (Test-Path $ApkPath)) {
    throw "APK not found: $ApkPath"
}

& (Join-Path $buildTools.FullName "apksigner.bat") verify --verbose $ApkPath
if ($LASTEXITCODE -ne 0) {
    throw "APK signature verification failed"
}

$badging = & (Join-Path $buildTools.FullName "aapt.exe") dump badging $ApkPath
$manifest = & (Join-Path $buildTools.FullName "aapt.exe") dump xmltree $ApkPath AndroidManifest.xml

foreach ($pattern in @(
    "package: name='io.zeronode.vpn'",
    "sdkVersion:'29'",
    "targetSdkVersion:'34'",
    "launchable-activity: name='io.zeronode.vpn.MainActivity'",
    "native-code: 'arm64-v8a' 'armeabi-v7a' 'x86_64'"
)) {
    if (-not ($badging | Select-String -SimpleMatch $pattern)) {
        throw "APK badging check failed: $pattern"
    }
}

foreach ($pattern in @(
    ".ZeroNodeVpnService",
    "android.permission.BIND_VPN_SERVICE",
    "android.permission.FOREGROUND_SERVICE"
)) {
    if (-not ($manifest | Select-String -SimpleMatch $pattern)) {
        throw "APK manifest check failed: $pattern"
    }
}

$inspectDir = Join-Path (Split-Path -Parent $ApkPath) "inspect-vpnservice"
Remove-Item -LiteralPath $inspectDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $inspectDir | Out-Null
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::ExtractToDirectory((Resolve-Path $ApkPath).Path, (Resolve-Path $inspectDir).Path)

$nativeLibraries = Get-ChildItem -LiteralPath (Join-Path $inspectDir "lib") -Recurse -Filter libmain.so
if ($nativeLibraries.Count -lt 3) {
    throw "Expected libmain.so for all Android ABIs, found $($nativeLibraries.Count)"
}

foreach ($library in $nativeLibraries) {
    $bytes = [System.IO.File]::ReadAllBytes($library.FullName)
    $text = [System.Text.Encoding]::ASCII.GetString($bytes)
    if (-not $text.Contains("Java_io_zeronode_vpn_NativeBridge_nativeConnect")) {
        throw "Native connect JNI symbol missing from $($library.FullName)"
    }
    if (-not $text.Contains("Java_io_zeronode_vpn_NativeBridge_nativeDisconnect")) {
        throw "Native disconnect JNI symbol missing from $($library.FullName)"
    }
    if (-not $text.Contains("Java_io_zeronode_vpn_NativeBridge_nativeStartPacketPump")) {
        throw "Native packet-pump start JNI symbol missing from $($library.FullName)"
    }
    if (-not $text.Contains("Java_io_zeronode_vpn_NativeBridge_nativeStopPacketPump")) {
        throw "Native packet-pump stop JNI symbol missing from $($library.FullName)"
    }
    if (-not $text.Contains("Java_io_zeronode_vpn_NativeBridge_nativeDiscover")) {
        throw "Native discover JNI symbol missing from $($library.FullName)"
    }
    if (-not $text.Contains("Java_io_zeronode_vpn_NativeBridge_nativeGetStatus")) {
        throw "Native getStatus JNI symbol missing from $($library.FullName)"
    }
}

Write-Host "Android APK verification passed: signature, VpnService, launcher, SDK levels, native ABIs, and Rust JNI symbols (connect/disconnect/discover/status/packet-pump) are present."
