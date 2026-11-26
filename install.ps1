<#
.SYNOPSIS
Install the kb command on Windows from a GitHub release (README.md, section "Install").

.DESCRIPTION
Downloads kb-<version>-x86_64-pc-windows-msvc.tar.gz and SHA256SUMS of the release (default:
the latest), verifies the archive's SHA-256 digest and entries, extracts it with the tar.exe
that ships with Windows 10 (1803) and later, checks that kb.exe runs, installs it as
<Dir>\kb.exe (default: $env:KB_INSTALL_DIR, else %LOCALAPPDATA%\Programs\kb\bin) and adds
that directory to the user PATH. An existing kb.exe that is not kb is only replaced with
-Force. Update later with `kb self update`. KB_RELEASES_URL selects another release location
(https:// or a local directory with the same download/v<version>/ layout). On Windows on Arm
the x64 build runs under emulation.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File install.ps1
.EXAMPLE
.\install.ps1 -Version 0.1.0 -Dir C:\tools\kb
#>
[CmdletBinding()]
param(
    [string]$Version = '',
    [string]$Dir = '',
    [switch]$Force,
    [switch]$NoPathUpdate
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3

$releases = 'https://github.com/4wl2d/kb/releases'
if ($env:KB_RELEASES_URL) { $releases = $env:KB_RELEASES_URL.TrimEnd('/') }
if (-not $Dir) {
    if ($env:KB_INSTALL_DIR) { $Dir = $env:KB_INSTALL_DIR }
    else { $Dir = Join-Path $env:LOCALAPPDATA 'Programs\kb\bin' }
}
$Version = $Version.TrimStart('v')
if ($Version -and $Version -notmatch '^[0-9A-Za-z.+-]+$') { throw "malformed version '$Version'" }
$target = 'x86_64-pc-windows-msvc'

function Get-AssetLocation([string]$Name, [string]$Release) {
    if ($Release) { return "$releases/download/v$Release/$Name" }
    return "$releases/latest/download/$Name"
}

function Copy-Asset([string]$Location, [string]$Destination) {
    if ($Location -match '^https://') {
        # Windows PowerShell 5.1 does not offer TLS 1.2 by default.
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $Location -OutFile $Destination -UseBasicParsing
    } elseif ($Location -match '://') {
        throw "only https:// URLs and local directories are accepted: $Location"
    } else {
        Copy-Item -LiteralPath $Location -Destination $Destination
    }
}

$tar = Join-Path $env:SystemRoot 'System32\tar.exe'
if (-not (Test-Path -LiteralPath $tar)) { $tar = 'tar.exe' }

$tmp = Join-Path ([IO.Path]::GetTempPath()) ('kb-install-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $sumsPath = Join-Path $tmp 'SHA256SUMS'
    Copy-Asset (Get-AssetLocation 'SHA256SUMS' $Version) $sumsPath
    $suffix = "-$target.tar.gz"
    $entries = @(Get-Content -LiteralPath $sumsPath | ForEach-Object {
        if ($_ -match '^([0-9a-fA-F]{64}) [ *](kb-(.+)' + [regex]::Escape($suffix) + ')\s*$') {
            [pscustomobject]@{ Hash = $Matches[1].ToLowerInvariant(); Name = $Matches[2]; Version = $Matches[3] }
        }
    })
    if (-not $Version) {
        if ($entries.Count -lt 1) { throw "the latest release has no archive for $target" }
        $Version = $entries[0].Version
    }
    $top = "kb-$Version-$target"
    $name = "$top.tar.gz"
    $match = @($entries | Where-Object { $_.Name -eq $name })
    if ($match.Count -ne 1) { throw "SHA256SUMS of release v$Version does not list $name exactly once" }

    Write-Host "kb install: downloading $name"
    $archive = Join-Path $tmp $name
    Copy-Asset (Get-AssetLocation $name $Version) $archive
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash.ToLowerInvariant()
    if ($actual -ne $match[0].Hash) { throw "sha256 mismatch for ${name}: expected $($match[0].Hash), got $actual" }

    # Only entries inside $top/, without '..' components.
    $names = @(& $tar -tzf $archive)
    if ($LASTEXITCODE -ne 0) { throw "$name is not a gzip-compressed tar" }
    foreach ($n in $names) {
        $inside = ($n -eq $top) -or ($n -eq "$top/") -or $n.StartsWith("$top/")
        if (-not $inside -or ("/$n/" -match '/\.\./') -or $n.Contains('\') -or $n.Contains(':')) {
            throw "unsafe archive entry '$n'"
        }
    }
    $x = Join-Path $tmp 'x'
    New-Item -ItemType Directory -Path $x | Out-Null
    & $tar -xzf $archive -C $x
    if ($LASTEXITCODE -ne 0) { throw "cannot extract $name" }
    $new = Join-Path $x "$top\kb.exe"
    $item = Get-Item -LiteralPath $new -ErrorAction SilentlyContinue
    if (-not $item -or $item.PSIsContainer -or $item.LinkType) { throw "$top/kb.exe is missing" }
    $reported = (& $new --version) -join ''
    if ($LASTEXITCODE -ne 0 -or $reported -ne "kb $Version") {
        throw "the downloaded kb reports '$reported', expected 'kb $Version'"
    }

    $dest = Join-Path $Dir 'kb.exe'
    if (Test-Path -LiteralPath $dest) {
        $current = ''
        try { $current = (& $dest --version 2>$null) -join '' } catch { $current = '' }
        if ($current -notmatch '^kb ' -and -not $Force) {
            throw "$dest exists and is not kb; move it away or pass -Force"
        }
    }
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $staged = Join-Path $Dir ".kb.new.$PID.exe"
    Copy-Item -LiteralPath $new -Destination $staged -Force
    if (Test-Path -LiteralPath $dest) {
        # A running kb.exe can be renamed but not overwritten.
        $aside = Join-Path $Dir ".kb.exe.old.$PID"
        Move-Item -LiteralPath $dest -Destination $aside -Force
        try { Move-Item -LiteralPath $staged -Destination $dest }
        catch { Move-Item -LiteralPath $aside -Destination $dest; throw }
        Remove-Item -LiteralPath $aside -Force -ErrorAction SilentlyContinue
    } else {
        Move-Item -LiteralPath $staged -Destination $dest
    }
    Write-Host "kb install: installed kb $Version ($target) as $dest"

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $userPath) { $userPath = '' }
    $listed = @($userPath -split ';' | Where-Object { $_ -and ($_.TrimEnd('\') -ieq $Dir.TrimEnd('\')) })
    if ($listed.Count -eq 0) {
        if ($NoPathUpdate) {
            Write-Host "kb install: add $Dir to PATH to run kb"
        } else {
            [Environment]::SetEnvironmentVariable('Path', (($userPath.TrimEnd(';'), $Dir) -join ';').TrimStart(';'), 'User')
            Write-Host "kb install: added $Dir to your user PATH; open a new terminal to run kb"
        }
    }
    Write-Host 'kb install: update later with: kb self update'
} finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
