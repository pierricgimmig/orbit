# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Builds OrbitTestCpp.exe with MSVC and orbit.h alone: nothing is linked. At
# run time the header loads orbit_api.dll by the search order in orbit.h; in
# the source tree point ORBIT_API_LIB at target\release\orbit_api.dll.
# -Static links orbit_api.lib instead. The build.sh beside this is the same
# for Linux and macOS.
param([switch]$Static)
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
$root = (git rev-parse --show-toplevel)
. "$root/tools/windows/msvc-env.ps1"
$include = "$root/crates/orbit-api/include"
$api = "$root/target/release/orbit_api.dll"
if (-not (Test-Path $api)) {
    Push-Location "$root/rust"
    try { cargo +1.88.0 build --release -p orbit-api; if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' } }
    finally { Pop-Location }
}
$flags = @('/nologo', '/O2', '/Zi', '/W3', '/EHsc', '/std:c++17', '/utf-8', "/I$include", 'OrbitTestCpp.cpp', '/Fe:OrbitTestCpp.exe', '/Fd:OrbitTestCpp.pdb')
if ($Static) {
    $libs = @("$root/target/release/orbit_api.lib", 'ws2_32.lib', 'ntdll.lib', 'userenv.lib', 'bcrypt.lib', 'advapi32.lib', 'kernel32.lib')
    & cl.exe @flags /DORBIT_STATIC /link /DEBUG @libs
    if ($LASTEXITCODE -ne 0) { throw 'cl failed' }
    Write-Host "built $PSScriptRoot\OrbitTestCpp.exe (static, orbit_api.lib linked in)"
} else {
    & cl.exe @flags /link /DEBUG
    if ($LASTEXITCODE -ne 0) { throw 'cl failed' }
    Write-Host "built $PSScriptRoot\OrbitTestCpp.exe (header-only; loads orbit_api.dll at run time)"
}
} finally { Pop-Location }
