#!/usr/bin/env bash
# DeCombat Vanilla++ on this Mac (DC_DEFAULTS.md MAP-67 .. MAP-69, MAP-89): a vanilla lobby whose one featured entry
# "Vanilla++" carries a community map as its curated set, its game server and the client on 127.0.0.1 playing that map.
# A thin wrapper over `dc-mapkit play`, which is the cross-platform path (it exports the project, writes the lobby's
# server.toml under the profile folder, picks free ports, starts the three binaries and stops them); this script only
# picks the binaries and the project.
#
# Usage: tools/vanilla_plus_mac.sh [project dir] [more `dc-mapkit play` flags: --headless --seconds N --keep-profile --modded]
#   `--modded` plays a modded lobby with the zip as its lobby-wide set instead of the entry.
#   Without a project dir the starter template is scaffolded into ./local_play/starter_map (the source of the
#   committed example examples/mods/starter_map; its export is the same package) and played.
# Environment: CA_ROM (the 2009 client folder; or `--rom DIR` among the flags), DC_BIN (folder of dc-mapkit,
#   dc-lobby-server, dc-dserver and dc-client; default target/release, else target/debug of this checkout),
#   DC_LOCAL_DIR (where the scaffolded project goes; default ./local_play).
# Logs: <project>/build/play/{lobby.log, lobby.out, dserver_*.log, client.log, client.net.jsonl}.
set -eu

HERE="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "${DC_BIN:-}" ]; then
  if [ -x "$HERE/target/release/dc-mapkit" ]; then DC_BIN="$HERE/target/release"; else DC_BIN="$HERE/target/debug"; fi
fi
for x in dc-mapkit dc-lobby-server dc-dserver dc-client; do
  if [ ! -x "$DC_BIN/$x" ]; then
    echo "$DC_BIN/$x is missing: cargo build -p dc-mapkit -p dc-lobby-server -p dc-dserver -p dc-client (or set DC_BIN)" >&2
    exit 2
  fi
done

PROJECT=""
if [ -n "${1:-}" ] && [ "${1#-}" = "$1" ]; then
  PROJECT="$1"
  shift
fi
if [ -z "$PROJECT" ]; then
  DIR="${DC_LOCAL_DIR:-$HERE/local_play}"
  PROJECT="$DIR/starter_map"
  if [ ! -f "$PROJECT/map.json" ]; then
    mkdir -p "$DIR"
    "$DC_BIN/dc-mapkit" new "$PROJECT" --template starter --name "Starter Map" --author "DeCombat" --seed 1
  fi
fi

if [ -z "${CA_ROM:-}" ] && ! printf '%s\n' "$@" | grep -q -- '^--rom$'; then
  echo "set CA_ROM to your 2009 Combat Arms client folder (or pass --rom DIR)" >&2
  exit 2
fi

echo "Vanilla++: dc-mapkit play $PROJECT (binaries in $DC_BIN)"
exec "$DC_BIN/dc-mapkit" play "$PROJECT" --binaries "$DC_BIN" "$@"
