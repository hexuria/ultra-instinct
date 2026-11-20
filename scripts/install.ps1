# Install `aui` + `ultra-instinct` from a GitHub release. No Rust toolchain
# needed:
#
#   irm https://raw.githubusercontent.com/hexuria/ultra-instinct/main/scripts/install.ps1 | iex
#
# It downloads the archive for this machine, checks it against the release's
# SHA256SUMS, and copies both binaries to $env:AUI_INSTALL_DIR (default
# "$env:LOCALAPPDATA\Programs\aui"). A checksum mismatch stops the install.
# When there is no release binary for this machine, or the download fails,
# it falls back to the source build of main (needs cargo and git). With
# AUI_VERSION set there is no fallback.
#
#   $env:AUI_VERSION      release tag to install (default: the latest release)
#   $env:AUI_INSTALL_DIR  where the binaries go
#   $env:AUI_BASE_URL     release download root (default: the GitHub releases
#                         of hexuria/ultra-instinct; set by the tests)

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = 'hexuria/ultra-instinct'
$Base = if ($env:AUI_BASE_URL) { $env:AUI_BASE_URL } else { "https://github.com/$Repo/releases" }
$Raw = "https://raw.githubusercontent.com/$Repo/main"
$Dir = if ($env:AUI_INSTALL_DIR) { $env:AUI_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\aui' }

function Say($msg) { Write-Host $msg }
function Warn($msg) { Write-Warning $msg }

# Copy both binaries from $From to $Dir. Copy beside the destination, then
# move: a running `aui` keeps its file where the filesystem allows it.
function Place($From) {
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    foreach ($bin in 'aui', 'ultra-instinct') {
        $src = Join-Path $From "$bin.exe"
        $dst = Join-Path $Dir "$bin.exe"
        $tmp = "$dst.new"
        Copy-Item $src $tmp -Force
        try {
            Move-Item $tmp $dst -Force
        } catch {
            Remove-Item $tmp -Force -ErrorAction SilentlyContinue
            throw "could not replace $dst (a running aui session holds it? run 'aui close' and retry): $($_.Exception.Message)"
        }
    }
    $ver = & (Join-Path $Dir 'aui.exe') --version
    Say "installed $ver to $Dir"
    $onPath = ($env:Path -split ';') +
        ([Environment]::GetEnvironmentVariable('Path', 'User') -split ';') +
        ([Environment]::GetEnvironmentVariable('Path', 'Machine') -split ';') |
        Where-Object { $_ } | ForEach-Object { [IO.Path]::GetFullPath($_) }
    if ($onPath -notcontains [IO.Path]::GetFullPath($Dir)) {
        Warn "warning: $Dir is not on PATH; add it to use aui"
    }
}

function Install-Source([string]$Why) {
    if ($env:AUI_VERSION) {
        throw "$Why; AUI_VERSION=$env:AUI_VERSION was not installed"
    }
    Warn "$Why; falling back to the source build"
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw 'no release binary was installed and cargo is not on PATH'
    }
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        throw 'the source build needs git on PATH'
    }
    # The repo, when this script runs from inside a checkout; else a cache
    # clone, updated via git pull (same layout as install-aui.sh).
    $src = $null
    if ($PSScriptRoot -and (Test-Path (Join-Path $PSScriptRoot '..\crates\ultra-instinct\Cargo.toml'))) {
        $src = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
    } else {
        $src = Join-Path $env:LOCALAPPDATA 'ultra-instinct'
        if (Test-Path (Join-Path $src '.git')) {
            git -C $src pull --ff-only | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "git -C $src pull --ff-only failed" }
        } else {
            git clone "https://github.com/$Repo" $src
            if ($LASTEXITCODE -ne 0) { throw 'git clone failed' }
        }
    }
    # Build into a scratch root so the binaries copied to $Dir are the ones
    # just built, whatever cargo's own install root is.
    $stage = Join-Path ([IO.Path]::GetTempPath()) ("aui-install-" + [Guid]::NewGuid().ToString('N'))
    $env:CARGO_INSTALL_ROOT = $stage
    try {
        Push-Location $src
        try {
            cargo install --locked --force --path crates/ultra-instinct --features 'jev clef model-text'
            if ($LASTEXITCODE -ne 0) { throw 'cargo install failed' }
        } finally { Pop-Location }
    } finally {
        Remove-Item Env:\CARGO_INSTALL_ROOT -ErrorAction SilentlyContinue
    }
    Place (Join-Path $stage 'bin')
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
}

function Target {
    # Only the x86_64 MSVC build ships for Windows; ARM64 Windows has none.
    if ($env:PROCESSOR_ARCHITECTURE -eq 'AMD64' -or
        [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'X64') {
        return 'x86_64-pc-windows-msvc'
    }
    return $null
}

$t = Target
if (-not $t) {
    Install-Source "no release binary for $([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) Windows"
    return
}
$archive = "aui-$t.tar.gz"
$url = if ($env:AUI_VERSION) { "$Base/download/$env:AUI_VERSION" } else { "$Base/latest/download" }

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("aui-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
    $file = Join-Path $tmp $archive
    try {
        Invoke-WebRequest -Uri "$url/$archive" -OutFile $file -UseBasicParsing
        Invoke-WebRequest -Uri "$url/SHA256SUMS" -OutFile (Join-Path $tmp 'SHA256SUMS') -UseBasicParsing
    } catch {
        Install-Source "could not download $url/$archive"
        return
    }
    $want = $null
    foreach ($line in Get-Content (Join-Path $tmp 'SHA256SUMS')) {
        $parts = $line -split '\s+'
        if ($parts[-1].TrimStart('*') -eq $archive) { $want = $parts[0]; break }
    }
    $got = (Get-FileHash -Algorithm SHA256 $file).Hash.ToLower()
    if (-not $want -or $want.ToLower() -ne $got) {
        throw "checksum mismatch for $archive`n  expected: $(if ($want) { $want } else { '<not listed in SHA256SUMS>' })`n  got:      $got`n  nothing was installed"
    }
    # bsdtar ships with Windows 10 1803+.
    tar -xzf $file -C $tmp
    Place (Join-Path $tmp "aui-$t")
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
