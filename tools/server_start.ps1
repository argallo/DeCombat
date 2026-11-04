# server_start.ps1: start dc-lobby-server on its own (no client), wait until its admin endpoint answers.
# PowerShell 5.1 compatible. See docs/SERVER.md. Stop it with tools/server_stop.ps1.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\server_start.ps1 -BinDir C:\DeCombat -Config C:\DeCombat\server.toml
#
# -BinDir   folder holding dc-lobby-server.exe and dc-dserver.exe (default: ..\target\release next to this script)
# -Config   server.toml (default: ..\examples\server.toml)
# -Admin    admin endpoint address (default 127.0.0.1:10090)
# -Token    admin token (optional; the same value must be given to server_stop.ps1)
# -WorkDir  working directory of the server: relative paths of server.toml start here (default: the folder of -Config)
# -Extra    more dc-lobby-server flags as one string, for example "--add-account alice:pw --stop-grace 20"
param(
    [string]$BinDir = (Join-Path $PSScriptRoot '..\target\release'),
    [string]$Config = (Join-Path $PSScriptRoot '..\examples\server.toml'),
    [string]$Admin = '127.0.0.1:10090',
    [string]$Token = '',
    [string]$WorkDir = '',
    [string]$Extra = ''
)
$ErrorActionPreference = 'Stop'

function Get-Status {
    $h = @{}
    if ($Token -ne '') { $h['X-Admin-Token'] = $Token }
    try {
        return Invoke-RestMethod -Uri ("http://" + $Admin + "/status") -Headers $h -TimeoutSec 2
    } catch {
        return $null
    }
}

$lobby = Join-Path $BinDir 'dc-lobby-server.exe'
$dserver = Join-Path $BinDir 'dc-dserver.exe'
foreach ($f in @($lobby, $dserver, $Config)) {
    if (-not (Test-Path $f)) { Write-Error ("not found: " + $f); exit 2 }
}
$Config = (Resolve-Path $Config).Path
if ($WorkDir -eq '') { $WorkDir = Split-Path -Parent $Config }

if ($null -ne (Get-Status)) {
    Write-Host ("a server already answers on http://" + $Admin + "/status; not starting another one")
    exit 1
}

$logDir = Join-Path $WorkDir 'logs'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Force $logDir | Out-Null }
$stamp = Get-Date -Format 'yyyyMMdd_HHmmss'
$out = Join-Path $logDir ("lobby_" + $stamp + ".out")
$err = Join-Path $logDir ("lobby_" + $stamp + ".err")

$argList = '--config "' + $Config + '" --dserver --dserver-exe "' + $dserver + '" --admin ' + $Admin
if ($Token -ne '') { $argList = $argList + ' --admin-token "' + $Token + '"' }
if ($Extra -ne '') { $argList = $argList + ' ' + $Extra }

Write-Host ("starting: " + $lobby + " " + $argList)
$p = Start-Process -FilePath $lobby -ArgumentList $argList -WorkingDirectory $WorkDir -WindowStyle Hidden `
    -RedirectStandardOutput $out -RedirectStandardError $err -PassThru
Set-Content -Path (Join-Path $logDir 'lobby.pid') -Value $p.Id -Encoding ascii

$deadline = (Get-Date).AddSeconds(30)
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 500
    if ($p.HasExited) {
        Write-Host ("the server exited at start-up with code " + $p.ExitCode + "; see " + $out + " and " + $err)
        Get-Content $out -Tail 20
        Get-Content $err -Tail 20
        exit 1
    }
    $s = Get-Status
    if ($null -ne $s) {
        Write-Host ("running: pid " + $p.Id + ", auth port " + $s.address.auth_port + ", db " + $s.db + ", rom " + $s.rom)
        Write-Host ("status:  http://" + $Admin + "/status    stop: tools\server_stop.ps1")
        Write-Host ("output:  " + $out)
        exit 0
    }
}
Write-Host ("no answer from http://" + $Admin + "/status after 30 s (pid " + $p.Id + "); see " + $out)
exit 1
