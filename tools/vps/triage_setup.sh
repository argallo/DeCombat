#!/usr/bin/env bash
# triage_setup.sh: once, as root on the VPS, after deploy_setup.sh and after you wrote deploy/triage.conf by hand: the
# Discord bug-report poller (docs/DEPLOY_FROM_ADMIN.md "Bug reports from Discord"). Provenance: ORIGINAL (DeCombat
# operator tooling). WRITTEN ON WINDOWS, NOT RUN ON LINUX YET.
#
#   sudo bash tools/vps/triage_setup.sh          install and enable the timer, run one poll
#   sudo bash tools/vps/triage_setup.sh --check  the timer's state, the conf's shape, the bot's view of the channel
#
# Needs /opt/decombat/deploy/triage.conf (root 0600), two lines: token=<bot token> and channel_id=<numeric id>. The setup
# never asks for the token and never writes it: you write the file yourself over an interactive ssh session. Makes, and
# keeps on a second run:
#   /opt/decombat/deploy/triage/             root:decombat 0750: reports.jsonl, state.json (root writes, the lobby reads)
#   /opt/decombat/deploy/triage/done.json    decombat 0600: the reports the dashboard marked done (the one file the lobby writes)
#   decombat-lobby.service.d/22-triage.conf  ReadWritePaths for done.json
#   decombat-triage.{service,timer}          root, every 5 minutes: discord_triage.sh poll
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

D=${DC_DEPLOY_DIR:-$DC_ROOT/deploy}
T=$D/triage
CONF=$D/triage.conf
SYSTEMCTL=${DC_TRIAGE_SYSTEMCTL:-systemctl}
CHECK=0

while [ $# -gt 0 ]; do
    case "$1" in
        --check) CHECK=1; shift ;;
        -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
        *) die "unknown option: $1 (--help)" ;;
    esac
done
need_root

if [ $CHECK -eq 1 ]; then
    "$SYSTEMCTL" --no-pager status decombat-triage.timer 2>/dev/null | sed -n '1,8p' || true
    bash "$DC_ROOT/tools/discord_triage.sh" setup-check
    bash "$DC_ROOT/tools/discord_triage.sh" status
    exit 0
fi

command -v curl >/dev/null 2>&1 || [ -n "${DC_TRIAGE_CURL:-}" ] || die "curl is missing: apt-get install curl"
command -v "$DC_PYTHON" >/dev/null 2>&1 || die "$DC_PYTHON is missing"
[ "$DC_TEST" = 1 ] || id "$DC_USER" >/dev/null 2>&1 || die "no user $DC_USER: run bootstrap.sh first"
[ -d "$D" ] || die "no deploy folder $D: run deploy_setup.sh first"
[ -f "$CONF" ] || die "no $CONF. Write it yourself, as root over an interactive ssh session (not through a script, not in a command line that keeps the token in the shell history): two lines, token=<the bot token> and channel_id=<the channel's number>; then chmod 600 $CONF and run this again (docs/DEPLOY_FROM_ADMIN.md \"Bug reports from Discord\")"
chmod 0600 "$CONF" 2>/dev/null || [ "$DC_TEST" = 1 ]
[ "$DC_TEST" = 1 ] || chown root:root "$CONF"

# ---- the scripts the units run (a deploy keeps them current; this works from a bare checkout too)
for f in discord_triage.sh triage_setup.sh common.sh; do
    [ -f "$HERE/$f" ] || die "no $HERE/$f"
    [ "$HERE/$f" -ef "$DC_ROOT/tools/$f" ] && continue
    mkdir -p "$DC_ROOT/tools"
    install -m 0755 "$HERE/$f" "$DC_ROOT/tools/$f"
    sed -i 's/\r$//' "$DC_ROOT/tools/$f"
done

# ---- the conf's shape, then the bot's view of the channel, before anything is enabled
bash "$DC_ROOT/tools/discord_triage.sh" setup-check || die "fix $CONF (or the bot's rights in the channel) and run this again; nothing was enabled"

# ---- folders: root writes, the lobby reads; done.json is the one file the lobby writes
dir_mode 0750 "$T"
[ "$DC_TEST" = 1 ] || chown root:"$DC_USER" "$T"
if [ ! -f "$T/done.json" ]; then
    printf '{"done": {}}\n' > "$T/done.json"
fi
chmod 0600 "$T/done.json" 2>/dev/null || [ "$DC_TEST" = 1 ]
[ "$DC_TEST" = 1 ] || chown "$DC_USER:$DC_USER" "$T/done.json"

# ---- units
mkdir -p "$DC_SYSTEMD_DIR/$DC_UNIT.service.d"
cat > "$DC_SYSTEMD_DIR/$DC_UNIT.service.d/22-triage.conf" <<UNIT
# triage_setup.sh: the lobby may write the list of reports marked done (docs/DEPLOY_FROM_ADMIN.md); "-": no error without it
[Service]
ReadWritePaths=-$T/done.json
UNIT
cat > "$DC_SYSTEMD_DIR/decombat-triage.service" <<UNIT
[Unit]
Description=DeCombat: read the new bug reports of the Discord channel
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
Environment=HOME=/root
ExecStart=/usr/bin/env bash $DC_ROOT/tools/discord_triage.sh poll
TimeoutStartSec=5min
Nice=10
PrivateTmp=yes
NoNewPrivileges=yes
UNIT
cat > "$DC_SYSTEMD_DIR/decombat-triage.timer" <<UNIT
[Unit]
Description=DeCombat: look for new bug reports every 5 minutes

[Timer]
OnBootSec=2min
OnUnitActiveSec=5min
AccuracySec=30s

[Install]
WantedBy=timers.target
UNIT
"$SYSTEMCTL" daemon-reload
"$SYSTEMCTL" enable --now decombat-triage.timer

# ---- one poll now (a failure is in state.json and in the dashboard, not a failed setup)
bash "$DC_ROOT/tools/discord_triage.sh" poll || say "the first poll failed: see above and $T/state.json"

say ""
say "Ready. The timer polls every 5 minutes; by hand: sudo bash $DC_ROOT/tools/discord_triage.sh poll | status"
say "The lobby needs one restart to see the drop-in (the next deploy does it): sudo systemctl restart $DC_UNIT"
say "The reports: $T/reports.jsonl, and the dashboard's Updates tab."
