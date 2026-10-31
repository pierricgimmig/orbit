# Copyright (c) 2026 The perfr Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Runs the two perfr tests that need the NT Kernel Logger (an elevated
# token): the perfr-etw live session test and the end-to-end ETW capture
# smoke. Writes everything to tools\windows\elevated-tests.log so a
# non-elevated session can read the outcome. Launch it elevated, e.g.
#
#   Start-Process -Verb RunAs -Wait powershell `
#     -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File', `
#     'C:\Users\pierr\git\perfr\tools\windows\run-elevated-tests.ps1'
#
# (one UAC prompt), or from an "Run as administrator" terminal:
#   powershell -ExecutionPolicy Bypass -File tools\windows\run-elevated-tests.ps1
$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$log = Join-Path $PSScriptRoot 'elevated-tests.log'
"" | Out-File -Encoding utf8 $log
function Log([string]$m) { $m | Tee-Object -Append -FilePath $log }

$elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
Log "elevated: $elevated"
if (-not $elevated) {
    Log "NOT elevated -- the kernel logger will refuse. Re-launch with Run as administrator."
}

$cargo = Join-Path $env:USERPROFILE '.cargo\bin'
$env:PATH = "$cargo;$env:PATH"
Set-Location $root

Log "=== 1/2: perfr-etw live session test (PERFR_ETW_LIVE=1) ==="
$env:PERFR_ETW_LIVE = '1'
& "$cargo\cargo.exe" +1.88.0 test -p perfr-etw --test live -- --nocapture *>&1 | Tee-Object -Append -FilePath $log
Log "etw-live exit: $LASTEXITCODE"
Remove-Item Env:PERFR_ETW_LIVE -ErrorAction SilentlyContinue

Log ""
Log "=== 2/2: end-to-end ETW capture smoke (scheduler, states, samples) ==="
$svc = "$root\target\debug\perfr.exe"
$tgt = "$root\target\release\PerfrTestRust.exe"
if (-not (Test-Path $svc)) { Log "building perfr..."; & "$cargo\cargo.exe" +1.88.0 build -p perfr *>&1 | Tee-Object -Append -FilePath $log }
if (-not (Test-Path $tgt)) { Log "building PerfrTestRust..."; & "$cargo\cargo.exe" +1.88.0 build --release -p perfr-test-rust *>&1 | Tee-Object -Append -FilePath $log }
& python "$root\tools\e2e\windows_etw_smoke.py" --service $svc --target $tgt --require --seconds 3 *>&1 | Tee-Object -Append -FilePath $log
Log "etw-smoke exit: $LASTEXITCODE"
Log "=== done ==="
