#!/usr/bin/env bash
# install.sh: install a DeCombat server tarball and / or a server ROM on the VPS (docs/VPS.md). Run bootstrap.sh
# once before. WRITTEN ON WINDOWS, NOT RUN ON LINUX YET.
#
#   sudo bash install.sh [TARBALL] [--rom SERVER_ROM.tgz] [--no-restart]
#
# TARBALL  decombat-servers-linux-x86_64-<commit>.tar.gz (the CI artifact, docs/VPS.md "Get the binaries"): its
#          bin/ goes to /opt/decombat/bin, its COMMIT.txt (plus the install time and the tarball's SHA-256) to
#          /opt/decombat/bin/COMMIT.txt; its tools/vps scripts, the systemd units (decombat-lobby.service and the
#          instance template decombat-lobby@.service) and the examples instance.sh reads (/opt/decombat/tools/
#          examples; also the two decombat-live-games units, docs/LIVE_GAMES.md, which nothing installs or enables)
#          are refreshed when present.
# --rom    a tar.gz of a server ROM folder (dc-server-rom, docs/SERVER_ROM.md; tools/vps/push.ps1 -Rom makes it):
#          unpacked next to /opt/decombat/rom, checked with `dc-server-rom --verify`, then swapped in.
# Afterwards every enabled lobby instance is restarted, one after the other (a graceful stop first: running matches
# end and are stored): the default decombat-lobby and each decombat-lobby@NAME of /opt/decombat/instances
# (docs/VPS.md "Several instances on one box"), unless --no-restart.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

TARBALL=""
ROM_TGZ=""
RESTART=1
while [ $# -gt 0 ]; do
    case "$1" in
        --rom) [ $# -ge 2 ] || die "--rom needs a .tgz"; ROM_TGZ=$2; shift 2 ;;
        --rom=*) ROM_TGZ=${1#*=}; shift ;;
        --no-restart) RESTART=0; shift ;;
        -h|--help) sed -n '2,18p' "$0"; exit 0 ;;
        -*) die "unknown option: $1 (see --help)" ;;
        *) [ -z "$TARBALL" ] || die "one tarball at a time"; TARBALL=$1; shift ;;
    esac
done
[ -n "$TARBALL" ] || [ -n "$ROM_TGZ" ] || die "nothing to install: give a tarball and / or --rom <file> (--help)"
need_root
[ -d "$DC_ROOT/bin" ] && id "$DC_USER" >/dev/null 2>&1 || die "run bootstrap.sh first ($DC_ROOT/bin or user $DC_USER missing)"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/decombat-install.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

