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
$kits = if ($env:ORBIT_FRIDA_DEVKIT_ROOT) { $env:ORBIT_FRIDA_DEVKIT_ROOT } else { Join-Path $root 'rust\target\frida-devkits' }
$sums = Get-Content (Join-Path $PSScriptRoot 'devkits.sha256')
foreach ($component in 'core', 'gum') {
    $name = "frida-$component-devkit-$version-$platform.tar.xz"
    $kit = Join-Path $kits "$Target\$component"
    if (Test-Path (Join-Path $kit ".verified-$version")) { continue }
    New-Item -ItemType Directory -Force (Join-Path $kits $Target) | Out-Null
    $download = Join-Path $kits "$Target\$name"
    Invoke-WebRequest -Uri "https://github.com/frida/frida/releases/download/$version/$name" -OutFile $download
    $expected = ($sums | Where-Object { $_ -match "\s$([regex]::Escape($name))$" } | ForEach-Object { ($_ -split '\s+')[0] })
    $actual = (Get-FileHash -Algorithm SHA256 $download).Hash.ToLowerInvariant()
    if (-not $expected -or $actual -ne $expected) { Remove-Item $download; throw "Invalid checksum: $name" }
    New-Item -ItemType Directory -Force $kit | Out-Null
    # Windows 10+ ships bsdtar, which reads .tar.xz.
    tar.exe -xf $download -C $kit
    if ($LASTEXITCODE -ne 0) { throw "could not extract $name" }
    New-Item -ItemType File -Force (Join-Path $kit ".verified-$version") | Out-Null
    Remove-Item $download
}
Write-Host "Frida $version devkits ready under $kits\$Target"
