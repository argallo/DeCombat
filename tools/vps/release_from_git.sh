#!/usr/bin/env bash
# release_from_git.sh: the beta release job, as root on the VPS, driven by the admin dashboard and a daily timer
# (docs/DEPLOY_FROM_ADMIN.md "Releases from the dashboard"). Provenance: ORIGINAL (DeCombat operator tooling).
# WRITTEN ON WINDOWS, NOT RUN ON LINUX YET.
#
#   release_from_git.sh check      fetch master into the mirror; status: "N commits since <last tag>" or "nothing new"
#   release_from_git.sh daily      what decombat-release-check.timer runs: check, then prepare when there is something new
#                                  and state.json has auto_prepare 1
#   release_from_git.sh prepare    export master (scan refusals must be 0), push it to the public master, wait for its
#                                  `ci` run (60 min at most): phase ready, or failed with the failed jobs
#   release_from_git.sh tag        from ready, while the public master is still the prepared commit: the annotated tag
#                                  v<MAJOR>.<MINOR+1>.0-beta<N+1>, pushed; wait for its `release` run; set the release
#                                  notes; phase published
#   release_from_git.sh notes FROM TO [TAG]   print the release notes of the mirror's first-parent commits FROM..TO
#   release_from_git.sh run        what decombat-release.path starts: the newest request of release/requests
#   release_from_git.sh status     print release/status.json
#
# A request is a file `<unix>-<check|prepare|tag>.json` the lobby (user decombat) drops into release/requests: JSON
# {"action": the name's action, "by": text}. Nothing in it is trusted but its shape; `by` is only written back as text
# and no field reaches a shell. One job at a time. Rate limits (a lobby can drop request files at will): a check 30 s
# after the last, a prepare 10 min after the last one started, a tag 60 s after the last one started.
# The GitHub token (release.conf, root 0600) goes to curl in a config on stdin and to git through GIT_ASKPASS and the
# environment: never on a command line, in a log or in status.json.
# Exit 0: nothing to do, done, or refused (status.json says why). Exit 1: the job failed.
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

