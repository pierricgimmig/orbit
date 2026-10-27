# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Builds PerfrTestC.exe with MSVC and perfr.h alone: nothing is linked. At
# run time the header loads perfr_api.dll by the search order in perfr.h; in
# the source tree point PERFR_API_LIB at target\release\perfr_api.dll.
# -Static links perfr_api.lib instead, the shape a program that must not
# LoadLibrary uses. The build.sh beside this is the same for Linux and macOS.
param([switch]$Static)
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
$root = (git rev-parse --show-toplevel)
. "$root/tools/windows/msvc-env.ps1"
$include = "$root/crates/perfr-api/include"
$api = "$root/target/release/perfr_api.dll"
if (-not (Test-Path $api)) {
    Push-Location "$root/rust"
    try { cargo +1.88.0 build --release -p perfr-api; if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' } }
    finally { Pop-Location }
}
$flags = @('/nologo', '/O2', '/Zi', '/W3', '/std:c11', '/utf-8', "/I$include", 'PerfrTestC.c', '/Fe:PerfrTestC.exe', '/Fd:PerfrTestC.pdb')
if ($Static) {
    # The Rust staticlib pulls in these system libraries.
    $libs = @("$root/target/release/perfr_api.lib", 'ws2_32.lib', 'ntdll.lib', 'userenv.lib', 'bcrypt.lib', 'advapi32.lib', 'kernel32.lib')
    & cl.exe @flags /DPERFR_STATIC /link /DEBUG @libs
    if ($LASTEXITCODE -ne 0) { throw 'cl failed' }
    Write-Host "built $PSScriptRoot\PerfrTestC.exe (static, perfr_api.lib linked in)"
} else {
    & cl.exe @flags /link /DEBUG
    if ($LASTEXITCODE -ne 0) { throw 'cl failed' }
    Write-Host "built $PSScriptRoot\PerfrTestC.exe (header-only; loads perfr_api.dll at run time)"
}
} finally { Pop-Location }
