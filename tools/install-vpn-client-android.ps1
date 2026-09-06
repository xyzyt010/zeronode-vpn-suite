# ZeroNode VPN Client — one-click Android installer (Windows host + adb).
#
# Downloads the stable arm64 VpnService APK from GitHub (or uses a local
# copy) and installs it to the connected device/emulator.
#
# 1. On the phone: enable Developer options > USB debugging, plug in USB.
# 2. Run: powershell -ExecutionPolicy Bypass -File .\install-vpn-client-android.ps1
# 3. On the phone: allow the VPN-connection prompt on first Connect.
#
# Offline: .\install-vpn-client-android.ps1 -OfflineApk .\ZeroNode-VPN-Client-Android-arm64.apk
param(
    [string]$ReleaseTag = "v0.3.1-client-only",
    [string]$OfflineApk = ""
)

$ErrorActionPreference = "Stop"

$Repo = "xyzyt010/zeronode-vpn-suite"
$ApkName = "ZeroNode-VPN-Client-Android-arm64.apk"

$sdkRoot = $env:ANDROID_HOME
if ($sdkRoot -and (Split-Path -Leaf $sdkRoot) -eq "platform-tools") {
    $sdkRoot = Split-Path -Parent $sdkRoot
}
if (-not $sdkRoot) { $sdkRoot = Join-Path $env:USERPROFILE "AppData\Local\Android\Sdk" }
$adb = Join-Path $sdkRoot "platform-tools\adb.exe"
if (-not (Test-Path $adb)) { throw "adb not found at $adb — install Android platform-tools first." }

# --- Get the APK. ---
if ($OfflineApk) {
    if (-not (Test-Path $OfflineApk)) { throw "Offline APK not found: $OfflineApk" }
    $apk = (Resolve-Path $OfflineApk).Path
    Write-Host "Using offline APK: $apk"
} else {
    $apk = Join-Path ([System.IO.Path]::GetTempPath()) $ApkName
    $url = "https://github.com/$Repo/releases/download/$ReleaseTag/$ApkName"
    Write-Host "Downloading $url ..."
    Invoke-WebRequest -Uri $url -OutFile $apk -UseBasicParsing
}
$size = (Get-Item $apk).Length
if ($size -lt 10MB) { throw "APK suspiciously small ($size bytes) — aborting." }
Write-Host "APK ready: $apk ($([math]::Round($size / 1MB, 1)) MB)"

# --- Device check. ---
$devices = & $adb devices | Select-String "`tdevice$"
if (-not $devices) {
    throw "No Android device found. Enable USB debugging, plug in, accept the RSA prompt, then retry."
}
Write-Host "Device: $($devices.Line)"

# --- Install + launch. ---
& $adb install -r $apk
if ($LASTEXITCODE -ne 0) { throw "adb install failed (exit $LASTEXITCODE)." }
Write-Host "Installed. Launching..."
& $adb shell monkey -p io.zeronode.vpn -c android.intent.category.LAUNCHER 1 | Out-Null
Write-Host ""
Write-Host "Done. On the phone open 'ZeroNode VPN' and allow the VPN prompt on first Connect."
Write-Host "(arm64 device required — matches the bundled Tor expert bundle.)"
