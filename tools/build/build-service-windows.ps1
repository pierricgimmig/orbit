# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Builds the service, the embedded viewer, the Frida files and the manual
# instrumentation SDK on Windows x64, into dist\windows\. The twin of
# tools/build/build-service-macos.sh and rust\build-service-musl.sh.
#
#   .\tools\build\build-service-windows.ps1            # dist\windows
#   .\tools\build\build-service-windows.ps1 -Output d  # elsewhere
#
# Runs from the repository root, where .cargo\config.toml adds the
# static-CRT rustflags to every cargo invocation.
#
# Needs Rust 1.88.0 (rustup), Visual Studio 2022 with the C++ workload (for
# the Frida adapters, through the cc crate) and network access the first time
# (the Frida devkits). Every binary links the C runtime statically
# (.cargo\config.toml), so the result depends on system DLLs only; the
# script checks that with dumpbin.
param([string]$Output = 'dist\windows')
$ErrorActionPreference = 'Stop'
Push-Location (Join-Path $PSScriptRoot '..\..')
try {
    if (-not (Test-Path 'crates\orbit-live-viewer\viewer-dist\index.html')) {
        throw 'crates\orbit-live-viewer\viewer-dist\index.html is missing: the service embeds the viewer at build time'
    }
    . .\tools\windows\msvc-env.ps1
    # rustup-init puts the toolchain in %USERPROFILE%\.cargo\bin; the PATH
    # entry is missing on some machines, so look there ourselves.
    if (-not (Get-Command rustup.exe -ErrorAction SilentlyContinue)) {
        $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
        if (-not (Test-Path (Join-Path $cargoBin 'rustup.exe'))) {
            throw "rustup is not on PATH and not in $cargoBin; install Rust from https://rustup.rs"
        }
        $env:PATH = "$cargoBin;$env:PATH"
    }
    $target = 'x86_64-pc-windows-msvc'
    rustup target add --toolchain 1.88.0 $target
    & .\tools\frida\devkits.ps1 $target
    if ($LASTEXITCODE -ne 0) { throw 'Frida devkits' }

    cargo +1.88.0 build --locked --release --target $target -p orbit-service
    if ($LASTEXITCODE -ne 0) { throw 'orbit-service build failed' }
    cargo +1.88.0 build --locked --release --target $target -p orbit-api -p orbit-test-rust -p orbit-frida-agent --features orbit-frida-agent/native
    if ($LASTEXITCODE -ne 0) { throw 'orbit-api / agent build failed' }
    cargo +1.88.0 build --locked --release --target $target -p orbit-frida-helper
    if ($LASTEXITCODE -ne 0) { throw 'orbit-frida-helper build failed' }

    New-Item -ItemType Directory -Force "$Output\include", "$Output\licenses" | Out-Null
    $files = @{
        "target\$target\release\orbit-service.exe" = 'orbit-service.exe'
        "target\$target\release\orbit_service.pdb" = 'orbit_service.pdb'
        "target\$target\release\orbit-frida-helper.exe" = 'orbit-frida-helper.exe'
        "target\$target\release\orbit_frida_agent.dll" = 'orbit_frida_agent.dll'
        "target\$target\release\orbit_api.dll" = 'orbit_api.dll'
        "target\$target\release\orbit_api.dll.lib" = 'orbit_api.dll.lib'
        "target\$target\release\orbit_api.lib" = 'orbit_api.lib'
        "target\$target\release\OrbitTestRust.exe" = 'OrbitTestRust.exe'
        'crates\orbit-api\include\orbit.h' = 'include\orbit.h'
        'crates\orbit-api\python\orbit_api\__init__.py' = 'orbit_api.py'
        'docs\building_windows.md' = 'README.md'
    }
    foreach ($entry in $files.GetEnumerator()) {
        if (-not (Test-Path $entry.Key)) {
            # The service's PDB is the one optional file: its name follows the
            # crate, and a build without line tables has none.
            if ($entry.Value -eq 'orbit_service.pdb') { continue }
            throw "missing $($entry.Key)"
        }
        Copy-Item $entry.Key (Join-Path $Output $entry.Value) -Force
    }
    Copy-Item 'tools\frida\licenses\*' "$Output\licenses\" -Force

    # Static CRT: nothing but system DLLs may be imported.
    foreach ($binary in 'orbit-service.exe', 'orbit-frida-helper.exe', 'orbit_frida_agent.dll', 'orbit_api.dll') {
        $imports = (dumpbin /nologo /dependents (Join-Path $Output $binary) | Select-String -Pattern '^\s+(\S+\.dll)\s*$' | ForEach-Object { $_.Matches[0].Groups[1].Value.ToLowerInvariant() })
        $runtime = $imports | Where-Object { $_ -like 'vcruntime*' -or $_ -like 'msvcp*' -or $_ -like 'msvcr*' -or $_ -like 'api-ms-win-crt-*' -or $_ -eq 'ucrtbase.dll' }
        if ($runtime) { throw "$binary links the C runtime dynamically: $($runtime -join ', ')" }
        Write-Host ("  {0}: {1}" -f $binary, ($imports -join ' '))
    }
    Write-Host "Windows service and SDK in $Output"
} finally {
    Pop-Location
}
