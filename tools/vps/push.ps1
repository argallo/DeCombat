# push.ps1: upload a DeCombat server tarball and / or a server ROM from Windows to the VPS and install them there
# (docs/VPS.md). PowerShell 5.1 compatible; uses the built-in OpenSSH client (ssh.exe, scp.exe) and tar.exe, no
# rsync. WRITTEN AND PARSE-CHECKED ON WINDOWS, NOT RUN AGAINST A VPS YET.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\vps\push.ps1 -Host <VPS address> -Key <key file> `
#       [-User root] [-Port 22] [-Tarball <decombat-servers-...tar.gz>] [-Rom <server ROM folder>] `
#       [-Bootstrap [-PublicAddress <VPS public IPv4>] [-ApplyFirewall]] [-NoRestart] [-Yes] [-DryRun]
#
# -Host       VPS address or name (alias of -Server; nothing is stored)
# -Key        SSH private key file (ssh -i)
# -User       SSH login (default root; another user needs sudo, which may ask for its password)
# -Port       SSH port (default 22)
# -Tarball    the server tarball (CI artifact decombat-servers-linux-x86_64-<commit>.tar.gz)
# -Rom        a server ROM folder made by dc-server-rom (about 650 MB): packed with tar.exe into a temporary
#             .tgz, uploaded, unpacked and checked with dc-server-rom --verify on the VPS. Asks first unless -Yes.
# -Bootstrap  run tools/vps/bootstrap.sh on the VPS first (user, folders, config, systemd unit); with -PublicAddress
#             it writes the advertised address, with -ApplyFirewall it adds the ufw rules (never enables ufw)
# -NoRestart  install without restarting the service
# -DryRun     print the commands instead of running them
# The scripts and example files are always uploaded from this checkout to ~/decombat-upload on the VPS.
param(
    [Alias('Host')][Parameter(Mandatory = $true)][string]$Server,
    [Parameter(Mandatory = $true)][string]$Key,
    [string]$User = 'root',
    [int]$Port = 22,
    [string]$Tarball = '',
    [string]$Rom = '',
    [switch]$Bootstrap,
    [string]$PublicAddress = '',
    [switch]$ApplyFirewall,
    [switch]$NoRestart,
    [switch]$Yes,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$stageName = 'decombat-upload'

function Fail([string]$msg) { Write-Host ("error: " + $msg); exit 2 }

function Invoke-Native([string]$exe, [string[]]$argv) {
    Write-Host ("> " + $exe + " " + ($argv -join ' '))
    if ($DryRun) { return }
    & $exe @argv
    if ($LASTEXITCODE -ne 0) { Fail ($exe + " exited with " + $LASTEXITCODE) }
}

foreach ($tool in @('ssh.exe', 'scp.exe', 'tar.exe')) {
    if ($null -eq (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Fail ($tool + " not found: install the Windows OpenSSH client (Settings > Apps > Optional features)")
    }
}
if (-not (Test-Path -LiteralPath $Key)) { Fail ("key file not found: " + $Key) }
if ($Tarball -eq '' -and $Rom -eq '' -and -not $Bootstrap) { Fail "nothing to do: give -Tarball, -Rom and / or -Bootstrap" }
if ($Tarball -ne '') {
    if (-not (Test-Path -LiteralPath $Tarball)) { Fail ("tarball not found: " + $Tarball) }
    $Tarball = (Resolve-Path -LiteralPath $Tarball).Path
    if ($Tarball -notmatch '\.(tar\.gz|tgz)$') { Fail "-Tarball must be the .tar.gz of the CI artifact" }
}
if ($Rom -ne '') {
    if (-not (Test-Path -LiteralPath (Join-Path $Rom 'server-rom.json'))) {
        Fail ($Rom + " has no server-rom.json: give a folder made by dc-server-rom (docs/SERVER_ROM.md), not the client folder")
    }
    $Rom = (Resolve-Path -LiteralPath $Rom).Path
}
if (($PublicAddress -ne '' -or $ApplyFirewall) -and -not $Bootstrap) { Fail "-PublicAddress and -ApplyFirewall belong to -Bootstrap" }

$sshOpts = @('-i', $Key, '-p', "$Port", '-o', 'StrictHostKeyChecking=accept-new')
$scpOpts = @('-i', $Key, '-P', "$Port", '-o', 'StrictHostKeyChecking=accept-new')
$target = $User + '@' + $Server
$sudo = ''
if ($User -ne 'root') { $sudo = 'sudo ' }

# 1. local staging folder: tools/vps + examples, with Unix line ends (a CRLF checkout would break bash and systemd)
$local = Join-Path ([IO.Path]::GetTempPath()) ($stageName + '-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
$stage = Join-Path $local $stageName
New-Item -ItemType Directory -Force (Join-Path $stage 'tools\vps') | Out-Null
New-Item -ItemType Directory -Force (Join-Path $stage 'examples') | Out-Null
$utf8 = New-Object System.Text.UTF8Encoding($false)
function Copy-Lf([string]$src, [string]$dst) {
    $text = [IO.File]::ReadAllText($src) -replace "`r`n", "`n"
    [IO.File]::WriteAllText($dst, $text, $utf8)
}
foreach ($f in @('common.sh', 'bootstrap.sh', 'install.sh', 'rom_check.sh', 'status.sh', 'bots.sh')) {
    Copy-Lf (Join-Path $repo ('tools\vps\' + $f)) (Join-Path $stage ('tools\vps\' + $f))
}
foreach ($f in @('server-vps.toml', 'decombat-lobby.service', 'decombat-bots.service', 'decombat.env')) {
    Copy-Lf (Join-Path $repo ('examples\' + $f)) (Join-Path $stage ('examples\' + $f))
}

$romTgz = ''
try {
    # 2. the server ROM: size, confirmation, pack
    if ($Rom -ne '') {
        $files = Get-ChildItem -LiteralPath $Rom -Recurse -File
        $bytes = ($files | Measure-Object -Property Length -Sum).Sum
        Write-Host ("server ROM " + $Rom + ": " + $files.Count + " files, " + [Math]::Round($bytes / 1MB, 1) + " MiB")
        Write-Host "It is the game's own data from your 2009 client: it goes only to your VPS (docs/VPS.md, security)."
        if (-not $Yes -and -not $DryRun) {
            $answer = Read-Host "upload it to $Server ? (y/N)"
            if ($answer -notmatch '^(y|yes)$') { Fail "server ROM upload cancelled" }
        }
        $romTgz = Join-Path $local 'server-rom.tgz'
        Invoke-Native 'tar.exe' @('-czf', $romTgz, '-C', $Rom, '.')
        if (-not $DryRun) {
            Write-Host ("packed: " + [Math]::Round((Get-Item -LiteralPath $romTgz).Length / 1MB, 1) + " MiB")
        }
    }

    # 3. upload: the staging folder (scripts, examples), then the tarball and the ROM archive into it
    Invoke-Native 'ssh.exe' ($sshOpts + @($target, "rm -rf ~/$stageName/tools ~/$stageName/examples && mkdir -p ~/$stageName"))
    Invoke-Native 'scp.exe' ($scpOpts + @('-r', (Join-Path $stage 'tools'), (Join-Path $stage 'examples'), ($target + ":$stageName/")))
    $remoteArgs = @()
    if ($Tarball -ne '') {
        $name = Split-Path -Leaf $Tarball
        Invoke-Native 'scp.exe' ($scpOpts + @($Tarball, ($target + ":$stageName/" + $name)))
        $remoteArgs += "~/$stageName/$name"
    }
    if ($romTgz -ne '') {
        Invoke-Native 'scp.exe' ($scpOpts + @($romTgz, ($target + ":$stageName/server-rom.tgz")))
        $remoteArgs += @('--rom', "~/$stageName/server-rom.tgz")
    }
    if ($NoRestart) { $remoteArgs += '--no-restart' }

    # 4. run the scripts on the VPS (-t: sudo may ask for a password)
    $cmds = @()
    if ($Bootstrap) {
        $b = $sudo + "bash ~/$stageName/tools/vps/bootstrap.sh"
        if ($PublicAddress -ne '') { $b = $b + " --public-ip " + $PublicAddress }
        if ($ApplyFirewall) { $b = $b + " --apply-firewall" }
        $cmds += $b
    }
    if ($remoteArgs.Count -gt 0) {
        $cmds += ($sudo + "bash ~/$stageName/tools/vps/install.sh " + ($remoteArgs -join ' '))
    }
    if ($romTgz -ne '') { $cmds += "rm -f ~/$stageName/server-rom.tgz" }
    $cmds += ($sudo + "bash ~/$stageName/tools/vps/status.sh --lines 15")
    Invoke-Native 'ssh.exe' ($sshOpts + @('-t', $target, ($cmds -join ' && ')))
    Write-Host "done. On the VPS: sudo bash /opt/decombat/tools/status.sh"
}
finally {
    Remove-Item -LiteralPath $local -Recurse -Force -ErrorAction SilentlyContinue
}
