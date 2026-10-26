# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Installs the Rust orbit-service on Windows x64 from the GitHub Release:
# the twin of install.sh. Downloads orbit-windows-x86_64.zip and its
# .sha256, refuses to install without a matching checksum, unpacks into
# %LOCALAPPDATA%\Programs\orbit (orbit-service.exe with the Frida files,
# orbit_api.dll and orbit.h beside it), and says how to put it on PATH.
#
#   irm https://raw.githubusercontent.com/pierricgimmig/orbit/main/tools/install/install.ps1 | iex
#
# Environment overrides, as for install.sh: ORBIT_INSTALL_BASE (the URL the
# assets are under), ORBIT_VERSION (a tag such as orbit-v0.1.0; default
# latest), ORBIT_INSTALL_DIR (default %LOCALAPPDATA%\Programs\orbit).
$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Say([string]$message) { Write-Host "orbit: $message" }
function Die([string]$message) { Write-Error "orbit: error: $message"; exit 1 }

if ([IntPtr]::Size -ne 8 -or $env:PROCESSOR_ARCHITECTURE -notin 'AMD64', 'ARM64') {
    Die 'only 64-bit Windows is supported'
}
if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
    Say 'this is an ARM64 machine; the x64 build runs under emulation and the kernel logger is untested there'
}

$base = if ($env:ORBIT_INSTALL_BASE) { $env:ORBIT_INSTALL_BASE }
        elseif ($env:ORBIT_VERSION) { "https://github.com/pierricgimmig/orbit/releases/download/$($env:ORBIT_VERSION)" }
        else { 'https://github.com/pierricgimmig/orbit/releases/latest/download' }
$version = if ($env:ORBIT_VERSION) { $env:ORBIT_VERSION } else { 'latest' }
$installDir = if ($env:ORBIT_INSTALL_DIR) { $env:ORBIT_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\orbit' }
$asset = 'orbit-windows-x86_64.zip'
$url = "$base/$asset"

Say "installing orbit-service $version for Windows x64"
$temp = Join-Path ([IO.Path]::GetTempPath()) ("orbit-install-" + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Force $temp | Out-Null
try {
    $archive = Join-Path $temp $asset
    try {
        Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
    } catch {
        Die ("could not download $url`n" +
             "The Rust release is tagged orbit-vX.Y.Z (not v1.* or nightly-*, which are the legacy C++ build).`n" +
             "Until such a tag exists there is nothing to install; build from source with .\tools\build\build-service-windows.ps1.")
    }
    try {
        Invoke-WebRequest -Uri "$url.sha256" -OutFile "$archive.sha256" -UseBasicParsing
    } catch {
        Die "no checksum file at $url.sha256; refusing to install an unverified binary"
    }
    $expected = ((Get-Content "$archive.sha256" -Raw) -split '\s+')[0].ToLowerInvariant()
    if ($expected -notmatch '^[0-9a-f]{64}$') { Die "malformed checksum file for $asset" }
    $actual = (Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { Die "checksum mismatch for $asset (expected $expected, got $actual)" }
    Say 'checksum verified'

    $unpacked = Join-Path $temp 'unpacked'
    Expand-Archive -Path $archive -DestinationPath $unpacked -Force
    $root = Join-Path $unpacked 'orbit'
    foreach ($required in 'orbit-service.exe', 'orbit-frida-helper.exe', 'orbit_frida_agent.dll', 'orbit_api.dll') {
        if (-not (Test-Path (Join-Path $root $required))) { Die "archive is missing $required" }
    }
    New-Item -ItemType Directory -Force $installDir | Out-Null
    # A running service holds its files; stop it before replacing them.
    Get-Process -Name 'orbit-service' -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$installDir*" } | ForEach-Object {
        Say "stopping orbit-service (pid $($_.Id)) in $installDir"
        $_ | Stop-Process -Force
    }
    Copy-Item -Path (Join-Path $root '*') -Destination $installDir -Recurse -Force
    Say "installed to $installDir"
} finally {
    Remove-Item -Recurse -Force $temp -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $installDir) {
    Say "note: $installDir is not on your PATH. Add it for this user with:"
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"$installDir;`" + [Environment]::GetEnvironmentVariable('Path', 'User'), 'User')"
}

Write-Host @"

orbit-service is installed. Start it and open the viewer in a browser:

  orbit-service
  # then open http://127.0.0.1:44766/

Manual scopes and Frida function hooks need no privilege. Call-stack
sampling, the scheduler and thread states come from the Windows kernel
logger (ETW), which needs an elevated prompt: start orbit-service from a
terminal opened with "Run as administrator" to record them. Auto-profiling
and the kernel hook engine are Linux only.

To instrument your own code: include\orbit.h is beside orbit-service
(nothing to link; it finds orbit_api.dll there), or use the orbit-api crate
or the orbit_api.py module.
"@
