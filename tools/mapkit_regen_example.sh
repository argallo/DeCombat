#!/usr/bin/env bash
# Regenerates examples/mods/starter_map, the committed community map example (DC_DEFAULTS.md MAP-70): the starter
# template of dc-mapkit as `new` makes it (name "Starter Map", author "DeCombat", seed 1), built and exported with
# `--no-rom-paths` (own textures only, CLEANROOM.md section 4), then the EXPORTED mod folder is copied over the
# example (the project itself is not committed; `build/`, `.mapkit/` and the zip stay out).
# macOS / Linux. Windows: run the four dc-mapkit commands by hand and copy build/mod/starter_map/ over the folder.
#
# Usage: tools/mapkit_regen_example.sh [dc-mapkit binary]      (default: cargo run -p dc-mapkit)
# Afterwards: cargo test -p dc-modkit example_starter_map  and  cargo test -p dc-mapkit --test starter_example
set -eu

HERE="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$HERE/examples/mods/starter_map"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/mapkit-starter.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

if [ -n "${1:-}" ]; then
  MAPKIT=("$1")
else
  MAPKIT=(cargo run -q -p dc-mapkit --bin dc-mapkit --manifest-path "$HERE/Cargo.toml" --)
fi

"${MAPKIT[@]}" new "$WORK/starter" --template starter --name "Starter Map" --author "DeCombat" --seed 1
"${MAPKIT[@]}" build "$WORK/starter"
"${MAPKIT[@]}" export "$WORK/starter" --no-rom-paths

rm -rf "$OUT"
mkdir -p "$OUT"
cp -R "$WORK/starter/build/mod/starter_map/." "$OUT/"
echo "regenerated $OUT:"
(cd "$OUT" && find . -type f | sort)
du -sh "$OUT"
