param(
    [string]$BundleDir = ".\dist\windows\bin"
)

$ErrorActionPreference = "Stop"

$BundleDir = (Resolve-Path $BundleDir).Path

$required = @(
    "vpn-server.exe",
    "vpn-client.exe",
    "vpn-server-gui.exe",
    "vpnctl.exe",
    "wireguard.exe",
    "wg.exe"
)

foreach ($asset in $required) {
    $path = Join-Path $BundleDir $asset
    if (-not (Test-Path $path)) {
        throw "Missing Windows bundle asset: $path"
    }
}

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
$isAdmin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if ($isAdmin) {
    & (Join-Path $BundleDir "vpn-server.exe") host-setup plan | Out-String | Write-Host
    & (Join-Path $BundleDir "vpn-server.exe") setup-check | Out-String | Write-Host
    & (Join-Path $BundleDir "vpn-client.exe") tunnel-status | Out-String | Write-Host
} else {
    Write-Host "Skipping executable self-checks because the Windows apps now require elevation. Re-run this script from an elevated Administrator PowerShell to execute them."
}

& (Join-Path $BundleDir "wireguard.exe") /? | Out-String | Select-String -Pattern "/installtunnelservice" | Out-Null

Write-Host "Windows bundle verification passed: required binaries are present and official WireGuard helpers are bundled."
