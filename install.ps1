<#
.SYNOPSIS
  Install hatch from a GitHub Release on Windows.

.DESCRIPTION
  Downloads the latest (or specified) hatch release zip for the current
  architecture, verifies its sha256, extracts the binary, and writes a User
  PATH entry so that `hatch` resolves in new shells.

.PARAMETER Version
  Tag to install. Defaults to "latest". Example: "v0.2.0".

.PARAMETER InstallDir
  Install prefix. The binary lands under "<InstallDir>\bin\hatch.exe".
  Defaults to "$env:LOCALAPPDATA\Hatch".

.PARAMETER Repo
  GitHub owner/name. Defaults to "hatch-dev/hatch".

.PARAMETER NoModifyPath
  Skip the User-PATH update.

.EXAMPLE
  iwr -useb https://raw.githubusercontent.com/hatch-dev/hatch/main/install.ps1 | iex
#>
[CmdletBinding()]
param(
    [string]$Version = $env:HATCH_VERSION,
    [string]$InstallDir = $env:HATCH_INSTALL,
    [string]$Repo = $env:HATCH_REPO,
    [switch]$NoModifyPath = [bool]$env:HATCH_NO_MODIFY_PATH
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

if (-not $Version)    { $Version = 'latest' }
if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'Hatch' }
if (-not $Repo)       { $Repo = 'hatch-dev/hatch' }

function Resolve-Target {
    $arch = (Get-CimInstance Win32_Processor | Select-Object -First 1).Architecture
    # 9 = x64, 12 = ARM64. Anything else is unsupported until we ship that target.
    switch ($arch) {
        9  { return 'x86_64-pc-windows-msvc' }
        12 { throw "ARM64 Windows builds are not yet published. Build from source for now." }
        default { throw "Unsupported processor architecture code: $arch" }
    }
}

function Resolve-Tag([string]$tag) {
    if ($tag -ne 'latest') { return $tag }
    $api = "https://api.github.com/repos/$Repo/releases/latest"
    try {
        $resp = Invoke-RestMethod -UseBasicParsing -Uri $api -Headers @{ 'User-Agent' = 'hatch-installer' }
    } catch {
        throw "Failed to query $api : $($_.Exception.Message)"
    }
    if (-not $resp.tag_name) { throw "GitHub API response missing tag_name" }
    return $resp.tag_name
}

function Get-Sha256([string]$path) {
    return (Get-FileHash -Algorithm SHA256 -Path $path).Hash.ToLower()
}

function Add-UserPath([string]$dir) {
    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $current) { $current = '' }
    $parts = $current -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ne $dir.TrimEnd('\') }
    $parts = ,$dir + $parts
    $new = ($parts -join ';').TrimEnd(';')
    [Environment]::SetEnvironmentVariable('Path', $new, 'User')
    # Make it visible in this session as well.
    if (";$env:Path;" -notlike "*;$dir;*") {
        $env:Path = "$dir;$env:Path"
    }
}

$target  = Resolve-Target
$tag     = Resolve-Tag $Version
$ver     = $tag.TrimStart('v')
$archive = "hatch-$ver-$target.zip"
$base    = "https://github.com/$Repo/releases/download/$tag"
$binDir  = Join-Path $InstallDir 'bin'

Write-Host "installing hatch $tag ($target) -> $binDir\hatch.exe"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("hatch-install-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp -Force | Out-Null
try {
    $archivePath  = Join-Path $tmp $archive
    $checksumPath = "$archivePath.sha256"

    Invoke-WebRequest -UseBasicParsing -Uri "$base/$archive"        -OutFile $archivePath
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$archive.sha256" -OutFile $checksumPath

    $expected = (Get-Content $checksumPath -Raw).Trim().Split(@(' ', "`t"), 2)[0].ToLower()
    if (-not $expected) { throw "checksum file $checksumPath is empty" }

    $actual = Get-Sha256 $archivePath
    if ($actual -ne $expected) {
        throw "checksum mismatch: expected $expected, got $actual"
    }

    Expand-Archive -Path $archivePath -DestinationPath $tmp -Force

    $extracted = Join-Path $tmp ("hatch-$ver-$target")
    $exe = Join-Path $extracted 'hatch.exe'
    if (-not (Test-Path $exe)) {
        throw "extracted archive missing hatch.exe at $exe"
    }

    New-Item -ItemType Directory -Path $binDir -Force | Out-Null
    Copy-Item -Path $exe -Destination (Join-Path $binDir 'hatch.exe') -Force

    $installed = Join-Path $binDir 'hatch.exe'
    try {
        $reported = & $installed --version 2>$null
        if ($reported) { Write-Host "installed: $reported" }
        else { Write-Host "installed: $installed" }
    } catch {
        Write-Host "installed: $installed"
    }

    if (-not $NoModifyPath) {
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $alreadyOnPath = $false
        if ($userPath) {
            foreach ($p in ($userPath -split ';')) {
                if ($p -and ($p.TrimEnd('\') -ieq $binDir.TrimEnd('\'))) {
                    $alreadyOnPath = $true
                    break
                }
            }
        }
        if (-not $alreadyOnPath) {
            Add-UserPath $binDir
            Write-Host ""
            Write-Host "added $binDir to your User PATH. Open a new terminal for it to take effect."
        }
    }
} finally {
    if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
}
