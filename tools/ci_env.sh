#!/usr/bin/env bash
# ci_env.sh: the build settings every release and deploy job shares, written to $GITHUB_ENV (docs/RELEASES.md).
#
#   bash tools/ci_env.sh [--crt-static]
#
# LABEL          what the packages are named after: the tag on a `v*` tag run, else the commit's first 8 digits
# SHORT_SHA      the commit's first 8 digits; DC_SHA overrides GITHUB_SHA (a deploy builds a ref other than its own)
# DC_GIT_COMMIT  what dc-host reports to the lobby in its hello (crates/dc-host/build.rs)
# DC_RELEASE_VERSION  on a `v*` tag run, the tag without its v: what dc-client knows as its version (crates/dc-client/build.rs,
#                LG-322). Other builds have none: they are development builds and skip the lobby's version check.
# RUSTFLAGS      --remap-path-prefix for the runner's home and the checkout, so no build machine path reaches a panic
#                message; --crt-static adds the static C runtime (a Windows player has no build tools)
# Without GITHUB_ENV the lines go to stdout, for a look.
set -euo pipefail
CRT=0
while [ $# -gt 0 ]; do
  case "$1" in
    --crt-static) CRT=1; shift ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done
SHA=${DC_SHA:-${GITHUB_SHA:-}}
case "$SHA" in
  ''|*[!0-9a-f]*) echo "no commit id: set DC_SHA or GITHUB_SHA" >&2; exit 2 ;;
esac
SHORT=${SHA:0:8}
LABEL=$SHORT
if [ "${GITHUB_REF_TYPE:-}" = tag ] && [ -z "${DC_SHA:-}" ]; then
  case "${GITHUB_REF_NAME:-}" in
    v[0-9]*) ;;
    *) echo "a release tag starts with v and a digit, got '${GITHUB_REF_NAME:-}'" >&2; exit 2 ;;
  esac
  case "$GITHUB_REF_NAME" in
    *[!0-9A-Za-z._-]*) echo "a release tag holds only letters, digits and . _ -, got '$GITHUB_REF_NAME'" >&2; exit 2 ;;
  esac
  LABEL=$GITHUB_REF_NAME
fi
HOME_DIR=${USERPROFILE:-$HOME}
WORK=${GITHUB_WORKSPACE:-$PWD}
case "$HOME_DIR$WORK" in
  *' '*) echo "RUSTFLAGS cannot hold a path with a space: $HOME_DIR, $WORK" >&2; exit 2 ;;
esac
# rustc takes the last matching rule, so the checkout's goes last
FLAGS="--remap-path-prefix=$HOME_DIR=/home --remap-path-prefix=$WORK=/src"
[ $CRT -eq 0 ] || FLAGS="-C target-feature=+crt-static $FLAGS"
{
  echo "LABEL=$LABEL"
  echo "SHORT_SHA=$SHORT"
  echo "DC_GIT_COMMIT=$SHORT"
  [ "$LABEL" = "$SHORT" ] || echo "DC_RELEASE_VERSION=${LABEL#v}"
  echo "RUSTFLAGS=$FLAGS"
} >> "${GITHUB_ENV:-/dev/stdout}"
