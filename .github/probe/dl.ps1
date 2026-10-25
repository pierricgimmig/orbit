# Bounded download attempt plus a snapshot of what the job left behind.
param([string]$Tag)
$ErrorActionPreference = 'Continue'
$uri = 'https://github.com/frida/frida/releases/download/17.17.0/frida-core-devkit-17.17.0-windows-x86_64.tar.xz'
$out = Join-Path $env:RUNNER_TEMP "dl-$Tag.bin"
Write-Host "--- [$Tag] leftover processes from the workspace"
Get-Process | Where-Object { $_.Path -and $_.Path -like 'D:\a\*' } | Select-Object Id, Name, Path | Format-Table -AutoSize | Out-String | Write-Host
Write-Host "--- [$Tag] ETW sessions"
(logman query -ets 2>&1 | Out-String) | Write-Host
Write-Host "--- [$Tag] established TCP connections from the workspace processes"
$ids = @(Get-Process | Where-Object { $_.Path -and $_.Path -like 'D:\a\*' } | Select-Object -ExpandProperty Id)
if ($ids.Count) { Get-NetTCPConnection -OwningProcess $ids -ErrorAction SilentlyContinue | Select-Object LocalPort, RemoteAddress, RemotePort, State | Format-Table -AutoSize | Out-String | Write-Host }
Write-Host "--- [$Tag] download, max 100 s"
$sw = [Diagnostics.Stopwatch]::StartNew()
& "$env:SystemRoot\System32\curl.exe" -fsSL --connect-timeout 30 --max-time 100 --speed-time 20 --speed-limit 10000 -o $out $uri
Write-Host "[$Tag] curl exit=$LASTEXITCODE after $([int]$sw.Elapsed.TotalSeconds)s size=$((Get-Item $out -ErrorAction SilentlyContinue).Length)"
Remove-Item $out -ErrorAction SilentlyContinue
