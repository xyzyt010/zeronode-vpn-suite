param(
    [ValidateSet("debug", "release")]
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$targetFlag = if ($Profile -eq "release") { "--release" } else { "" }
$binaries = @("vpn-server", "vpn-client", "vpn-server-gui", "vpnctl")
$distRoot = Join-Path $root "dist\windows"
$binDir = Join-Path $distRoot "bin"
$targetDir = Join-Path $root "target\windows"
$wireGuardCache = Join-Path $root "target\wireguard-windows"
$wireGuardMsi = Join-Path $wireGuardCache "wireguard-amd64-1.1.msi"
$wireGuardExtract = Join-Path $wireGuardCache "msi-extract"
$wireGuardAssets = @("wireguard.exe", "wg.exe")

New-Item -ItemType Directory -Force -Path $binDir | Out-Null
Remove-Item -Path (Join-Path $binDir "*") -Force -ErrorAction SilentlyContinue

Push-Location $root
try {
    if ([string]::IsNullOrWhiteSpace($targetFlag)) {
        cargo build --target-dir $targetDir -p vpn-server -p vpn-client -p vpn-server-gui -p vpnctl
    } else {
        cargo build --target-dir $targetDir -p vpn-server -p vpn-client -p vpn-server-gui -p vpnctl --release
    }
} finally {
    Pop-Location
}
if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed with exit code $LASTEXITCODE"
}

$profileDir = if ($Profile -eq "release") { "release" } else { "debug" }
foreach ($binary in $binaries) {
    $source = Join-Path $targetDir "$profileDir\$binary.exe"
    $destination = Join-Path $binDir "$binary.exe"
    Copy-Item -LiteralPath $source -Destination $destination -Force
}

$wireGuardInstallDirs = @()
if ($env:ProgramFiles) {
    $wireGuardInstallDirs += Join-Path $env:ProgramFiles "WireGuard"
}
$programFilesX86 = ${env:ProgramFiles(x86)}
if ($programFilesX86) {
    $wireGuardInstallDirs += Join-Path $programFilesX86 "WireGuard"
}

$stagedWireGuardAssets = $false
foreach ($installDir in $wireGuardInstallDirs) {
    $missing = $wireGuardAssets | Where-Object { -not (Test-Path (Join-Path $installDir $_)) }
    if ($missing.Count -eq 0) {
        foreach ($asset in $wireGuardAssets) {
            Copy-Item -LiteralPath (Join-Path $installDir $asset) -Destination (Join-Path $binDir $asset) -Force
        }
        $stagedWireGuardAssets = $true
        break
    }
}

if (-not $stagedWireGuardAssets) {
    New-Item -ItemType Directory -Force -Path $wireGuardCache | Out-Null
    if (-not (Test-Path $wireGuardMsi)) {
        Invoke-WebRequest `
            -Uri "https://download.wireguard.com/windows-client/wireguard-amd64-1.1.msi" `
            -OutFile $wireGuardMsi
    }

    Remove-Item -LiteralPath $wireGuardExtract -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $wireGuardExtract | Out-Null
    $msiArgs = "/a `"$((Resolve-Path $wireGuardMsi).Path)`" TARGETDIR=`"$((Resolve-Path $wireGuardExtract).Path)`" /qn"
    $msi = Start-Process msiexec.exe -ArgumentList $msiArgs -Wait -PassThru
    if ($msi.ExitCode -ne 0) {
        throw "msiexec extraction failed with exit code $($msi.ExitCode)"
    }

    foreach ($asset in $wireGuardAssets) {
        $source = Join-Path $wireGuardExtract "WireGuard\$asset"
        if (-not (Test-Path $source)) {
            throw "Expected WireGuard asset missing after MSI extraction: $source"
        }
        Copy-Item -LiteralPath $source -Destination (Join-Path $binDir $asset) -Force
    }
}

$manifest = @(
    "ZeroNode VPN Windows bundle"
    "Built: $(Get-Date -Format s)"
    "Profile: $Profile"
    ""
    "Included binaries:"
)
foreach ($binary in $binaries) {
    $manifest += " - $binary.exe"
}
$manifest += " - wireguard.exe (official WireGuard Windows tunnel service helper)"
$manifest += " - wg.exe (official WireGuard command-line helper)"

# Generate custom icon.ico inside distRoot from assets/icon.png
$pngPath = Join-Path $root "assets\icon.png"
if (Test-Path $pngPath) {
    $pngBytes = [System.IO.File]::ReadAllBytes($pngPath)
    $ico = [System.Collections.Generic.List[byte]]::new()

    # ICO Header: 6 bytes
    $ico.Add(0); $ico.Add(0) # Reserved
    $ico.Add(1); $ico.Add(0) # Type: 1 = Icon
    $ico.Add(1); $ico.Add(0) # Count: 1

    # Directory Entry: 16 bytes
    $ico.Add(0) # Width: 0 means 256px
    $ico.Add(0) # Height: 0 means 256px
    $ico.Add(0) # Colors: 0 for >=8bpp
    $ico.Add(0) # Reserved
    $ico.Add(1); $ico.Add(0) # Planes: 1
    $ico.Add(32); $ico.Add(0) # Bits per pixel: 32

    # Size of PNG bytes: 4 bytes
    $sizeBytes = [System.BitConverter]::GetBytes([uint32]$pngBytes.Length)
    $ico.AddRange($sizeBytes)

    # Offset to PNG data: 4 bytes (22)
    $offsetBytes = [System.BitConverter]::GetBytes([uint32]22)
    $ico.AddRange($offsetBytes)

    # Raw PNG data
    $ico.AddRange($pngBytes)

    [System.IO.File]::WriteAllBytes((Join-Path $distRoot "icon.ico"), $ico.ToArray())
}

Set-Content -LiteralPath (Join-Path $distRoot "BUILD-INFO.txt") -Value $manifest
Copy-Item -LiteralPath (Join-Path $root "README.md") -Destination (Join-Path $distRoot "README.md") -Force

Write-Host "Windows artifacts staged in $distRoot"
