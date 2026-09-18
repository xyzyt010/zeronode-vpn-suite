param(
    [string]$ApkPath = ".\dist\android\zeronode-vpn-client-vpnservice-release.apk",
    # Expect the -OfflineIpDb flavor assets (assets/ipdb/*.mmdb). Default
    # verification is unchanged when the switch is absent.
    [switch]$OfflineDb
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
if ($LASTEXITCODE -ne 0) { throw "aapt badging failed" }
$manifest = & (Join-Path $buildTools.FullName "aapt.exe") dump xmltree $ApkPath AndroidManifest.xml
if ($LASTEXITCODE -ne 0) { throw "aapt manifest dump failed" }

foreach ($pattern in @(
    "package: name='io.zeronode.vpn' versionCode='5' versionName='0.3.3-android'",
    "sdkVersion:'29'",
    "targetSdkVersion:'34'"
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

if (-not ($badging -match "^native-code: 'arm64-v8a'\s*$")) {
    throw "Expected an arm64-v8a-only APK"
}

$manifestText = $manifest -join "`n"
if ($manifestText -notmatch 'android:extractNativeLibs\([^)]*\)=\(type 0x12\)0xffffffff') {
    throw "Native executable extraction must be enabled"
}
$aliasBlocks = [regex]::Matches($manifestText, '(?ms)^      E: activity-alias\b.*?(?=^      E: |\z)')
$launcherStates = [ordered]@{
    LauncherDefault = "0xffffffff"
    LauncherWeather = "0x0"
    LauncherGarden = "0x0"
}
foreach ($launcher in $launcherStates.Keys) {
    $blocks = @($aliasBlocks | Where-Object {
        $_.Value -match ('android:name\([^)]*\)="(?:io\.zeronode\.vpn)?\.' + $launcher + '"')
    })
    if ($blocks.Count -ne 1) { throw "Missing or duplicate launcher alias: $launcher" }
    foreach ($pattern in @(
        'android:targetActivity\([^)]*\)="(?:io\.zeronode\.vpn)?\.MainActivity"',
        'android:exported\([^)]*\)=\(type 0x12\)0xffffffff',
        ('android:enabled\([^)]*\)=\(type 0x12\)' + $launcherStates[$launcher] + '\b'),
        '"android.intent.action.MAIN"',
        '"android.intent.category.LAUNCHER"'
    )) {
        if ($blocks[0].Value -notmatch $pattern) {
            throw "Launcher alias check failed for ${launcher}: $pattern"
        }
    }
}

$resources = & (Join-Path $buildTools.FullName "aapt.exe") dump resources $ApkPath
if ($LASTEXITCODE -ne 0) { throw "aapt resources dump failed" }
$weatherResource = [regex]::Match(($resources -join "`n"), 'resource (0x[0-9a-fA-F]+) io\.zeronode\.vpn:drawable/ic_alias_weather_adaptive:')
if (-not $weatherResource.Success) { throw "Weather adaptive drawable is missing" }
$weatherAlias = @($aliasBlocks | Where-Object { $_.Value -match '"(?:io\.zeronode\.vpn)?\.LauncherWeather"' })[0].Value
foreach ($attribute in @("icon", "roundIcon")) {
    if ($weatherAlias -notmatch ('android:' + $attribute + '\([^)]*\)=@' + $weatherResource.Groups[1].Value + '\b')) {
        throw "LauncherWeather $attribute must reference the weather adaptive drawable"
    }
}
$zeronodeResource = [regex]::Match(($resources -join "`n"), 'resource (0x[0-9a-fA-F]+) io\.zeronode\.vpn:drawable/ic_alias_zeronode_adaptive:')
if (-not $zeronodeResource.Success) { throw "ZeroNode adaptive drawable is missing" }
$defaultAlias = @($aliasBlocks | Where-Object { $_.Value -match '"(?:io\.zeronode\.vpn)?\.LauncherDefault"' })[0].Value
foreach ($attribute in @("icon", "roundIcon")) {
    if ($defaultAlias -notmatch ('android:' + $attribute + '\([^)]*\)=@' + $zeronodeResource.Groups[1].Value + '\b')) {
        throw "LauncherDefault $attribute must reference the ZeroNode adaptive drawable"
    }
}
$zeronodeXml = & (Join-Path $buildTools.FullName "aapt.exe") dump xmltree $ApkPath res/drawable/ic_alias_zeronode_adaptive.xml
if ($LASTEXITCODE -ne 0 -or -not ($zeronodeXml -match 'E: adaptive-icon\b')) {
    throw "ZeroNode drawable is not a compiled adaptive icon"
}
$weatherXml = & (Join-Path $buildTools.FullName "aapt.exe") dump xmltree $ApkPath res/drawable/ic_alias_weather_adaptive.xml
if ($LASTEXITCODE -ne 0 -or -not ($weatherXml -match 'E: adaptive-icon\b')) {
    throw "Weather drawable is not a compiled adaptive icon"
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::OpenRead((Resolve-Path $ApkPath).Path)
try {
    $requiredApkEntries = @(
        "AndroidManifest.xml",
        "resources.arsc",
        "classes.dex",
        "lib/arm64-v8a/libmain.so",
        "lib/arm64-v8a/libTor.so",
        "lib/arm64-v8a/liblyrebird.so",
        "assets/tor/data/geoip",
        "assets/tor/data/geoip6",
        "assets/tor/data/torrc-defaults",
        "assets/tor/pluggable_transports/pt_config.json",
        "assets/globe/2k_earth_nightmap.jpg",
        "assets/globe/2k_earth_clouds.jpg",
        "assets/globe/countries_50m.geojson",
        "assets/globe/country_centroids.json",
        "res/drawable/ic_alias_weather_adaptive.xml",
        "res/drawable/ic_alias_weather_background.xml",
        "res/drawable/ic_alias_weather_foreground.xml",
        "res/drawable/ic_alias_zeronode_adaptive.xml",
        "res/drawable/ic_alias_zeronode_background.xml",
        "res/drawable/ic_alias_zeronode_foreground.xml",
        "res/drawable/ic_alias_garden.png"
    )
    # Additive DB-flavor expectations only (fetched via tools/fetch-ipdb.ps1,
    # packaged by tools/build-android-vpnservice.ps1 -OfflineIpDb).
    if ($OfflineDb) {
        $requiredApkEntries += @(
            "assets/ipdb/dbip-city-ipv4.mmdb",
            "assets/ipdb/dbip-city-ipv6.mmdb",
            "assets/ipdb/origin-asn-ipv4.mmdb",
            "assets/ipdb/origin-asn-ipv6.mmdb"
        )
    }
    foreach ($entryName in $requiredApkEntries) {
        $entries = @($archive.Entries | Where-Object { $_.FullName -ceq $entryName })
        if ($entries.Count -ne 1 -or $entries[0].Length -eq 0) {
            throw "Required APK entry missing, duplicate, or empty: $entryName"
        }
    }
} finally {
    $archive.Dispose()
}

$inspectDir = Join-Path (Split-Path -Parent $ApkPath) "inspect-vpnservice"
Remove-Item -LiteralPath $inspectDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $inspectDir | Out-Null
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::ExtractToDirectory((Resolve-Path $ApkPath).Path, (Resolve-Path $inspectDir).Path)

$nativeLibraries = @(Get-ChildItem -LiteralPath (Join-Path $inspectDir "lib") -Recurse -Filter libmain.so)
if ($nativeLibraries.Count -ne 1 -or $nativeLibraries[0].Directory.Name -ne "arm64-v8a") {
    throw "Expected exactly one arm64-v8a libmain.so"
}
foreach ($name in @("libmain.so", "libTor.so", "liblyrebird.so")) {
    $libraryPath = Join-Path $inspectDir "lib\arm64-v8a\$name"
    $header = New-Object byte[] 20
    $stream = [System.IO.File]::OpenRead($libraryPath)
    try {
        $count = $stream.Read($header, 0, $header.Length)
    } finally {
        $stream.Dispose()
    }
    if ($count -ne 20 -or $header[0] -ne 0x7f -or $header[1] -ne 0x45 -or
        $header[2] -ne 0x4c -or $header[3] -ne 0x46 -or $header[4] -ne 2 -or
        $header[5] -ne 1 -or $header[18] -ne 0xb7 -or $header[19] -ne 0) {
        throw "Expected an ELF64 AArch64 binary: $libraryPath"
    }
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

Write-Host "Android APK verification passed: signature, version, VpnService, launcher aliases, weather + ZeroNode adaptive icons, required assets, arm64 Tor/Lyrebird, and Rust JNI symbols."
