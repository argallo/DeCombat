#!/usr/bin/env bash
# backup_test.sh: tests of tools/vps/backup.sh and restore.sh on a temporary root with a real database, real gpg encryption
# and a stand-in rclone, systemctl and curl. The database comes from the lobby's own schema when a dc-lobby-server binary is
# there (DC_LOBBY_BIN, or target/debug under the repository), else from a small stand-in schema; the output says which.
#   bash tools/vps/tests/backup_test.sh
# Runs under Git Bash on Windows and on Linux. Needs gpg and python. Exit 0 = all passed.
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VPS=$(cd "$HERE/.." && pwd)
REPO=$(cd "$VPS/../.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/dc-backup-test.XXXXXX")
export GNUPGHOME="$T/gnupg"
mkdir -p "$GNUPGHOME"
chmod 0700 "$GNUPGHOME" 2>/dev/null
cleanup() {
    [ -z "${WRITER:-}" ] || kill "$WRITER" 2>/dev/null
    gpgconf --homedir "$GNUPGHOME" --kill all >/dev/null 2>&1
    rm -rf "$T"
}
trap cleanup EXIT

export DC_TEST=1 DC_ROOT="$T/opt" DC_SYSTEMD_DIR="$T/systemd"
export DC_BACKUP_CONF="$T/opt/config/backup.conf" DC_ALERTS_CONF="$T/opt/config/alerts.conf" DC_RESTORE_WAIT=0
unset DC_ADMIN DC_BOT_LOBBY DC_INSTANCE DC_UNIT
PY=python3
python3 -c 'import sqlite3' >/dev/null 2>&1 || PY=python
export DC_PYTHON=$PY

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL: %s\n' "$*"; }
check() { local d=$1; shift; if "$@"; then ok; else bad "$d"; fi; }
has() { if printf '%s\n' "$2" | grep -qF -- "$3"; then ok; else bad "$1: no '$3' in: $2"; fi; }

# ---- stand-ins
mkdir -p "$T/bin" "$T/opt/config" "$T/opt/data" "$T/opt/logs" "$T/opt/instances/modded/config" "$T/opt/instances/modded/data" "$T/remote"
cat > "$T/bin/rclone" <<'S'
#!/usr/bin/env bash
# copyto SRC offsite:NAME | lsl offsite:NAME | delete ... | lsf offsite: ; RCLONE_BROKEN=copy|size fails that part
echo "rclone $*" >> "$STUB_LOG"
if [ "${1:-}" = --config ]; then shift 2; fi
map() { printf '%s/%s' "$STUB_REMOTE" "${1#offsite:}"; }
case "$1" in
  copyto)
    [ "${RCLONE_BROKEN:-}" != copy ] || exit 3
    case "$2" in
      offsite:*) cp "$(map "$2")" "$3" ;;
      *) cp "$2" "$(map "$3")" ;;
    esac ;;
  lsl)
    [ "${RCLONE_BROKEN:-}" != size ] || { echo "1 2026-01-01 00:00:00 x"; exit 0; }
    f=$(map "$2"); [ -f "$f" ] && echo "$(wc -c < "$f" | tr -d ' ') 2026-01-01 00:00:00 $(basename "$f")" ;;
  lsf) ls -1 "$STUB_REMOTE" ;;
  delete) ;;