# ---- binaries
if [ -n "$TARBALL" ]; then
    [ -f "$TARBALL" ] || die "no such file: $TARBALL"
    tar -xzf "$TARBALL" -C "$WORK" --no-same-owner
    LOBBY=$(find "$WORK" -maxdepth 4 -type f -path '*/bin/dc-lobby-server' | head -n 1)
    [ -n "$LOBBY" ] || die "$TARBALL has no bin/dc-lobby-server"
    PKG=$(dirname "$(dirname "$LOBBY")")
    for b in $DC_BINS_REQUIRED; do
        [ -f "$PKG/bin/$b" ] || die "$TARBALL has no bin/$b"
    done
    installed=""
    for b in $DC_BINS_REQUIRED $DC_BINS_OPTIONAL; do
        src="$PKG/bin/$b"
        [ -f "$src" ] || { say "  (no $b in the tarball)"; continue; }
        is_elf "$src" || die "bin/$b is not a Linux (ELF) binary"
        # copy next to the target, then rename: a running lobby keeps its old file
        install -m 0755 -o root -g root "$src" "$DC_ROOT/bin/.$b.new"
        mv -f "$DC_ROOT/bin/.$b.new" "$DC_ROOT/bin/$b"
        installed="$installed $b"
    done
    {
        if [ -f "$PKG/COMMIT.txt" ]; then cat "$PKG/COMMIT.txt"; else echo "commit: unknown (no COMMIT.txt in the tarball)"; fi
        echo "installed: $(date -u +%Y-%m-%dT%H:%M:%SZ) from $(basename "$TARBALL")"
        echo "tarball sha256: $(sha256sum "$TARBALL" | cut -d' ' -f1)"
    } > "$DC_ROOT/bin/COMMIT.txt"
    chmod 0644 "$DC_ROOT/bin/COMMIT.txt"
    say "installed to $DC_ROOT/bin:$installed"
    sed -n '1,3p' "$DC_ROOT/bin/COMMIT.txt"
    # the scripts and the unit travel with the binaries
    if [ -d "$PKG/tools/vps" ]; then
        for f in "$PKG"/tools/vps/*.sh "$PKG"/tools/vps/*.py; do
            [ -f "$f" ] || continue
            install -m 0755 -o root -g root "$f" "$DC_ROOT/tools/$(basename "$f")"
            sed -i 's/\r$//' "$DC_ROOT/tools/$(basename "$f")"
        done
    fi
    # the default unit and the instance template
    for u in decombat-lobby.service decombat-lobby@.service decombat-backup.service decombat-backup.timer \
        decombat-health.service decombat-health.timer; do
        UNIT_SRC="$PKG/examples/$u"
        UNIT_DST="$DC_SYSTEMD_DIR/$u"
        [ "$u" != decombat-lobby.service ] || UNIT_DST="$DC_SYSTEMD_DIR/$DC_UNIT.service"
        [ -f "$UNIT_SRC" ] || continue
        tr -d '\r' < "$UNIT_SRC" > "$WORK/unit"
        if ! cmp -s "$WORK/unit" "$UNIT_DST"; then
            install -m 0644 -o root -g root "$WORK/unit" "$UNIT_DST"
            systemctl daemon-reload
            say "systemd unit $(basename "$UNIT_DST") updated from the tarball (local changes: systemctl edit <unit>)"
        fi
    done
    # OPS-22: the hardening drop-in is refreshed where the box has it; a deploy never adds it to a box that has not
    # gone through migrate_nonroot.sh harden, and a timer is never enabled here
    if [ -f "$PKG/examples/decombat-hardening.conf" ]; then
        for d in "$DC_SYSTEMD_DIR"/decombat-*.d; do
            [ -f "$d/10-hardening.conf" ] || continue
            tr -d '\r' < "$PKG/examples/decombat-hardening.conf" > "$WORK/hardening"
            if ! cmp -s "$WORK/hardening" "$d/10-hardening.conf"; then
                install -m 0644 -o root -g root "$WORK/hardening" "$d/10-hardening.conf"
                systemctl daemon-reload
                say "hardening drop-in of $(basename "$d") updated from the tarball"
            fi
        done
    fi
    # OPS-22: the one-time migration reads this; it finds it beside the installed tools
    if [ -f "$PKG/examples/decombat-hardening.conf" ]; then
        install -d -m 0755 -o root -g root "$DC_ROOT/tools/examples"
        tr -d '\r' < "$PKG/examples/decombat-hardening.conf" > "$WORK/example"
        install -m 0644 -o root -g root "$WORK/example" "$DC_ROOT/tools/examples/decombat-hardening.conf"
    fi
    # what instance.sh create reads (the installed tools have no examples/ next to them otherwise)
    if [ -f "$PKG/examples/server-vps.toml" ]; then
        install -d -m 0755 -o root -g root "$DC_ROOT/tools/examples"
        for f in server-vps.toml decombat-lobby@.service decombat-live-games.service decombat-live-games.timer; do
            [ -f "$PKG/examples/$f" ] || continue
            tr -d '\r' < "$PKG/examples/$f" > "$WORK/example"
            install -m 0644 -o root -g root "$WORK/example" "$DC_ROOT/tools/examples/$f"
        done
    fi
fi

# ---- server ROM
if [ -n "$ROM_TGZ" ]; then
    [ -f "$ROM_TGZ" ] || die "no such file: $ROM_TGZ"
    NEW="$DC_ROOT/rom.new"
    OLD="$DC_ROOT/rom.old"
    rm -rf "$NEW" "$OLD"
    install -d -m 0750 -o root -g "$DC_USER" "$NEW"
    say "unpacking $(basename "$ROM_TGZ") ($(du -h "$ROM_TGZ" | cut -f1)) ..."
    tar -xzf "$ROM_TGZ" -C "$NEW" --no-same-owner
    [ -f "$NEW/server-rom.json" ] || die "$ROM_TGZ holds no server-rom.json at its top: not a server ROM folder (dc-server-rom)"
    chown -R root:"$DC_USER" "$NEW"
    find "$NEW" -type d -exec chmod 0750 {} +
    find "$NEW" -type f -exec chmod 0640 {} +
    bash "$HERE/rom_check.sh" "$NEW"
    if [ -d "$DC_ROOT/rom" ]; then mv "$DC_ROOT/rom" "$OLD"; fi
    mv "$NEW" "$DC_ROOT/rom"
    rm -rf "$OLD"
    say "server ROM installed at $DC_ROOT/rom"
fi

# ---- restart: every enabled instance, one after the other
if [ "$RESTART" -eq 1 ]; then
    for n in default $(list_instances | grep -vx default || true); do
        instance_paths "$n"
        unit=$INST_UNIT
        [ "$n" != default ] || unit=$DC_UNIT
        if systemctl is-enabled --quiet "$unit" 2>/dev/null; then
            say "restarting $unit (graceful stop first) ..."
            # a unit left in a crash loop by a bad build hits systemd's start limit ("start request repeated too
            # quickly"): clear it so the restart of a good build is not refused
            systemctl reset-failed "$unit" 2>/dev/null || true
            systemctl restart "$unit"
            sleep 3
            systemctl --no-pager --lines=0 status "$unit" || true
        elif [ "$n" = default ]; then
            say "$unit is not enabled; first start: sudo systemctl enable --now $unit (docs/VPS.md)"
        else
            say "$unit is not enabled (instance.sh $n start); not restarted"
        fi
    done
fi
