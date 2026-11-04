# server_net.ps1: the players' network table of a running dc-lobby-server through its admin endpoint (GET /net,
# LG-185): every player of every running match with the newest 10 s link window its game server measured.
# PowerShell 5.1 compatible. See docs/SERVER.md section "Network".
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\server_net.ps1 [-Admin 127.0.0.1:10090] [-Token t]
#       [-Watch] [-Every 10]
#
# -Watch  print the table again every -Every seconds (Ctrl+C ends); without it the table is printed once
param(
    [string]$Admin = '127.0.0.1:10090',
    [string]$Token = '',
    [switch]$Watch,
    [int]$Every = 10
)
$ErrorActionPreference = 'Stop'
$h = @{}
if ($Token -ne '') { $h['X-Admin-Token'] = $Token }
$base = "http://" + $Admin

function Show-Net {
    try {
        $n = Invoke-RestMethod -Uri ($base + "/net") -Headers $h -TimeoutSec 3
    } catch {
        Write-Host ("no server answers on " + $base + ": " + $_.Exception.Message)
        return $false
    }
    $now = Get-Date -Format 'HH:mm:ss'
    $players = @($n.players)
    Write-Host ("[" + $now + "] " + $n.games + " match(es), " + $players.Count + " player(s)")
    if ($players.Count -eq 0) { return $true }
    $rows = foreach ($p in $players) {
        $has = $null -ne $p.report_age_secs
        [pscustomobject]@{
            nick     = $p.nick
            account  = $p.account
            room     = $p.room
            cl       = $p.client
            'rtt'    = $(if ($has) { $p.rtt_ms.mean } else { '-' })
            'min'    = $(if ($has) { $p.rtt_ms.min } else { '-' })
            'max'    = $(if ($has) { $p.rtt_ms.max } else { '-' })
            'jit'    = $(if ($has) { $p.input_jitter_ms } else { '-' })
            'in/s'   = $(if ($has) { $p.input_rate } else { '-' })
            'gap'    = $(if ($has) { $p.longest_gap_ms } else { '-' })
            'resend' = $(if ($has) { $p.resends } else { '-' })
            'drop'   = $(if ($has) { $p.flow_drops } else { '-' })
            'dg in'  = $(if ($has) { $p.datagrams_in } else { '-' })
            'dg out' = $(if ($has) { $p.datagrams_out } else { '-' })
            'age'    = $(if ($has) { $p.report_age_secs } else { '-' })
            'tcp s'  = $p.tcp_age_secs
        }
    }
    $rows | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
    return $true
}

if (-not $Watch) {
    if (Show-Net) { exit 0 } else { exit 1 }
}
while ($true) {
    Show-Net | Out-Null
    Start-Sleep -Seconds $Every
}
