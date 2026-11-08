#!/usr/bin/env bash
# triage_test.sh: tests of tools/vps/discord_triage.sh and triage_setup.sh (docs/DEPLOY_FROM_ADMIN.md "Bug reports
# from Discord") on a temp root with a stand-in curl that answers as the Discord REST API from a queue of files. No
# network. Runs under Git Bash on Windows and on Linux: bash tools/vps/tests/triage_test.sh
# Provenance: ORIGINAL
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VPS=$(cd "$HERE/.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/dc-triage-test.XXXXXX")
trap 'rm -rf "${T:?}"' EXIT

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL: %s\n' "$*"; }
expect() { local d=$1; shift; if "$@"; then ok; else bad "$d"; fi; }
has() { grep -qF -- "$2" "$1" 2>/dev/null; }
lacks() { ! grep -qF -- "$2" "$1" 2>/dev/null; }
json() { "$PY" -c 'import json,sys; d=json.load(open(sys.argv[1],encoding="utf-8")); v=eval(sys.argv[2],{"d":d}); print(json.dumps(v) if not isinstance(v,str) else v)' "$1" "$2"; }
# jl FILE EXPR: the same over the lines of a .jsonl file (d is the list of objects)
jl() { "$PY" -c 'import json,sys; d=[json.loads(l) for l in open(sys.argv[1],encoding="utf-8") if l.strip()]; v=eval(sys.argv[2],{"d":d}); print(json.dumps(v) if not isinstance(v,str) else v)' "$1" "$2"; }

PY=""
for c in python3 python; do if "$c" -c "pass" >/dev/null 2>&1; then PY=$c; break; fi; done
[ -n "$PY" ] || { echo "no python: skipped"; exit 0; }

for f in "$VPS/discord_triage.sh" "$VPS/triage_setup.sh" "$HERE/triage_test.sh"; do
    expect "bash -n $(basename "$f")" bash -n "$f"
done

export DC_TEST=1 DC_ROOT="$T/opt" DC_SYSTEMD_DIR="$T/systemd" DC_PYTHON="$PY" STUB_LOG="$T/stub.log"
unset DC_ADMIN DC_INSTANCE DC_UNIT DC_DEPLOY_DIR
D="$DC_ROOT/deploy"
TD="$D/triage"
REPORTS="$TD/reports.jsonl"
STATE="$TD/state.json"
CHAN=1234567890123456
# a token of the right shape that no secret scanner takes for a real one
TOKEN="tok.$(date +%s).qwertyuiopasdfghjklzxcvbnm-ASDF"
export EXPECT_TOKEN=$TOKEN EXPECT_CHANNEL=$CHAN
mkdir -p "$D" "$DC_SYSTEMD_DIR" "$DC_ROOT/tools"

# ---- stand-ins: curl answers the next entry of $T/q (NNN.body, NNN.code, NNN.head), the sleeper logs
S="$T/stubs"
mkdir -p "$S" "$T/q"
cat > "$S/curl.py" <<'X'
import glob, os, re, sys
q, log = sys.argv[1], sys.argv[2]
a = sys.argv[3:]
head, out, fmt = None, None, ""
i = 0
while i < len(a):
    if a[i] == "-D": head = a[i + 1]; i += 1
    elif a[i] == "-o": out = a[i + 1]; i += 1
    elif a[i] == "-w": fmt = a[i + 1]; i += 1
    elif a[i] == "--max-time": i += 1
    i += 1
url, auth, ua = "", "none", ""
for ln in sys.stdin.read().splitlines():
    m = re.match(r'^(\w+) = "(.*)"$', ln)
    if not m: continue
    if m.group(1) == "url": url = m.group(2)
    elif m.group(2).startswith("Authorization: Bot "):
        auth = "ok" if m.group(2)[len("Authorization: Bot "):] == os.environ.get("EXPECT_TOKEN") else "wrong"
    elif m.group(2).startswith("User-Agent: "): ua = m.group(2)[12:]
