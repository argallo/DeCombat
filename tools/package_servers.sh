#!/usr/bin/env bash
# package_servers.sh: packs built servers into the tarball tools/vps/install.sh and tools/host/update.sh install. The
# one packing recipe of .github/workflows/release.yml and deploy-servers.yml. docs/RELEASES.md.
#
#   tools/package_servers.sh --bin-dir DIR --name NAME [--out DIR] [--commit SHA] [--run URL]
#
# --bin-dir  the folder with the six binaries (target/x86_64-unknown-linux-gnu/release)
# --name     the tarball's base name; its one top folder has the same name
# --out      where the tarball goes (default dist/)
# --commit   the full commit id for COMMIT.txt (default: DC_SHA, GITHUB_SHA, or this checkout's HEAD)
# --run      a link to the build, for COMMIT.txt
# Inside: bin/, tools/vps/*.sh, tools/host/*.sh, examples/ (config template and units), COMMIT.txt, SHA256SUMS of bin/.
# Prints the tarball's path. No config, no token and no ROM ever goes in.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
BIN=""; NAME=""; OUT="dist"; SHA="${DC_SHA:-${GITHUB_SHA:-}}"; RUN=""
while [ $# -gt 0 ]; do
  case "$1" in
    --bin-dir) BIN=$2; shift 2 ;;
    --name) NAME=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --commit) SHA=$2; shift 2 ;;
    --run) RUN=$2; shift 2 ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "unknown option $1 (--help)" >&2; exit 2 ;;
  esac
done
[ -n "$BIN" ] && [ -n "$NAME" ] || { echo "--bin-dir and --name are required (--help)" >&2; exit 2; }
[ -n "$SHA" ] || SHA=$(git -C "$HERE" rev-parse HEAD)
STRIP=${DC_PACKAGE_STRIP:-strip}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
PKG="$OUT/$NAME"
rm -rf "${OUT:?}/${NAME:?}"
mkdir -p "$PKG/bin" "$PKG/tools/vps" "$PKG/tools/host" "$PKG/examples"
for b in dc-lobby-server dc-dserver dc-server-rom dc-botclient dc-config dc-host; do
  [ -f "$BIN/$b" ] || { echo "no $BIN/$b" >&2; exit 1; }
  cp "$BIN/$b" "$PKG/bin/$b"
  chmod 0755 "$PKG/bin/$b"
  "$STRIP" "$PKG/bin/$b"
done
cp "$HERE"/tools/vps/*.sh "$HERE"/tools/vps/*.py "$PKG/tools/vps/"
cp "$HERE"/tools/host/*.sh "$PKG/tools/host/"
for f in server-vps.toml decombat-lobby.service decombat-lobby@.service decombat-bots.service decombat-host.service decombat.env decombat-hardening.conf decombat-backup.service decombat-backup.timer decombat-health.service decombat-health.timer decombat-live-games.service decombat-live-games.timer backup.conf alerts.conf; do
  cp "$HERE/examples/$f" "$PKG/examples/"
done
{
  echo "commit: $SHA"
  [ -z "$RUN" ] || echo "run: $RUN"
  echo "built: $(date -u +%Y-%m-%dT%H:%M:%SZ) on $(lsb_release -ds 2>/dev/null || uname -sr)"
  echo "target: x86_64-unknown-linux-gnu, glibc $(ldd --version 2>/dev/null | head -n 1 | awk '{print $NF}')"
  echo "rustc: $(rustc -V 2>/dev/null || echo unknown)"
} > "$PKG/COMMIT.txt"
(cd "$PKG" && sha256sum bin/* > SHA256SUMS)
tar -czf "$OUT/$NAME.tar.gz" -C "$OUT" "$NAME"
rm -rf "${OUT:?}/${NAME:?}"
echo "$OUT/$NAME.tar.gz"
