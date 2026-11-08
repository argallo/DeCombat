#!/usr/bin/env bash
# discord_triage.sh: the bug-report poller, as root on the VPS, run by decombat-triage.timer every 5 minutes
# (docs/DEPLOY_FROM_ADMIN.md "Bug reports from Discord"). Provenance: ORIGINAL (DeCombat operator tooling).
# WRITTEN ON WINDOWS, NOT RUN ON LINUX YET.
#
#   discord_triage.sh poll          read the new messages of the bug-report channel through the Discord REST API and
#                                   append them, oldest first, to deploy/triage/reports.jsonl; update state.json
#   discord_triage.sh status        print state.json and the number of stored reports
#   discord_triage.sh setup-check   check deploy/triage.conf and ask Discord once whether the bot sees the channel
#
# deploy/triage.conf (root 0600, read as KEY=value data, never sourced):
#   token=<the bot token>           channel_id=<the numeric id of the bug-report channel>
# The token reaches curl in a config on standard input: never on a command line, in a log, in state.json or in a report.
#
# reports.jsonl: one JSON object per line, oldest first, no id twice:
#   {"id": "<snowflake>", "at": <unix>, "author": {"username": text, "name": text}, "content": text, "attachments": [urls]}
# state.json: {"last_id": text, "updated": <unix>, "fetched": <new reports of the last poll>, "ok": bool, "message": text}
# The first poll reads the newest 100 messages; later polls read `after=<last_id>`, up to 20 pages of 100.
# Exit 0: done, or a rate limit longer than this run waits (the next run retries). Exit 1: the poll failed (state.json
# says why and keeps last_id).
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

D=${DC_DEPLOY_DIR:-$DC_ROOT/deploy}
T=$D/triage
CONF=$D/triage.conf
REPORTS=$T/reports.jsonl
STATE=$T/state.json
W=$T/work
# the seams tools/vps/tests/triage_test.sh replaces
CURL=${DC_TRIAGE_CURL:-curl}
API=${DC_TRIAGE_API:-https://discord.com/api/v10}
NAP=${DC_TRIAGE_SLEEP:-sleep}
# DC-DEFAULT: OPS-26 (proposed) the poll limits
PAGES_MAX=${DC_TRIAGE_PAGES:-20}
WAIT_MAX=${DC_TRIAGE_WAIT_MAX:-60}
TRIES_MAX=${DC_TRIAGE_TRIES:-3}
ID_RE='^[0-9]{5,25}$'
TOKEN_RE='^[A-Za-z0-9._-]{20,200}$'

# conf KEY: a line KEY=value of triage.conf (the last one wins), read as data
conf() {
    sed -n "s/^$1=//p" "$CONF" 2>/dev/null | tr -d '\r' | tail -n 1
}

# group-readable for the lobby: root writes, decombat reads
share() {
    chmod 0640 "$@" 2>/dev/null || true
    [ "$DC_TEST" = 1 ] || chgrp "$DC_USER" "$@" 2>/dev/null || true
}

TOKEN="" CHANNEL="" LAST=""

# state_set OK FETCHED MESSAGE: state.json, whole, with the last_id in $LAST
state_set() {
    mkdir -p "$T"
    "$DC_PYTHON" -c '
import json, os, sys, time
path, last, ok, fetched, msg = sys.argv[1:6]
msg = "".join(c if c.isprintable() else " " for c in msg)[:300]
d = {"last_id": last, "updated": int(time.time()), "fetched": int(fetched), "ok": ok == "true", "message": msg}
with open(path + ".tmp", "w", encoding="utf-8") as f:
    json.dump(d, f, indent=1)
os.replace(path + ".tmp", path)
' "$STATE" "$LAST" "$1" "$2" "$3" && share "$STATE"
    return 0
}

fail() {
    say "triage: failed: $1"
    state_set false 0 "$1"
    exit 1
}

load_conf() {
    [ -f "$CONF" ] || return 1
    TOKEN=$(conf token)
    CHANNEL=$(conf channel_id)
    [[ $TOKEN =~ $TOKEN_RE ]] && [[ $CHANNEL =~ $ID_RE ]]
}

# get PATH: GET API/PATH; the body in $W/page.json, the headers in $W/head.txt, the status in $CODE
CODE=""
get() {
    mkdir -p "$W"
    rm -f "$W/page.json" "$W/head.txt"
    CODE=$({
        printf 'url = "%s/%s"\n' "$API" "$1"
        printf 'header = "Authorization: Bot %s"\n' "$TOKEN"
        printf 'header = "User-Agent: DiscordBot (https://decombat.com, 1.0)"\n'
        printf 'header = "Accept: application/json"\n'
    } | "$CURL" -sS --max-time 30 -K - -D "$W/head.txt" -o "$W/page.json" -w '%{http_code}' 2>/dev/null)
    [ -f "$W/page.json" ] || : > "$W/page.json"
    [ -f "$W/head.txt" ] || : > "$W/head.txt"
    case "$CODE" in 2[0-9][0-9]) return 0 ;; esac
    return 1
}

