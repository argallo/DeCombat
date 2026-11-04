#!/usr/bin/env bash
# rom_check.sh: check a server ROM on the VPS against its manifest (docs/VPS.md, docs/SERVER_ROM.md). WRITTEN ON
# WINDOWS, NOT RUN ON LINUX YET.
#
#   bash rom_check.sh [ROM_DIR]            (default /opt/decombat/rom)
#
# Runs `dc-server-rom --verify <ROM_DIR>` (dc-rez, crates/dc-rez/src/bin/dc-server-rom.rs): identify_rom checks the
# manifest's build sentinels and dc_rom::server_rom::verify hashes every file and every kept archive entry against
# the SHA-256 and size in server-rom.json; exit 0 = every file matches. Then checks that the service user can read
# the folder. Needs the binaries installed (install.sh <tarball>).
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

case "${1:-}" in -h|--help) sed -n '2,10p' "$0"; exit 0 ;; esac
ROM=${1:-$DC_ROOT/rom}
TOOL="$DC_ROOT/bin/dc-server-rom"

[ -d "$ROM" ] || die "no folder $ROM"
[ -f "$ROM/server-rom.json" ] || die "$ROM/server-rom.json missing: not a server ROM (make one with dc-server-rom, docs/SERVER_ROM.md)"
[ -x "$TOOL" ] || die "$TOOL missing: install the server tarball first (install.sh <tarball>)"

files=$(find "$ROM" -type f | wc -l)
size=$(du -sh "$ROM" | cut -f1)
say "server ROM $ROM: $files files, $size"
"$TOOL" --verify "$ROM" || die "dc-server-rom --verify failed: the upload is incomplete or changed; upload it again"

if [ "$(id -u)" -eq 0 ] && id "$DC_USER" >/dev/null 2>&1 && command -v runuser >/dev/null 2>&1; then
    if runuser -u "$DC_USER" -- test -r "$ROM/server-rom.json"; then
        say "readable by $DC_USER"
    else
        die "$DC_USER cannot read $ROM (expected root:$DC_USER, folders 0750, files 0640)"
    fi
fi
