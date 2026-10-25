# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Downloads the pinned, checksum-verified Frida devkits for Windows x64: the
# PowerShell twin of devkits.sh, for a box without Git Bash. Same version,
# same checksum file, same destination.
param([string]$Target = 'x86_64-pc-windows-msvc')
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$version = '17.17.0'
switch ($Target) {
    'x86_64-pc-windows-msvc' { $platform = 'windows-x86_64' }
    default { throw "Unsupported native Frida target for this script: $Target (use devkits.sh elsewhere)" }
}
$kits = if ($env:ORBIT_FRIDA_DEVKIT_ROOT) { $env:ORBIT_FRIDA_DEVKIT_ROOT } else { Join-Path $root 'target\frida-devkits' }
$sums = Get-Content (Join-Path $PSScriptRoot 'devkits.sha256')
foreach ($component in 'core', 'gum') {
    $name = "frida-$component-devkit-$version-$platform.tar.xz"
    $kit = Join-Path $kits "$Target\$component"
    if (Test-Path (Join-Path $kit ".verified-$version")) { continue }
    New-Item -ItemType Directory -Force (Join-Path $kits $Target) | Out-Null
    $download = Join-Path $kits "$Target\$name"
    $uri = "https://github.com/frida/frida/releases/download/$version/$name"
    # curl, which Windows 10+ ships, with a hard stall limit: Invoke-WebRequest
    # once sat 88 minutes on this download in CI, past its own -TimeoutSec,
    # which does not cover a response body that stops arriving.
    $curl = Join-Path (Join-Path $env:SystemRoot 'System32') 'curl.exe'
    Write-Host "Downloading $name"
    & $curl -fsSL --retry 3 --retry-delay 5 --max-time 900 --speed-time 60 --speed-limit 10000 -o $download $uri
    if ($LASTEXITCODE -ne 0) { Remove-Item $download -ErrorAction SilentlyContinue; throw "could not download $uri (curl exit $LASTEXITCODE)" }
    $expected = ($sums | Where-Object { $_ -match "\s$([regex]::Escape($name))$" } | ForEach-Object { ($_ -split '\s+')[0] })
    $actual = (Get-FileHash -Algorithm SHA256 $download).Hash.ToLowerInvariant()
    if (-not $expected -or $actual -ne $expected) { Remove-Item $download; throw "Invalid checksum: $name" }
    New-Item -ItemType Directory -Force $kit | Out-Null
    # Windows 10+ ships bsdtar, which reads .tar.xz. By full path: with Git
    # on PATH ahead of System32, a bare tar.exe is GNU tar, which reads the
    # drive letter of $download as a remote host.
    & (Join-Path (Join-Path $env:SystemRoot 'System32') 'tar.exe') -xf $download -C $kit
    if ($LASTEXITCODE -ne 0) { throw "could not extract $name" }
    New-Item -ItemType File -Force (Join-Path $kit ".verified-$version") | Out-Null
    Remove-Item $download
}
Write-Host "Frida $version devkits ready under $kits\$Target"
