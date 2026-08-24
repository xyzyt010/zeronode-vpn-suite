param(
    [string]$ApkPath = ".\dist\android\zeronode-vpn-client-vpnservice-release.apk"
)

$ErrorActionPreference = "Stop"

$sdkRoot = "C:\Users\hemsh_sfya5gq\AppData\Local\Android\Sdk"
$env:ANDROID_HOME = $sdkRoot
$env:PATH = "$sdkRoot\platform-tools;$env:PATH"

if (-not (Test-Path $ApkPath)) {
    throw "APK not found at $ApkPath"
}

$devices = adb devices | Select-String "device$"
if (-not $devices) {
    throw "No Android devices are connected. Attach a device or start an emulator first."
}

if (Get-Command android -ErrorAction SilentlyContinue) {
    android run --type ACTIVITY --activity io.zeronode.vpn/.MainActivity --apks $ApkPath
    if ($LASTEXITCODE -eq 0) {
        exit 0
    }
    Write-Warning "android run failed; falling back to adb install."
}

adb install -r $ApkPath
adb shell monkey -p io.zeronode.vpn 1
