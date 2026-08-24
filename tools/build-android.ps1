param(
    [ValidateSet("debug", "release")]
    [string]$Profile = "debug"
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$sdkRoot = "C:\Users\hemsh_sfya5gq\AppData\Local\Android\Sdk"
$cmdlineTools = Join-Path $sdkRoot "cmdline-tools\latest\bin"
$ndkRoot = Get-ChildItem (Join-Path $sdkRoot "ndk") -Directory | Sort-Object Name -Descending | Select-Object -First 1

if (-not (Test-Path $sdkRoot)) {
    throw "Android SDK root not found at $sdkRoot"
}
if (-not $ndkRoot) {
    throw "Android NDK was not found under $sdkRoot\\ndk"
}

$env:ANDROID_HOME = $sdkRoot
$env:ANDROID_NDK_ROOT = $ndkRoot.FullName
$env:PATH = "$cmdlineTools;$sdkRoot\platform-tools;$env:PATH"

if (-not (Get-Command cargo-apk -ErrorAction SilentlyContinue)) {
    cargo install cargo-apk
}

rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android

if ($Profile -eq "release" -and -not $env:CARGO_APK_RELEASE_KEYSTORE) {
    $signingDir = Join-Path $root "target\android-signing"
    $keystore = Join-Path $signingDir "zeronode-dev-release.keystore"
    $password = "zeronode-dev"
    New-Item -ItemType Directory -Force -Path $signingDir | Out-Null

    if (-not (Test-Path $keystore)) {
        keytool `
            -genkeypair `
            -v `
            -keystore $keystore `
            -storepass $password `
            -keypass $password `
            -alias zeronode `
            -keyalg RSA `
            -keysize 2048 `
            -validity 10000 `
            -dname "CN=ZeroNode Development, OU=VPN, O=ZeroNode, L=Local, ST=Local, C=US"
    }

    $env:CARGO_APK_RELEASE_KEYSTORE = $keystore
    $env:CARGO_APK_RELEASE_KEYSTORE_PASSWORD = $password
}

$cargoArgs = @("apk", "build", "-p", "vpn-client", "--lib")
if ($Profile -eq "release") {
    $cargoArgs += "--release"
}

Push-Location $root
try {
    cargo @cargoArgs
} finally {
    Pop-Location
}
if ($LASTEXITCODE -ne 0) {
    throw "cargo apk build failed with exit code $LASTEXITCODE"
}

$profileDir = if ($Profile -eq "release") { "release" } else { "debug" }
$apkName = "zeronode-vpn-client.apk"
$sourceApk = Join-Path $root "target\$profileDir\apk\$apkName"
$distDir = Join-Path $root "dist\android"
New-Item -ItemType Directory -Force -Path $distDir | Out-Null
Copy-Item -LiteralPath $sourceApk -Destination (Join-Path $distDir "zeronode-vpn-client-$Profile.apk") -Force
Write-Host "Android APK staged in $distDir"
