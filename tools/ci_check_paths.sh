#!/usr/bin/env bash
# ci_check_paths.sh: fails when a built binary still names the build machine's home folder (docs/RELEASES.md).
#
#   bash tools/ci_check_paths.sh [--warn] FILE...
#
# --warn  say so without failing (the lanes where the C code of a dependency may still carry its own path)
set -euo pipefail
WARN=0
if [ "${1:-}" = --warn ]; then WARN=1; shift; fi
[ $# -gt 0 ] || { echo "no file given" >&2; exit 2; }
FOUND=0
for f in "$@"; do
  [ -f "$f" ] || { echo "no such file: $f" >&2; exit 2; }
  hits=$(grep -a -i -o -E '[A-Za-z:/\]*(runneradmin|/Users/runner|/home/runner)[^[:cntrl:]"]{0,60}' "$f" | sort -u | head -n 5 || true)
  if [ -n "$hits" ]; then
    FOUND=1
    if [ $WARN -eq 1 ]; then echo "::warning::$f names a build machine path"; else echo "::error::$f names a build machine path"; fi
    printf '%s\n' "$hits"
  fi
done
[ $FOUND -eq 0 ] || [ $WARN -eq 1 ] || exit 1