# retry_after: whole seconds to wait after a 429, from the Retry-After header or the body's retry_after
retry_after() {
    "$DC_PYTHON" -c '
import json, math, re, sys
w = None
try:
    for ln in open(sys.argv[1], encoding="utf-8", errors="replace"):
        m = re.match(r"(?i)^retry-after:\s*([0-9.]+)", ln)
        if m:
            w = float(m.group(1))
except OSError:
    pass
if w is None:
    try:
        w = float(json.load(open(sys.argv[2], encoding="utf-8")).get("retry_after"))
    except (OSError, ValueError, TypeError, AttributeError):
        w = 5.0
print(int(min(max(math.ceil(w), 1), 86400)))
' "$W/head.txt" "$W/page.json" 2>/dev/null || echo 5
}

# fetch PATH: get with the 429 handling. 0 ok; 1 failed (FAIL_MSG); 2 rate limit longer than this run waits
FAIL_MSG=""
fetch() {
    local try=0 wait
    while :; do
        if get "$1"; then return 0; fi
        case "$CODE" in
            429)
                wait=$(retry_after)
                try=$((try + 1))
                if [ "$wait" -gt "$WAIT_MAX" ] || [ $try -ge "$TRIES_MAX" ]; then
                    FAIL_MSG="rate limited by Discord (retry after $wait s); the next run tries again"
                    return 2
                fi
                say "triage: rate limited, waiting $wait s"
                "$NAP" "$wait"
                ;;
            401) FAIL_MSG="Discord refused the token (HTTP 401): make a new one in the Developer Portal and rewrite triage.conf"; return 1 ;;
            403) FAIL_MSG="the bot may not read the channel (HTTP 403): it needs View Channel and Read Message History there"; return 1 ;;
            404) FAIL_MSG="no such channel (HTTP 404): check channel_id and that the bot is in the server"; return 1 ;;
            *) FAIL_MSG="Discord answered HTTP ${CODE:-none} (no answer or an error)"; return 1 ;;
        esac
    done
}

