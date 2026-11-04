#!/usr/bin/env bash
# healthcheck_test.sh: tests of tools/vps/healthcheck.sh on a temporary root with stand-ins for systemctl, curl, openssl and df.
#   bash tools/vps/tests/healthcheck_test.sh
# Runs under Git Bash on Windows and on Linux. Exit 0 = all passed.
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VPS=$(cd "$HERE/.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/dc-health-test.XXXXXX")
trap 'rm -rf "$T"' EXIT

export DC_TEST=1 DC_ROOT="$T/opt" DC_SYSTEMD_DIR="$T/systemd" DC_ALERTS_CONF="$T/opt/config/alerts.conf"
unset DC_ADMIN DC_BOT_LOBBY DC_INSTANCE DC_UNIT
PY=python3
python3 -c 'import json' >/dev/null 2>&1 || PY=python
export DC_PYTHON=$PY

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL: %s\n' "$*"; }
check() { local d=$1; shift; if "$@"; then ok; else bad "$d"; fi; }
has() { if printf '%s\n' "$2" | grep -qF -- "$3"; then ok; else bad "$1: no '$3' in: $2"; fi; }
hasnt() { if printf '%s\n' "$2" | grep -qF -- "$3"; then bad "$1: '$3' in: $2"; else ok; fi; }

mkdir -p "$T/bin" "$T/opt/config" "$T/opt/data" "$T/opt/logs" "$T/opt/backups" "$T/systemd"
cat > "$T/bin/systemctl" <<'S'
#!/usr/bin/env bash
for last; do :; done
case "$1" in
  is-enabled) case " $STUB_ENABLED " in *" $last "*) exit 0 ;; esac; exit 1 ;;
  is-active)
    case " ${STUB_DOWN:-} " in *" $last "*) [ "$2" = --quiet ] || echo failed; exit 3 ;; esac
    [ "$2" = --quiet ] || echo active; exit 0 ;;
esac
S
cat > "$T/bin/openssl" <<'S'
#!/usr/bin/env bash
case "$1" in
  s_client) [ -n "${STUB_NO_TLS:-}" ] || echo '-----BEGIN CERTIFICATE-----' ;;
  x509) [ -z "${STUB_CERT_OLD:-}" ] ;;
esac
S
cat > "$T/bin/curl" <<'S'
#!/usr/bin/env bash
# the webhook and ping requests read their address from stdin (-K -)
case " $* " in
  *" -K - "*)
    u=$(cat)
    case "$u" in
      *hook*)
        [ -z "${STUB_HOOK_BROKEN:-}" ] || exit 22
        while [ $# -gt 0 ]; do [ "$1" = -d ] && printf '%s\n' "$2" >> "$STUB_POSTS"; shift; done ;;
      *ping*) echo ping >> "$STUB_PINGS" ;;
    esac
    exit 0 ;;
  *https://*) [ -n "${STUB_NO_WEB:-}" ] && { printf '000'; exit 7; }; printf '200'; exit 0 ;;
  *http://*/ops/alerts*) [ -n "${STUB_NO_ADMIN:-}" ] && exit 7; printf '%s' "${STUB_OPS:-[]}"; exit 0 ;;
  *http://*/status*) [ -n "${STUB_NO_ADMIN:-}" ] && exit 7; printf '%s' "${STUB_STATUS:-{\"rooms\":[]\}}"; exit 0 ;;
