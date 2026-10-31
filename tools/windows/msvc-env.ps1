# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Puts the x64 MSVC toolchain (cl, link, the Windows SDK) on this session's
# PATH, the way a "Developer PowerShell" would. Dot-source it:
#
#   . tools/windows/msvc-env.ps1
#
# Finds Visual Studio 2019 or later through vswhere, imports vcvars64.bat
# into the current session, and does nothing when cl is already there.

if (Get-Command cl.exe -ErrorAction SilentlyContinue) {
    if ((Get-Command cl.exe).Source -like '*.exe') { return }
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) {
    throw 'Visual Studio is not installed (vswhere.exe not found); install the "Desktop development with C++" workload.'
}
$installation = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $installation) {
    throw 'No Visual Studio with the C++ build tools was found; install the "Desktop development with C++" workload.'
}
$vcvars = Join-Path $installation 'VC\Auxiliary\Build\vcvars64.bat'
if (-not (Test-Path $vcvars)) {
    throw "vcvars64.bat is missing under $installation"
}
# Run the batch file in cmd, then copy every variable it set into this session.
$lines = cmd.exe /c "`"$vcvars`" >nul 2>&1 && set"
foreach ($line in $lines) {
    if ($line -match '^([^=]+)=(.*)$') {
        Set-Item -Path ("Env:" + $Matches[1]) -Value $Matches[2]
    }
}
if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
    throw "vcvars64.bat ran but cl.exe is still not on PATH"
}
