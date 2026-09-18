param(
    [ValidateSet("debug", "release")]
    [string]$Profile = "release",
    [switch]$SkipNative,
    # Comma-separated ABIs to package. Default: arm64 only (matches Tor expert bundle).
    [string]$Abis = "arm64-v8a",
    # DB flavor: package vendor/ipdb/*.mmdb as assets/ipdb/ and generate
    # BuildFlavor.java with OFFLINE_IPDB=true (default false, no behavior change).
    [switch]$OfflineIpDb
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$sdkRoot = $env:ANDROID_HOME
if ($sdkRoot -and (Split-Path -Leaf $sdkRoot) -eq "platform-tools") {
    $sdkRoot = Split-Path -Parent $sdkRoot
}
if (-not $sdkRoot) {
    $sdkRoot = Join-Path $env:USERPROFILE "AppData\Local\Android\Sdk"
}
if (-not (Test-Path $sdkRoot)) {
    throw "Android SDK root not found. Set ANDROID_HOME or install SDK under $sdkRoot"
}

$buildTools = Get-ChildItem (Join-Path $sdkRoot "build-tools") -Directory |
    Where-Object { Test-Path (Join-Path $_.FullName "aapt2.exe") } |
    Sort-Object Name -Descending |
    Select-Object -First 1
if (-not $buildTools) {
    throw "No Android build-tools with aapt2 were found under $sdkRoot"
}

$platform = Join-Path $sdkRoot "platforms\android-34\android.jar"
if (-not (Test-Path $platform)) {
    throw "Android platform android-34 not found under $sdkRoot\platforms"
}

# NDK + cargo for native libmain.so
$ndkRoot = Get-ChildItem (Join-Path $sdkRoot "ndk") -Directory | Sort-Object Name -Descending | Select-Object -First 1
if ($ndkRoot) {
    $env:ANDROID_NDK_ROOT = $ndkRoot.FullName
}
$env:ANDROID_HOME = $sdkRoot

$abiMap = [ordered]@{
    "arm64-v8a"   = "aarch64-linux-android"
    "armeabi-v7a" = "armv7-linux-androideabi"
    "x86_64"      = "x86_64-linux-android"
}
$requestedAbis = @($Abis.Split(",") | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if (-not $requestedAbis) {
    $requestedAbis = @("arm64-v8a")
}
foreach ($abi in $requestedAbis) {
    if (-not $abiMap.Contains($abi)) {
        throw "Unknown ABI '$abi'. Supported: $($abiMap.Keys -join ', ')"
    }
}

$appRoot = Join-Path $root "apps\android-client"
$assetsDir = Join-Path $appRoot "assets"
$lyrebird = Join-Path $assetsDir "tor\pluggable_transports\lyrebird"
$keystore = Join-Path $root "target\android-signing\zeronode-dev-release.keystore"
$requiredEntries = @(
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
$requiredFiles = @($keystore, (Join-Path $appRoot "AndroidManifest.xml"))
$requiredFiles += @($requiredEntries | ForEach-Object { Join-Path $appRoot $_ })
if ($requestedAbis -contains "arm64-v8a") {
    $requiredFiles += $lyrebird
    $requiredFiles += Join-Path $appRoot "jniLibs\arm64-v8a\libTor.so"
}
foreach ($requiredFile in $requiredFiles) {
    if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
        throw "Missing required build input: $requiredFile. Restore existing resources/signing key; no key will be generated."
    }
    if ((Get-Item -LiteralPath $requiredFile).Length -eq 0) {
        throw "Required build input is empty: $requiredFile"
    }
}

if (-not $SkipNative) {
    Write-Host "Building Rust native libraries for: $($requestedAbis -join ', ')"
    $sdk = $sdkRoot
    $ndkRootLocal = Get-ChildItem (Join-Path $sdk "ndk") -Directory | Sort-Object Name -Descending | Select-Object -First 1
    if (-not $ndkRootLocal) { throw "Android NDK not found under $sdk\ndk" }
    $bin = Join-Path $ndkRootLocal.FullName "toolchains\llvm\prebuilt\windows-x86_64\bin"
    $env:ANDROID_HOME = $sdk
    $env:ANDROID_NDK_ROOT = $ndkRootLocal.FullName
    $env:PATH = "$bin;$env:USERPROFILE\.cargo\bin;$env:PATH"
    Set-Item env:CC_aarch64_linux_android "$bin\aarch64-linux-android29-clang.cmd"
    Set-Item env:CXX_aarch64_linux_android "$bin\aarch64-linux-android29-clang++.cmd"
    Set-Item env:AR_aarch64_linux_android "$bin\llvm-ar.exe"
    Set-Item env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER "$bin\aarch64-linux-android29-clang.cmd"
    Set-Item env:CC_armv7_linux_androideabi "$bin\armv7a-linux-androideabi29-clang.cmd"
    Set-Item env:CXX_armv7_linux_androideabi "$bin\armv7a-linux-androideabi29-clang++.cmd"
    Set-Item env:AR_armv7_linux_androideabi "$bin\llvm-ar.exe"
    Set-Item env:CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER "$bin\armv7a-linux-androideabi29-clang.cmd"
    Set-Item env:CC_x86_64_linux_android "$bin\x86_64-linux-android29-clang.cmd"
    Set-Item env:CXX_x86_64_linux_android "$bin\x86_64-linux-android29-clang++.cmd"
    Set-Item env:AR_x86_64_linux_android "$bin\llvm-ar.exe"
    Set-Item env:CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER "$bin\x86_64-linux-android29-clang.cmd"

    foreach ($abi in $requestedAbis) {
        $triple = $abiMap[$abi]
        $cargoArgs = @("build", "-p", "vpn-client", "--lib", "--target", $triple)
        if ($Profile -eq "release") { $cargoArgs += "--release" }
        Write-Host "  cargo $($cargoArgs -join ' ')"
        Push-Location $root
        try {
            cargo @cargoArgs
            if ($LASTEXITCODE -ne 0) {
                throw "cargo build failed for $triple (exit $LASTEXITCODE)"
            }
        } finally {
            Pop-Location
        }
    }
}

$outRoot = Join-Path $root "target\android-vpnservice\$Profile"
$classesDir = Join-Path $outRoot "classes"
$dexDir = Join-Path $outRoot "dex"
$resZip = Join-Path $outRoot "compiled-res.zip"
$assetsZip = Join-Path $outRoot "assets-link"
$unsignedApk = Join-Path $outRoot "zeronode-vpnservice-unsigned.apk"
$alignedApk = Join-Path $outRoot "zeronode-vpnservice-aligned.apk"
$signedApk = Join-Path $root "dist\android\zeronode-vpn-client-vpnservice-$Profile.apk"

Remove-Item -LiteralPath $outRoot -Recurse -Force -ErrorAction SilentlyContinue
$genDir = Join-Path $outRoot "gen"
New-Item -ItemType Directory -Force -Path $classesDir, $dexDir, $genDir, (Split-Path -Parent $signedApk) | Out-Null

Write-Host "Compiling resources..."
& (Join-Path $buildTools.FullName "aapt2.exe") compile --dir (Join-Path $appRoot "res") -o $resZip
if ($LASTEXITCODE -ne 0) {
    throw "aapt2 compile failed with exit code $LASTEXITCODE"
}

# Link WITHOUT -A: Windows aapt2 writes asset zip entries with backslashes
# (assets/globe\file.jpg) which AssetManager cannot open. We inject assets
# ourselves with forward-slash paths after link.
$assetsDir = Join-Path $appRoot "assets"
$aaptLinkArgs = @(
    "link",
    "-I", $platform,
    "--manifest", (Join-Path $appRoot "AndroidManifest.xml"),
    "--min-sdk-version", "29",
    "--target-sdk-version", "34",
    "--version-code", "5",
    "--version-name", "0.3.3-android",
    "--java", $genDir,
    "-o", $unsignedApk,
    $resZip
)

Write-Host "Linking APK..."
& (Join-Path $buildTools.FullName "aapt2.exe") @aaptLinkArgs
if ($LASTEXITCODE -ne 0) {
    throw "aapt2 link failed with exit code $LASTEXITCODE"
}

# Build-flavor marker (additive; always generated so both flavors compile).
# Default OFFLINE_IPDB=false keeps behavior identical to previous builds;
# -OfflineIpDb sets true and the pack step below adds assets/ipdb/*.mmdb.
# Generated AFTER aapt2 link so link output (R.java) can never clobber it,
# and BEFORE genSources collection so javac always sees it.
$flavorValue = if ($OfflineIpDb) { "true" } else { "false" }
$flavorDir = Join-Path $genDir "io\zeronode\vpn"
New-Item -ItemType Directory -Force -Path $flavorDir | Out-Null
# NOTE: Set-Content -Encoding UTF8 writes a BOM, which javac -source 8
# rejects ("illegal character: '\ufeff'"). Write BOM-less UTF-8 instead.
$flavorLines = @(
    "package io.zeronode.vpn;",
    "",
    "/** Generated by tools/build-android-vpnservice.ps1. Do not edit. */",
    "public final class BuildFlavor {",
    "    private BuildFlavor() {}",
    "    /** True only for the -OfflineIpDb flavor (assets/ipdb/*.mmdb). */",
    "    public static final boolean OFFLINE_IPDB = $flavorValue;",
    "}"
)
[System.IO.File]::WriteAllLines(
    (Join-Path $flavorDir "BuildFlavor.java"),
    $flavorLines,
    (New-Object System.Text.UTF8Encoding $false))
Write-Host "BuildFlavor.OFFLINE_IPDB = $flavorValue"

$sources = Get-ChildItem -LiteralPath (Join-Path $appRoot "src") -Recurse -Filter *.java |
    Select-Object -ExpandProperty FullName
if (-not $sources) {
    throw "No Java sources found under $appRoot\src"
}
$genSources = Get-ChildItem -LiteralPath $genDir -Recurse -Filter *.java |
    Select-Object -ExpandProperty FullName
$allSources = @($sources) + @($genSources)

Write-Host "Compiling Java ($($allSources.Count) files)..."
# org.json is in Android SDK — use bootclasspath only.
# NOTE: newer JDKs print an "obsolete source value 8" warning on stderr.
# PowerShell 5.1 turns ANY native stderr into a terminating error when
# $ErrorActionPreference='Stop' (even with 2>$null), so relax it just for
# javac — real failures still throw via $LASTEXITCODE below.
$prevPref = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
try {
    & javac -Xlint:all,-options -source 8 -target 8 -bootclasspath $platform -classpath $platform -d $classesDir $allSources
    $javacExit = $LASTEXITCODE
} finally {
    $ErrorActionPreference = $prevPref
}
if ($javacExit -ne 0) {
    throw "javac failed with exit code $javacExit"
}

$classFiles = Get-ChildItem -LiteralPath $classesDir -Recurse -Filter *.class |
    Select-Object -ExpandProperty FullName
if (-not $classFiles) {
    throw "javac produced no class files under $classesDir"
}

Write-Host "Dexing ($($classFiles.Count) class files)..."
# Avoid Windows "input line is too long": jar the classes, then d8 the jar.
$classesJar = Join-Path $outRoot "classes.jar"
if (Test-Path $classesJar) { Remove-Item $classesJar -Force }
Push-Location $classesDir
try {
    & jar cf $classesJar *
    if ($LASTEXITCODE -ne 0) {
        # jar may not be on PATH — use Compress-Archive is zip not jar; try manual zip as jar
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        if (Test-Path $classesJar) { Remove-Item $classesJar -Force }
        $zip = [System.IO.Compression.ZipFile]::Open($classesJar, [System.IO.Compression.ZipArchiveMode]::Create)
        try {
            Get-ChildItem -Recurse -File | ForEach-Object {
                $entryName = $_.FullName.Substring((Get-Location).Path.Length + 1).Replace("\", "/")
                [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                    $zip, $_.FullName, $entryName,
                    [System.IO.Compression.CompressionLevel]::Optimal
                ) | Out-Null
            }
        } finally {
            $zip.Dispose()
        }
    }
} finally {
    Pop-Location
}
& (Join-Path $buildTools.FullName "d8.bat") --min-api 29 --output $dexDir $classesJar
if ($LASTEXITCODE -ne 0) {
    throw "d8 failed with exit code $LASTEXITCODE"
}

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Add-ZipEntry {
    param($Zip, [string]$SourcePath, [string]$EntryName)
    if (-not (Test-Path -LiteralPath $SourcePath -PathType Leaf) -or
        (Get-Item -LiteralPath $SourcePath).Length -eq 0) {
        throw "Cannot package missing or empty file: $SourcePath"
    }
    # Remove existing entry if present
    $existing = $Zip.GetEntry($EntryName)
    if ($existing) { $existing.Delete() }
    [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
        $Zip,
        $SourcePath,
        $EntryName.Replace("\", "/"),
        [System.IO.Compression.CompressionLevel]::Optimal
    ) | Out-Null
}

Write-Host "Packing dex + assets + native libs into APK..."
$zip = [System.IO.Compression.ZipFile]::Open($unsignedApk, [System.IO.Compression.ZipArchiveMode]::Update)
try {
    # Drop any broken backslash asset entries aapt2 may have left.
    $toDelete = @($zip.Entries | Where-Object {
        $_.FullName.StartsWith("assets") -and $_.FullName.Contains("\")
    })
    foreach ($e in $toDelete) {
        Write-Host "  - remove broken entry $($e.FullName)"
        $e.Delete()
    }

    Add-ZipEntry -Zip $zip -SourcePath (Join-Path $dexDir "classes.dex") -EntryName "classes.dex"

    # Assets with POSIX paths (required by Android AssetManager)
    if (Test-Path $assetsDir) {
        Get-ChildItem -LiteralPath $assetsDir -Recurse -File | ForEach-Object {
            $rel = $_.FullName.Substring($assetsDir.Length).TrimStart("\", "/")
            $entry = ("assets/" + ($rel -replace "\\", "/"))
            Add-ZipEntry -Zip $zip -SourcePath $_.FullName -EntryName $entry
            Write-Host "  + $entry"
        }
    }

    # -OfflineIpDb flavor: vendor MMDBs straight into assets/ipdb/ with
    # POSIX entry names (AssetManager opens "ipdb/<name>"). Files must be
    # fetched first via tools/fetch-ipdb.ps1; Add-ZipEntry fails loudly on
    # missing/empty inputs. Default flavor skips this block entirely.
    if ($OfflineIpDb) {
        $ipdbDir = Join-Path $root "vendor\ipdb"
        foreach ($mmdb in @("dbip-city-ipv4.mmdb", "dbip-city-ipv6.mmdb",
                "origin-asn-ipv4.mmdb", "origin-asn-ipv6.mmdb")) {
            Add-ZipEntry -Zip $zip -SourcePath (Join-Path $ipdbDir $mmdb) `
                -EntryName "assets/ipdb/$mmdb"
            Write-Host "  + assets/ipdb/$mmdb"
        }
    }

    foreach ($abi in $requestedAbis) {
        $sourceLib = Join-Path $root "target\$($abiMap[$abi])\$Profile\libmain.so"
        if (-not (Test-Path $sourceLib)) {
            throw "Missing Rust native library for $abi at $sourceLib (use -SkipNative only when already built)"
        }
        Add-ZipEntry -Zip $zip -SourcePath $sourceLib -EntryName "lib/$abi/libmain.so"
        Write-Host "  + lib/$abi/libmain.so"
    }

    # All jniLibs for requested ABIs (libmain.so already added; also Tor + OpenVPN)
    $jniRoot = Join-Path $appRoot "jniLibs"
    if (Test-Path $jniRoot) {
        foreach ($abi in $requestedAbis) {
            $abiDir = Join-Path $jniRoot $abi
            if (-not (Test-Path $abiDir)) { continue }
            Get-ChildItem $abiDir -File | ForEach-Object {
                if ($_.Name -eq "libmain.so") { return }
                $entry = "lib/$abi/$($_.Name)"
                Add-ZipEntry -Zip $zip -SourcePath $_.FullName -EntryName $entry
                Write-Host "  + $entry"
            }
        }
    }

    if ($requestedAbis -contains "arm64-v8a") {
        Add-ZipEntry -Zip $zip -SourcePath $lyrebird -EntryName "lib/arm64-v8a/liblyrebird.so"
        Write-Host "  + lib/arm64-v8a/liblyrebird.so"
    }
} finally {
    $zip.Dispose()
}

Write-Host "Zipalign..."
& (Join-Path $buildTools.FullName "zipalign.exe") -f -p 4 $unsignedApk $alignedApk
if ($LASTEXITCODE -ne 0) {
    throw "zipalign failed with exit code $LASTEXITCODE"
}

if (-not (Test-Path -LiteralPath $keystore -PathType Leaf)) {
    throw "Existing signing key is required: $keystore. No replacement key will be generated."
}
$password = "zeronode-dev"

Write-Host "Signing..."
& (Join-Path $buildTools.FullName "apksigner.bat") sign `
    --ks $keystore `
    --ks-pass "pass:$password" `
    --key-pass "pass:$password" `
    --ks-key-alias zeronode `
    --out $signedApk `
    $alignedApk
if ($LASTEXITCODE -ne 0) {
    throw "apksigner failed with exit code $LASTEXITCODE"
}

& (Join-Path $buildTools.FullName "apksigner.bat") verify --verbose $signedApk
if ($LASTEXITCODE -ne 0) {
    throw "apksigner verify failed with exit code $LASTEXITCODE"
}

# Quick content audit
Write-Host "APK content audit:"
Add-Type -AssemblyName System.IO.Compression.FileSystem
$audit = [System.IO.Compression.ZipFile]::OpenRead($signedApk)
try {
    $names = $audit.Entries | ForEach-Object { $_.FullName }
    $wantList = @("classes.dex", "AndroidManifest.xml", "resources.arsc") + $requiredEntries
    if ($OfflineIpDb) {
        $wantList += @("assets/ipdb/dbip-city-ipv4.mmdb",
            "assets/ipdb/dbip-city-ipv6.mmdb",
            "assets/ipdb/origin-asn-ipv4.mmdb",
            "assets/ipdb/origin-asn-ipv6.mmdb")
    }
    foreach ($abi in $requestedAbis) {
        $wantList += "lib/$abi/libmain.so"
    }
    if ($requestedAbis -contains "arm64-v8a") {
        $wantList += "lib/arm64-v8a/libTor.so"
        $wantList += "lib/arm64-v8a/liblyrebird.so"
    }
    foreach ($want in $wantList) {
        if ($names -notcontains $want -or $audit.GetEntry($want).Length -eq 0) {
            throw "Required APK entry missing or empty: $want"
        }
        Write-Host "  [OK] $want"
    }
} finally {
    $audit.Dispose()
}

Write-Host ""
Write-Host "Android VpnService APK staged at $signedApk"
Get-Item $signedApk | Format-List Name, Length, LastWriteTime