path = re.sub(r"^https?://[^/]+/api/v10/", "", url)
argv_has_token = any(os.environ.get("EXPECT_TOKEN", "\0") in x for x in sys.argv)
with open(log, "a", encoding="utf-8") as f:
    f.write("curl GET %s auth=%s argv_token=%s ua=%s\n" % (path, auth, argv_has_token, ua.split(" ")[0]))
files = sorted(glob.glob(os.path.join(q, "*.body")))
if not files:
    body, code, hd = "{}", "500", ""
    with open(log, "a", encoding="utf-8") as f: f.write("EMPTY QUEUE\n")
else:
    base = files[0][:-5]
    body = open(base + ".body", encoding="utf-8").read()
    code = open(base + ".code").read().strip() if os.path.isfile(base + ".code") else "200"
    hd = open(base + ".head", encoding="utf-8").read() if os.path.isfile(base + ".head") else ""
    for ext in (".body", ".code", ".head"):
        if os.path.isfile(base + ext): os.remove(base + ext)
if out: open(out, "w", encoding="utf-8").write(body)
if head: open(head, "w", encoding="utf-8").write("HTTP/2 %s\r\n%s" % (code, hd))
sys.stdout.write(fmt.replace("%{http_code}", code))
X
cat > "$S/curl" <<X
#!/usr/bin/env bash
exec "$PY" "$S/curl.py" "$T/q" "\$STUB_LOG" "\$@"
X
cat > "$S/nap" <<'X'
#!/usr/bin/env bash
echo "sleep $*" >> "$STUB_LOG"
X
chmod +x "$S/curl" "$S/nap"
export DC_TRIAGE_CURL="$S/curl" DC_TRIAGE_SLEEP="$S/nap"
N=0
# q CODE BODY [HEADERS]: queue one answer
q() {
    N=$((N + 1))
    local f
    f=$(printf '%s/%04d' "$T/q" "$N")
    printf '%s' "$2" > "$f.body"
    printf '%s' "$1" > "$f.code"
    [ -z "${3:-}" ] || printf '%b' "$3" > "$f.head"
}
poll() { bash "$VPS/discord_triage.sh" poll > "$T/out" 2>&1; rc=$?; }
requests() { grep -c '^curl ' "$STUB_LOG" 2>/dev/null || true; }
reset_log() { : > "$STUB_LOG"; }
: > "$STUB_LOG"
SCRIPT="$DC_ROOT/tools/discord_triage.sh"

# ---- triage_setup.sh: refuses without a conf, then installs
bash "$VPS/triage_setup.sh" > "$T/out" 2>&1
expect "setup without a conf refuses" test $? -ne 0
expect "and says where to write it" has "$T/out" "triage.conf"
expect "and enables nothing" test ! -e "$DC_SYSTEMD_DIR/decombat-triage.timer"
printf 'token=%s\nchannel_id=%s\n' "$TOKEN" "$CHAN" > "$D/triage.conf"
q 200 '{"id":"'$CHAN'","name":"bug-reports"}'
q 200 '[]'
SYSCTL="$S/systemctl"
printf '#!/usr/bin/env bash\necho "systemctl $*" >> "$STUB_LOG"\n' > "$SYSCTL"
chmod +x "$SYSCTL"
out=$(DC_TRIAGE_SYSTEMCTL="$SYSCTL" bash "$VPS/triage_setup.sh" 2>&1)
rc=$?
expect "setup succeeds ($out)" test "$rc" -eq 0
expect "setup never prints the token" test -z "$(printf '%s' "$out" | grep -F "$TOKEN")"
expect "it asked Discord for the channel first" has "$STUB_LOG" "curl GET channels/$CHAN auth=ok"
expect "the bot name is shown" has <(printf '%s' "$out") "bug-reports"
expect "the poller is installed" test -x "$SCRIPT"
expect "done.json starts empty" test "$(json "$TD/done.json" 'd')" = '{"done": {}}'
expect "the timer is every 5 minutes" has "$DC_SYSTEMD_DIR/decombat-triage.timer" "OnUnitActiveSec=5min"
expect "the service runs the poller" has "$DC_SYSTEMD_DIR/decombat-triage.service" "discord_triage.sh poll"
expect "the lobby may write done.json" has "$DC_SYSTEMD_DIR/decombat-lobby.service.d/22-triage.conf" "ReadWritePaths=-$TD/done.json"
expect "setup enables the timer" has "$STUB_LOG" "enable --now decombat-triage.timer"
expect "and runs one poll" test "$(json "$STATE" 'd["ok"]')" = true
printf 'token=short\nchannel_id=%s\n' "$CHAN" > "$D/triage.conf.bad"
cp "$D/triage.conf" "$D/triage.conf.good"
cp "$D/triage.conf.bad" "$D/triage.conf"
reset_log
DC_TRIAGE_SYSTEMCTL="$SYSCTL" bash "$VPS/triage_setup.sh" > "$T/out" 2>&1
expect "setup with a malformed token refuses" test $? -ne 0
expect "and calls nobody" test "$(requests)" = 0
cp "$D/triage.conf.good" "$D/triage.conf"
rm -f "$D/triage.conf.bad" "$D/triage.conf.good" "$STATE" "$REPORTS"

