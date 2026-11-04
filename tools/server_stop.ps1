# server_stop.ps1: graceful stop of a dc-lobby-server through its admin endpoint (POST /stop, LG-130), then wait
# until it is gone. PowerShell 5.1 compatible. See docs/SERVER.md.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\server_stop.ps1 [-Admin 127.0.0.1:10090] [-Token t]
#
# -Wait   seconds to wait for the server to exit (default 60; the server itself waits --stop-grace for its games)
param(
    [string]$Admin = '127.0.0.1:10090',
    [string]$Token = '',
    [int]$Wait = 60
)
$ErrorActionPreference = 'Stop'
$h = @{}
if ($Token -ne '') { $h['X-Admin-Token'] = $Token }
$base = "http://" + $Admin

try {
    $s = Invoke-RestMethod -Uri ($base + "/status") -Headers $h -TimeoutSec 3
} catch {
    Write-Host ("no server answers on " + $base + " (already stopped?)")
    exit 0
}
$games = @($s.rooms | Where-Object { $null -ne $_.game }).Count
Write-Host ("stopping: " + $s.online + " online, " + @($s.rooms).Count + " room(s), " + $games + " match(es) running")
try {
    $r = Invoke-RestMethod -Uri ($base + "/stop") -Method Post -Headers $h -TimeoutSec 5
    Write-Host ("server: " + $r.message)
} catch {
    Write-Host ("stop request failed: " + $_.Exception.Message)
    exit 1
}

$deadline = (Get-Date).AddSeconds($Wait)
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 500
    try {
        Invoke-RestMethod -Uri ($base + "/status") -Headers $h -TimeoutSec 1 | Out-Null
    } catch {
        Write-Host "stopped"
        exit 0
    }
}
Write-Host ("the server still answers after " + $Wait + " s")
exit 1
