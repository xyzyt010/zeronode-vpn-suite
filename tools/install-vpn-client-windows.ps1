# ZeroNode VPN Client — one-click Windows installer.
#
# Downloads the stable vpn-client from GitHub (or uses a local copy),
# installs it to Program Files, forces always-run-as-admin (VPN needs it
# for Wintun + routes), creates Desktop + Start Menu shortcuts, and launches.
#
# Usage (download + install):
#   powershell -ExecutionPolicy Bypass -File .\install-vpn-client-windows.ps1
#
# Usage (specific release):
#   .\install-vpn-client-windows.ps1 -ReleaseTag v0.3.1-client-only
#
# Usage (offline, already-downloaded exe):
#   .\install-vpn-client-windows.ps1 -OfflineExe .\ZeroNode-VPN-Client-Windows-x64.exe
param(
    [string]$ReleaseTag = "v0.3.1-client-only",
    [string]$InstallDir = (Join-Path $env:ProgramFiles "ZeroNode VPN"),
    [string]$OfflineExe = ""
)

$ErrorActionPreference = "Stop"

$Repo = "xyzyt010/zeronode-vpn-suite"
$ExeName = "ZeroNode-VPN-Client-Windows-x64.exe"
$BinDir = Join-Path $InstallDir "bin"
$InstalledExe = Join-Path $BinDir "vpn-client.exe"

function Test-IsAdmin {
    ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

# --- Re-launch elevated so Program Files copy + HKLM flag always succeed. ---
if (-not (Test-IsAdmin)) {
    Write-Host "Requesting Administrator (one UAC click) for install..."
    $me = (Resolve-Path $PSCommandPath).Path
    $args = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "`"$me`"",
        "-ReleaseTag", "`"$ReleaseTag`"",
        "-InstallDir", "`"$InstallDir`"")
    if ($OfflineExe) { $args += @("-OfflineExe", "`"$OfflineExe`"") }
    Start-Process -FilePath "powershell.exe" -ArgumentList $args -Verb RunAs
    exit 0
}

# --- Get the exe (download or offline). ---
$tmpExe = Join-Path ([System.IO.Path]::GetTempPath()) $ExeName
if ($OfflineExe) {
    if (-not (Test-Path $OfflineExe)) { throw "Offline exe not found: $OfflineExe" }
    $tmpExe = (Resolve-Path $OfflineExe).Path
    Write-Host "Using offline exe: $tmpExe"
} else {
    $url = "https://github.com/$Repo/releases/download/$ReleaseTag/$ExeName"
    $sumsUrl = "https://github.com/$Repo/releases/download/$ReleaseTag/SHA256SUMS.txt"
    Write-Host "Downloading $url ..."
    Invoke-WebRequest -Uri $url -OutFile $tmpExe -UseBasicParsing
    # Verify hash when the release publishes SHA256SUMS.txt (best effort).
    try {
        $sumsFile = Join-Path ([System.IO.Path]::GetTempPath()) "zn-client-SHA256SUMS.txt"
        Invoke-WebRequest -Uri $sumsUrl -OutFile $sumsFile -UseBasicParsing
        $want = (Select-String -Path $sumsFile -Pattern ([regex]::Escape($ExeName)) |
            Select-Object -First 1).Line.Split()[0].ToUpper()
        $got = (Get-FileHash -Path $tmpExe -Algorithm SHA256).Hash.ToUpper()
        if ($want -and $want -ne $got) {
            throw "SHA256 mismatch! want=$want got=$got — download may be corrupt."
        }
        Write-Host "SHA256 OK: $got"
    } catch {
        Write-Warning "Hash check skipped/failed (non-fatal): $_"
    }
}
$size = (Get-Item $tmpExe).Length
if ($size -lt 10MB) { throw "Exe suspiciously small ($size bytes) — aborting." }
Write-Host "Client exe ready: $tmpExe ($([math]::Round($size / 1MB, 1)) MB)"

# --- Install to Program Files. ---
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
Copy-Item -Path $tmpExe -Destination $InstalledExe -Force
Unblock-File -Path $InstalledExe -ErrorAction SilentlyContinue
Write-Host "Installed to $InstalledExe"

# --- Always run as admin (VPN needs it; otherwise tunnels silently fail). ---
$layers = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers"
if (-not (Test-Path $layers)) { New-Item -Path $layers -Force | Out-Null }
New-ItemProperty -Path $layers -Name $InstalledExe -Value "RUNASADMIN" `
    -PropertyType String -Force | Out-Null
Write-Host "Set always-run-as-admin for $InstalledExe"

# --- Shortcuts (Desktop for all users + Start Menu) with the admin flag. ---
$shell = New-Object -COM WScript.Shell
$links = @(
    (Join-Path ([Environment]::GetFolderPath("CommonDesktopDirectory")) "ZeroNode VPN.lnk"),
    (Join-Path "$env:ProgramData\Microsoft\Windows\Start Menu\Programs" "ZeroNode VPN.lnk")
)
foreach ($lnk in $links) {
    $sc = $shell.CreateShortcut($lnk)
    $sc.TargetPath = $InstalledExe
    $sc.WorkingDirectory = $BinDir
    $sc.Description = "ZeroNode VPN Client (always admin)"
    $sc.Save()
    $bytes = [System.IO.File]::ReadAllBytes($lnk)
    $bytes[0x15] = $bytes[0x15] -bor 0x20  # SLDF_RUNAS_USER
    [System.IO.File]::WriteAllBytes($lnk, $bytes)
    Write-Host "Shortcut: $lnk"
}

# --- Launch (UAC consent appears once thanks to RUNASADMIN). ---
Write-Host "Launching ZeroNode VPN Client..."
Start-Process -FilePath $InstalledExe
Write-Host ""
Write-Host "Done. Look for 'ZeroNode VPN Suite' on the taskbar."
Write-Host "If Windows asks for VPN permission, click Yes — Connect needs it."
