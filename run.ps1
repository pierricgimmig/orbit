# Start-Process powershell -Verb runAs
./stop.ps1

devenv /runexit D:\Unity\UnityGame.sln
Wait-Process -Name "UnityGame"

$started = $false
$target_process = "UnityGame"

Do {
    $status = Get-Process $target_process -ErrorAction SilentlyContinue

    If (!($status)) { Write-Host 'Waiting for process to start' ; Start-Sleep -Milliseconds 200 }
    
    Else { Write-Host ' Process has started ', $target_process ; $started = $true }

}
Until ( $started )


Start-Process -FilePath 'build_default_relwithdebinfo\bin\OrbitService'
Start-Process -FilePath 'build_default_relwithdebinfo\bin\Orbit' -ArgumentList "--local --process_name=D:\Unity\UnityGame.exe"