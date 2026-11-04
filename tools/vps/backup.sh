#!/usr/bin/env bash
# backup.sh: an encrypted backup of every lobby instance's database, off the box (docs/OPS.md "Backups"). As root, from
# the timer decombat-backup.timer or by hand. NOT RUN ON A LINUX VPS YET; tests: tools/vps/tests/backup_test.sh.
# DC-DEFAULT: OPS-20 (proposed), see DC_DEFAULTS.md
#
#   sudo bash backup.sh [--config FILE] [--dry-run]
#
# For each instance: a consistent copy of the running database through SQLite's online backup API (as the service user,
# so no root-owned -wal or -shm file is left beside the live DB), checked with integrity_check; the instance's config
# and its TLS and identity keys beside it. All of it goes into one tar.gz that is encrypted to a PUBLIC key before it
# touches the disk under its final name: nothing here can decrypt it, the private key stays with the owner. There is no
# unencrypted mode, so the keys never leave unencrypted; INCLUDE_KEYS=0 leaves them out.
# The newest KEEP stay in BACKUP_DIR, the new one goes to RCLONE_REMOTE, its size is compared there, and only then is
# BACKUP_DIR/last_ok touched (healthcheck.sh watches that file). Any failure: exit 1, a line on stderr, and a post to
# the webhook of alerts.conf.
#
# Config (examples/backup.conf; KEY=VALUE, never sourced): AGE_RECIPIENT or GPG_RECIPIENT_FILE, RCLONE_REMOTE,
# RCLONE_CONFIG, KEEP (14), KEEP_REMOTE_DAYS (0 = keep all), BACKUP_DIR, INCLUDE_KEYS (1), LOCAL_ONLY (0).
set -Eeuo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

CONF=${DC_BACKUP_CONF:-$DC_ROOT/config/backup.conf}
DRY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --config) CONF=${2:?--config needs a file}; shift 2 ;;
        --dry-run) DRY=1; shift ;;
        -h|--help) sed -n '2,18p' "$0"; exit 0 ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done
need_root

STAGE=""
SNAP=""
PART=""
cleanup() {
    [ -z "$STAGE" ] || rm -rf "$STAGE"
    [ -z "$SNAP" ] || rm -rf "$SNAP"
    [ -z "$PART" ] || rm -f "$PART"
}
fail() {
    printf 'backup FAILED: %s\n' "$*" >&2
    alert_post "DeCombat backup FAILED on $(hostname 2>/dev/null || echo host): $*" || true
    cleanup
    trap - ERR EXIT
    exit 1
}
trap 'fail "stopped at line $LINENO of backup.sh"' ERR
trap cleanup EXIT

[ -r "$CONF" ] || fail "no config $CONF (copy examples/backup.conf, docs/OPS.md)"
AGE_TO=$(env_get "$CONF" AGE_RECIPIENT)
GPG_FILE=$(env_get "$CONF" GPG_RECIPIENT_FILE)
REMOTE=$(env_get "$CONF" RCLONE_REMOTE)
RCONF=$(env_get "$CONF" RCLONE_CONFIG)
KEEP=$(env_get "$CONF" KEEP); KEEP=${KEEP:-14}
KEEP_REMOTE=$(env_get "$CONF" KEEP_REMOTE_DAYS); KEEP_REMOTE=${KEEP_REMOTE:-0}
DIR=$(env_get "$CONF" BACKUP_DIR); DIR=${DIR:-$DC_ROOT/backups}
KEYS=$(env_get "$CONF" INCLUDE_KEYS); KEYS=${KEYS:-1}
LOCAL_ONLY=$(env_get "$CONF" LOCAL_ONLY); LOCAL_ONLY=${LOCAL_ONLY:-0}

case "$KEEP" in ''|*[!0-9]*|0) fail "KEEP must be a number of at least 1, got '$KEEP'" ;; esac
case "$KEEP_REMOTE" in ''|*[!0-9]*) fail "KEEP_REMOTE_DAYS must be a number, got '$KEEP_REMOTE'" ;; esac
if [ -n "$AGE_TO" ]; then
    command -v age >/dev/null 2>&1 || fail "AGE_RECIPIENT is set and age is not installed (apt-get install -y age)"
    EXT=age
elif [ -n "$GPG_FILE" ]; then
    command -v gpg >/dev/null 2>&1 || fail "GPG_RECIPIENT_FILE is set and gpg is not installed"
    [ -r "$GPG_FILE" ] || fail "cannot read the public key $GPG_FILE"
    EXT=gpg
else
    fail "no encryption key: set AGE_RECIPIENT or GPG_RECIPIENT_FILE in $CONF; a backup is never written unencrypted"
fi
if [ -z "$REMOTE" ] && [ "$LOCAL_ONLY" != 1 ]; then
    fail "no RCLONE_REMOTE in $CONF: a backup on the same disk does not survive the VPS (LOCAL_ONLY=1 accepts that)"
fi
if [ -n "$REMOTE" ]; then
    command -v rclone >/dev/null 2>&1 || fail "RCLONE_REMOTE is set and rclone is not installed (apt-get install -y rclone)"
fi
rclone_run() {
    if [ -n "$RCONF" ]; then rclone --config "$RCONF" "$@"; else rclone "$@"; fi
}