# ingest: append the page in $W/page.json to reports.jsonl, oldest first, without ids already there.
# Prints "<messages in the page> <highest id> <appended>"; exit 2 when the page is not a list of messages.
ingest() {
    "$DC_PYTHON" -c '
import datetime, json, os, sys
reports, page = sys.argv[1:3]
try:
    with open(page, encoding="utf-8") as f:
        msgs = json.load(f)
except (OSError, ValueError):
    sys.exit(2)
if not isinstance(msgs, list):
    sys.exit(2)
seen = set()
try:
    with open(reports, encoding="utf-8", errors="replace") as f:
        for ln in f:
            try:
                seen.add(str(json.loads(ln).get("id")))
            except (ValueError, AttributeError):
                pass
except OSError:
    pass

def clean(v, n):
    v = v if isinstance(v, str) else ""
    return "".join(c if (c.isprintable() or c in "\n\t") else " " for c in v.replace("\r", ""))[:n]

rows, top = [], 0
for m in msgs:
    if not isinstance(m, dict) or not isinstance(m.get("id"), str) or not m["id"].isdigit() or len(m["id"]) > 25:
        continue
    top = max(top, int(m["id"]))
    rows.append(m)
rows.sort(key=lambda m: int(m["id"]))
out = []
for m in rows:
    if m["id"] in seen:
        continue
    seen.add(m["id"])
    a = m.get("author") if isinstance(m.get("author"), dict) else {}
    raw = m.get("attachments") if isinstance(m.get("attachments"), list) else []
    atts = [clean(x.get("url"), 500) for x in raw[:10] if isinstance(x, dict) and isinstance(x.get("url"), str)]
    content = clean(m.get("content"), 4000)
    if not content.strip() and not atts:
        continue
    try:
        at = int(datetime.datetime.fromisoformat(str(m.get("timestamp"))).timestamp())
    except ValueError:
        at = (int(m["id"]) >> 22) // 1000 + 1420070400
    username = clean(a.get("username"), 80)
    out.append(json.dumps({"id": m["id"], "at": at,
                           "author": {"username": username, "name": clean(a.get("global_name"), 80) or username},
                           "content": content, "attachments": atts}, ensure_ascii=False))
if out:
    with open(reports, "a", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")
        f.flush()
        os.fsync(f.fileno())
print(len(rows), top, len(out))
' "$REPORTS" "$W/page.json"
}

do_poll() {
    need_root
    command -v "$CURL" >/dev/null 2>&1 || die "curl is missing: apt-get install curl"
    command -v "$DC_PYTHON" >/dev/null 2>&1 || die "$DC_PYTHON is missing"
    mkdir -p "$T"
    if [ "$DC_TEST" != 1 ] && command -v flock >/dev/null 2>&1; then
        exec 9> "$T/.lock"
        flock -n 9 || { say "triage: another poll is running"; exit 0; }
    fi
    LAST=$("$DC_PYTHON" -c '
import json, sys
try:
    v = json.load(open(sys.argv[1], encoding="utf-8")).get("last_id", "")
except (OSError, ValueError, AttributeError):
    v = ""
print(v if isinstance(v, str) and v.isdigit() else "")
' "$STATE" 2>/dev/null)
    load_conf || fail "deploy/triage.conf is missing or malformed: token= (letters, digits, . _ -) and channel_id= (digits)"
    local added=0 pages=0 q n top add rc out had_last
    while [ $pages -lt "$PAGES_MAX" ]; do
        pages=$((pages + 1))
        had_last=0
        q="channels/$CHANNEL/messages?limit=100"
        if [ -n "$LAST" ]; then q="$q&after=$LAST"; had_last=1; fi
        fetch "$q"
        rc=$?
        if [ $rc -ne 0 ]; then
            say "triage: $FAIL_MSG"
            state_set false "$added" "$FAIL_MSG"
            [ $rc -eq 2 ] && exit 0
            exit 1
        fi
        if ! out=$(ingest); then
            fail "Discord's answer was not a list of messages (malformed JSON); nothing stored from it"
        fi
        read -r n top add <<< "$out"
        added=$((added + add))
        if [ "$top" -gt 0 ] && { [ -z "$LAST" ] || [ "$top" -gt "$LAST" ]; }; then LAST=$top; fi
        # a full page after an id means more may follow; the first poll keeps the newest 100 only
        [ "$n" -ge 100 ] && [ $had_last -eq 1 ] || break
    done
    share "$REPORTS"
    rm -rf "$W"
    local msg="$added new report"
    [ "$added" -eq 1 ] || msg="${msg}s"
    state_set true "$added" "$msg"
    say "triage: $msg (last id ${LAST:-none})"
    return 0
}

do_status() {
    if [ -f "$STATE" ]; then cat "$STATE"; else say "no state yet: no poll has run"; fi
    local n=0
    [ ! -f "$REPORTS" ] || n=$(grep -c . "$REPORTS" 2>/dev/null || true)
    say "reports stored: ${n:-0}"
}

do_check() {
    local bad=0 name
    [ -f "$CONF" ] || die "no $CONF: write it by hand (docs/DEPLOY_FROM_ADMIN.md \"Bug reports from Discord\")"
    if [ "$DC_TEST" != 1 ]; then
        local mode
        mode=$(stat -c %a "$CONF" 2>/dev/null || echo "?")
        [ "$mode" = 600 ] || say "warning: $CONF has mode $mode, not 600: chmod 600 $CONF"
    fi
    TOKEN=$(conf token)
    CHANNEL=$(conf channel_id)
    [[ $TOKEN =~ $TOKEN_RE ]] || { say "token= is missing or has an unexpected shape"; bad=1; }
    [[ $CHANNEL =~ $ID_RE ]] || { say "channel_id= is missing or not a number"; bad=1; }
    [ $bad -eq 0 ] || exit 1
    command -v "$CURL" >/dev/null 2>&1 || die "curl is missing: apt-get install curl"
    if fetch "channels/$CHANNEL"; then
        name=$("$DC_PYTHON" -c '
import json, sys
try:
    n = json.load(open(sys.argv[1], encoding="utf-8")).get("name", "")
except (OSError, ValueError, AttributeError):
    n = ""
print(("#" + "".join(c for c in str(n) if c.isprintable())[:80]) if n else "name unknown")
' "$W/page.json" 2>/dev/null)
        rm -rf "$W"
        say "ok: the bot sees channel $CHANNEL ($name)"
    else
        rm -rf "$W"
        die "$FAIL_MSG"
    fi
}

case "${1:-}" in
    poll) do_poll ;;
    status) do_status ;;
    setup-check) do_check ;;
    -h|--help|"") sed -n '2,21p' "$0"; [ -n "${1:-}" ] ;;
    *) die "unknown verb: $1 (poll, status, setup-check)" ;;
esac
