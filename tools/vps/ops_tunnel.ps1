# ops_tunnel.ps1: open the lobby's operator dashboard (docs/OPS.md section 15) from Windows: an SSH tunnel to the
# loopback admin port of the VPS, then the browser on http://127.0.0.1:<port>/ops. PowerShell 5.1 compatible; uses
# the built-in OpenSSH client (ssh.exe). Nothing is stored; the tunnel closes when you press Enter.
# DC-DEFAULT: MON-4 (proposed) the dashboard is reached through ssh only
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\vps\ops_tunnel.ps1 -Host <VPS address>
#       [-Key <key file>] [-User root] [-Port 22] [-AdminPort 10090] [-LocalPort <port>] [-Token <admin token>]
#       [-NoBrowser] [-Probe]
#
# -Host       VPS address or name
# -Key        SSH private key file (default: $env:DC_VPS_KEY, else ~\.ssh\decombat_vps when it exists, else ssh's own)
# -User       SSH login (default root; another user needs sudo without a password to read the token)
# -AdminPort  the lobby instance's admin port on the VPS (10090 for the first instance; `status.sh --instance <n>`
#             names another's)
# -LocalPort  the port on this PC (default: the same as -AdminPort)
# -Token      the admin token; default: read over ssh from the instance's decombat.env (DC_ADMIN_TOKEN), never printed
# -NoBrowser  only the tunnel (open http://127.0.0.1:<LocalPort>/ops yourself)
# -Probe      open the tunnel, ask GET /status once, print the HTTP status, close (a check of the helper itself)
param(
    [Alias('Host')][Parameter(Mandatory = $true)][string]$Server,
    [string]$Key = '',
    [string]$User = 'root',
    [int]$Port = 22,
    [int]$AdminPort = 10090,
    [int]$LocalPort = 0,
    [string]$Token = '',
    [string]$EnvFile = '/opt/decombat/config/decombat.env',
    [switch]$NoBrowser,
    [switch]$Probe
)
$ErrorActionPreference = 'Stop'
if ($LocalPort -le 0) { $LocalPort = $AdminPort }
if (-not $Key) {
    if ($env:DC_VPS_KEY) { $Key = $env:DC_VPS_KEY }
    elseif (Test-Path (Join-Path $HOME '.ssh\decombat_vps')) { $Key = Join-Path $HOME '.ssh\decombat_vps' }
}
$sshBase = @('-p', "$Port", '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10')
if ($Key) { $sshBase += @('-i', $Key) }
$target = "$User@$Server"

if (-not $Token -and -not $Probe) {
    # read-only: the token line of the instance's env file (sudo -n: root needs no password, another user must not be asked)
    $cmd = "sudo -n sed -n 's/^DC_ADMIN_TOKEN=//p' $EnvFile 2>/dev/null | head -n 1"
    $Token = (& ssh.exe @sshBase $target $cmd | Out-String).Trim()
}

$fwd = "127.0.0.1:${LocalPort}:127.0.0.1:${AdminPort}"
$sshArgs = $sshBase + @('-N', '-o', 'ExitOnForwardFailure=yes', '-o', 'ServerAliveInterval=30', '-L', $fwd, $target)
$ssh = Start-Process -FilePath 'ssh.exe' -ArgumentList $sshArgs -PassThru -NoNewWindow
try {
    $up = $false
    for ($i = 0; $i -lt 60 -and -not $ssh.HasExited; $i++) {
        try {
            $c = New-Object System.Net.Sockets.TcpClient
            $c.Connect('127.0.0.1', $LocalPort)
            $c.Close()
            $up = $true
            break
        } catch { Start-Sleep -Milliseconds 250 }
    }
    if (-not $up) { throw "the tunnel did not come up (ssh exited: $($ssh.HasExited)); check -Host, -Key and that port $LocalPort is free" }
    $base = "http://127.0.0.1:$LocalPort"
    if ($Probe) {
        try {
            $r = Invoke-WebRequest -UseBasicParsing -TimeoutSec 10 -Uri "$base/status"
            Write-Output "tunnel up: GET /status answered $($r.StatusCode)"
        } catch {
            $code = $_.Exception.Response.StatusCode.value__
            Write-Output "tunnel up: GET /status answered $code"
        }
        return
    }
    $url = "$base/ops"
    if ($Token) { $url = "$url" + '?token=' + [uri]::EscapeDataString($Token) }
    if (-not $NoBrowser) { Start-Process $url }
    Write-Output "dashboard: $base/ops (tunnel to $Server port $AdminPort). Press Enter to close the tunnel."
    [void](Read-Host)
} finally {
    if (-not $ssh.HasExited) { Stop-Process -Id $ssh.Id -Force }
}
