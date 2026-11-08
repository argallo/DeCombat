#!/usr/bin/env bash
# deploy_from_git.sh: deploys requested from the admin dashboard, as root on the VPS (docs/DEPLOY_FROM_ADMIN.md).
# Provenance: ORIGINAL (DeCombat operator tooling). WRITTEN ON WINDOWS, NOT RUN ON LINUX YET.
#
#   deploy_from_git.sh fetch     what decombat-src-fetch.timer runs: fetch master into the mirror, write available.json
#   deploy_from_git.sh run       what decombat-deploy.path starts: take the newest request of deploy/requests and deploy it
#                                (a `fetch-<unix>.json` there is the dashboard's "Check now": one fetch, never a deploy;
#                                a `packs-<unix>.json` is the dashboard's "Sync packs": packs_sync.sh per declared
#                                source into the named mods folder, never a deploy, LG-434)
#   deploy_from_git.sh status    print deploy/status.json
#
# A request is a small JSON file the lobby (user decombat) drops into $DC_ROOT/deploy/requests. Nothing in it is trusted
# but its shape: `sha` must be hex and name a commit on the mirror's master, `targets` a subset of vps / hosts,
# `end_matches` a boolean, `by` is only written back as text; no field ever reaches a shell. One deploy at a time.
# Steps: git archive of that commit, the build deploy_all.sh makes (DC_GIT_COMMIT=<short sha>; the VPS set and dc-host),
# the tarball of tools/package_servers.sh, then for `vps` the tarball's own tools/vps/deploy_remote.sh install (install.sh:
# binaries, units, scripts; config and DB untouched; every enabled lobby instance restarted), for `hosts` the tarball and
# desired.txt into deploy/host, where the lobby offers it to its host agents (specs/HOST_AGENT.md "Updates").
# Progress and the outcome: deploy/status.json (phase, log tail, ok), deploy/history.jsonl, deploy/logs/<id>.log.
# A host offer is signed (ssh-keygen -Y sign, namespace decombat-host-update) with deploy/sign/host_update_ed25519, a
# key only root holds, over the commit, tarball, SHA-256, size and a sequence number that only grows; the hosts' updater
# checks it against the public key pinned at its setup, so a lobby that relays the offer cannot forge or replay one.
# Rate limit (a lobby can drop request files at will): no deploy within 60 s of the last one, none of the same commit
# within 5 min.
# Exit 0: nothing to do, deployed, or a refused request (status.json says why). Exit 1: the deploy failed.
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

D=${DC_DEPLOY_DIR:-$DC_ROOT/deploy}
MIRROR=${DC_SRC_GIT:-$DC_ROOT/src.git}
CONF=$D/deploy.conf
# the seams tools/vps/tests/deploy_test.sh replaces
GIT=${DC_DEPLOY_GIT:-git}
PGREP=${DC_DEPLOY_PGREP:-pgrep}
SETTLE=${DC_DEPLOY_SETTLE:-1}
MATCH_POLL=${DC_DEPLOY_MATCH_POLL:-30}
NAME_RE='^[0-9]{1,12}-[0-9a-f]{8}\.json$'
# LG-402: the dashboard's "Check now"; its content is never read (the lobby's ops event says who asked)
FETCH_RE='^fetch-[0-9]{1,12}\.json$'
RATE_FETCH=${DC_DEPLOY_RATE_FETCH:-30}
PACKS_RE='^packs-[0-9]{1,12}\.json$'
RATE_PACKS=${DC_DEPLOY_RATE_PACKS:-60}
SIGN_NS=decombat-host-update
SIGN_ID=decombat-deploy
SSHKEYGEN=${DC_DEPLOY_SSHKEYGEN:-ssh-keygen}
# DC-DEFAULT: LG-392 (proposed) the root-side rate limit
RATE_ANY=${DC_DEPLOY_RATE_ANY:-60}
RATE_SAME=${DC_DEPLOY_RATE_SAME:-300}
LOGS_KEPT=50

# conf KEY [DEFAULT]: a line KEY=value of deploy.conf, read as data, never sourced
conf() {
    local v
    v=$(sed -n "s/^$1=//p" "$CONF" 2>/dev/null | tr -d '\r' | tail -n 1)
    printf '%s' "${v:-${2:-}}"
}
BRANCH=$(conf BRANCH master)
case "$BRANCH" in ''|*[!A-Za-z0-9._/-]*|-*) die "deploy.conf: BRANCH must be a plain branch name" ;; esac

# group-readable for the lobby: root writes, decombat reads
share() {
    chmod 0640 "$@" 2>/dev/null || true
    [ "$DC_TEST" = 1 ] || chgrp "$DC_USER" "$@" 2>/dev/null || true
}

git_m() { "$GIT" --git-dir="$MIRROR" "$@"; }

