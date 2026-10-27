# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Installs the Rust perfr on Windows x64 from the GitHub Release:
# the twin of install.sh. Downloads perfr-windows-x86_64.zip and its
# .sha256, refuses to install without a matching checksum, unpacks into
# %LOCALAPPDATA%\Programs\perfr (perfr.exe with the Frida files,
# perfr_api.dll and perfr.h beside it), and says how to put it on PATH.
#
#   irm https://raw.githubusercontent.com/perfr/perfr/main/tools/install/install.ps1 | iex
#
# Environment overrides, as for install.sh: PERFR_INSTALL_BASE (the URL the
# assets are under), PERFR_VERSION (a tag such as perfr-v0.1.0; default
# latest), PERFR_INSTALL_DIR (default %LOCALAPPDATA%\Programs\perfr).
$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Say([string]$message) { Write-Host "perfr: $message" }
function Die([string]$message) { Write-Error "perfr: error: $message"; exit 1 }

if ([IntPtr]::Size -ne 8 -or $env:PROCESSOR_ARCHITECTURE -notin 'AMD64', 'ARM64') {
    Die 'only 64-bit Windows is supported'
}
if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
    Say 'this is an ARM64 machine; the x64 build runs under emulation and the kernel logger is untested there'
}

$base = if ($env:PERFR_INSTALL_BASE) { $env:PERFR_INSTALL_BASE }
        elseif ($env:PERFR_VERSION) { "https://github.com/perfr/perfr/releases/download/$($env:PERFR_VERSION)" }
        else { 'https://github.com/perfr/perfr/releases/latest/download' }
$version = if ($env:PERFR_VERSION) { $env:PERFR_VERSION } else { 'latest' }
$installDir = if ($env:PERFR_INSTALL_DIR) { $env:PERFR_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\perfr' }
$asset = 'perfr-windows-x86_64.zip'
$url = "$base/$asset"

Say "installing perfr $version for Windows x64"
$temp = Join-Path ([IO.Path]::GetTempPath()) ("perfr-install-" + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Force $temp | Out-Null
try {
    $archive = Join-Path $temp $asset
    try {
        Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
    } catch {
        Die ("could not download $url`n" +
             "The Rust release is tagged perfr-vX.Y.Z (not v1.* or nightly-*, which are the legacy C++ build).`n" +
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
    $root = Join-Path $unpacked 'perfr'
    foreach ($required in 'perfr.exe', 'perfr-frida-helper.exe', 'perfr_frida_agent.dll', 'perfr_api.dll') {
        if (-not (Test-Path (Join-Path $root $required))) { Die "archive is missing $required" }
    }
    New-Item -ItemType Directory -Force $installDir | Out-Null
    # A running service holds its files; stop it before replacing them.
    Get-Process -Name 'perfr' -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$installDir*" } | ForEach-Object {
        Say "stopping perfr (pid $($_.Id)) in $installDir"
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

perfr is installed. Start it and open the viewer in a browser:

  perfr
  # then open http://127.0.0.1:44766/

Manual scopes and Frida function hooks need no privilege. Call-stack
sampling, the scheduler and thread states come from the Windows kernel
logger (ETW), which needs an elevated prompt: start perfr from a
terminal opened with "Run as administrator" to record them. Auto-profiling
and the kernel hook engine are Linux only.

To instrument your own code: include\perfr.h is beside perfr
(nothing to link; it finds perfr_api.dll there), or use the perfr-api crate
or the perfr_api.py module.
"@
