<#
.SYNOPSIS
  Installs, upgrades or removes Frankenstein Harness (fh.exe) for the current user. No administrator rights needed.

.EXAMPLE
  iwr -useb https://raw.githubusercontent.com/cardimvitor/frankenstein-harness/main/scripts/install.ps1 | iex
  .\install.ps1 -Version v0.1.0
  .\install.ps1 -Uninstall

.NOTES
  Downloads fh-<version>-x86_64-pc-windows-msvc.zip from the GitHub release, verifies its SHA-256 from the published
  .sha256 file, installs fh.exe to %LOCALAPPDATA%\Programs\fh and adds that folder to the user PATH.
  -Source <folder or base URL> installs from a local folder or another host instead of GitHub (offline installs, tests).
#>
[CmdletBinding()]
param(
    [string]$Version = 'latest',
    [string]$InstallDir = $(if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA 'Programs\fh' } else { Join-Path $HOME '.fh-bin' }),
    [string]$Source = '',
    [string]$Repo = 'cardimvitor/frankenstein-harness',
    [switch]$NoPath,
    [switch]$NoVerify,
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'
$Target = 'x86_64-pc-windows-msvc'

function Write-Step([string]$m) { Write-Host "==> $m" }

# Returns the PATH value with $Dir added (unchanged when already present). Comparison ignores case and a trailing slash.
function Add-PathEntry([string]$Current, [string]$Dir) {
    $norm = { param($p) $p.TrimEnd('\', '/').ToLowerInvariant() }
    $parts = @($Current -split [IO.Path]::PathSeparator | Where-Object { $_ })
    if ($parts | Where-Object { (& $norm $_) -eq (& $norm $Dir) }) { return $Current }
    return (($parts + $Dir) -join [IO.Path]::PathSeparator)
}

function Remove-PathEntry([string]$Current, [string]$Dir) {
    $norm = { param($p) $p.TrimEnd('\', '/').ToLowerInvariant() }
    $parts = @($Current -split [IO.Path]::PathSeparator | Where-Object { $_ -and ((& $norm $_) -ne (& $norm $Dir)) })
    return ($parts -join [IO.Path]::PathSeparator)
}

function Get-FileSha256([string]$Path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant() }

# Fetches a file from a folder (-Source) or a URL into $Dest.
function Get-Payload([string]$From, [string]$Dest) {
    if ($From -match '^https?://') { Invoke-WebRequest -UseBasicParsing -Uri $From -OutFile $Dest }
    else { Copy-Item -LiteralPath $From -Destination $Dest -Force }
}

function Resolve-Release {
    # returns @{ Name; ArchiveUrl; ShaUrl } for the requested version
    if ($Source) {
        $base = $Source.TrimEnd('\', '/')
        $sep = if ($base -match '^https?://') { '/' } else { [IO.Path]::DirectorySeparatorChar }
        $zip = if ($base -match '^https?://') { $null } else { Get-ChildItem -LiteralPath $base -Filter "fh-*-$Target.zip" | Sort-Object Name | Select-Object -Last 1 }
        if ($zip) { return @{ Name = $zip.Name; ArchiveUrl = $zip.FullName; ShaUrl = "$($zip.FullName).sha256" } }
        if ($Version -eq 'latest') { throw "-Source is a URL: give -Version (for example v0.1.0)" }
        $n = "fh-$($Version.TrimStart('v'))-$Target.zip"
        return @{ Name = $n; ArchiveUrl = "$base$sep$n"; ShaUrl = "$base$sep$n.sha256" }
    }
    $api = if ($Version -eq 'latest') { "https://api.github.com/repos/$Repo/releases/latest" } else { "https://api.github.com/repos/$Repo/releases/tags/$Version" }
    $rel = Invoke-RestMethod -UseBasicParsing -Uri $api -Headers @{ 'User-Agent' = 'fh-installer' }
    $asset = $rel.assets | Where-Object { $_.name -like "fh-*-$Target.zip" } | Select-Object -First 1
    if (-not $asset) { throw "Release $($rel.tag_name) has no $Target build. See https://github.com/$Repo/releases" }
    $sha = $rel.assets | Where-Object { $_.name -eq "$($asset.name).sha256" } | Select-Object -First 1
    return @{ Name = $asset.name; ArchiveUrl = $asset.browser_download_url; ShaUrl = $(if ($sha) { $sha.browser_download_url } else { $null }) }
}

function Install-Fh {
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        Write-Warning 'git was not found on PATH. fh uses git for its per-task checkpoints: install Git for Windows (https://git-scm.com/download/win).'
    }
    $rel = Resolve-Release
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("fh-install-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $zip = Join-Path $tmp $rel.Name
        Write-Step "Downloading $($rel.Name)"
        Get-Payload $rel.ArchiveUrl $zip
        if ($rel.ShaUrl) {
            $shaFile = "$zip.sha256"
            Get-Payload $rel.ShaUrl $shaFile
            $expected = ((Get-Content -LiteralPath $shaFile -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
            $actual = Get-FileSha256 $zip
            if ($expected -ne $actual) { throw "Checksum mismatch for $($rel.Name): expected $expected, got $actual. Nothing was installed." }
            Write-Step 'Checksum verified'
        } else { Write-Warning 'No .sha256 file was published for this release; the download could not be verified.' }
        $x = Join-Path $tmp 'x'
        Expand-Archive -LiteralPath $zip -DestinationPath $x -Force
        $exe = Get-ChildItem -LiteralPath $x -Recurse -Filter 'fh.exe' | Select-Object -First 1
        if (-not $exe) { throw 'The archive does not contain fh.exe.' }
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        $dest = Join-Path $InstallDir 'fh.exe'
        # a running fh.exe cannot be overwritten, but it can be renamed: upgrades work while it is in use
        if (Test-Path -LiteralPath $dest) {
            $old = "$dest.old"
            if (Test-Path -LiteralPath $old) { Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue }
            Move-Item -LiteralPath $dest -Destination $old -Force
        }
        Copy-Item -LiteralPath $exe.FullName -Destination $dest -Force
        foreach ($doc in 'README.md', 'LICENSE') {
            $f = Get-ChildItem -LiteralPath $x -Recurse -Filter $doc | Select-Object -First 1
            if ($f) { Copy-Item -LiteralPath $f.FullName -Destination (Join-Path $InstallDir $doc) -Force }
        }
        if ($IsWindows -or $env:OS -eq 'Windows_NT') { Unblock-File -LiteralPath $dest -ErrorAction SilentlyContinue }
        Write-Step "Installed $dest"
        if (-not $NoPath) {
            $user = [Environment]::GetEnvironmentVariable('Path', 'User')
            $new = Add-PathEntry $user $InstallDir
            if ($new -ne $user) {
                [Environment]::SetEnvironmentVariable('Path', $new, 'User')
                Write-Step "Added $InstallDir to your user PATH (open a new terminal to pick it up)"
            }
        }
        if (-not $NoVerify) {
            $v = & $dest --help 2>&1 | Select-Object -First 1
            Write-Step "Check: $v"
        }
        Write-Host ''
        Write-Host 'Next: set FH_ENDPOINT, FH_MODEL and FH_API_KEY (or run `fh auth set`), then `fh doctor`.'
        Write-Host 'Building classic .NET Framework projects needs Visual Studio or Build Tools (MSBuild) on this machine.'
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Uninstall-Fh {
    if (Test-Path -LiteralPath $InstallDir) {
        Remove-Item -LiteralPath $InstallDir -Recurse -Force
        Write-Step "Removed $InstallDir"
    } else { Write-Step "Nothing installed in $InstallDir" }
    if (-not $NoPath) {
        $user = [Environment]::GetEnvironmentVariable('Path', 'User')
        $new = Remove-PathEntry $user $InstallDir
        if ($new -ne $user) { [Environment]::SetEnvironmentVariable('Path', $new, 'User'); Write-Step 'Removed it from your user PATH' }
    }
    Write-Host 'Your settings, skill store and keychain entry were left in place (data: %LOCALAPPDATA%\frankenstein-harness, config: %APPDATA%\frankenstein-harness).'
}

# dot-sourcing (. .\install.ps1) only loads the functions, which is how the tests exercise them
if ($MyInvocation.InvocationName -ne '.') {
    if ($Uninstall) { Uninstall-Fh } else { Install-Fh }
}
