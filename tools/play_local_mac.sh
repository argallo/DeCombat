#!/usr/bin/env bash
# DeCombat: play alone with bots on this Mac (lobby + game server + bots + client on 127.0.0.1).
# A transcription of the friend pack's play_local.bat (the Windows local-play launcher)
# for macOS / unix shells. Written on Windows on 2026-10-29; first run on a Mac the same day (docs/MACOS.md).
#
# Usage: tools/play_local_mac.sh "<2009 client folder>" [name] [bots 0-7] [mode] [map]
# Environment: DC_BIN (folder of the four binaries; default target/release, else target/debug of this checkout),
#   DC_LOCAL_AUTH (first of four TCP ports, default 10000), DC_LOCAL_GAME (UDP game port, default 27888),
#   DC_LOCAL_CLIENT_EXTRA (more dc-client flags), DC_LOCAL_MAX_MINUTES (bot lifetime, default 120),
#   DC_LOCAL_PILOT=0: no pilot; the client opens the 2009 front end at the login screen and you click through the
#   servers, channels, lobby, room creation and start yourself (the bots join the first room they see and get
#   ready; mode and map arguments are ignored). Default 1: the pilot creates and starts a room hands-free.
# Files: server/ca.db (local accounts) and logs/ under DC_LOCAL_DIR (default ./local_play next to the checkout root).
set -u

HERE="$(cd "$(dirname "$0")/.." && pwd)"
ROM="${1:-}"
NICK="${2:-Player}"
BOTS="${3:-3}"
MODE="${4:-}"
MAP="${5:-}"
AUTH="${DC_LOCAL_AUTH:-10000}"
GAME="${DC_LOCAL_GAME:-27888}"
MAX_MIN="${DC_LOCAL_MAX_MINUTES:-120}"
if [ -z "${DC_BIN:-}" ]; then
  if [ -x "$HERE/target/release/dc-client" ]; then DC_BIN="$HERE/target/release"; else DC_BIN="$HERE/target/debug"; fi
fi
DIR="${DC_LOCAL_DIR:-$HERE/local_play}"

ROM="${ROM%/}"
if [ -z "$ROM" ] || [ ! -d "$ROM/Game" ]; then
  echo "usage: $0 \"<2009 Combat Arms client folder (holds Engine.exe and Game)>\" [name] [bots] [mode] [map]" >&2
  exit 2
fi
ROM="$(cd "$ROM" && pwd)"   # absolute: the processes below start from $DIR
for x in dc-client dc-lobby-server dc-dserver dc-botclient; do
  if [ ! -x "$DC_BIN/$x" ]; then
    echo "$DC_BIN/$x is missing: cargo build -p dc-client -p dc-lobby-server -p dc-dserver -p dc-botclient" >&2
    exit 2
  fi
done
case "$BOTS" in ''|*[!0-9]*) BOTS=3 ;; esac
[ "$BOTS" -gt 7 ] && BOTS=7
LOBBY_PORT=$((AUTH + 2))
CLAN_PORT=$((AUTH + 3))
WEB_PORT=$((AUTH + 8080))
MEMBERS=$((BOTS + 1))
BOT_SECS=$((MAX_MIN * 60))
mkdir -p "$DIR/logs" "$DIR/server"
cd "$DIR" || exit 2
LOGS="$DIR/logs"

export CA_ROM="$ROM"
export DC_ROOM__BLAST_DAMAGE_SCALE=2
export DC_MODES__PLAYABLE='["TDM", "TDM_CLAN", "COOP", "FFA", "ELIM", "ELIM_CLAN", "FLAG", "FLAG_CLAN", "BOMB", "BOMB_CLAN", "INFECT", "BOUNTY"]'
export DC_CLANWEB="127.0.0.1:$WEB_PORT"
AUTH_ADDR="127.0.0.1:$AUTH"

PIDS=()
cleanup() {
  for p in "${PIDS[@]:-}"; do
    [ -n "$p" ] || continue
    pkill -TERM -P "$p" 2>/dev/null   # the lobby's dc-dserver children
    kill -TERM "$p" 2>/dev/null
  done
}
trap cleanup EXIT INT TERM

echo "Starting the lobby server on $AUTH_ADDR (logs/lobby.out) ..."
"$DC_BIN/dc-lobby-server" --port "$AUTH" --lobbyport "$LOBBY_PORT" --clanport "$CLAN_PORT" --bind 127.0.0.1 \
  --lobbyip 127.0.0.1 --gameip 127.0.0.1 --gameport "$GAME" --db "$DIR/server/ca.db" --log "$LOGS/lobby.log" \
  --pktlog "$LOGS/lobby.pkt.log" --autochar --clanweb -1 --dserver --dserver-exe "$DC_BIN/dc-dserver" \
  --dserver-logdir "$LOGS" --dserver-verbose "--dserver-args=-tick 30 -bindip 127.0.0.1" \
  --start-exp 225000 --start-gp 500000 --start-nx 100000 --start-nx-credit 100000 \
  >"$LOGS/lobby.out" 2>"$LOGS/lobby.out.err" &
PIDS+=($!)

tries=0
until nc -z 127.0.0.1 "$AUTH" 2>/dev/null; do
  tries=$((tries + 1))
  if [ "$tries" -ge 40 ]; then
    echo "The lobby server did not start listening on port $AUTH; see logs/lobby.out and logs/lobby.out.err" >&2
    exit 3
  fi
  sleep 1
done
echo "Lobby is up (pid ${PIDS[0]})."

i=0
while [ "$i" -lt "$BOTS" ]; do
  i=$((i + 1))
  "$DC_BIN/dc-botclient" --lobby "$AUTH_ADDR" --name "Bot$i" --room any --duration "$BOT_SECS" --match-wait 900 \
    --shooter --hunt --respawn-after 0 --skill 40 --matches 0 --rom "$ROM" --log "$LOGS/Bot$i.events.jsonl" \
    --report "$LOGS/Bot$i.report.json" >"$LOGS/Bot$i.out" 2>"$LOGS/Bot$i.out.err" &
  PIDS+=($!)
  echo "Bot$i started (pid $!)."
done

export DC_PROF_TAIL="$LOGS/dc-client.tail.csv"
if [ "${DC_LOCAL_PILOT:-1}" = "0" ]; then
  # the real front end, driven by you: no pilot, no room flags
  FLOW=(--menu lobby)
  echo "Starting the game: the login screen opens for $NICK; create a room yourself, the $BOTS bots join it."
else
  FLOW=(--room new --start-after "$MEMBERS" --menu lobby --menu-pilot --pilot-keep-result)
  [ -n "$MODE" ] && FLOW+=(--mode "$MODE")
  [ -n "$MAP" ] && FLOW+=(--pilot-map "$MAP")
  echo "Starting the game: you log in as $NICK, a room is created and starts once the $BOTS bots are in."
fi
# shellcheck disable=SC2086
"$DC_BIN/dc-client" --lobby "$AUTH_ADDR" --name "$NICK" --match-wait 900 \
  --rom "$ROM" "${FLOW[@]}" --net-log "$LOGS/dc-client.net.jsonl" --sound-log "$LOGS/dc-client.sound.jsonl" \
  --profile "$LOGS/dc-client.prof.json" ${DC_LOCAL_CLIENT_EXTRA:-}
rc=$?
echo "dc-client ended with code $rc. Logs are in $LOGS."
exit "$rc"
