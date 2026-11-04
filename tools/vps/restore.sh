#!/usr/bin/env bash
# restore.sh: read a backup of backup.sh back (docs/OPS.md "Restore"). Without --apply it changes nothing on the box: it
# decrypts, unpacks into a scratch folder, opens each database and prints its rows. NOT RUN ON A LINUX VPS YET; tests:
# tools/vps/tests/backup_test.sh.
# DC-DEFAULT: OPS-20 (proposed), see DC_DEFAULTS.md
#
#   sudo bash restore.sh FILE|--from-remote NAME --key PRIVATE_KEY_FILE [--to DIR]
#   sudo bash restore.sh FILE|--from-remote NAME --key PRIVATE_KEY_FILE --apply --instance NAME [--with-keys] [--with-config]
#   sudo bash restore.sh --list
#
# --key          age: the identity file; gpg: an exported secret key (no passphrase prompt in a script: use a key without
#                one on the machine doing the restore, or decrypt elsewhere and upload the .tar.gz, which needs no --key)
# --from-remote  fetch NAME from RCLONE_REMOTE of backup.conf first
# --to DIR       keep the unpacked files there (they are NOT encrypted any more; default: a scratch folder, removed)
# --apply        stop that instance's unit, move its database aside to <data>/before-restore-<time>/, put the backup's in
#                place, start the unit and check it is active. Refuses before any change when the backup's database
#                fails its check. --with-keys / --with-config also restore that instance's keys / config files.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

CONF=${DC_BACKUP_CONF:-$DC_ROOT/config/backup.conf}
FILE=""; FROM=""; KEY=""; TO=""; APPLY=0; INST=""; WITH_KEYS=0; WITH_CONFIG=0; LIST=0
while [ $# -gt 0 ]; do
    case "$1" in
        --config) CONF=${2:?--config needs a file}; shift 2 ;;
        --from-remote) FROM=${2:?--from-remote needs a name}; shift 2 ;;
        --key) KEY=${2:?--key needs a file}; shift 2 ;;
        --to) TO=${2:?--to needs a folder}; shift 2 ;;
        --apply) APPLY=1; shift ;;
        --instance) INST=${2:?--instance needs a name}; shift 2 ;;
        --with-keys) WITH_KEYS=1; shift ;;
        --with-config) WITH_CONFIG=1; shift ;;
        --list) LIST=1; shift ;;
        -h|--help) sed -n '2,18p' "$0"; exit 0 ;;
        -*) die "unknown option: $1 (see --help)" ;;
        *) [ -z "$FILE" ] || die "one backup at a time"; FILE=$1; shift ;;
    esac
done
need_root

REMOTE=$(env_get "$CONF" RCLONE_REMOTE)
RCONF=$(env_get "$CONF" RCLONE_CONFIG)
DIR=$(env_get "$CONF" BACKUP_DIR); DIR=${DIR:-$DC_ROOT/backups}
rclone_run() {
    if [ -n "$RCONF" ]; then rclone --config "$RCONF" "$@"; else rclone "$@"; fi
}

if [ "$LIST" -eq 1 ]; then
    say "local ($DIR):"
    # shellcheck disable=SC2012
    ls -1 "$DIR" 2>/dev/null | grep -E '^decombat-.*\.tar\.gz' | sort || say "  none"
    if [ -n "$REMOTE" ]; then
        say "remote ($REMOTE):"
        rclone_run lsf "$REMOTE" || die "cannot list $REMOTE"
    fi
    exit 0
fi

[ -n "$FILE" ] || [ -n "$FROM" ] || die "give a backup file or --from-remote NAME (--help)"
[ "$APPLY" -eq 0 ] || [ -n "$INST" ] || die "--apply needs --instance NAME: which lobby to overwrite is never guessed"
[ -z "$INST" ] || valid_instance_name "$INST" || die "not an instance name: $INST"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/decombat-restore.XXXXXX")
chmod 0700 "$WORK"
trap 'rm -rf "$WORK"' EXIT