# ---- an empty channel
reset_log
q 200 '[]'
poll
expect "an empty channel is fine" test "$rc" -eq 0
expect "state.json is ok" test "$(json "$STATE" '[d["ok"], d["fetched"], d["last_id"]]')" = '[true, 0, ""]'
expect "no after on the first poll" has "$STUB_LOG" "channels/$CHAN/messages?limit=100 auth=ok argv_token=False ua=DiscordBot"
expect "no reports file content" test ! -s "$REPORTS"
for k in last_id updated fetched ok message; do
    expect "state.json has $k" test "$(json "$STATE" "\"$k\" in d")" = true
done

# ---- three messages, newest first as Discord sends them; a join message without text is skipped
reset_log
AT=$("$PY" -c 'import datetime; print(int(datetime.datetime.fromisoformat("2026-11-08T10:00:00.123000+00:00").timestamp()))')
q 200 '[
 {"id":"1003","timestamp":"2026-11-08T10:02:00.000000+00:00","content":"third <b>bold</b>\r\nline two","author":{"username":"carol","global_name":null},"attachments":[]},
 {"id":"1002","timestamp":"2026-11-08T10:01:00.000000+00:00","content":"","type":7,"author":{"username":"bob","global_name":"Bob"},"attachments":[]},
 {"id":"1001","timestamp":"2026-11-08T10:00:00.123000+00:00","content":"M32 acts like a grenade","author":{"username":"alice","global_name":"Alice A"},"attachments":[{"url":"https://cdn.discordapp.com/attachments/1/2/shot.png?ex=1","filename":"shot.png"}]}
]'
poll
expect "the poll succeeds" test "$rc" -eq 0
expect "two reports (the empty join message is skipped)" test "$(jl "$REPORTS" 'len(d)')" = 2
expect "oldest first" test "$(jl "$REPORTS" '[x["id"] for x in d]')" = '["1001", "1003"]'
expect "the author has both names" test "$(jl "$REPORTS" 'd[0]["author"]')" = '{"username": "alice", "name": "Alice A"}'
expect "a missing display name falls back to the username" test "$(jl "$REPORTS" 'd[1]["author"]["name"]')" = carol
expect "the time is unix" test "$(jl "$REPORTS" 'd[0]["at"]')" = "$AT"
expect "the attachment url" test "$(jl "$REPORTS" 'd[0]["attachments"]')" = '["https://cdn.discordapp.com/attachments/1/2/shot.png?ex=1"]'
expect "text is kept as text" test "$(jl "$REPORTS" '[d[1]["content"]]')" = '["third <b>bold</b>\nline two"]'
expect "state counts the new reports" test "$(json "$STATE" '[d["fetched"], d["last_id"], d["ok"]]')" = '[2, "1003", true]'
expect "one request" test "$(requests)" = 1

# ---- the next poll asks after the last id; a repeated message is not stored twice
reset_log
q 200 '[{"id":"1003","timestamp":"2026-11-08T10:02:00.000000+00:00","content":"third again","author":{"username":"carol"},"attachments":[]},
        {"id":"1004","timestamp":"2026-11-08T10:03:00.000000+00:00","content":"fourth","author":{"username":"dave"},"attachments":[]}]'