D=${DC_DEPLOY_DIR:-$DC_ROOT/deploy}
R=$D/release
MIRROR=${DC_SRC_GIT:-$DC_ROOT/src.git}
PUB=$D/public
CONF=$D/release.conf
STATE=$R/state.json
ST=$R/status.json
TOOL=$R/tool
# the seams tools/vps/tests/release_test.sh replaces
GIT=${DC_RELEASE_GIT:-git}
CURL=${DC_RELEASE_CURL:-curl}
EXPORT=${DC_RELEASE_EXPORT:-}
API=${DC_RELEASE_API:-https://api.github.com}
SETTLE=${DC_RELEASE_SETTLE:-1}
POLL=${DC_RELEASE_POLL:-60}
CI_MAX=${DC_RELEASE_CI_MAX:-3600}
REL_MAX=${DC_RELEASE_REL_MAX:-3600}
# DC-DEFAULT: OPS-25 (proposed) the root-side rate limits of the release job
RATE_CHECK=${DC_RELEASE_RATE_CHECK:-30}
RATE_PREPARE=${DC_RELEASE_RATE_PREPARE:-600}
RATE_TAG=${DC_RELEASE_RATE_TAG:-60}
BRANCH=master
NAME_RE='^[0-9]{1,12}-(check|prepare|tag)\.json$'
TAG_RE='^v([0-9]{1,4})\.([0-9]{1,4})\.([0-9]{1,4})-beta([0-9]{1,4})$'
EXPORT_NAME=DeCombat
EXPORT_EMAIL=contact@decombat.com
LOGS_KEPT=50
export GIT_TERMINAL_PROMPT=0

# conf KEY [DEFAULT]: a line KEY=value of release.conf, read as data, never sourced
conf() {
    local v
    v=$(sed -n "s/^$1=//p" "$CONF" 2>/dev/null | tr -d '\r' | tail -n 1)
    printf '%s' "${v:-${2:-}}"
}
SLUG=$(conf PUBLIC_REPO argallo/DeCombat)
[[ $SLUG =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || die "release.conf: PUBLIC_REPO must be OWNER/REPO"
PUBLIC_URL=${DC_RELEASE_PUBLIC_URL:-https://github.com/$SLUG.git}

# group-readable for the lobby: root writes, decombat reads
share() {
    chmod 0640 "$@" 2>/dev/null || true
    [ "$DC_TEST" = 1 ] || chgrp "$DC_USER" "$@" 2>/dev/null || true
}

git_m() { "$GIT" --git-dir="$MIRROR" "$@"; }

ACTION="" ID="" BY="" LOG="" STARTED=0 TOKEN=""

# ---- JSON files: state.json (root only) and status.json (the dashboard's)

# jset FILE key=text | key:=json ...: merge into a JSON object file (dotted keys are nested); status.json also gets its
# defaults, `updated` and, from $LOG, the log tail
jset() {
    "$DC_PYTHON" -c '
import json, os, sys, time
path, log, is_status = sys.argv[1], sys.argv[2], sys.argv[3] == "1"
try:
    with open(path, encoding="utf-8") as f:
        d = json.load(f)
except (OSError, ValueError):
    d = {}
if not isinstance(d, dict):
    d = {}
for kv in sys.argv[4:]:
    k, _, v = kv.partition("=")
    if k.endswith(":"):
        k, v = k[:-1], json.loads(v)
    cur = d
    parts = k.split(".")
    for p in parts[:-1]:
        if not isinstance(cur.get(p), dict):
            cur[p] = {}
        cur = cur[p]
    if isinstance(v, str):
        v = "".join(c if c.isprintable() else " " for c in v)[:500]
    cur[parts[-1]] = v
if is_status:
    for k, v in (("phase", "idle"), ("private_sha", ""), ("public_sha", ""), ("last_tag", ""), ("next_tag", ""),
                 ("commits_since", 0), ("release_url", ""), ("message", ""), ("log_tail", []), ("ok", None), ("by", "")):
        d.setdefault(k, v)
    if not isinstance(d.get("ci"), dict):
        d["ci"] = {}
    for k in ("status", "conclusion", "url"):
        d["ci"].setdefault(k, "")
    d["updated"] = int(time.time())
    if log:
        try:
            with open(log, encoding="utf-8", errors="replace") as f:
                d["log_tail"] = [l.rstrip("\n")[:300] for l in f.readlines()[-20:]]
        except OSError:
            pass
with open(path + ".tmp", "w", encoding="utf-8") as f:
    json.dump(d, f, indent=1)
os.replace(path + ".tmp", path)
' "$@"
}
# jget FILE KEY: a value as text (dotted keys; nothing for a missing key or null; JSON for numbers, booleans, objects)
jget() {
    "$DC_PYTHON" -c '
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        v = json.load(f)
    for p in sys.argv[2].split("."):
        v = v.get(p) if isinstance(v, dict) else None
except (OSError, ValueError):
    v = None
if v is not None:
    print(v if isinstance(v, str) else json.dumps(v))
' "$1" "$2" 2>/dev/null
}
st_get() { jget "$STATE" "$1"; }
st_set() { jset "$STATE" "" 0 "$@" && chmod 0600 "$STATE" 2>/dev/null; return 0; }
status_set() {
    local log=""
    [ -n "$LOG" ] && [ -f "$LOG" ] && log=$LOG
    jset "$ST" "$log" 1 "$@" && share "$ST"
    return 0
}
# hist OK MESSAGE: one line of history.jsonl (the newest 100 kept)
hist() {
    "$DC_PYTHON" -c '
import json, sys, time
(path, action, rid, by, started, ok, msg, priv, pub, tag) = sys.argv[1:11]
rec = {"action": action, "id": rid, "by": by, "started": int(started or 0), "finished": int(time.time()),
       "ok": ok == "true", "message": msg[:500], "private_sha": priv, "public_sha": pub, "tag": tag}
try:
    with open(path, encoding="utf-8") as f:
        lines = f.read().splitlines()[-99:]
except OSError:
    lines = []
lines.append(json.dumps(rec))
with open(path + ".tmp", "w", encoding="utf-8") as f:
    f.write("\n".join(lines) + "\n")
' "$R/history.jsonl" "$ACTION" "$ID" "$BY" "$STARTED" "$1" "$2" "$(jget "$ST" private_sha)" "$(jget "$ST" public_sha)" \
        "$(jget "$ST" next_tag)" && mv -f "$R/history.jsonl.tmp" "$R/history.jsonl" && share "$R/history.jsonl"
    return 0
}

logl() { [ -n "$LOG" ] && say "$*" >> "$LOG"; return 0; }
phase() { logl "== $1: $2"; status_set "phase=$1" "ok:=null" "message=$2" "by=$BY"; }
fail() {
    logl "FAILED: $1"
    status_set phase=failed "ok:=false" "message=$1"
    hist false "$1"
    say "release $ACTION: failed: $1"
    exit 1
}
# refuse MESSAGE: nothing done; the phase stays as it is, the message says why
refuse() {
    logl "refused: $1"
    status_set "message=refused: $1"
    say "release $ACTION refused: $1"
    exit 0
}
new_log() {
    mkdir -p "$R/logs"
    LOG="$R/logs/$1.log"
    : > "$LOG"
    share "$LOG"
    local old
    old=$(ls -t "$R"/logs/*.log 2>/dev/null | sed -n "$((LOGS_KEPT + 1)),\$p")
    [ -z "$old" ] || printf '%s\n' "$old" | while IFS= read -r f; do rm -f -- "$f"; done
}

# ---- the mirror (the deploy's: its read-only key)

fetch_mirror() {
    local key=$D/key/id_ed25519 kh=$D/key/known_hosts
    if [ -f "$key" ]; then
        GIT_SSH_COMMAND="ssh -i $key -o IdentitiesOnly=yes -o UserKnownHostsFile=$kh -o StrictHostKeyChecking=yes -o BatchMode=yes -o ConnectTimeout=20 -o ServerAliveInterval=10 -o ServerAliveCountMax=3" \
            git_m fetch --quiet --prune origin "+refs/heads/$BRANCH:refs/heads/$BRANCH"
    elif [ "$DC_TEST" = 1 ] && [ "${DC_RELEASE_TEST_NO_KEY:-0}" = 1 ]; then
        git_m fetch --quiet --prune origin "+refs/heads/$BRANCH:refs/heads/$BRANCH"
    else
        echo "no deploy key at $key (deploy_setup.sh makes it)" >&2
        return 1
    fi
}
resolve() { git_m rev-parse --verify --quiet "$1^{commit}" 2>/dev/null; }
# since LAST MASTER: first-parent commits after LAST up to MASTER (all of master without LAST)
since() {
    if [ -n "$1" ]; then git_m rev-list --count --first-parent "$1..$2" 2>/dev/null || echo 0
    else git_m rev-list --count --first-parent "$2" 2>/dev/null || echo 0; fi
}

# next_tag TAG: v<MAJOR>.<MINOR+1>.0-beta<N+1> (the client compares MAJOR.MINOR.PATCH only, so each beta bumps MINOR)
next_tag() {
    [[ $1 =~ $TAG_RE ]] || return 1
    printf 'v%d.%d.0-beta%d' "$((10#${BASH_REMATCH[1]}))" "$((10#${BASH_REMATCH[2]} + 1))" "$((10#${BASH_REMATCH[4]} + 1))"
}

# ---- the release text: commit message and notes, without a line the export scan would refuse

# release_text MODE TAG FROM TO: MODE message (one line, the export commit's) or notes (the release body)
release_text() {
    local subj=$R/work/subjects.txt
    mkdir -p "$R/work"
    if [ -n "$3" ]; then git_m log --first-parent --reverse --format=%s "$3..$4" > "$subj" 2>/dev/null || return 1
    else git_m log --first-parent --reverse -n 50 --format=%s "$4" > "$subj" 2>/dev/null || return 1; fi
    "$DC_PYTHON" -I -c '
import re, sys
mode, tooldir, tag, subj = sys.argv[1:5]
sys.dont_write_bytecode = True
sys.path.insert(0, tooldir)
try:
    import export_public as e
    pats = [re.compile(p) for p, _ in e.SOURCE_WORDS] + [re.compile(p) for p, _ in e.SECRETS]
    pats += [rx for _, rx in e.load_private()]
except (Exception, SystemExit) as x:
    print("the export scan list could not be read: %s" % x, file=sys.stderr)
    sys.exit(1)
m = re.match(r"^v\d+\.\d+\.\d+-beta(\d+)$", tag)
n = int(m.group(1)) if m else 0
rows, dropped = [], 0
with open(subj, encoding="utf-8", errors="replace") as f:
    for s in f.read().split("\n"):
        s = "".join(c if c.isprintable() else " " for c in s).strip()
        if not s:
            continue
        s = re.sub(r"^Merge [^\s:]+: *", "", s)
        if any(p.search(s) for p in pats):
            dropped += 1
            continue
        rows.append(s.replace("@", "(at)"))
if mode == "message":
    text = "; ".join(rows)
    print(("Beta %d: %s" % (n, text[:200])) if text else "Beta %d" % n)
    sys.exit(0)
areas = {"CLI": "Client", "VAR": "Client", "REN": "Client", "DSV": "Match servers", "GMD": "Match servers",
         "LG": "Lobby and servers", "OPS": "Lobby and servers", "CRATE": "Lobby and servers"}
order = ["Client", "Match servers", "Lobby and servers", "Other"]
groups = {a: [] for a in order}
for s in rows:
    area = "Other"
    for pre in re.findall(r"\b([A-Z]+)-\d+", s):
        if pre in areas:
            area = areas[pre]
            break
    groups[area].append(s)
out = ["# DeCombat %s" % tag, "",
       "A beta%d or beta%d client offers this update itself at login (Update now); older clients need a fresh download "
       "of the Windows zip from this page." % (n - 1, n - 2), ""]
for a in order:
    if groups[a]:
        out += ["## " + a, ""] + ["- " + s for s in groups[a]] + [""]
if not rows:
    out += ["Small fixes.", ""]
if dropped:
    out += ["%d further internal change%s not listed." % (dropped, "" if dropped == 1 else "s"), ""]
out.append("Windows still warns about the unsigned exe (SmartScreen \"More info > Run anyway\"; a Defender flag is a false "
           "positive). Check the zip" + chr(39) + "s hash against SHA256SUMS on this page.")
print("\n".join(out))
' "$1" "$TOOL" "$2" "$subj"
}

# ---- GitHub

load_token() {
    TOKEN=$(conf GITHUB_TOKEN "")
    [[ $TOKEN =~ ^[A-Za-z0-9_]{20,255}$ ]]
}
# api METHOD PATH [JSON-FILE]: the answer's body in $R/work/api.json; 1 on no answer or an HTTP status not 2xx
api() {
    local code out=$R/work/api.json
    mkdir -p "$R/work"
    rm -f "$out"
    code=$({
        printf 'url = "%s/repos/%s/%s"\n' "$API" "$SLUG" "$2"
        printf 'header = "Authorization: Bearer %s"\n' "$TOKEN"
        printf 'header = "Accept: application/vnd.github+json"\n'
        printf 'header = "X-GitHub-Api-Version: 2022-11-28"\n'
        printf 'header = "User-Agent: decombat-release"\n'
        [ -z "${3:-}" ] || printf 'header = "Content-Type: application/json"\n'
    } | "$CURL" -sS --max-time 30 -X "$1" -K - -o "$out" -w '%{http_code}' ${3:+--data-binary "@$3"} 2>>"${LOG:-/dev/null}")
    [ -f "$out" ] || : > "$out"
    case "$code" in 2[0-9][0-9]) return 0 ;; esac
    logl "GitHub API $1 $2: HTTP ${code:-none}"
    return 1
}
# the git calls that write to the public repository: the token from the environment through an askpass helper
askpass() {
    local a=$R/work/askpass.sh
    mkdir -p "$R/work"
    printf '#!/bin/sh\ncase "$1" in\n  Username*) printf "%%s\\n" x-access-token ;;\n  *) printf "%%s\\n" "$DC_RELEASE_TOKEN" ;;\nesac\n' > "$a"
    chmod 0700 "$a"
    printf '%s' "$a"
}
git_push() { # git_push DIR REFSPEC
    DC_RELEASE_TOKEN=$TOKEN GIT_ASKPASS=$(askpass) "$GIT" -c credential.helper= -C "$1" push --quiet "$PUBLIC_URL" "$2"
}
public_master() { "$GIT" ls-remote "$PUBLIC_URL" "refs/heads/$BRANCH" 2>>"${LOG:-/dev/null}" | cut -f1 | head -n 1; }

# poll_run WORKFLOW HEAD_BRANCH MAX PHASE: wait for the newest run of that workflow on public_sha. Sets RUN_URL,
# RUN_CONC, FAILED_JOBS. 0 every job green, 1 failed, 2 no finished run within MAX seconds.
RUN_URL="" RUN_CONC="" FAILED_JOBS=""
poll_run() {
    local wf=$1 br=$2 max=$3 ph=$4 deadline line id state
    deadline=$(( $(date +%s) + max ))
    while :; do
        line=""
        if api GET "actions/runs?head_sha=$PUBLIC_SHA&per_page=50"; then
            line=$("$DC_PYTHON" -c '
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        runs = json.load(f).get("workflow_runs") or []
except (OSError, ValueError, AttributeError):
    runs = []
runs = [r for r in runs if isinstance(r, dict) and r.get("name") == sys.argv[2] and r.get("head_branch") == sys.argv[3]]
if runs:
    r = max(runs, key=lambda r: int(r.get("id") or 0))
    print("\x1f".join(str(x) for x in (int(r.get("id") or 0), r.get("status") or "-", r.get("conclusion") or "-", r.get("html_url") or "-")))
' "$R/work/api.json" "$wf" "$br")
        fi
        if [ -n "$line" ]; then
            IFS=$'\x1f' read -r id state RUN_CONC RUN_URL <<< "$line"
            [ "$RUN_CONC" != - ] || RUN_CONC=""
            [ "$RUN_URL" != - ] || RUN_URL=""
            status_set "phase=$ph" "ci.status=$state" "ci.conclusion=$RUN_CONC" "ci.url=$RUN_URL"
            if [ "$state" = completed ]; then
                FAILED_JOBS=""
                if api GET "actions/runs/$id/jobs?per_page=100"; then
                    FAILED_JOBS=$("$DC_PYTHON" -c '
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        jobs = json.load(f).get("jobs") or []
except (OSError, ValueError, AttributeError):
    jobs = None
if jobs is None or not jobs:
    print("(the jobs could not be read)")
else:
    bad = ["%s %s" % (j.get("name"), j.get("html_url")) for j in jobs
           if isinstance(j, dict) and j.get("conclusion") not in ("success", "skipped")]
    print(", ".join(bad))
' "$R/work/api.json")
                else
                    FAILED_JOBS="(the jobs could not be read)"
                fi
                logl "$wf run $RUN_URL: $RUN_CONC${FAILED_JOBS:+; not green: $FAILED_JOBS}"
                [ "$RUN_CONC" = success ] && [ -z "$FAILED_JOBS" ] && return 0
                [ -n "$FAILED_JOBS" ] || FAILED_JOBS="conclusion $RUN_CONC"
                return 1
            fi
        fi
        [ "$(date +%s)" -lt "$deadline" ] || return 2
        sleep "$POLL"
    done
}

# ---- one job at a time; a job the machine lost half way is written off

lock() { # lock wait|nowait
    command -v flock >/dev/null 2>&1 || return 0
    exec 9> "$R/lock"
    if [ "$1" = wait ]; then flock 9; else flock -n 9; fi
}
write_off() {
    case "$(jget "$ST" phase)" in
        exporting|pushed|ci_running|tagging|publishing)
            status_set phase=failed "ok:=false" "message=interrupted (the release job ended before it finished; prepare or tag again)" ;;
    esac
}
# age KEY: seconds since the unix time state.json holds under KEY (a large number when there is none)
age() {
    local t
    t=$(st_get "$1" | tr -cd '0-9')
    echo $(( $(date +%s) - ${t:-0} ))
}

# ---- check

CHECK_NEW=0
cmd_check() {
    ACTION=${ACTION:-check}
    [ -d "$MIRROR" ] || die "no mirror at $MIRROR: run deploy_setup.sh first"
    [ -f "$STATE" ] || die "no $STATE: run release_setup.sh first"
    local err master last last_tag full="" n ph next msg
    if ! err=$(fetch_mirror 2>&1); then
        err=$(printf '%s' "$err" | tail -n 2 | tr '\n' ' ')
        status_set "ok:=false" "message=fetch failed: $err" "checked:=$(date +%s)"
        say "release check: fetch failed: $err"
        return 1
    fi
    master=$(git_m rev-parse --verify --quiet "refs/heads/$BRANCH" 2>/dev/null)
    last=$(st_get last_released_private_sha)
    last_tag=$(st_get last_tag)
    next=$(next_tag "$last_tag" || true)
    [ -z "$last" ] || full=$(resolve "$last" || true)
    if [ -n "$last" ] && [ -z "$full" ]; then
        status_set "ok:=false" "message=the last released commit ${last:0:12} is not in the mirror" "checked:=$(date +%s)"
        say "release check: the last released commit ${last:0:12} is not in the mirror"
        return 1
    fi
    if [ -n "$full" ] && ! git_m merge-base --is-ancestor "$full" "$master" 2>/dev/null; then
        status_set "ok:=false" "message=the last released commit ${full:0:8} is not on $BRANCH" "checked:=$(date +%s)"
        say "release check: the last released commit ${full:0:8} is not on $BRANCH"
        return 1
    fi
    n=$(since "$full" "$master")
    CHECK_NEW=$n
    if [ "$n" -eq 0 ]; then msg="nothing new since $last_tag"
    elif [ "$n" -eq 1 ]; then msg="1 commit since $last_tag"
    else msg="$n commits since $last_tag"; fi
    ph=$(jget "$ST" phase)
    case "$ph" in
        ''|idle|published)
            status_set phase=idle "ok:=true" "message=$msg" "private_sha=$master" "public_sha=$(st_get last_public_sha)" \
                "last_tag=$last_tag" "next_tag=$next" "commits_since:=$n" "checked:=$(date +%s)" ;;
        *)
            # a prepared or running release keeps its phase and message
            status_set "commits_since:=$n" "last_tag=$last_tag" "checked:=$(date +%s)" ;;
    esac
    say "release check: $msg (master ${master:0:8})"
}

cmd_daily() {
    ACTION=daily
    lock nowait || { say "a release job is running; the daily check skips"; exit 0; }
    write_off
    cmd_check || exit 1
    if [ "$(st_get auto_prepare)" != 1 ]; then say "auto_prepare is off: no prepare"; exit 0; fi
    [ "$CHECK_NEW" -gt 0 ] || exit 0
    # a release that waits for its tag is the owner's to tag (or to prepare again from the dashboard)
    if [ "$(jget "$ST" phase)" = ready ]; then say "a release is ready to tag: no prepare"; exit 0; fi
    ACTION=prepare
    ID=daily-$(date +%s)
    BY="the daily timer"
    STARTED=$(date +%s)
    new_log "$ID"
    prepare
}

# ---- prepare

PUBLIC_SHA="" PRIVATE_SHA="" NEXT=""
prepare() {
    load_token || fail "no token: GITHUB_TOKEN in $CONF (release_setup.sh)"
    [ -f "$STATE" ] || fail "no $STATE: run release_setup.sh first"
    local last last_tag full="" master prep_priv prep_pub pubm out=$R/work/export msg rc summary refusals n
    last=$(st_get last_released_private_sha)
    last_tag=$(st_get last_tag)
    NEXT=$(next_tag "$last_tag") || fail "state.json: last_tag \"$last_tag\" is not v<MAJOR>.<MINOR>.<PATCH>-beta<N>"
    logl "prepare $ID requested by $BY"
    fetch_mirror >> "$LOG" 2>&1 || fail "could not fetch $BRANCH (deploy key? network?)"
    master=$(git_m rev-parse --verify --quiet "refs/heads/$BRANCH" 2>/dev/null) || fail "the mirror has no $BRANCH"
    [ -z "$last" ] || full=$(resolve "$last") || fail "the last released commit ${last:0:12} is not in the mirror"
    if [ -n "$full" ]; then
        git_m merge-base --is-ancestor "$full" "$master" 2>/dev/null || fail "the last released commit ${full:0:8} is not on $BRANCH"
        [ "$full" != "$master" ] || refuse "nothing new since $last_tag"
    fi
    n=$(since "$full" "$master")
    PRIVATE_SHA=$master
    prep_priv=$(st_get prepared.private_sha)
    prep_pub=$(st_get prepared.public_sha)
    if [ "$prep_priv" = "$master" ]; then
        [ "$(jget "$ST" phase)" != ready ] || refuse "${master:0:8} is already prepared as $(st_get prepared.next_tag): tag it, or wait for a new commit"
    fi
    [ "$(age last_prepare_at)" -ge "$RATE_PREPARE" ] || refuse "the last prepare started $(age last_prepare_at)s ago; wait $RATE_PREPARE s between prepares"
    st_set "last_prepare_at:=$(date +%s)"

    # the same commit, already on the public master (its CI failed, or the job was cut off): look at CI again
    if [ "$prep_priv" = "$master" ] && [ -n "$prep_pub" ]; then
        pubm=$(public_master)
        if [ "$pubm" = "$prep_pub" ]; then
            PUBLIC_SHA=$prep_pub
            NEXT=$(st_get prepared.next_tag)
            status_set "private_sha=$master" "public_sha=$PUBLIC_SHA" "next_tag=$NEXT" "last_tag=$last_tag" "commits_since:=$n" \
                "ci.status=" "ci.conclusion=" "ci.url=" "release_url="
            phase ci_running "${PUBLIC_SHA:0:8} is already on the public $BRANCH: waiting for its ci run again"
            wait_ci
            return
        fi
    fi

    status_set "private_sha=$master" "public_sha=" "next_tag=$NEXT" "last_tag=$last_tag" "commits_since:=$n" \
        "ci.status=" "ci.conclusion=" "ci.url=" "release_url="
    phase exporting "exporting ${master:0:8} ($n commits since $last_tag) as $NEXT"
    [ -f "$TOOL/export_public.py" ] || fail "no export tool at $TOOL/export_public.py (release_setup.sh --export-tool)"
    [ -f "$TOOL/private_strings.txt" ] || fail "no scan word list at $TOOL/private_strings.txt (release_setup.sh --private-strings)"

    # the public clone at the public master
    [ -d "$PUB/.git" ] || fail "no public clone at $PUB (release_setup.sh)"
    "$GIT" -C "$PUB" remote set-url origin "$PUBLIC_URL" >> "$LOG" 2>&1
    "$GIT" -C "$PUB" fetch --quiet --prune origin >> "$LOG" 2>&1 || fail "could not fetch the public repository"
    "$GIT" -C "$PUB" checkout --quiet -B "$BRANCH" "origin/$BRANCH" >> "$LOG" 2>&1 || fail "the public clone has no origin/$BRANCH"
    "$GIT" -C "$PUB" reset --quiet --hard "origin/$BRANCH" >> "$LOG" 2>&1 || fail "could not reset the public clone"
    "$GIT" -C "$PUB" clean -qfdx >> "$LOG" 2>&1
    pubm=$("$GIT" -C "$PUB" rev-parse "origin/$BRANCH" 2>/dev/null)

    msg=$(release_text message "$NEXT" "$full" "$master" 2>>"$LOG") || fail "could not write the commit message (the export scan list?)"
    logl "message: $msg"
    rm -rf "$out"
    local args=(--repo "$MIRROR" --commit "$master" --out "$out" --parent "$PUB" --name "$EXPORT_NAME" --email "$EXPORT_EMAIL"
        --message "$msg")
    if [ -n "$EXPORT" ]; then
        "$EXPORT" "${args[@]}" > "$R/work/export.out" 2>&1
    else
        "$DC_PYTHON" -I "$TOOL/export_public.py" "${args[@]}" > "$R/work/export.out" 2>&1
    fi
    rc=$?
    cat "$R/work/export.out" >> "$LOG"
    summary=$(grep '^export_public commit=' "$R/work/export.out" | tail -n 1)
    refusals=$(printf '%s' "$summary" | sed -n 's/.* refusals=\([0-9][0-9]*\).*/\1/p')
    [ -n "$summary" ] || fail "the export tool stopped (exit $rc): $(tail -n 1 "$R/work/export.out" | cut -c1-200)"
    [ "$refusals" = 0 ] || fail "the export scan refused: ${summary#export_public }"
    [ "$rc" -eq 0 ] || fail "the export tool failed (exit $rc) after the scan: $(tail -n 1 "$R/work/export.out" | cut -c1-200)"
    PUBLIC_SHA=$("$GIT" -C "$out" rev-parse HEAD 2>/dev/null)
    [[ $PUBLIC_SHA =~ ^[0-9a-f]{40}$ ]] || fail "the export left no commit"
    [ "$("$GIT" -C "$out" rev-parse HEAD~1 2>/dev/null)" = "$pubm" ] || fail "the export commit is not a child of the public $BRANCH"

    phase pushed "pushing ${PUBLIC_SHA:0:8} to the public $BRANCH"
    git_push "$out" "HEAD:refs/heads/$BRANCH" >> "$LOG" 2>&1 || fail "the push to the public $BRANCH failed (token? did someone else push?)"
    st_set "prepared.private_sha=$master" "prepared.from_private_sha=$full" "prepared.public_sha=$PUBLIC_SHA" \
        "prepared.next_tag=$NEXT" "prepared.ci_ok:=false" "prepared.tag_attempted:=false"
    status_set "public_sha=$PUBLIC_SHA"
    phase ci_running "pushed ${PUBLIC_SHA:0:8}; waiting for its ci run (at most $((CI_MAX / 60)) min)"
    wait_ci
}
wait_ci() {
    poll_run ci "$BRANCH" "$CI_MAX" ci_running
    case $? in
        0) ;;
        1) fail "ci failed on ${PUBLIC_SHA:0:8}: $FAILED_JOBS" ;;
        *) fail "no finished ci run on ${PUBLIC_SHA:0:8} within $((CI_MAX / 60)) min${RUN_URL:+ ($RUN_URL)}" ;;
    esac
    st_set "prepared.ci_ok:=true"
    logl "ready: ${PUBLIC_SHA:0:8} is green; tag $NEXT"
    status_set phase=ready "ok:=true" "message=${PUBLIC_SHA:0:8} is green on every ci job: ready to tag $NEXT"
    hist true "ready to tag $NEXT"
    say "release prepare: ready to tag $NEXT on ${PUBLIC_SHA:0:8}"
}

# ---- tag

tag() {
    load_token || fail "no token: GITHUB_TOKEN in $CONF (release_setup.sh)"
    local ph tagged remote pubm notes=$R/work/notes.md body=$R/work/release.json rel
    ph=$(jget "$ST" phase)
    PUBLIC_SHA=$(st_get prepared.public_sha)
    PRIVATE_SHA=$(st_get prepared.private_sha)
    NEXT=$(st_get prepared.next_tag)
    logl "tag $ID requested by $BY"
    if [ "$(st_get prepared.ci_ok)" != true ] || [ -z "$PUBLIC_SHA" ] || ! [[ $NEXT =~ $TAG_RE ]]; then
        refuse "nothing is ready to tag (prepare first; the phase must be ready)"
    fi
    if [ "$ph" != ready ] && ! { [ "$ph" = failed ] && [ "$(st_get prepared.tag_attempted)" = true ]; }; then
        refuse "the phase is $ph, not ready"
    fi
    [ "$(age last_tag_at)" -ge "$RATE_TAG" ] || refuse "the last tag started $(age last_tag_at)s ago; wait $RATE_TAG s"
    st_set "last_tag_at:=$(date +%s)"
    pubm=$(public_master)
    [ -n "$pubm" ] || fail "could not read the public $BRANCH"
    [ "$pubm" = "$PUBLIC_SHA" ] || fail "the public $BRANCH moved to ${pubm:0:8} since the prepare of ${PUBLIC_SHA:0:8}: prepare again"
    st_set "prepared.tag_attempted:=true"
    status_set "private_sha=$PRIVATE_SHA" "public_sha=$PUBLIC_SHA" "next_tag=$NEXT" "release_url="
    phase tagging "tagging ${PUBLIC_SHA:0:8} as $NEXT"
    remote=$("$GIT" ls-remote "$PUBLIC_URL" "refs/tags/$NEXT" "refs/tags/$NEXT^{}" 2>>"$LOG")
    tagged=$(printf '%s\n' "$remote" | awk -v t="refs/tags/$NEXT^{}" '$2 == t {print $1}')
    [ -n "$tagged" ] || tagged=$(printf '%s\n' "$remote" | awk -v t="refs/tags/$NEXT" '$2 == t {print $1}')
    if [ -n "$tagged" ]; then
        [ "$tagged" = "$PUBLIC_SHA" ] || fail "the tag $NEXT is already on ${tagged:0:8}, not ${PUBLIC_SHA:0:8}: delete it on GitHub first"
        logl "the tag $NEXT is already on ${PUBLIC_SHA:0:8}: waiting for its release run"
    else
        "$GIT" -C "$PUB" fetch --quiet origin >> "$LOG" 2>&1 || fail "could not fetch the public repository"
        "$GIT" -C "$PUB" cat-file -e "$PUBLIC_SHA^{commit}" 2>/dev/null || fail "the public clone does not have ${PUBLIC_SHA:0:8}"
        "$GIT" -C "$PUB" tag -d "$NEXT" >/dev/null 2>&1
        "$GIT" -C "$PUB" -c user.name="$EXPORT_NAME" -c user.email="$EXPORT_EMAIL" -c tag.gpgsign=false \
            tag -a "$NEXT" -m "DeCombat $NEXT" "$PUBLIC_SHA" >> "$LOG" 2>&1 || fail "could not make the tag $NEXT"
        git_push "$PUB" "refs/tags/$NEXT" >> "$LOG" 2>&1 || fail "the push of the tag $NEXT failed (token?)"
    fi
    phase publishing "$NEXT pushed; waiting for its release run (at most $((REL_MAX / 60)) min)"
    poll_run release "$NEXT" "$REL_MAX" publishing
    case $? in
        0) ;;
        1) fail "the release run of $NEXT failed: $FAILED_JOBS (delete the tag $NEXT and any draft release on GitHub, then tag again)" ;;
        *) fail "no finished release run of $NEXT within $((REL_MAX / 60)) min${RUN_URL:+ ($RUN_URL)}; tag again to keep waiting" ;;
    esac
    api GET "releases/tags/$NEXT" || fail "the release run passed but GitHub has no release $NEXT"
    rel=$("$DC_PYTHON" -c '
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        d = json.load(f)
    print("%d\x1f%s" % (int(d["id"]), d.get("html_url") or "-"))
except (OSError, ValueError, KeyError, TypeError):
    pass
' "$R/work/api.json")
    [ -n "$rel" ] || fail "could not read the release $NEXT"
    local rid url
    IFS=$'\x1f' read -r rid url <<< "$rel"
    [ "$url" != - ] || url=""
    release_text notes "$NEXT" "$(st_get prepared.from_private_sha)" "$PRIVATE_SHA" > "$notes" 2>>"$LOG" || fail "could not write the release notes"
    "$DC_PYTHON" -c '
import json, sys
with open(sys.argv[1], encoding="utf-8") as f:
    body = f.read()
with open(sys.argv[2], "w", encoding="utf-8") as f:
    json.dump({"body": body, "prerelease": True}, f)
' "$notes" "$body" || fail "could not write the release body"
    api PATCH "releases/$rid" "$body" || fail "could not set the notes of the release $NEXT ($url)"
    st_set "last_tag=$NEXT" "last_released_private_sha=$PRIVATE_SHA" "last_public_sha=$PUBLIC_SHA" "prepared:=null"
    logl "published $NEXT: $url"
    status_set phase=published "ok:=true" "message=published $NEXT" "release_url=$url" "last_tag=$NEXT" "commits_since:=0"
    hist true "published $NEXT"
    say "release tag: published $NEXT ($url)"
}