fetch_mirror() {
    local key=$D/key/id_ed25519 kh=$D/key/known_hosts
    if [ -f "$key" ]; then
        GIT_SSH_COMMAND="ssh -i $key -o IdentitiesOnly=yes -o UserKnownHostsFile=$kh -o StrictHostKeyChecking=yes -o BatchMode=yes -o ConnectTimeout=20 -o ServerAliveInterval=10 -o ServerAliveCountMax=3" \
            git_m fetch --quiet --prune origin "+refs/heads/$BRANCH:refs/heads/$BRANCH"
    elif [ "$DC_TEST" = 1 ] && [ "${DC_DEPLOY_TEST_NO_KEY:-0}" = 1 ]; then
        # the shell tests fetch from a local origin
        git_m fetch --quiet --prune origin "+refs/heads/$BRANCH:refs/heads/$BRANCH"
    else
        # never root's own ssh identity or known hosts
        echo "no deploy key at $key (deploy_setup.sh makes it)" >&2
        return 1
    fi
}

installed_commit() { sed -n 's/^commit: \([0-9a-f]*\).*/\1/p' "$DC_ROOT/bin/COMMIT.txt" 2>/dev/null | head -n 1; }

# write_available OK ERROR: available.json from the mirror as it is (the running build, master, the commits between)
write_available() {
    local ok=$1 err=$2 running full="" master found=0 log_new="" log_old="" count=0
    running=$(installed_commit)
    master=$(git_m rev-parse --verify --quiet "refs/heads/$BRANCH" 2>/dev/null || true)
    if [ -n "$running" ] && [ "${#running}" -ge 7 ]; then
        full=$(git_m rev-parse --verify --quiet "$running^{commit}" 2>/dev/null || true)
    fi
    local fmt='%H%x1f%h%x1f%s%x1f%at'
    if [ -n "$master" ]; then
        if [ -n "$full" ]; then
            found=1
            log_new=$(git_m log -n 50 --format="$fmt" "$full..$master" 2>/dev/null || true)
            count=$(git_m rev-list --count "$full..$master" 2>/dev/null || echo 0)
            log_old=$(git_m log -n 10 --format="$fmt" "$full" 2>/dev/null || true)
        else
            log_new=$(git_m log -n 20 --format="$fmt" "$master" 2>/dev/null || true)
            count=0
        fi
    fi
    mkdir -p "$D"
    printf '%s\n\x1d\n%s\n' "$log_new" "$log_old" | "$DC_PYTHON" -c '
import json, sys, time
ok, err, branch, master, running, found, count, out = sys.argv[1:9]
raw = sys.stdin.read().split("\n\x1d\n")
def rows(text):
    r = []
    for line in text.splitlines():
        p = line.split("\x1f")
        if len(p) == 4:
            r.append({"sha": p[0], "short": p[0][:8], "subject": p[2][:200], "date": int(p[3] or 0)})
    return r
new = rows(raw[0] if raw else "")
old = rows(raw[1] if len(raw) > 1 else "")
doc = {"ok": ok == "1", "error": err[:500], "checked": int(time.time()), "branch": branch,
       "master": master, "master_short": master[:8], "running": running, "running_found": found == "1",
       "new_count": int(count or 0) if found == "1" else None, "new": new, "older": old}
with open(out + ".tmp", "w", encoding="utf-8") as f:
    json.dump(doc, f, indent=1)
' "$ok" "$err" "$BRANCH" "$master" "$running" "$found" "$count" "$D/available.json" \
        && mv -f "$D/available.json.tmp" "$D/available.json" && share "$D/available.json"
}

cmd_fetch() {
    [ -d "$MIRROR" ] || die "no mirror at $MIRROR: run deploy_setup.sh first"
    local err
    if err=$(fetch_mirror 2>&1); then
        write_available 1 ""
        say "fetched $BRANCH: $(git_m rev-parse --short=8 "refs/heads/$BRANCH" 2>/dev/null)"
    else
        err=$(printf '%s' "$err" | tail -n 2 | tr '\n' ' ')
        write_available 0 "fetch failed: $err"
        say "fetch failed: $err"
        return 1
    fi
}

# ---- one deploy

ID="" SHA="" FULL="" SHORT="" SUBJECT="" BY="" TARGETS="" END=0 STARTED=0 LOG=/dev/null