if [ -n "$FROM" ]; then
    [ -n "$REMOTE" ] || die "no RCLONE_REMOTE in $CONF"
    case "$FROM" in */*|*..*) die "--from-remote takes a file name, not a path" ;; esac
    rclone_run copyto "$REMOTE/$FROM" "$WORK/$FROM" || die "cannot fetch $REMOTE/$FROM"
    FILE="$WORK/$FROM"
fi
[ -f "$FILE" ] || die "no such file: $FILE"

mkdir "$WORK/out"
case "$FILE" in
    *.age)
        [ -n "$KEY" ] || die "--key: the age identity file"
        age -d -i "$KEY" "$FILE" | tar -xzf - -C "$WORK/out" --no-same-owner
        ;;
    *.gpg)
        [ -n "$KEY" ] || die "--key: the exported gpg secret key"
        dir_mode 0700 "$WORK/gpg"
        gpg --homedir "$WORK/gpg" --batch --no-tty --quiet --import "$KEY"
        gpg --homedir "$WORK/gpg" --batch --no-tty --quiet --pinentry-mode loopback --passphrase "" \
            --decrypt "$FILE" | tar -xzf - -C "$WORK/out" --no-same-owner
        gpgconf --homedir "$WORK/gpg" --kill all >/dev/null 2>&1 || true
        ;;
    *.tar.gz)
        tar -xzf "$FILE" -C "$WORK/out" --no-same-owner
        ;;
    *) die "not a backup name: $FILE" ;;
esac

cat "$WORK/out/MANIFEST.txt" 2>/dev/null || die "the archive holds no MANIFEST.txt: not a backup of backup.sh"
FOUND=0
for d in "$WORK"/out/*/; do
    [ -f "${d}ca.db" ] || continue
    n=$(basename "$d")
    facts=$(sqlite_check "${d}ca.db") || die "$n: the backup's database fails its check: $facts"
    say "$n: $facts"
    FOUND=$((FOUND + 1))
done
[ "$FOUND" -gt 0 ] || die "no database in the archive"

if [ -n "$TO" ]; then
    mkdir -p "$TO"
    chmod 0700 "$TO"
    cp -R "$WORK/out/." "$TO/"
    say "unpacked to $TO: these files are not encrypted, delete them when done"
fi

if [ "$APPLY" -eq 1 ]; then
    src="$WORK/out/$INST"
    [ -f "$src/ca.db" ] || die "the archive holds no instance '$INST'"
    instance_paths "$INST"
    unit=$INST_UNIT data=$INST_DATA conf=$INST_CONF envf=$INST_ENV
    db=$(conf_get "$INST" paths db); db=${db:-$data/ca.db}
    aside="$(dirname "$db")/before-restore-$(date -u +%Y%m%d-%H%M%S)"
    say "stopping $unit (graceful: running matches end) ..."
    systemctl stop "$unit" || die "cannot stop $unit; nothing changed"
    dir_mode 0700 "$aside"
    for f in "$db" "$db-wal" "$db-shm"; do
        [ ! -f "$f" ] || mv "$f" "$aside/"
    done
    install -m 0640 "$src/ca.db" "$db"
    if [ "$WITH_KEYS" -eq 1 ] && [ -d "$src/keys" ]; then
        for f in "$src"/keys/*; do
            [ ! -f "$f" ] || { cp -p "$(dirname "$db")/$(basename "$f")" "$aside/" 2>/dev/null || true; install -m 0600 "$f" "$(dirname "$db")/$(basename "$f")"; }
        done
    fi
    if [ "$WITH_CONFIG" -eq 1 ]; then
        [ ! -f "$src/config/server.toml" ] || install -m 0640 "$src/config/server.toml" "$conf"
        [ ! -f "$src/config/decombat.env" ] || install -m 0640 "$src/config/decombat.env" "$envf"
    fi
    if [ "$DC_TEST" != 1 ]; then
        chown -R "$DC_USER:$DC_USER" "$aside" "$db"
        for f in "$(dirname "$db")"/*.pem; do [ ! -f "$f" ] || chown "$DC_USER:$DC_USER" "$f"; done
        if [ "$WITH_CONFIG" -eq 1 ]; then chown root:"$DC_USER" "$conf" "$envf"; fi
    fi
    systemctl reset-failed "$unit" 2>/dev/null || true
    systemctl start "$unit" || die "$unit did not start; the old database is in $aside"
    sleep "${DC_RESTORE_WAIT:-3}"
    systemctl is-active --quiet "$unit" || die "$unit is not active after the restore; the old database is in $aside"
    say "restored instance $INST; the old database is in $aside (it holds accounts: delete it when you are sure)"
fi