# ---- requests

parse_request() { # parse_request FILE ACTION: "by=<text>", or exit 1 with the reason (never the file's text)
    "$DC_PYTHON" -c '
import json, os, stat, sys
p, want = sys.argv[1], sys.argv[2]
try:
    fd = os.open(p, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_BINARY", 0))
    with os.fdopen(fd, "rb") as f:
        if not stat.S_ISREG(os.fstat(f.fileno()).st_mode):
            raise ValueError("not a regular file")
        raw = f.read(4097)
    if len(raw) > 4096:
        raise ValueError("over 4096 bytes")
    d = json.loads(raw.decode("utf-8"))
    if not isinstance(d, dict):
        raise ValueError("not a JSON object")
    if d.get("action") != want:
        raise ValueError("action must be \"%s\" as the file name says" % want)
    by = d.get("by", "")
    by = "".join(c if c.isprintable() else " " for c in (by if isinstance(by, str) else ""))[:120]
except (OSError, ValueError, UnicodeDecodeError) as e:
    print(str(e)[:200])
    sys.exit(1)
print("by=" + by)
' "$1" "$2"
}

cmd_run() {
    need_root
    [ -d "$R/requests" ] || die "no $R/requests: run release_setup.sh first"
    mkdir -p "$R/logs" "$R/work"
    lock wait
    sleep "$SETTLE"
    write_off
    local f newest="" n
    while IFS= read -r -d '' f; do
        n=${f##*/}
        if [ -L "$f" ] || [ ! -f "$f" ] || ! [[ $n =~ $NAME_RE ]]; then
            rm -rf -- "$f"
            continue
        fi
        if [ -n "$newest" ]; then
            if [[ $n > ${newest##*/} ]]; then
                say "request ${newest##*/} superseded by $n"
                rm -f -- "$newest"
            else
                say "request $n superseded by ${newest##*/}"
                rm -f -- "$f"
                continue
            fi
        fi
        newest=$f
    done < <(find "$R/requests" -mindepth 1 -maxdepth 1 -print0)
    [ -n "$newest" ] || { say "no request"; exit 0; }
    n=${newest##*/}
    ID=${n%.json}
    ACTION=${ID#*-}
    STARTED=$(date +%s)
    local fields
    fields=$(parse_request "$newest" "$ACTION") || { sleep "$SETTLE"; fields=$(parse_request "$newest" "$ACTION"); } || true
    rm -f -- "$newest"
    if ! printf '%s\n' "$fields" | grep -q '^by='; then
        status_set "message=request refused: $fields"
        say "request $ID refused: $fields"
        exit 0
    fi
    BY=$(printf '%s\n' "$fields" | sed -n 's/^by=//p')
    case "$ACTION" in
        check)
            local last
            last=$(jget "$ST" checked | tr -cd '0-9')
            if [ $(( $(date +%s) - ${last:-0} )) -lt "$RATE_CHECK" ]; then
                say "check request ignored: checked $(( $(date +%s) - ${last:-0} ))s ago (limit $RATE_CHECK s)"
                exit 0
            fi
            cmd_check || exit 1
            ;;
        prepare) new_log "$ID"; prepare ;;
        tag) new_log "$ID"; tag ;;
    esac
}

main() {
    case "${1:-}" in
        check)
            need_root
            lock nowait || { say "a release job is running; try again later"; exit 0; }
            cmd_check
            ;;
        daily) need_root; cmd_daily ;;
        prepare|tag)
            need_root
            ACTION=$1 ID=$1-$(date +%s) BY="root (by hand)" STARTED=$(date +%s)
            lock nowait || { say "a release job is running; try again later"; exit 0; }
            write_off
            new_log "$ID"
            if [ "$1" = prepare ]; then prepare; else tag; fi
            ;;
        notes)
            [ $# -ge 3 ] || die "notes FROM TO [TAG]"
            local from to t
            from=$(resolve "$2") || die "$2 is not a commit of the mirror"
            to=$(resolve "$3") || die "$3 is not a commit of the mirror"
            t=${4:-$(st_get prepared.next_tag)}
            [ -n "$t" ] || t=$(next_tag "$(st_get last_tag)" || true)
            [[ $t =~ $TAG_RE ]] || die "give the tag: notes FROM TO v<MAJOR>.<MINOR>.<PATCH>-beta<N>"
            release_text notes "$t" "$from" "$to"
            ;;
        run) cmd_run ;;
        status) cat "$ST" 2>/dev/null || say "no release job yet" ;;
        -h|--help) sed -n '2,24p' "$0" ;;
        *) die "check, daily, prepare, tag, notes, run or status (--help)" ;;
    esac
}
# the whole script is read before it runs: install.sh may replace this file while a job runs
main "$@"
exit $?