# status PHASE OK MESSAGE: status.json for the dashboard (OK: "" while running, true / false at the end)
status() {
    "$DC_PYTHON" -c '
import json, os, sys, time
(out, rid, sha, short, subject, by, targets, end, started, phase, ok, msg, log) = sys.argv[1:14]
tail = []
try:
    with open(log, encoding="utf-8", errors="replace") as f:
        tail = [l.rstrip("\n")[:300] for l in f.readlines()[-40:]]
except OSError:
    pass
final = ok in ("true", "false")
now = int(time.time())
doc = {"id": rid, "sha": sha, "short": short, "subject": subject, "by": by, "targets": targets.split(),
       "end_matches": end == "1", "phase": phase, "ok": (ok == "true") if final else None, "message": msg[:500],
       "started": int(started or now), "updated": now, "finished": now if final else None, "log_tail": tail}
with open(out + ".tmp", "w", encoding="utf-8") as f:
    json.dump(doc, f, indent=1)
if final and os.environ.get("DC_NOHIST") != "1":
    rec = {k: doc[k] for k in ("id", "sha", "short", "subject", "by", "targets", "started", "finished", "ok", "message")}
    hist = out.rsplit("/", 1)[0] + "/history.jsonl"
    try:
        with open(hist, encoding="utf-8") as f:
            lines = f.read().splitlines()[-49:]
    except OSError:
        lines = []
    lines.append(json.dumps(rec))
    with open(hist + ".tmp", "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
' "$D/status.json" "$ID" "$FULL" "$SHORT" "$SUBJECT" "$BY" "$TARGETS" "$END" "$STARTED" "$1" "$2" "$3" "$LOG" || return 0
    mv -f "$D/status.json.tmp" "$D/status.json" && share "$D/status.json"
    if [ -f "$D/history.jsonl.tmp" ]; then mv -f "$D/history.jsonl.tmp" "$D/history.jsonl" && share "$D/history.jsonl"; fi
}
phase() { say "== $1: $2" >> "$LOG"; status "$1" "" "$2"; }
fail() { say "FAILED: $1" >> "$LOG"; status failed false "$1"; say "deploy $ID: failed: $1"; exit 1; }

# parse_request FILE: the checked fields as KEY=value lines, or exit 1 with the reason (never the file's text)
parse_request() {
    "$DC_PYTHON" -c '
import json, os, re, stat, sys
p = sys.argv[1]
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
    sha = d.get("sha")
    if not isinstance(sha, str) or not re.fullmatch(r"[0-9a-f]{7,40}", sha):
        raise ValueError("sha must be 7 to 40 lower-case hex digits")
    t = d.get("targets", ["vps", "hosts"])
    if not isinstance(t, list) or not t or any(x not in ("vps", "hosts") for x in t):
        raise ValueError("targets must be a non-empty list of vps / hosts")
    end = d.get("end_matches", False)
    if not isinstance(end, bool):
        raise ValueError("end_matches must be true or false")
    by = d.get("by", "")
    by = "".join(c if c.isprintable() else " " for c in (by if isinstance(by, str) else ""))[:120]
except (OSError, ValueError, UnicodeDecodeError) as e:
    print(str(e)[:200])
    sys.exit(1)
order = [x for x in ("vps", "hosts") if x in t]
print("sha=" + sha)
print("targets=" + " ".join(order))
print("end=" + ("1" if end else "0"))
print("by=" + by)
' "$1"
}

matches_running() { "$PGREP" -x dc-dserver >/dev/null 2>&1; }

cmd_run() {
    need_root
    [ -d "$D/requests" ] || die "no $D/requests: run deploy_setup.sh first"
    mkdir -p "$D/logs" "$D/work" "$D/host"
    if command -v flock >/dev/null 2>&1; then
        exec 9> "$D/lock"
        flock -n 9 || { say "a deploy is running; the request waits"; exit 0; }
    fi
    sleep "$SETTLE"
    # a deploy that died half way (the machine went down) is written off before the next one
    if [ -f "$D/status.json" ] && grep -q '"finished": null' "$D/status.json"; then
        "$DC_PYTHON" -c '
import json, sys, time
p = sys.argv[1]
d = json.load(open(p, encoding="utf-8"))
d.update(phase="failed", ok=False, message="interrupted (the deploy service ended before it finished)", finished=int(time.time()))
json.dump(d, open(p + ".tmp", "w", encoding="utf-8"), indent=1)
' "$D/status.json" && mv -f "$D/status.json.tmp" "$D/status.json" && share "$D/status.json"
    fi
    # LG-434: a pack sync that died half way is written off the same way (else the dashboard waits 3 h on it)
    packs_write_off
    # the newest well-named request wins; older ones and anything else in the folder go (a symlink is removed, never
    # read), so the path unit does not start this again for a stray file
    local f newest="" n want_fetch=0 packs_req=""
    while IFS= read -r -d '' f; do
        n=${f##*/}
        if [ ! -L "$f" ] && [ -f "$f" ] && [[ $n =~ $FETCH_RE ]]; then
            # a check request: removed unread, answered below (never a deploy)
            rm -f -- "$f"
            want_fetch=1
            continue
        fi
        if [ ! -L "$f" ] && [ -f "$f" ] && [[ $n =~ $PACKS_RE ]]; then
            # LG-434: a pack sync request: the newest one is answered below (never a deploy), the others go
            if [ -n "$packs_req" ]; then
                if [[ ${n} > ${packs_req##*/} ]]; then rm -f -- "$packs_req"; packs_req=$f; else rm -f -- "$f"; fi
            else
                packs_req=$f
            fi
            continue
        fi
        if [ -L "$f" ] || [ ! -f "$f" ] || ! [[ $n =~ $NAME_RE ]]; then
            rm -rf -- "$f"
            continue
        fi
        if [ -n "$newest" ]; then
            if [[ ${n} > ${newest##*/} ]]; then
                say "request ${newest##*/} superseded by $n"
                rm -f -- "$newest"
            else
                say "request $n superseded by ${newest##*/}"
                rm -f -- "$f"
                continue
            fi
        fi
        newest=$f
    done < <(find "$D/requests" -mindepth 1 -maxdepth 1 -print0)
    if [ -z "$newest" ]; then
        # LG-434: a pack sync first (its own state file), then the check
        [ -n "$packs_req" ] && packs_for_dashboard "$packs_req"
        # LG-402: the lock is held for one fetch only (ssh timeouts above); a deploy request skips it, a deploy fetches itself
        [ "$want_fetch" = 1 ] && fetch_for_dashboard
        [ "$want_fetch" = 1 ] || [ -n "$packs_req" ] || say "no request"
        exit 0
    fi
    # a deploy request beside a pack sync request: the deploy goes first; the sync request stays for the next run
    # (the path unit starts this again while requests/ is not empty)
    ID=$(basename "$newest" .json)
    STARTED=$(date +%s)
    LOG="$D/logs/$ID.log"
    : > "$LOG"
    share "$LOG"
    local fields
    # the lobby writes a request in one write; a reader that came too early tries once more
    fields=$(parse_request "$newest") || { sleep "$SETTLE"; fields=$(parse_request "$newest"); } || true
    if ! printf '%s\n' "$fields" | grep -q '^sha='; then
        rm -f "$newest"
        BY="" TARGETS=""
        say "request $ID refused: $fields" >> "$LOG"
        status failed false "request refused: $fields"
        say "request $ID refused: $fields"
        exit 0
    fi
    rm -f "$newest"
    SHA=$(printf '%s\n' "$fields" | sed -n 's/^sha=//p')
    TARGETS=$(printf '%s\n' "$fields" | sed -n 's/^targets=//p')
    END=$(printf '%s\n' "$fields" | sed -n 's/^end=//p')
    BY=$(printf '%s\n' "$fields" | sed -n 's/^by=//p')
    FULL=$SHA SHORT=${SHA:0:8}
    say "deploy $ID: $SHA ($TARGETS) requested by $BY" >> "$LOG"
    local last
    last=$(recent "")
    if [ -n "$last" ]; then
        refuse_rate "the last deploy finished ${last}s ago; wait $RATE_ANY s between deploys"
    fi
    prune_logs
    deploy
}

# fetch_for_dashboard: one fetch for a "Check now", unless the last check is younger than RATE_FETCH seconds
fetch_for_dashboard() {
    local age
    age=$("$DC_PYTHON" -c '
import json, sys, time
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        print(int(time.time()) - int(json.load(f).get("checked") or 0))
except (OSError, ValueError, AttributeError):
    print(999999)
' "$D/available.json" 2>/dev/null || echo 999999)
    if [ "${age:-999999}" -lt "$RATE_FETCH" ]; then
        say "check request ignored: checked ${age}s ago (limit $RATE_FETCH s)"
        return 0
    fi
    cmd_fetch || true
}

# ---- LG-434: the dashboard's "Sync packs": a packs-<unix>.json names the declared [[packs.sources]] and the mods
# folder. Root trusts nothing in it but its shape, and syncs only from repositories the operator listed in
# $D/packs.allow (root-owned, one `owner/repo` per line, never written by the lobby). The folder must be an existing
# `mods` folder under $DC_ROOT with no symbolic link in its path, checked when the request is read and again right
# before packs_sync.sh installs; packs_sync.sh does the fetching, checking and installing under a timeout, and its
# output is kept in $D/packs.json for the dashboard (rows to paste, log tail). Never a deploy, never a change to
# server.toml.
PACKS_TIMEOUT=${DC_DEPLOY_PACKS_TIMEOUT:-900}

# parse_packs_request FILE: KEY=value lines (by, mods_dir, then one `source=<repo> <tag>` per source), or exit 1
parse_packs_request() {
    "$DC_PYTHON" -c '
import json, os, re, stat, sys
p, root, allow_path = sys.argv[1], os.path.realpath(sys.argv[2]), sys.argv[3]
branches = {"main", "master", "latest", "HEAD", "develop", "dev", "trunk"}
def plain(s):
    return isinstance(s, str) and re.fullmatch(r"[A-Za-z0-9._-]+", s) is not None and s not in (".", "..")
def is_repo(s):
    # the same rule as dc-config is_github_repo: two plain names, at most 100 characters in all
    return isinstance(s, str) and len(s) <= 100 and s.count("/") == 1 and all(plain(h) for h in s.split("/"))
def is_tag(s):
    # the same rule as dc-config is_release_tag: v or a digit first, plain after, at most 64 characters, no branch name
    return (isinstance(s, str) and s not in branches and len(s) <= 64
            and re.fullmatch(r"(v[0-9]|[0-9])[A-Za-z0-9._-]*", s) is not None)
def no_symlink(path):
    acc = ""
    drive, rest = os.path.splitdrive(path)
    for c in rest.split(os.sep):
        if not c:
            continue
        acc = acc + os.sep + c
        if os.path.islink(drive + acc):
            return False
    return True
try:
    try:
        allowed = {l.strip() for l in open(allow_path, encoding="utf-8") if l.strip() and not l.lstrip().startswith("#")}
    except OSError:
        raise ValueError("no " + allow_path + ": root lists the pack repositories the dashboard may sync, one owner/repo per line")
    fd = os.open(p, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_BINARY", 0))
    with os.fdopen(fd, "rb") as f:
        if not stat.S_ISREG(os.fstat(f.fileno()).st_mode):
            raise ValueError("not a regular file")
        raw = f.read(8193)
    if len(raw) > 8192:
        raise ValueError("over 8192 bytes")
    d = json.loads(raw.decode("utf-8"))
    if not isinstance(d, dict):
        raise ValueError("not a JSON object")
    mods = d.get("mods_dir")
    if not isinstance(mods, str) or not mods or not mods.isprintable():
        raise ValueError("mods_dir must be a plain path")
    real = os.path.realpath(mods)
    if not real.isprintable() or any(ord(c) < 32 for c in real):
        raise ValueError("mods_dir must be a plain path")
    if not os.path.isdir(real) or os.path.basename(real) != "mods" or not real.startswith(root + os.sep):
        raise ValueError("mods_dir must be an existing mods folder under " + root)
    if real != os.path.normpath(mods) or not no_symlink(real):
        raise ValueError("mods_dir must be the real path of the folder, with no symbolic link in it")
    src = d.get("sources")
    if not isinstance(src, list) or not src or len(src) > 8:
        raise ValueError("sources must be a list of 1 to 8 {repo, tag}")
    out = []
    for s in src:
        repo, tag = (s.get("repo"), s.get("tag")) if isinstance(s, dict) else (None, None)
        if not is_repo(repo):
            raise ValueError("a source repo is not a GitHub owner/repo")
        if not is_tag(tag):
            raise ValueError("a source tag is not a release tag (a branch moves; a pack is its hash)")
        if repo not in allowed:
            raise ValueError("repo " + repo + " is not in " + allow_path + " (root lists the repositories the dashboard may sync)")
        out.append(repo + " " + tag)
    by = d.get("by", "")
    by = "".join(c if c.isprintable() else " " for c in (by if isinstance(by, str) else ""))[:120]
except (OSError, ValueError, UnicodeDecodeError, AttributeError) as e:
    print(str(e)[:200])
    sys.exit(1)
print("by=" + by)
print("mods_dir=" + real)
for s in out:
    print("source=" + s)
' "$1" "$DC_ROOT" "$D/packs.allow"
}

# mods_dir_still_good DIR: the folder is still the real, link-free mods folder under the root (right before a write)
mods_dir_still_good() {
    "$DC_PYTHON" -c '
import os, sys
d, root = sys.argv[1], os.path.realpath(sys.argv[2])
real = os.path.realpath(d)
ok = real == os.path.normpath(d) and os.path.isdir(real) and os.path.basename(real) == "mods" and real.startswith(root + os.sep)
acc = ""
drive, rest = os.path.splitdrive(real)
for c in rest.split(os.sep):
    if c:
        acc = acc + os.sep + c
        if os.path.islink(drive + acc):
            ok = False
sys.exit(0 if ok else 1)
' "$1" "$DC_ROOT"
}

# packs_status PHASE OK MESSAGE ROWS_FILE LOG_FILE: $D/packs.json for the dashboard
packs_status() {
    "$DC_PYTHON" -c '
import json, sys, time
out, phase, ok, msg, rows_f, log_f, by, sources, mods_dir, started = sys.argv[1:11]
def tail(p, n):
    try:
        with open(p, encoding="utf-8", errors="replace") as f:
            return [l.rstrip("\n")[:300] for l in f.readlines()[-n:]]
    except OSError:
        return []
try:
    rows = open(rows_f, encoding="utf-8", errors="replace").read()[:16000]
except OSError:
    rows = ""
now = int(time.time())
doc = {"phase": phase, "ok": None if ok == "" else ok == "true", "message": msg, "by": by,
       "sources": sources.split("\n") if sources else [], "mods_dir": mods_dir, "started": int(started),
       "updated": now, "finished": None if ok == "" else now, "rows": rows, "log_tail": tail(log_f, 40)}
with open(out + ".tmp", "w", encoding="utf-8") as f:
    json.dump(doc, f, indent=1)
' "$D/packs.json" "$1" "$2" "$3" "$4" "$5" "$PK_BY" "$PK_SOURCES" "$PK_MODS" "$PK_STARTED" \
        && mv -f "$D/packs.json.tmp" "$D/packs.json" && share "$D/packs.json"
}

# packs_note MESSAGE: a note on the last outcome (the dashboard shows it), the outcome itself kept
packs_note() {
    [ -f "$D/packs.json" ] || return 0
    "$DC_PYTHON" -c '
import json, sys, time
p = sys.argv[1]
try:
    d = json.load(open(p, encoding="utf-8"))
except (OSError, ValueError):
    sys.exit(0)
d["message"] = sys.argv[2][:300]
d["updated"] = int(time.time())
json.dump(d, open(p + ".tmp", "w", encoding="utf-8"), indent=1)
' "$D/packs.json" "$1" && mv -f "$D/packs.json.tmp" "$D/packs.json" && share "$D/packs.json"
}

# packs_write_off: a sync that died half way (the service ended before it finished) is written off before the next
packs_write_off() {
    [ -f "$D/packs.json" ] && grep -q '"finished": null' "$D/packs.json" || return 0
    "$DC_PYTHON" -c '
import json, sys, time
p = sys.argv[1]
d = json.load(open(p, encoding="utf-8"))
d.update(phase="failed", ok=False, message="interrupted (the deploy service ended before the sync finished)", finished=int(time.time()), updated=int(time.time()))
json.dump(d, open(p + ".tmp", "w", encoding="utf-8"), indent=1)
' "$D/packs.json" && mv -f "$D/packs.json.tmp" "$D/packs.json" && share "$D/packs.json"
}

PK_BY="" PK_SOURCES="" PK_MODS="" PK_STARTED=0

# packs_for_dashboard FILE: answer one Sync packs request (the file is removed first; its text never reaches a shell)
packs_for_dashboard() {
    local f=$1 fields age sync rows log ok=true msg="" src repo tag rc
    fields=$(parse_packs_request "$f") || { sleep "$SETTLE"; fields=$(parse_packs_request "$f"); } || true
    rm -f -- "$f"
    PK_STARTED=$(date +%s)
    PK_BY=$(printf '%s\n' "$fields" | sed -n 's/^by=//p')
    if ! printf '%s\n' "$fields" | grep -q '^source='; then
        PK_SOURCES="" PK_MODS=""
        say "pack sync request refused: $fields"
        packs_status failed false "request refused: $fields" /dev/null /dev/null
        return 0
    fi
    # the rate limit counts from the last sync that succeeded (a refused request holds nobody back)
    age=$("$DC_PYTHON" -c '
import json, sys, time
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        d = json.load(f)
    fin = d.get("finished")
    print(int(time.time()) - int(fin) if d.get("ok") is True and isinstance(fin, int) else 999999)
except (OSError, ValueError, TypeError, AttributeError, OverflowError):
    print(999999)
' "$D/packs.json" 2>/dev/null || echo 999999)
    case "$age" in ''|*[!0-9-]*) age=999999 ;; esac
    if [ "$age" -ge 0 ] && [ "$age" -lt "$RATE_PACKS" ]; then
        say "pack sync request ignored: synced ${age}s ago (limit $RATE_PACKS s)"
        packs_note "sync request ignored: synced ${age} s ago; wait $RATE_PACKS s between syncs"
        return 0
    fi
    PK_MODS=$(printf '%s\n' "$fields" | sed -n 's/^mods_dir=//p')
    PK_SOURCES=$(printf '%s\n' "$fields" | sed -n 's/^source=//p')
    sync="$DC_ROOT/tools/packs_sync.sh"
    [ -f "$sync" ] || sync="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/packs_sync.sh"
    mkdir -p "$D/logs"
    log="$D/logs/packs-$PK_STARTED.log"
    rows="$D/logs/packs-$PK_STARTED.rows"
    : > "$log"; : > "$rows"
    packs_status running "" "syncing $(printf '%s\n' "$PK_SOURCES" | wc -l | tr -d ' ') source(s)" "$rows" "$log"
    while IFS= read -r src; do
        [ -n "$src" ] || continue
        repo=${src%% *}; tag=${src#* }
        say "packs: $repo $tag -> $PK_MODS" >> "$log"
        # the folder is checked again right before root writes into it (a swap for a link in between is refused)
        if ! mods_dir_still_good "$PK_MODS"; then
            ok=false; msg="$PK_MODS is no longer the real mods folder (changed since the request): nothing installed"
            say "$msg" >> "$log"
            break
        fi
        # DC_PACKS_BASE_URL (tests): a folder laid out as <base>/<owner>/<repo>/<tag>/ stands in for GitHub
        if [ -n "${DC_PACKS_BASE_URL:-}" ]; then
            set -- --base-url "$DC_PACKS_BASE_URL/$repo/$tag"
        else
            set --
        fi
        timeout "$PACKS_TIMEOUT" bash "$sync" "$repo" "$tag" --mods-dir "$PK_MODS" "$@" > "$log.part" 2>&1
        rc=$?
        cat "$log.part" >> "$log"
        if [ "$rc" -ne 0 ]; then
            if [ "$rc" -eq 124 ]; then
                msg="$repo $tag: packs_sync.sh did not finish within $PACKS_TIMEOUT s"
            else
                msg="$repo $tag: packs_sync.sh failed (see the log tail)"
            fi
            ok=false
            rm -f "$log.part"
            break
        fi
        # the rows the script prints, kept for the operator to paste
        printf '# %s %s\n' "$repo" "$tag" >> "$rows"
        "$DC_PYTHON" -c '
import sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read().splitlines()
keep = []
for l in text:
    t = l.strip()
    if (t.startswith("{ id = ") or t == "[[mods.set]]" or t.startswith(("id = ", "version = ", "sha256 = "))
            or (t.startswith("# ") and "rows" in t)):
        keep.append(l)
print(chr(10).join(keep))
' "$log.part" >> "$rows"
        rm -f "$log.part"
    done <<< "$PK_SOURCES"
    if [ "$ok" = true ]; then
        msg="installed or present: $(printf '%s\n' "$PK_SOURCES" | tr '\n' ';' | sed 's/;$//; s/;/, /g')"
        say "pack sync done: $msg"
        packs_status done true "$msg" "$rows" "$log"
    else
        say "pack sync failed: $msg"
        packs_status failed false "$msg" "$rows" "$log"
    fi
    return 0
}

# recent SHA: seconds since the last finished deploy (of that commit, when given) if within the limit, else nothing
recent() {
    "$DC_PYTHON" -c '
import json, sys, time
hist, sha, lim_any, lim_same = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
try:
    with open(hist, encoding="utf-8") as f:
        rows = [json.loads(l) for l in f.read().splitlines() if l.strip()]
except (OSError, ValueError):
    rows = []
now = int(time.time())
for r in reversed(rows):
    age = now - int(r.get("finished") or 0)
    if sha and r.get("sha") == sha and age < lim_same:
        print(age); break
    if not sha and age < lim_any:
        print(age); break
' "$D/history.jsonl" "$1" "$RATE_ANY" "$RATE_SAME" 2>/dev/null
}
refuse_rate() {
    say "refused: $1" >> "$LOG"
    DC_NOHIST=1 status failed false "refused: $1"
    say "deploy $ID refused: $1"
    exit 0
}
prune_logs() {
    local old
    old=$(ls -t "$D"/logs/*.log 2>/dev/null | sed -n "$((LOGS_KEPT + 1)),\$p")
    [ -z "$old" ] || printf '%s\n' "$old" | while IFS= read -r f; do rm -f -- "$f"; done
}

# sign_offer FULL NAME SUM SIZE: desired.txt for the hosts, signed with a sequence number one above the last
sign_offer() {
    local key=$D/sign/host_update_ed25519 seq msg
    [ -f "$key" ] || fail "no signing key $key: run deploy_setup.sh"
    seq=$(( $(cat "$D/sign/seq" 2>/dev/null | tr -cd '0-9' | sed 's/^$/0/') + 1 ))
    msg=$D/sign/offer.msg
    offer_message "$1" "$2" "$3" "$4" "$seq" > "$msg"
    rm -f "$msg.sig"
    "$SSHKEYGEN" -q -Y sign -f "$key" -n "$SIGN_NS" "$msg" >> "$LOG" 2>&1 || fail "could not sign the host offer"
    printf '%s\n' "$seq" > "$D/sign/seq.tmp" && mv -f "$D/sign/seq.tmp" "$D/sign/seq"
    printf 'commit=%s\ntarball=%s\nsha256=%s\nsize=%s\nseq=%s\nsig=%s\n' "$1" "$2" "$3" "$4" "$seq" \
        "$(base64 < "$msg.sig" | tr -d '\n\r')" > "$D/host/desired.txt.tmp" \
        && mv -f "$D/host/desired.txt.tmp" "$D/host/desired.txt" || fail "could not write $D/host/desired.txt"
}
# the exact bytes both sides sign and verify (tools/host/update.sh offer_message is the same)
offer_message() {
    printf 'decombat-host-update 1\ncommit=%s\ntarball=%s\nsha256=%s\nsize=%s\nseq=%s\n' "$@"
}

deploy() {
    local W=$D/work
    phase fetching "fetching $BRANCH"
    fetch_mirror >> "$LOG" 2>&1 || fail "could not fetch $BRANCH from the repository (deploy key? network?)"
    FULL=$(git_m rev-parse --verify --quiet "$SHA^{commit}" 2>/dev/null) || fail "$SHA is not a commit of the repository (or a short id that is not unique)"
    printf '%s' "$FULL" | grep -Eqx '[0-9a-f]{40}' || fail "$SHA did not resolve to one commit"
    git_m merge-base --is-ancestor "$FULL" "refs/heads/$BRANCH" 2>/dev/null || fail "${FULL:0:8} is not on $BRANCH"
    SHORT=${FULL:0:8}
    SUBJECT=$(git_m log -1 --format=%s "$FULL" 2>/dev/null | cut -c1-200)
    local again
    again=$(recent "$FULL")
    [ -z "$again" ] || refuse_rate "${FULL:0:8} was deployed ${again}s ago; the same commit waits $RATE_SAME s"
    write_available 1 ""

    # ---- the source of that commit beside the old one; the warm cargo target moves over
    phase building "building $SHORT"
    rm -rf "$W/src.new" && mkdir -p "$W/src.new" || fail "no work folder $W"
    git_m archive --format=tar "$FULL" | tar -xf - -C "$W/src.new" || fail "git archive of $SHORT failed"
    if [ -d "$W/src/target" ]; then mv "$W/src/target" "$W/src.new/target"; fi
    rm -rf "$W/src" && mv "$W/src.new" "$W/src" || fail "could not move the source into place"
    local cargo jobs rel=$W/src/target/release b
    cargo=$(conf CARGO "")
    [ -n "$cargo" ] || cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
    jobs=$(conf BUILD_JOBS "")
    case "$jobs" in ''|*[!0-9]*) jobs="" ;; esac
    for b in dc-lobby-server dc-dserver dc-server-rom dc-config dc-botclient dc-host; do rm -f "$rel/$b"; done
    # deploy_all.sh's two builds: the VPS set, then the host agent (the same target; dc-dserver and dc-server-rom are shared)
    ( cd "$W/src" && DC_GIT_COMMIT=$SHORT nice -n 10 "$cargo" build --release ${jobs:+-j "$jobs"} \
        -p dc-lobby-server -p dc-dserver -p dc-rez -p dc-botclient -p dc-config ) >> "$LOG" 2>&1 || fail "the build of $SHORT failed (log tail below)"
    ( cd "$W/src" && DC_GIT_COMMIT=$SHORT nice -n 10 "$cargo" build --release ${jobs:+-j "$jobs"} \
        -p dc-host --bin dc-host ) >> "$LOG" 2>&1 || fail "the dc-host build of $SHORT failed (log tail below)"
    for b in dc-lobby-server dc-dserver dc-server-rom dc-config dc-botclient dc-host; do
        [ -x "$rel/$b" ] || fail "the build left no $b"
    done

    phase packaging "packing $SHORT"
    local name=decombat-servers-linux-x86_64-$SHORT tgz sum
    rm -rf "$W/out" && mkdir -p "$W/out"
    tgz=$(bash "$W/src/tools/package_servers.sh" --bin-dir "$rel" --name "$name" --out "$W/out" --commit "$FULL" 2>>"$LOG" | tail -n 1)
    [ -f "$tgz" ] || fail "tools/package_servers.sh made no tarball"
    sum=$(sha256sum "$tgz" | cut -d' ' -f1)
    say "tarball $(basename "$tgz") sha256 $sum" >> "$LOG"

    if printf '%s' " $TARGETS " | grep -q ' vps '; then
        if [ "$END" != 1 ]; then
            local waited=0 limit
            limit=$(( $(conf MATCH_WAIT_MIN 15) * 60 ))
            while matches_running; do
                [ "$waited" -lt "$limit" ] || fail "matches were still running after $((limit / 60)) min; deploy again with \"end running matches\""
                [ "$waited" -gt 0 ] || phase waiting "waiting for the running matches to end (at most $((limit / 60)) min)"
                sleep "$MATCH_POLL"
                waited=$((waited + MATCH_POLL))
            done
        fi
        phase installing "installing $SHORT and restarting the lobbies"
        rm -rf "$W/pkg" && mkdir -p "$W/pkg" && tar -xzf "$tgz" -C "$W/pkg" --no-same-owner || fail "could not unpack the tarball"
        local remote=${DC_DEPLOY_REMOTE:-$W/pkg/$name/tools/vps/deploy_remote.sh} required
        required=$(conf REQUIRED_INSTANCES "")
        case "$required" in *[!a-z0-9_\ -]*) fail "deploy.conf: REQUIRED_INSTANCES holds something that is not an instance name" ;; esac
        # shellcheck disable=SC2086
        bash "$remote" install "$tgz" "$sum" "$FULL" $required >> "$LOG" 2>&1 || fail "the install did not finish cleanly (log tail below)"
    fi

    if printf '%s' " $TARGETS " | grep -q ' hosts '; then
        phase publishing "offering $SHORT to the game hosts"
        install -m 0640 "$tgz" "$D/host/.$name.tar.gz.new" && mv -f "$D/host/.$name.tar.gz.new" "$D/host/$name.tar.gz" || fail "could not write $D/host"
        share "$D/host/$name.tar.gz"
        sign_offer "$FULL" "$name.tar.gz" "$sum" "$(wc -c < "$tgz" | tr -d ' ')"
        share "$D/host/desired.txt"
        # this build and the one before it stay
        local keep
        keep=$(ls -t "$D"/host/decombat-servers-linux-x86_64-*.tar.gz 2>/dev/null | sed -n '3,$p')
        [ -z "$keep" ] || printf '%s\n' "$keep" | while read -r f; do rm -f "$f"; done
    fi
    write_available 1 ""
    local what="deployed $SHORT"
    printf '%s' " $TARGETS " | grep -q ' vps ' && what="$what to the VPS"
    printf '%s' " $TARGETS " | grep -q ' hosts ' && what="$what, offered to the game hosts"
    say "$what" >> "$LOG"
    status done true "$what"
    say "deploy $ID: $what"
}

main() {
    case "${1:-}" in
        fetch) need_root; cmd_fetch ;;
        run) cmd_run ;;
        status) cat "$D/status.json" 2>/dev/null || say "no deploy yet" ;;
        -h|--help) sed -n '2,20p' "$0" ;;
        *) die "fetch, run or status (--help)" ;;
    esac
}
# the whole script is read before it runs: install.sh replaces this file during a deploy
main "$@"
exit $?
