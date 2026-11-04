#!/usr/bin/env bash
# bots.sh: headless dc-botclient players on the VPS itself (docs/VPS.md "Bots"). WRITTEN ON WINDOWS, NOT RUN ON
# LINUX YET.
#
#   sudo bash bots.sh start N [--prefix NAME] [--args "DC-BOTCLIENT FLAGS"]   N bots in the background (as decombat)
#   sudo bash bots.sh stop                                                     stop them (by their own PIDs)
#   sudo bash bots.sh status                                                   which bots run, the end of their logs
#   bash bots.sh run N [--prefix NAME] [--args "..."]                          foreground (the decombat-bots unit)
#   every command also takes --instance NAME (or DC_INSTANCE=NAME): the bots of that lobby instance (docs/VPS.md
#   "Several instances on one box"): its lobby DC_BOT_LOBBY from /opt/decombat/instances/NAME/decombat.env, its logs
#
# Each bot logs in on the lobby of this host through loopback (127.0.0.1:10000): with the VPS unit's flags a loopback
# login needs no account and no password (LG-32, LG-187), so the bots need no registration. They read the server ROM
# (/opt/decombat/rom, BOT-82), join the first listed room (--room any: a room a player opened), hunt and shoot back,
# keep their seat for every next match (--matches 0) and start again 10 s after they exit (the room ended, the lobby
# restarted). Names NAME1..NAMEn (default VpsBot). Logs: /opt/decombat/logs/bots/<name>.{out,events.jsonl}
# (a named instance: /opt/decombat/instances/NAME/logs/bots/).
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

BOT_BIN="$DC_ROOT/bin/dc-botclient"
# one game session per start; --match-wait: how long a bot waits for a room to appear and for its match to start
BOT_SESSION_S=${DC_BOT_SESSION_S:-3600}

CMD=${1:-}
[ -n "$CMD" ] || { sed -n '2,17p' "$0"; exit 2; }
shift
N=0
if [ "$CMD" = start ] || [ "$CMD" = run ]; then
    N=${1:?"$CMD needs the number of bots"}
    shift
    case "$N" in ''|*[!0-9]*) die "the number of bots must be a number: $N" ;; esac
    [ "$N" -ge 1 ] && [ "$N" -le 7 ] || die "1..7 bots (a room holds 8)"
fi
PREFIX=VpsBot
EXTRA=""
while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) PREFIX=${2:?--prefix needs a name}; shift 2 ;;
        --args) EXTRA=${2:?--args needs flags}; shift 2 ;;
        --instance) DC_INSTANCE=${2:?--instance needs a name}; shift 2 ;;
        -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done
case "$PREFIX" in [A-Za-z]*) ;; *) die "--prefix must start with a letter" ;; esac
case "$PREFIX" in *[!A-Za-z0-9_]*) die "--prefix: letters, digits and _ only" ;; esac

# the instance's lobby (DC_BOT_LOBBY in its decombat.env; the default instance: 127.0.0.1:10000) and log folder
instance_select "$DC_INSTANCE"
BOT_DIR="$INST_LOGS/bots"
BOT_LOBBY=$DC_BOT_LOBBY

# one bot, restarted after every exit until it is stopped
bot_loop() {
    local name=$1
    while :; do
        # BOT-101: the lobby's unpacked packs, for a room on a community map of a featured entry
        local cache=()
        [ ! -d "$DC_ROOT/data/entry_mods/cache" ] || cache=(--mod-cache "$DC_ROOT/data/entry_mods/cache")
        # shellcheck disable=SC2086
        "$BOT_BIN" --lobby "$BOT_LOBBY" --name "$name" --room any --duration "$BOT_SESSION_S" ${cache[@]+"${cache[@]}"} \
            --match-wait "$BOT_SESSION_S" --shooter --hunt --respawn-after 0 --skill "${BOT_SKILL:-30-65}" --matches 0 --filler "$PREFIX" \
            --rom "$DC_ROOT/rom" --log "$BOT_DIR/$name.events.jsonl" $EXTRA >> "$BOT_DIR/$name.out" 2>&1 || true
        sleep 10
    done
}

case "$CMD" in
    run)
        [ -x "$BOT_BIN" ] || die "no $BOT_BIN (install.sh installs it from the server tarball)"
        mkdir -p "$BOT_DIR"
        # started by `bots.sh start` (setsid: this shell leads its own process group): stop reads this pid
        if [ "${DC_BOTS_DETACHED:-0}" = 1 ]; then echo "$$" > "$BOT_DIR/bots.pid"; fi
        # TERM / INT (systemctl stop, bots.sh stop, Ctrl-C): the whole process group, loops and bots together
        trap 'trap - TERM INT; kill -TERM 0 2>/dev/null; exit 0' TERM INT
        for i in $(seq 1 "$N"); do
            bot_loop "$PREFIX$i" &
            say "bot $PREFIX$i (loop pid $!) -> $BOT_LOBBY, log $BOT_DIR/$PREFIX$i.out"
            sleep 1
        done
        wait
        ;;
    start)
        need_root
        [ -x "$BOT_BIN" ] || die "no $BOT_BIN (install.sh installs it from the server tarball)"
        install -d -m 0750 -o "$DC_USER" -g "$DC_USER" "$BOT_DIR"
        if [ -f "$BOT_DIR/bots.pid" ] && kill -0 "$(cat "$BOT_DIR/bots.pid")" 2>/dev/null; then
            die "bots already run (pid $(cat "$BOT_DIR/bots.pid")); bots.sh stop first"
        fi
        rm -f "$BOT_DIR/bots.pid"
        # setsid: a process group of its own, so stop ends the loops and the bots together
        sudo -u "$DC_USER" env DC_BOTS_DETACHED=1 setsid bash "$HERE/bots.sh" run "$N" --prefix "$PREFIX" --instance "$DC_INSTANCE" \
            ${EXTRA:+--args "$EXTRA"} > "$BOT_DIR/bots.out" 2>&1 < /dev/null &
        sleep 2
        [ -f "$BOT_DIR/bots.pid" ] || die "the bots did not start; see $BOT_DIR/bots.out"
        say "started $N bot(s) ${PREFIX}1..${PREFIX}${N} (group $(cat "$BOT_DIR/bots.pid")); logs in $BOT_DIR"
        ;;
    stop)
        need_root
        [ -f "$BOT_DIR/bots.pid" ] || { say "no $BOT_DIR/bots.pid: no bots started by bots.sh start"; exit 0; }
        pid=$(cat "$BOT_DIR/bots.pid")
        if kill -0 "$pid" 2>/dev/null; then
            kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid"
            sleep 3
            kill -KILL -- "-$pid" 2>/dev/null || true
            say "bots stopped (group $pid)"
        else
            say "bots were not running (stale pid $pid)"
        fi
        rm -f "$BOT_DIR/bots.pid"
        ;;
    status)
        if [ -f "$BOT_DIR/bots.pid" ] && kill -0 "$(cat "$BOT_DIR/bots.pid")" 2>/dev/null; then
            say "bots.sh group pid $(cat "$BOT_DIR/bots.pid") running"
        else
            say "no bots.sh group running (the decombat-bots unit: systemctl status decombat-bots)"
        fi
        pgrep -a -f "$BOT_BIN" || say "(no dc-botclient process)"
        for f in "$BOT_DIR"/*.out; do
            [ -f "$f" ] || continue
            printf '\n== %s\n' "$f"
            tail -n 5 "$f"
        done
        ;;
    *)
        die "unknown command: $CMD (start N | stop | status | run N)"
        ;;
esac