encrypt() { # OUT, tar.gz on stdin
    if [ "$EXT" = age ]; then
        age -r "$AGE_TO" -o "$1"
    else
        local home rc=0
        home=$(mktemp -d "$DIR/.gpg.XXXXXX")
        gpg --homedir "$home" --batch --yes --no-tty --quiet --trust-model always \
            --recipient-file "$GPG_FILE" --output "$1" --encrypt || rc=$?
        gpgconf --homedir "$home" --kill all >/dev/null 2>&1 || true
        rm -rf "$home"
        return $rc
    fi
}

INSTANCES=$(list_instances || true)
[ -n "$INSTANCES" ] || fail "no lobby instance under $DC_ROOT"

if [ "$DRY" -eq 1 ]; then
    say "dry run: encrypt with $EXT; keep $KEEP in $DIR; remote '${REMOTE:-none}'; keys included: $KEYS"
    for n in $INSTANCES; do
        instance_paths "$n"
        db=$(conf_get "$n" paths db); db=${db:-$INST_DATA/ca.db}
        if [ -f "$db" ]; then say "  $n: $db ($(du -h "$db" | cut -f1))"; else say "  $n: $db MISSING"; fi
    done
    trap - ERR EXIT
    exit 0
fi

command -v "$DC_PYTHON" >/dev/null 2>&1 || fail "$DC_PYTHON is not installed"
dir_mode 0700 "$DIR"
STAGE=$(mktemp -d "$DIR/.stage.XXXXXX")
chmod 0700 "$STAGE"
{
    echo "created: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "host: $(hostname 2>/dev/null || echo unknown)"
    sed -n 1p "$DC_ROOT/bin/COMMIT.txt" 2>/dev/null || echo "commit: unknown"
} > "$STAGE/MANIFEST.txt"

for n in $INSTANCES; do
    instance_paths "$n"
    conf=$INST_CONF envf=$INST_ENV data=$INST_DATA
    db=$(conf_get "$n" paths db); db=${db:-$data/ca.db}
    [ -f "$db" ] || fail "instance $n: no database at $db"
    dest="$STAGE/$n"
    dir_mode 0700 "$dest" "$dest/config"
    # the copy is made by the service user, in a folder of its own beside the DB
    SNAP="$(dirname "$db")/.backup-snap.$$"
    rm -rf "$SNAP"
    dir_mode 0700 "$SNAP"
    if [ "$DC_TEST" != 1 ]; then chown "$DC_USER:$DC_USER" "$SNAP"; fi
    facts=$(as_service_user sqlite_snapshot "$db" "$SNAP/ca.db") || fail "instance $n: the snapshot of $db failed: $facts"
    mv "$SNAP/ca.db" "$dest/ca.db"
    rm -rf "$SNAP"
    SNAP=""
    echo "$n: $facts" >> "$STAGE/MANIFEST.txt"
    say "$n: $facts"
    [ ! -f "$conf" ] || cp -p "$conf" "$dest/config/server.toml"
    [ ! -f "$envf" ] || cp -p "$envf" "$dest/config/decombat.env"
    if [ "$KEYS" = 1 ]; then
        dir_mode 0700 "$dest/keys"
        keyfile=$(conf_get "$n" identity key_file)
        for f in "$(dirname "$db")"/identity_key.pem "$(dirname "$db")"/tls_cert.pem "$(dirname "$db")"/tls_key.pem \
            "$(dirname "$db")"/fullchain.pem "$(dirname "$db")"/privkey.pem ${keyfile:+"$keyfile"}; do
            [ ! -f "$f" ] || cp -p "$f" "$dest/keys/$(basename "$f")"
        done
    fi
done

NAME="decombat-$(date -u +%Y%m%d-%H%M%S).tar.gz.$EXT"
PART="$DIR/.$NAME.part"
tar -czf - -C "$STAGE" . | encrypt "$PART"
[ -s "$PART" ] || fail "the encrypted archive is empty"
rm -rf "$STAGE"
STAGE=""
mv "$PART" "$DIR/$NAME"
PART=""
chmod 0600 "$DIR/$NAME"
say "wrote $DIR/$NAME ($(du -h "$DIR/$NAME" | cut -f1))"

# the newest KEEP stay; the sort is by the UTC time in the names
# shellcheck disable=SC2012
ls -1 "$DIR" | grep -E '^decombat-[0-9]{8}-[0-9]{6}\.tar\.gz\.(age|gpg)$' | sort -r | tail -n +"$((KEEP + 1))" | while read -r old; do
    rm -f "$DIR/$old"
    say "removed old $old"
done

if [ -n "$REMOTE" ]; then
    rclone_run copyto "$DIR/$NAME" "$REMOTE/$NAME" || fail "the upload to $REMOTE failed; the backup stays in $DIR/$NAME"
    want=$(wc -c < "$DIR/$NAME" | tr -d ' ')
    got=$(rclone_run lsl "$REMOTE/$NAME" | awk 'NR==1{print $1}') || got=""
    [ "$got" = "$want" ] || fail "$REMOTE/$NAME is ${got:-missing} bytes, expected $want"
    say "uploaded to $REMOTE/$NAME ($want bytes)"
    if [ "$KEEP_REMOTE" -gt 0 ]; then
        rclone_run delete --min-age "${KEEP_REMOTE}d" "$REMOTE" || fail "pruning $REMOTE failed; the new backup is there"
    fi
fi
printf '%s\n' "$NAME" > "$DIR/last_ok"
say "backup ok: $NAME"