esac
S
cat > "$T/bin/df" <<'S'
#!/usr/bin/env bash
echo "Filesystem 1024-blocks Used Available Capacity Mounted"
echo "/dev/x 100 50 50 ${STUB_DISK:-50}% /"
S
cat > "$T/bin/timeout" <<'S'
#!/usr/bin/env bash
shift; exec "$@"
S
chmod +x "$T"/bin/*
export PATH="$T/bin:$PATH" STUB_POSTS="$T/posts" STUB_PINGS="$T/pings" STUB_ENABLED="decombat-lobby"
for s in healthcheck common; do tr -d '\r' < "$VPS/$s.sh" > "$T/$s.sh"; done

printf '[server]\nauth_port = 10000\nhttp_port = 0\n' > "$T/opt/config/server.toml"
printf 'x\n' > "$T/opt/data/tls_cert.pem"
: > "$T/opt/backups/last_ok"
printf 'WEBHOOK_URL=https://example.invalid/hook\nPING_URL=https://example.invalid/ping\nLABEL=Test box\nFAIL_AFTER=2\nDISK_MAX_PCT=90\nBACKUP_MAX_HOURS=30\nBACKUP_DIR=%s/opt/backups\n' "$T" > "$DC_ALERTS_CONF"

run() { bash "$T/healthcheck.sh" 2>&1; }
posts() { cat "$STUB_POSTS" 2>/dev/null; }
reset_posts() { : > "$STUB_POSTS"; }

# ---- all fine: exit 0, nothing posted, the dead-man is pinged
reset_posts
out=$(run); rc=$?
check "all fine: exit 0: $out" test "$rc" -eq 0
check "all fine: no post" test ! -s "$STUB_POSTS"
check "the dead-man was pinged" test -s "$STUB_PINGS"
out=$(bash "$T/healthcheck.sh" --print 2>&1); rc=$?
check "--print exit 0" test "$rc" -eq 0
for name in default.unit default.auth default.web default.cert default.admin box.disk box.backup; do
    has "--print shows $name" "$out" "$name"
done
has "the web port follows auth + 8080" "$out" "18080"

# ---- a stopped unit: quiet for one run, told at the second, not again at the third, recovery told once
export STUB_DOWN="decombat-lobby"
reset_posts
out=$(run); rc=$?
check "down: exit 1" test "$rc" -eq 1
check "down: the first run is quiet (a deploy's restart)" test ! -s "$STUB_POSTS"
out=$(run); rc=$?
check "down: still exit 1" test "$rc" -eq 1
has "down: told at the second run" "$(posts)" "[DOWN] Test box: decombat-lobby is failed"
reset_posts
out=$(run)
check "down: told only once" test ! -s "$STUB_POSTS"
unset STUB_DOWN
out=$(run); rc=$?
check "back: exit 0" test "$rc" -eq 0
has "back: told" "$(posts)" "[OK] Test box"
reset_posts
out=$(run)
check "back: told only once" test ! -s "$STUB_POSTS"

# ---- a webhook that does not answer: the change is told when it answers again
export STUB_NO_TLS=1
reset_posts
out=$(run)
out=$(STUB_HOOK_BROKEN=1 run)
has "a failed post says so" "$out" "the webhook post failed"
check "nothing posted while it is broken" test ! -s "$STUB_POSTS"
out=$(run)
has "told when the hook is back" "$(posts)" "no TLS answer on port 10000"
unset STUB_NO_TLS
out=$(run)

# ---- EMAIL_TO: the same change goes out as a mail through the lobby's [mail] settings (written to a file here)
printf '[mail]\nsmtp_host = "smtp.example.invalid"\nsmtp_port = 465\nuser = "u"\nfrom = "DeCombat <noreply@example.com>"\n' > "$T/mail.toml"
printf 'DC_MAIL__PASSWORD=secret-not-printed\n' > "$T/mail.env"
cp "$DC_ALERTS_CONF" "$T/alerts.webhook"
printf 'EMAIL_TO=ops@example.com\n' >> "$DC_ALERTS_CONF"
export DC_ALERT_MAIL_TOML="$T/mail.toml" DC_ALERT_MAIL_ENV="$T/mail.env" DC_ALERT_MAIL_FILE="$T/mails" STUB_DOWN="decombat-lobby"
: > "$T/mails"; reset_posts
out=$(run); out=$(run)
has "mail: the subject names the change" "$(cat "$T/mails")" "Subject: [DeCombat alert] [DOWN] Test box"
has "mail: to EMAIL_TO" "$(cat "$T/mails")" "To: ops@example.com"
has "mail: the webhook still gets it" "$(posts)" "[DOWN] Test box"
check "mail: the password is in no mail" bash -c "! grep -q secret-not-printed '$T/mails'"
unset STUB_DOWN
out=$(run)
has "mail: recovery mailed too" "$(cat "$T/mails")" "[OK] Test box"
printf 'EMAIL_TO=ops@example.com\n' > "$DC_ALERTS_CONF.mailonly"
grep -v '^WEBHOOK_URL=' "$DC_ALERTS_CONF" > "$DC_ALERTS_CONF.mailonly"
: > "$T/mails"
out=$(DC_ALERTS_CONF="$DC_ALERTS_CONF.mailonly" STUB_DOWN=decombat-lobby run); out=$(DC_ALERTS_CONF="$DC_ALERTS_CONF.mailonly" STUB_DOWN=decombat-lobby run)
has "mail only (no webhook): mailed" "$(cat "$T/mails")" "[DOWN] Test box"
out=$(DC_ALERTS_CONF="$DC_ALERTS_CONF.mailonly" run)
cp "$T/alerts.webhook" "$DC_ALERTS_CONF"
unset DC_ALERT_MAIL_TOML DC_ALERT_MAIL_ENV DC_ALERT_MAIL_FILE

# ---- each check can fail
one() { # DESCRIPTION WANT VAR=VALUE...
    local d=$1 want=$2
    shift 2
    out=$(env "$@" bash "$T/healthcheck.sh" --print 2>&1); rc=$?
    check "$d: exit 1" test "$rc" -eq 1
    has "$d" "$out" "$want"
}
one "web port" "no https answer on port 18080" STUB_NO_WEB=1
one "admin" "admin /status on 127.0.0.1:10090 does not answer" STUB_NO_ADMIN=1
one "disk" "disk 95% full" STUB_DISK=95
one "certificate" "expires within 14 days" STUB_CERT_OLD=1
one "host agent" "game host offline: us-west" 'STUB_STATUS={"hosts":[{"name":"us-west","online":false},{"name":"eu","online":true}]}'
out=$(env 'STUB_STATUS={"hosts":[{"name":"eu","online":true}]}' bash "$T/healthcheck.sh" --print 2>&1); rc=$?
check "a host online is fine" test "$rc" -eq 0
# ---- the monitor's checks (MON-5): failing ones count, host lines are left to the admin check, an old lobby adds none
OPS_BAD='[{"name":"silent_drops","ok":false,"text":"match 4 (room 1-2, W): 3 silent drops"},{"name":"host.us-west","ok":false,"text":"x"},{"name":"version.eu","ok":false,"text":"game host eu runs build abc, the lobby def"},{"name":"errors","ok":true,"text":"2 problem events"}]'
out=$(env "STUB_OPS=$OPS_BAD" bash "$T/healthcheck.sh" --print 2>&1); rc=$?
check "ops: exit 1" test "$rc" -eq 1
has "ops: silent drops" "$out" "default.ops.silent_drops fail  match 4 (room 1-2, W): 3 silent drops"
has "ops: build mismatch" "$out" "game host eu runs build abc, the lobby def"
has "ops: a fine check is listed" "$out" "default.ops.errors"
hasnt "ops: host lines skipped" "$out" "default.ops.host"
out=$(env 'STUB_OPS={"ok": false, "message": "not found"}' bash "$T/healthcheck.sh" --print 2>&1); rc=$?
check "ops: an older lobby (404) adds nothing: exit 0" test "$rc" -eq 0
hasnt "ops: an older lobby adds no line" "$out" "default.ops"
reset_posts
out=$(env "STUB_OPS=$OPS_BAD" bash "$T/healthcheck.sh" 2>&1)
out=$(env "STUB_OPS=$OPS_BAD" bash "$T/healthcheck.sh" 2>&1)
has "ops: told at the second run" "$(posts)" "3 silent drops"
OPS_FINE='[{"name":"silent_drops","ok":true,"text":"no match with 3 or more silent drops"},{"name":"version.eu","ok":true,"text":"game host eu runs the lobby build"}]'
out=$(env "STUB_OPS=$OPS_FINE" bash "$T/healthcheck.sh" 2>&1)
has "ops: recovery told" "$(posts)" "[OK] Test box: no match with 3 or more silent drops"
reset_posts
out=$(run)
touch -d '3 days ago' "$T/opt/backups/last_ok"
out=$(bash "$T/healthcheck.sh" --print 2>&1); rc=$?
check "old backup: exit 1" test "$rc" -eq 1
has "old backup" "$out" "the last backup is 72 hours old"
rm -f "$T/opt/backups/last_ok"
out=$(bash "$T/healthcheck.sh" --print 2>&1)
has "no backup" "$out" "no finished backup yet"
: > "$T/opt/backups/last_ok"

# ---- HOSTS_OPTIONAL, no enabled unit, bots
printf 'HOSTS_OPTIONAL=us-west\n' >> "$DC_ALERTS_CONF"
out=$(env 'STUB_STATUS={"hosts":[{"name":"us-west","online":false}]}' bash "$T/healthcheck.sh" --print 2>&1); rc=$?
check "an optional host is no alarm: $out" test "$rc" -eq 0
out=$(STUB_ENABLED="" bash "$T/healthcheck.sh" --print 2>&1); rc=$?
check "no enabled unit: exit 1" test "$rc" -eq 1
has "no enabled unit" "$out" "no enabled lobby unit"
out=$(STUB_ENABLED="decombat-lobby decombat-bots" STUB_DOWN="decombat-bots" bash "$T/healthcheck.sh" --print 2>&1); rc=$?
check "bots down: exit 1" test "$rc" -eq 1
has "bots down" "$out" "decombat-bots is not active"
out=$(bash "$T/healthcheck.sh" --print 2>&1)
hasnt "bots not enabled: not checked" "$out" "box.bots"

printf 'healthcheck_test: %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