poll
expect "asks after=1003" has "$STUB_LOG" "messages?limit=100&after=1003 "
expect "one new report" test "$(jl "$REPORTS" '[x["id"] for x in d]')" = '["1001", "1003", "1004"]'
expect "the stored text of a repeat is untouched" test "$(jl "$REPORTS" 'd[1]["content"].startswith("third <b>")')" = true
expect "fetched is 1" test "$(json "$STATE" 'd["fetched"]')" = 1
q 200 '[]'
poll
expect "nothing new: nothing added" test "$(jl "$REPORTS" 'len(d)')" = 3
expect "last_id stays" test "$(json "$STATE" 'd["last_id"]')" = 1004
cp "$REPORTS" "$T/reports.before"
q 200 '[{"id":"1004","timestamp":"2026-11-08T10:03:00.000000+00:00","content":"fourth","author":{"username":"dave"},"attachments":[]}]'
poll
expect "a poll that returns only known ids changes nothing" cmp -s "$REPORTS" "$T/reports.before"

# ---- 429: Retry-After header, then the body's retry_after, then one too long
reset_log
q 429 '{"message":"You are being rate limited.","retry_after":9.0,"global":false}' 'Retry-After: 2\r\n'
q 200 '[{"id":"1005","timestamp":"2026-11-08T10:04:00.000000+00:00","content":"after the wait","author":{"username":"erin"},"attachments":[]}]'
poll
expect "a 429 waits the header's seconds" has "$STUB_LOG" "sleep 2"
expect "and retries" test "$rc" -eq 0
expect "the report arrived" test "$(jl "$REPORTS" 'd[-1]["id"]')" = 1005
reset_log
q 429 '{"message":"You are being rate limited.","retry_after":1.5,"global":false}'
q 200 '[]'
poll
expect "without a header the body's retry_after (rounded up)" has "$STUB_LOG" "sleep 2"
reset_log
q 429 '{"retry_after":3}' 'Retry-After: 3\r\n'
q 429 '{"retry_after":3}' 'Retry-After: 3\r\n'
q 429 '{"retry_after":3}' 'Retry-After: 3\r\n'
poll
expect "429 three times in a row gives up with exit 0" test "$rc" -eq 0
expect "and says so" test "$(json "$STATE" '[d["ok"], "rate limited" in d["message"], d["last_id"]]')" = '[false, true, "1005"]'
reset_log
q 429 '{"retry_after":600}' 'Retry-After: 600\r\n'
poll
expect "a wait over the limit is not slept" test "$(grep -c '^sleep' "$STUB_LOG")" = 0
expect "exit 0 and the message names the seconds" test "$rc,$(json "$STATE" '"600 s" in d["message"]')" = "0,true"
rm -f "$T"/q/*

# ---- failures keep last_id and the reports
cp "$REPORTS" "$T/reports.before"
for c1 in "401|Discord refused the token" "403|may not read the channel" "404|no such channel" "500|HTTP 500"; do
    q "${c1%%|*}" '{"message":"x","code":0}'
    poll
    expect "HTTP ${c1%%|*} fails" test "$rc" -eq 1
    expect "HTTP ${c1%%|*}: the message" has "$STATE" "${c1#*|}"
    expect "HTTP ${c1%%|*}: ok false, last_id kept" test "$(json "$STATE" '[d["ok"], d["last_id"]]')" = '[false, "1005"]'
done
expect "the reports are untouched by failures" cmp -s "$REPORTS" "$T/reports.before"
q 200 'this is { not json'
poll
expect "malformed JSON fails" test "$rc" -eq 1
expect "state says malformed" has "$STATE" "malformed JSON"
q 200 '{"message":"an object, not a list"}'
poll
expect "a 200 that is no list fails too" test "$rc" -eq 1
expect "and stores nothing" cmp -s "$REPORTS" "$T/reports.before"
q 200 '[{"id":"not-a-number","content":"x"},{"content":"no id"},"string",7,{"id":"1006","timestamp":"garbage","content":"odd times","author":"nobody","attachments":"no"}]'
poll
expect "garbage entries are skipped, odd fields tolerated" test "$rc" -eq 0
expect "only the usable one is stored" test "$(jl "$REPORTS" '[d[-1]["id"], len(d)]')" = '["1006", 5]'
expect "a bad timestamp falls back to the id's time" test "$(jl "$REPORTS" 'd[-1]["at"] >= 1420070400')" = true

# ---- a full page continues after its highest id; the first poll does not
rm -f "$STATE" "$REPORTS" "$T"/q/*
reset_log
"$PY" - "$T" <<'X'
import json, sys
t = sys.argv[1]
def msg(i): return {"id": str(5000 + i), "timestamp": "2026-11-08T10:00:00+00:00", "content": "m%d" % i, "author": {"username": "u"}, "attachments": []}
for name, rng in (("a", range(100, 0, -1)), ("b", range(200, 100, -1)), ("c", range(210, 200, -1))):
    open("%s/page_%s.json" % (t, name), "w").write(json.dumps([msg(i) for i in rng]))
X
q 200 "$(cat "$T/page_a.json")"
poll
expect "a first poll of 100 messages takes only those" test "$(requests)" = 1
expect "100 stored" test "$(jl "$REPORTS" 'len(d)')" = 100
expect "last_id is the newest" test "$(json "$STATE" 'd["last_id"]')" = 5100
reset_log
q 200 "$(cat "$T/page_b.json")"
q 200 "$(cat "$T/page_c.json")"
poll
expect "the next poll pages: a full page then a short one" test "$(requests)" = 2
expect "the second asks after the first page's highest id" has "$STUB_LOG" "after=5200 "
expect "210 stored, no id twice" test "$(jl "$REPORTS" '[len(d), len(set(x["id"] for x in d))]')" = '[210, 210]'
expect "oldest first overall" test "$(jl "$REPORTS" '[x["id"] for x in d] == sorted(x["id"] for x in d)')" = true
expect "last_id is the newest" test "$(json "$STATE" '[d["last_id"], d["fetched"]]')" = '["5210", 110]'
rm -f "$T"/q/*

# ---- the token: never in a log, the state, a report or a command line; the conf may be malformed or missing
leaks=$(grep -rlF -- "$TOKEN" "$TD" "$DC_ROOT/tools" "$DC_SYSTEMD_DIR" 2>/dev/null)
expect "no stored file holds the token (${leaks})" test -z "$leaks"
expect "curl never had it on its command line" lacks "$STUB_LOG" "argv_token=True"
expect "the header carried it" has "$STUB_LOG" "auth=ok"
expect "the poll output never shows it" lacks "$T/out" "$TOKEN"
cp "$D/triage.conf" "$T/conf.keep"
printf 'token=%s\r\nchannel_id=%s\r\n' "$TOKEN" "$CHAN" > "$D/triage.conf"
q 200 '[]'
poll
expect "CRLF in the conf is fine" test "$rc" -eq 0
printf 'channel_id=%s\n' "$CHAN" > "$D/triage.conf"
reset_log
poll
expect "a conf without a token fails" test "$rc" -eq 1
expect "without calling Discord" test "$(requests)" = 0
expect "state names the conf" has "$STATE" "triage.conf"
rm -f "$D/triage.conf"
poll
expect "no conf fails the same way" test "$rc" -eq 1
cp "$T/conf.keep" "$D/triage.conf"

# ---- status and setup-check
out=$(bash "$VPS/discord_triage.sh" status 2>&1)
expect "status prints state.json" test -n "$(printf '%s' "$out" | grep '"last_id"')"
expect "and the report count" test -n "$(printf '%s' "$out" | grep 'reports stored: 210')"
q 404 '{"message":"Unknown Channel","code":10003}'
out=$(bash "$VPS/discord_triage.sh" setup-check 2>&1)
expect "setup-check fails on an unknown channel" test $? -ne 0
expect "and says why" test -n "$(printf '%s' "$out" | grep 'no such channel')"
bash "$VPS/discord_triage.sh" bogus > "$T/out" 2>&1
expect "an unknown verb is refused" test $? -ne 0

printf 'passed %d, failed %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
