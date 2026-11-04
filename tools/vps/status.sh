#!/usr/bin/env bash
# status.sh: one look at the DeCombat server on the VPS (docs/VPS.md). WRITTEN ON WINDOWS, NOT RUN ON LINUX YET.
#
#   bash status.sh [--instance NAME] [--lines N] [--admin ADDR:PORT]      (sudo for the journal, the log files and
#                                                                     a named instance's decombat.env)
#
# Prints: the installed commit, the systemd unit state, the listening DeCombat sockets, the loopback admin
# endpoint's GET /status (and GET /net when the build has it), the end of the lobby log, and the last game
# server `link` lines (rtt, input jitter, resends per client; the remote-jitter statistics).
# An admin token, if the unit uses one, comes from the environment: DC_ADMIN_TOKEN=... bash status.sh
# --instance NAME (or DC_INSTANCE=NAME): a lobby instance of docs/VPS.md "Several instances on one box": its unit
# decombat-lobby@NAME, its admin endpoint (DC_ADMIN in /opt/decombat/instances/NAME/decombat.env), its logs. Without
# it: the default instance in /opt/decombat (unit decombat-lobby, admin 127.0.0.1:10090), as before.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

LINES=30
while [ $# -gt 0 ]; do
    case "$1" in
        --lines) LINES=${2:?--lines needs a number}; shift 2 ;;
        --admin) DC_ADMIN=${2:?--admin needs ADDR:PORT}; _DC_ADMIN_GIVEN=1; shift 2 ;;
        --instance) DC_INSTANCE=${2:?--instance needs a name}; shift 2 ;;
        -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done

instance_select "$DC_INSTANCE"

section() { printf '\n== %s\n' "$*"; }

section "build"
cat "$DC_ROOT/bin/COMMIT.txt" 2>/dev/null || say "no $DC_ROOT/bin/COMMIT.txt (install.sh not run yet)"

section "unit $DC_UNIT (instance $DC_INSTANCE)"
systemctl --no-pager --lines=0 status "$DC_UNIT" || true

section "listening sockets (TCP $DC_TCP_PORTS, admin $DC_ADMIN, UDP $DC_UDP_RANGE)"
if command -v ss >/dev/null 2>&1; then
    ss -lntup 2>/dev/null | grep -E 'dc-lobby|dc-dserv' || say "(no dc-lobby-server / dc-dserver socket; ss needs root to name processes)"
fi

admin_get() {
    local path=$1
    local hdr=()
    if [ -n "${DC_ADMIN_TOKEN:-}" ]; then hdr=(-H "X-Admin-Token: $DC_ADMIN_TOKEN"); fi
    curl -sS --max-time 3 -w '\n%{http_code}' "${hdr[@]}" "http://$DC_ADMIN$path" 2>&1 || true
}
pretty() {
    if command -v python3 >/dev/null 2>&1; then python3 -m json.tool 2>/dev/null || cat; else cat; fi
}

section "GET /status ($DC_ADMIN)"
out=$(admin_get /status)
code=${out##*$'\n'}
body=${out%$'\n'*}
if [ "$code" = "200" ]; then printf '%s\n' "$body" | pretty; else say "no answer (HTTP $code): $body"; fi

section "GET /net"
out=$(admin_get /net)
code=${out##*$'\n'}
body=${out%$'\n'*}
case "$code" in
    200) printf '%s\n' "$body" | pretty ;;
    404) say "(this build has no /net endpoint)" ;;
    *) say "no answer (HTTP $code)" ;;
esac

section "lobby log: last $LINES lines ($INST_LOGS/dc-lobby-server.log)"
if [ -r "$INST_LOGS/dc-lobby-server.log" ]; then
    tail -n "$LINES" "$INST_LOGS/dc-lobby-server.log"
else
    say "(not readable; run with sudo) journal instead:"
    journalctl -u "$DC_UNIT" -n "$LINES" --no-pager 2>/dev/null || true
fi

section "game servers: last link lines ($INST_LOGS/dserver)"
latest=$(ls -t "$INST_LOGS"/dserver/dserver_*.log 2>/dev/null | head -n 3 || true)
if [ -n "$latest" ]; then
    for f in $latest; do
        say "-- $(basename "$f")"
        grep -E 'link (10 s|summary)' "$f" | tail -n 6 || say "(no link lines)"
    done
else
    say "(no game server log yet, or not readable without sudo)"
fi