esac
S
cat > "$T/bin/curl" <<'S'
#!/usr/bin/env bash
cat >> "$STUB_LOG.curl.conf" 2>/dev/null
echo "curl $*" >> "$STUB_LOG.curl"
S
cat > "$T/bin/systemctl" <<'S'
#!/usr/bin/env bash
echo "systemctl $*" >> "$STUB_LOG"
exit 0
S
chmod +x "$T"/bin/*
export PATH="$T/bin:$PATH" STUB_LOG="$T/stub.log" STUB_REMOTE="$T/remote"
touch "$STUB_LOG"
for s in backup restore; do tr -d '\r' < "$VPS/$s.sh" > "$T/$s.sh"; done
tr -d '\r' < "$VPS/common.sh" > "$T/common.sh"

# ---- the databases: the lobby's own schema when its binary is there
BIN=${DC_LOBBY_BIN:-}
for b in "$REPO/target/debug/dc-lobby-server" "$REPO/target/debug/dc-lobby-server.exe"; do
    [ -n "$BIN" ] || { [ -x "$b" ] && BIN=$b; }
done
PORT=$((21000 + RANDOM % 8000))
# the lobby makes its schema at its start and the account command needs the file: a short start on loopback ports
first_start() { # FILE
    local p=$PORT pid
    PORT=$((PORT + 10))
    "$BIN" --db "$1" --port "$p" --lobbyport "$((p + 2))" --clanport "$((p + 3))" --clanweb 0 >/dev/null 2>&1 &
    pid=$!
    for _ in $(seq 1 100); do
        [ -s "$1" ] && "$BIN" account --db "$1" list >/dev/null 2>&1 && break
        sleep 0.1
    done
    kill "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
}
make_db() { # FILE NAME
    if [ -n "$BIN" ]; then
        [ -f "$1" ] || first_start "$1"
        "$BIN" account --db "$1" add "$2" --password "correct-horse-1" >/dev/null || bad "account add on $1"
    else
        "$PY" - "$1" "$2" <<'P'
import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
c.execute("PRAGMA journal_mode=WAL")
c.execute("CREATE TABLE IF NOT EXISTS accounts (user_no INTEGER PRIMARY KEY, login TEXT)")
c.execute("INSERT INTO accounts (login) VALUES (?)", (sys.argv[2],))
c.commit()
P
    fi
}
if [ -n "$BIN" ]; then echo "database: the lobby's own schema ($BIN)"; else echo "database: stand-in schema (no dc-lobby-server binary)"; fi
: > "$T/opt/config/server.toml"
printf 'DC_SERVER__ADVERTISE_IP=192.0.2.1\n' > "$T/opt/config/decombat.env"
: > "$T/opt/instances/modded/config/server.toml"
make_db "$T/opt/data/ca.db" BackupAlice
make_db "$T/opt/data/ca.db" BackupBob
make_db "$T/opt/instances/modded/data/ca.db" BackupModded
printf 'SECRET-IDENTITY-KEY-TEXT\n' > "$T/opt/data/identity_key.pem"
printf 'SECRET-TLS-KEY-TEXT\n' > "$T/opt/data/tls_key.pem"
# a row that exists only in the write-ahead log of an open connection: a copy of the file would miss it
"$PY" - "$T/opt/data/ca.db" "$T/writer.ready" <<'P' &
import sqlite3, sys, time
c = sqlite3.connect(sys.argv[1])
c.execute("PRAGMA journal_mode=WAL")
c.execute("PRAGMA wal_autocheckpoint=0")
c.execute("CREATE TABLE IF NOT EXISTS backup_test_wal (v TEXT)")
c.execute("INSERT INTO backup_test_wal VALUES ('only-in-the-wal')")
c.commit()
open(sys.argv[2], "w").close()
time.sleep(120)
P
WRITER=$!
for _ in $(seq 1 100); do [ -f "$T/writer.ready" ] && break; sleep 0.1; done
check "the writer holds the database open" test -f "$T/writer.ready"

# ---- the key pair: on the VPS only the public file
gpg --batch --quiet --pinentry-mode loopback --passphrase '' --quick-generate-key "backup-test@example.invalid" default default never 2>/dev/null
gpg --batch --quiet --export --armor backup-test@example.invalid > "$T/opt/config/public.asc"
gpg --batch --quiet --pinentry-mode loopback --passphrase '' --export-secret-keys --armor backup-test@example.invalid > "$T/secret.asc"
check "a public key was exported" test -s "$T/opt/config/public.asc"
check "a secret key was exported" test -s "$T/secret.asc"

conf() { # lines of the config
    printf '%s\n' "$@" > "$DC_BACKUP_CONF"
}
BDIR="$T/opt/backups"
run_backup() { bash "$T/backup.sh" 2>&1; }

# ---- refusals
conf "RCLONE_REMOTE=offsite:" "KEEP=2"
out=$(run_backup); rc=$?
check "no key: refused" test "$rc" -ne 0
has "no key: says why" "$out" "never written unencrypted"
check "no key: nothing written" test ! -e "$BDIR/last_ok"
conf "GPG_RECIPIENT_FILE=$T/opt/config/public.asc"
out=$(run_backup); rc=$?
check "no remote: refused" test "$rc" -ne 0
has "no remote: says why" "$out" "does not survive the VPS"
conf "GPG_RECIPIENT_FILE=$T/opt/config/public.asc" "RCLONE_REMOTE=offsite:" "KEEP=0"
out=$(run_backup); rc=$?
check "KEEP=0: refused" test "$rc" -ne 0

# ---- a good run
conf "GPG_RECIPIENT_FILE=$T/opt/config/public.asc" "RCLONE_REMOTE=offsite:" "KEEP=2" "BACKUP_DIR=$BDIR"
out=$(run_backup); rc=$?
check "backup ok: $out" test "$rc" -eq 0
check "last_ok written" test -s "$BDIR/last_ok"
NAME=$(cat "$BDIR/last_ok" 2>/dev/null)
A="$BDIR/$NAME"
check "the archive is there" test -s "$A"
check "the archive is on the remote" test -s "$T/remote/$NAME"
cmp -s "$A" "$T/remote/$NAME" && ok || bad "the remote copy differs"
# no plaintext: neither the keys, nor names, nor a stage folder left behind
for needle in SECRET-IDENTITY SECRET-TLS BackupAlice MANIFEST; do
    if grep -qa -- "$needle" "$A"; then bad "'$needle' readable in the archive"; else ok; fi
done
if ls -A "$BDIR" | grep -Eq '^\.(stage|gpg)|\.part$'; then bad "scratch left in $BDIR: $(ls -A "$BDIR")"; else ok; fi
if ls -A "$T/opt/data" | grep -q 'backup-snap'; then bad "snapshot folder left beside the DB"; else ok; fi

# ---- restore: verify only
: > "$T/stub.log"
kill "$WRITER" 2>/dev/null; wait "$WRITER" 2>/dev/null; WRITER=""
out=$(bash "$T/restore.sh" "$A" --key "$T/secret.asc" --to "$T/out" 2>&1); rc=$?
check "restore reads the backup: $out" test "$rc" -eq 0
has "the manifest is shown" "$out" "created:"
has "both instances checked" "$out" "modded:"
if grep -q systemctl "$T/stub.log"; then bad "a verify-only restore touched a unit"; else ok; fi
check "restored default DB" test -s "$T/out/default/ca.db"
check "restored modded DB" test -s "$T/out/modded/ca.db"
check "restored key" test -s "$T/out/default/keys/identity_key.pem"
rows() { "$PY" - "$1" "$2" <<'P'
import pathlib, sqlite3, sys
c = sqlite3.connect(pathlib.Path(sys.argv[1]).resolve().as_uri() + "?mode=ro", uri=True)
print(c.execute(sys.argv[2]).fetchone()[0])
P
}
if [ -f "$T/out/default/ca.db" ]; then
    n=$(rows "$T/out/default/ca.db" "SELECT count(*) FROM accounts WHERE login IN ('BackupAlice','BackupBob')")
    check "the restored DB has its accounts (got $n)" test "$n" = 2
    n=$(rows "$T/out/default/ca.db" "SELECT count(*) FROM backup_test_wal WHERE v='only-in-the-wal'")
    check "the row that was only in the WAL is in the copy (got $n)" test "$n" = 1
    check "the copy is one file" test ! -e "$T/out/default/ca.db-wal"
    if [ -n "$BIN" ]; then
        l=$("$BIN" account --db "$T/out/default/ca.db" list 2>&1)
        has "the lobby's own CLI opens the restored DB" "$l" "BackupAlice"
    fi
fi

# ---- a wrong key or a damaged file changes nothing
gpgconf --homedir "$GNUPGHOME" --kill all >/dev/null 2>&1
printf 'not a key\n' > "$T/nokey"
out=$(bash "$T/restore.sh" "$A" --key "$T/nokey" 2>&1); rc=$?
check "a wrong key fails" test "$rc" -ne 0
head -c 2000 /dev/zero > "$T/broken.db"
mkdir -p "$T/bad/default"; cp "$T/broken.db" "$T/bad/default/ca.db"; echo "created: x" > "$T/bad/MANIFEST.txt"
tar -czf "$T/bad.tar.gz" -C "$T/bad" .
before=$(cksum < "$T/opt/data/ca.db")
: > "$T/stub.log"
out=$(bash "$T/restore.sh" "$T/bad.tar.gz" --apply --instance default 2>&1); rc=$?
check "a damaged database is refused" test "$rc" -ne 0
if grep -q 'systemctl stop' "$T/stub.log"; then bad "the unit was stopped for a damaged backup"; else ok; fi
check "the live DB is untouched" test "$(cksum < "$T/opt/data/ca.db")" = "$before"
out=$(bash "$T/restore.sh" "$A" --key "$T/secret.asc" --apply 2>&1); rc=$?
check "--apply without --instance is refused" test "$rc" -ne 0

# ---- restore: apply over a changed DB, from the remote
make_db "$T/opt/data/ca.db" BackupLater
: > "$T/stub.log"
out=$(bash "$T/restore.sh" --from-remote "$NAME" --key "$T/secret.asc" --apply --instance default --with-keys 2>&1); rc=$?
check "apply ok: $out" test "$rc" -eq 0
log=$(cat "$T/stub.log")
has "the unit was stopped" "$log" "systemctl stop decombat-lobby"
has "the unit was started" "$log" "systemctl start decombat-lobby"
n=$(rows "$T/opt/data/ca.db" "SELECT count(*) FROM accounts WHERE login='BackupLater'")
check "the live DB is the backup's now (got $n)" test "$n" = 0
n=$(rows "$T/opt/data/ca.db" "SELECT count(*) FROM accounts WHERE login='BackupAlice'")
check "with its accounts (got $n)" test "$n" = 1
aside=$(ls -d "$T"/opt/data/before-restore-* 2>/dev/null | head -n 1)
check "the old DB was kept aside" test -f "$aside/ca.db"
n=$(rows "$T/opt/instances/modded/data/ca.db" "SELECT count(*) FROM accounts WHERE login='BackupModded'")
check "the other instance is untouched (got $n)" test "$n" = 1
check "keys back in place" test -s "$T/opt/data/identity_key.pem"

# ---- failures are loud: exit 1, the webhook, last_ok not moved; pruning keeps the newest
printf 'WEBHOOK_URL=https://example.invalid/hook\n' > "$DC_ALERTS_CONF"
stamp_before=$(cat "$BDIR/last_ok")
sleep 1.1
: > "$STUB_LOG.curl"
: > "$STUB_LOG.curl.conf"
out=$(RCLONE_BROKEN=copy run_backup); rc=$?
check "a failed upload fails the run" test "$rc" -ne 0
has "and says so" "$out" "the upload to offsite: failed"
check "last_ok does not move" test "$(cat "$BDIR/last_ok")" = "$stamp_before"
has "the webhook heard it" "$(cat "$STUB_LOG.curl")" "backup FAILED"
has "the hook's address is not on the command line, only in curl's config" "$(cat "$STUB_LOG.curl.conf")" "example.invalid"
if grep -q example.invalid "$STUB_LOG.curl"; then bad "the webhook address on a command line"; else ok; fi
sleep 1.1
out=$(RCLONE_BROKEN=size run_backup); rc=$?
check "a remote file of the wrong size fails the run" test "$rc" -ne 0
sleep 1.1
out=$(run_backup); rc=$?
check "the next good run is fine: $out" test "$rc" -eq 0
kept=$(ls -1 "$BDIR" | grep -c '^decombat-')
check "KEEP=2 keeps two (got $kept)" test "$kept" = 2
check "the newest is one of them" test -f "$BDIR/$(cat "$BDIR/last_ok")"

# ---- --list, keys left out, dry run
out=$(bash "$T/restore.sh" --list 2>&1); rc=$?
check "--list works" test "$rc" -eq 0
has "--list names the backup" "$out" "$(cat "$BDIR/last_ok")"
conf "GPG_RECIPIENT_FILE=$T/opt/config/public.asc" "LOCAL_ONLY=1" "KEEP=5" "INCLUDE_KEYS=0" "BACKUP_DIR=$BDIR"
sleep 1.1
out=$(run_backup); rc=$?
check "local only with no keys: ok: $out" test "$rc" -eq 0
out=$(bash "$T/restore.sh" "$BDIR/$(cat "$BDIR/last_ok")" --key "$T/secret.asc" --to "$T/out2" 2>&1); rc=$?
check "reads back: $out" test "$rc" -eq 0
if [ -e "$T/out2/default/keys" ]; then bad "INCLUDE_KEYS=0 still packed keys"; else ok; fi
out=$(bash "$T/backup.sh" --dry-run 2>&1); rc=$?
check "dry run ok" test "$rc" -eq 0
has "dry run names the DB" "$out" "ca.db"

printf 'backup_test: %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
