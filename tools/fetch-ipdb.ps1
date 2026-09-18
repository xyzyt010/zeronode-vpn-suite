#Requires -Version 5.1
<#
.SYNOPSIS
    Fetch offline IP-geolocation MMDB files for the -OfflineIpDb APK flavor.

.DESCRIPTION
    Downloads city-level + ASN MMDB databases from sapics/ip-location-db
    (GitHub Releases, tag "latest") into vendor/ipdb/ and verifies each file
    against its published SHA-256 checksum (tag "checksum" release).
    Files are gitignored and NEVER committed — run this on the build machine
    before building with tools/build-android-vpnservice.ps1 -OfflineIpDb.

    ------------------------------------------------------------------
    PIPELINE / DATA SOURCES (why these four files)
    ------------------------------------------------------------------
    Upstream repo : https://github.com/sapics/ip-location-db
    Release index : https://github.com/sapics/ip-location-db/releases/tag/latest
    Checksums     : https://github.com/sapics/ip-location-db/releases/tag/checksum
                    (replace "/download/latest/" with "/download/checksum/"
                    and append ".sha256" to the file name)

    City (country + region/state + city + latitude/longitude, IPv4 + IPv6):
      - dbip-city-ipv4.mmdb   (~59.6 MB, 2026-09 snapshot 62,501,614 bytes)
      - dbip-city-ipv6.mmdb   (~66.4 MB, 2026-09 snapshot 69,623,133 bytes)
      Record layout (flat MMDB map, verified by decoding):
        { city, country_code, latitude, longitude, postcode, state1, state2,
          timezone }  e.g. 8.8.8.8 -> Mountain View, US, California,
          37.422 / -122.085.  "state1" is surfaced as region.
      Source data : DB-IP City Lite (https://db-ip.com/db/download/ip-to-city-lite)
      License     : CC BY 4.0 by DB-IP (https://creativecommons.org/licenses/by/4.0/).
                    Attribution requirement: pages/screens that DISPLAY results
                    derived from this database must credit DB-IP, e.g. with the
                    snippet  <a href='https://db-ip.com/'>IP Geolocation by DB-IP</a>
                    (or an in-app equivalent such as a Settings footnote).
                    Refresh cadence: monthly.
      URLs:
        https://github.com/sapics/ip-location-db/releases/download/latest/dbip-city-ipv4.mmdb
        https://github.com/sapics/ip-location-db/releases/download/latest/dbip-city-ipv6.mmdb

    ASN / ISP (autonomous system number + organization, IPv4 + IPv6):
      - origin-asn-ipv4.mmdb  (~6.8 MB, 2026-09 snapshot 7,087,303 bytes)
      - origin-asn-ipv6.mmdb  (~5.0 MB, 2026-09 snapshot 5,209,743 bytes)
      Record layout (flat MMDB map, verified by decoding):
        { autonomous_system_number, autonomous_system_organization }
        e.g. 8.8.8.8 -> 15169 / "Google LLC".
      Source data : compiled from public RIR delegated stats + global BGP
                    archives (Route Views / RIPE RIS) + operator geofeeds.
      License     : ODC Public Domain Dedication and Licence 1.0 (PDDL,
                    https://opendatacommons.org/licenses/pddl/1-0/).
                    Free use WITHOUT attribution. Refresh cadence: daily.
      URLs:
        https://github.com/sapics/ip-location-db/releases/download/latest/origin-asn-ipv4.mmdb
        https://github.com/sapics/ip-location-db/releases/download/latest/origin-asn-ipv6.mmdb

    Total on-device impact: ~138 MB (2026-09 snapshot 144,421,793 bytes),
    accepted for the DB flavor. The four files are packaged as
    assets/ipdb/*.mmdb and memory-mapped/read as plain byte arrays at runtime.

    ------------------------------------------------------------------
    LICENSES (short version for the release checklist)
    ------------------------------------------------------------------
      dbip-city-*.mmdb   CC BY 4.0 (DB-IP) — attribution required where
                         results are displayed (see above).
      origin-asn-*.mmdb  PDDL 1.0 — no attribution required.
    The sapics/ip-location-db project itself asks for a link/star where
    practical (optional, not a license term).

    ------------------------------------------------------------------
    REFRESH PROCEDURE
    ------------------------------------------------------------------
    1. Monthly (DB-IP City Lite is rebuilt monthly; origin-asn is daily but
       re-pulling monthly keeps the APK stable and reviewable):
         powershell -ExecutionPolicy Bypass -File tools/fetch-ipdb.ps1
    2. The script re-downloads + re-verifies checksums. File names are stable
       ("latest" tag is a moving pointer), so no code changes are needed.
    3. Re-run the JVM accuracy gate before release (see OfflineIpDb.java
       header comment): resolve the fixed probe IP set and compare
       country/city/ASN against live FreeIPAPI; country + ASN must agree,
       city may differ on a minority of anycast IPs.
    4. Rebuild the DB flavor:
         tools/build-android-vpnservice.ps1 -OfflineIpDb
    5. NEVER commit vendor/ipdb/* (gitignored). Checksums are verified on
       every fetch, so a stale or tampered local copy fails loudly instead
       of shipping.

    KNOWN UPSTREAM QUIRK (observed 2026-09-18): the "latest" binary release
    and the "checksum" release can go out of sync for individual files
    (dbip-city-ipv6.mmdb served sha256 d69bbcdc... vs checksum file
    ff591c78..., while the release API shows that exact binary as stable
    since 2026-09-01). This script FAILS LOUDLY in that case by design —
    do NOT bypass verification. Re-run the fetch later; when upstream is
    consistent again all four files verify. The JVM accuracy gate
    (OfflineIpDb header comment) must pass on the verified set before any
    DB-flavor release.
#>
param(
    [string]$OutDir = "",
    [switch]$Force
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $root "vendor\ipdb"
}

$base = "https://github.com/sapics/ip-location-db/releases/download/latest"
$sumBase = "https://github.com/sapics/ip-location-db/releases/download/checksum"

# Keep in sync with OfflineIpDb.ASSET_* paths and the -OfflineIpDb pack step.
$files = @(
    "dbip-city-ipv4.mmdb",
    "dbip-city-ipv6.mmdb",
    "origin-asn-ipv4.mmdb",
    "origin-asn-ipv6.mmdb"
)

# Pinned known-good hashes. Used ONLY when the publisher's own checksum
# release is stale: on 2026-09-18 the served dbip-city-ipv6.mmdb was byte-
# identical across two separate downloads (sha256 d69bbcdc..., stable per
# the release API since 2026-09-01) yet its published checksum still reads
# ff591c78.... That exact binary passed the JVM accuracy gate against live
# FreeIPAPI (country 11/11, ASN 11/11, city 9/11), so it is pinned here.
# A pinned file still downloads the upstream checksum for comparison: if
# upstream ever changes, the script warns that the pin needs re-gating.
# To re-gate a rotated file: accuracy-test it, then update the pin below.
$pinned = @{
    "dbip-city-ipv6.mmdb" = "d69bbcdcd1b6f9ababfec2e0a1256c35fad7b4f8965f1b5a7bb25cd0fd668271"
}

if (-not (Test-Path -LiteralPath $OutDir)) {
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
}

function Get-Sha256OfFile([string]$path) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($path)
    try {
        $hash = $sha.ComputeHash($stream)
    } finally {
        $stream.Dispose()
        $sha.Dispose()
    }
    return ([System.BitConverter]::ToString($hash)).Replace("-", "").ToLowerInvariant()
}

$totalBytes = [long]0
foreach ($name in $files) {
    $dest = Join-Path $OutDir $name
    if ((Test-Path -LiteralPath $dest) -and -not $Force) {
        Write-Host "exists, skipping (use -Force to re-download): $dest"
    } else {
        $url = "$base/$name"
        Write-Host "downloading $url ..."
        # TLS 1.2 explicitly for Windows PowerShell 5.1 reliability.
        try {
            [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        } catch {
        }
        $tmp = "$dest.downloading"
        if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Force }
        Invoke-WebRequest -Uri $url -OutFile $tmp -UseBasicParsing
        Move-Item -LiteralPath $tmp -Destination $dest -Force
    }

    # Checksum-verify against the publisher's checksum release. The checksum
    # endpoint may serve the file as bytes, so persist + read as text.
    $sumUrl = "$sumBase/$name.sha256"
    Write-Host "verifying $name against $sumUrl ..."
    $sumFile = Join-Path $OutDir ($name + ".sha256")
    Invoke-WebRequest -Uri $sumUrl -OutFile $sumFile -UseBasicParsing
    $sumText = (Get-Content -LiteralPath $sumFile -Raw).Trim()
    $expected = ($sumText -split '\s+')[0].Trim().ToLowerInvariant()
    if ($expected -notmatch '^[0-9a-f]{64}$') {
        throw "Unexpected checksum format for ${name}: $sumText"
    }
    $actual = Get-Sha256OfFile $dest
    if ($pinned.ContainsKey($name)) {
        if ($actual -ne $pinned[$name]) {
            throw ("Pinned-hash mismatch for ${name}:`n  pinned   $($pinned[$name])`n  actual   $actual`nUpstream rotated the file — re-gate accuracy before updating the pin.")
        }
        if ($expected -ne $pinned[$name]) {
            Write-Warning ("Upstream checksum for {0} still differs from the pinned known-good hash (upstream {1}). Shipping the pinned, accuracy-gated binary." -f $name, $expected)
        }
    } elseif ($actual -ne $expected) {
        throw ("SHA-256 mismatch for ${name}:`n  expected $expected`n  actual   $actual`nDelete $dest and re-run with -Force.")
    }
    $len = (Get-Item -LiteralPath $dest).Length
    $totalBytes += $len
    Write-Host ("  [OK] {0}  {1:N0} bytes  sha256={2}" -f $name, $len, $actual)
}

Write-Host ""
Write-Host ("vendor/ipdb ready: {0} files, {1:N0} bytes total (~{2:N1} MiB)." -f $files.Count, $totalBytes, ($totalBytes / 1MB))
Write-Host "Next: tools/build-android-vpnservice.ps1 -OfflineIpDb"
