#!/usr/bin/env bash
# release_test.sh: tests of tools/vps/release_from_git.sh and release_setup.sh (docs/DEPLOY_FROM_ADMIN.md "Releases
# from the dashboard") on a temp root: a real git private origin, mirror and public repository (local folders), a
# stand-in export tool, a stand-in curl that answers as the GitHub API from files, and a logging git wrapper. No
# network. Runs under Git Bash on Windows and on Linux: bash tools/vps/tests/release_test.sh
# DC_RELEASE_TEST_TOOL=<file> runs the prepare steps with that export tool instead of the stand-in.
# Provenance: ORIGINAL
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VPS=$(cd "$HERE/.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/dc-release-test.XXXXXX")
trap 'rm -rf "${T:?}"' EXIT

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL: %s\n' "$*"; }
expect() { local d=$1; shift; if "$@"; then ok; else bad "$d"; fi; }
has() { grep -qF -- "$2" "$1" 2>/dev/null; }
lacks() { ! grep -qF -- "$2" "$1" 2>/dev/null; }
not() { ! "$@"; }
json() { "$PY" -c 'import json,sys; d=json.load(open(sys.argv[1],encoding="utf-8")); v=eval(sys.argv[2],{"d":d}); print(json.dumps(v) if not isinstance(v,str) else v)' "$1" "$2"; }

PY=""
for c in python3 python; do if "$c" -c "pass" >/dev/null 2>&1; then PY=$c; break; fi; done
[ -n "$PY" ] || { echo "no python: skipped"; exit 0; }
command -v git >/dev/null 2>&1 || { echo "no git: skipped"; exit 0; }

for f in "$VPS/release_from_git.sh" "$VPS/release_setup.sh" "$HERE/release_test.sh"; do
    expect "bash -n $(basename "$f")" bash -n "$f"
done

export DC_TEST=1 DC_ROOT="$T/opt" DC_SYSTEMD_DIR="$T/systemd" DC_PYTHON="$PY" STUB_LOG="$T/stub.log"
export DC_RELEASE_SETTLE=0 DC_RELEASE_POLL=0 DC_RELEASE_CI_MAX=0 DC_RELEASE_REL_MAX=0 DC_RELEASE_TEST_NO_KEY=1
# the rate limits have their own tests below
export DC_RELEASE_RATE_CHECK=0 DC_RELEASE_RATE_PREPARE=0 DC_RELEASE_RATE_TAG=0
: > "$T/gitconfig"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL="$T/gitconfig"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com
unset DC_ADMIN DC_INSTANCE DC_UNIT GITHUB_TOKEN
D="$DC_ROOT/deploy"
R="$D/release"
ST="$R/status.json"
SS="$R/state.json"
# a token of the right shape that no secret scanner takes for a real one
TOKEN="tok_$(date +%s)_qwertyuiopasdfghjklzxcvbnm"
export EXPECT_TOKEN=$TOKEN

# ---- the private origin, the mirror (as deploy_setup.sh leaves it), the public repository at beta6
O="$T/origin"
mkdir -p "$O"
git -C "$O" init -q -b master
commit() { # commit SUBJECT [FILE TEXT]
    if [ $# -ge 3 ]; then printf '%s\n' "$3" > "$O/$2"; else printf 'change %s\n' "$RANDOM$RANDOM" >> "$O/README"; fi
    git -C "$O" add -A >/dev/null
    git -C "$O" commit -q -m "$1"
    git -C "$O" rev-parse HEAD
}
A=$(commit "Merge beta6-base: the build of beta6 (CLI-1)")
mkdir -p "$D/requests" "$DC_ROOT/bin" "$DC_ROOT/tools" "$DC_SYSTEMD_DIR"
git init -q --bare "$DC_ROOT/src.git"
git --git-dir="$DC_ROOT/src.git" remote add origin "$O"

P="$T/public.git"
git init -q --bare -b master "$P"
git clone -q "$P" "$T/pubwork" 2>/dev/null
printf 'public beta6\n' > "$T/pubwork/README"
git -C "$T/pubwork" add -A && git -C "$T/pubwork" commit -q -m "Beta 6" && git -C "$T/pubwork" push -q origin HEAD:refs/heads/master
PUB6=$(git -C "$T/pubwork" rev-parse HEAD)
export DC_RELEASE_PUBLIC_URL="$P"
pub_master() { git --git-dir="$P" rev-parse refs/heads/master; }

# ---- stand-ins
S="$T/stubs"
mkdir -p "$S" "$T/api"
cat > "$S/git" <<'X'
#!/usr/bin/env bash
printf 'git' >> "$STUB_LOG.git"; printf ' %q' "$@" >> "$STUB_LOG.git"; printf '\n' >> "$STUB_LOG.git"
exec git "$@"
X
# the stand-in GitHub API: answers from $T/api/<METHOD_path>.json (a 40-hex sha in the path may be written SHA),
# a <key>.code file for another HTTP status; logs the method, path, whether the token came in a header, and the data
cat > "$S/curl.py" <<'X'
import json, os, re, sys
api, log = sys.argv[1], sys.argv[2]
a = sys.argv[3:]
method, out, fmt, data = "GET", None, "", None
i = 0
while i < len(a):
    if a[i] == "-X": method = a[i + 1]; i += 1
    elif a[i] == "-o": out = a[i + 1]; i += 1
    elif a[i] == "-w": fmt = a[i + 1]; i += 1
    elif a[i] == "--data-binary": data = open(a[i + 1][1:], encoding="utf-8").read(); i += 1
    elif a[i] == "--max-time": i += 1
    i += 1
url, auth = "", "none"
for ln in sys.stdin.read().splitlines():
    m = re.match(r'^(\w+) = "(.*)"$', ln)
    if not m: continue
    if m.group(1) == "url": url = m.group(2)
    elif m.group(2).startswith("Authorization: Bearer "):
        auth = "ok" if m.group(2)[len("Authorization: Bearer "):] == os.environ.get("EXPECT_TOKEN") else "wrong"
path = re.sub(r"^https?://[^/]+/repos/[^/]+/[^/]+/", "", url)
argv_has_token = any(os.environ.get("EXPECT_TOKEN", "\0") in x for x in sys.argv)
key = re.sub(r"[^A-Za-z0-9]", "_", method + " " + path)
key2 = re.sub(r"[^A-Za-z0-9]", "_", method + " " + re.sub(r"[0-9a-f]{40}", "SHA", path))
with open(log, "a", encoding="utf-8") as f:
    f.write("curl %s %s auth=%s argv_token=%s\n" % (method, path, auth, argv_has_token))
    if data is not None:
        f.write("DATA " + json.dumps(json.loads(data)) + "\n")
body, code = "{}", "404"
for k in (key, key2):
    p = os.path.join(api, k + ".json")
    if os.path.isfile(p):
        body = open(p, encoding="utf-8").read()
        c = os.path.join(api, k + ".code")
        code = open(c).read().strip() if os.path.isfile(c) else "200"
        break
if out:
    open(out, "w", encoding="utf-8").write(body)
sys.stdout.write(fmt.replace("%{http_code}", code))
X
cat > "$S/curl" <<X
#!/usr/bin/env bash
exec "$PY" "$S/curl.py" "$T/api" "\$STUB_LOG" "\$@"
X
# the stand-in export tool: the same arguments, word lists, summary line and parent handling as the real one
cat > "$T/export_tool.py" <<'X'
import argparse, io, os, re, shutil, subprocess, sys, tarfile
HERE = os.path.dirname(os.path.abspath(__file__))
SOURCE_WORDS = [(r"\bquokka-secret\b", "fixture word")]
SECRETS = [(r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----", "private key block")]
def load_private():
    p = os.path.join(HERE, "private_strings.txt")
    if not os.path.isfile(p):
        sys.exit("export tool: %s is missing" % p)
    rows = [l.rstrip("\r\n") for l in open(p, encoding="utf-8")]
    return [("value %d" % n, re.compile(re.escape(l))) for n, l in enumerate(rows, 1) if l.strip() and not l.startswith("#")]
def git(cwd, *a, env=None):
    r = subprocess.run(["git", "-C", cwd] + list(a), capture_output=True, env=env)
    if r.returncode:
        sys.exit("git %s: %s" % (" ".join(a), r.stderr.decode()))
    return r.stdout
def main():
    ap = argparse.ArgumentParser()
    for k in ("--repo", "--commit", "--out", "--name", "--email", "--message", "--parent"):
        ap.add_argument(k)
    a = ap.parse_args()
    full = git(a.repo, "rev-parse", "--verify", a.commit + "^{commit}").decode().strip()
    os.makedirs(a.out)
    tarfile.open(fileobj=io.BytesIO(git(a.repo, "archive", "--format=tar", full))).extractall(a.out)
    pats = [rx for _, rx in load_private()] + [re.compile(p) for p, _ in SOURCE_WORDS]
    hits = 0
    for d, _, ns in os.walk(a.out):
        for n in ns:
            t = open(os.path.join(d, n), encoding="latin-1").read()
            hits += sum(1 for rx in pats if rx.search(t))
    print("export_public commit=%s files=1 left_out=0 refusals=%d {}" % (full[:12], hits))
    if hits:
        print("REFUSED: no commit made")
        return 1
    env = dict(os.environ, GIT_AUTHOR_NAME=a.name, GIT_AUTHOR_EMAIL=a.email, GIT_COMMITTER_NAME=a.name, GIT_COMMITTER_EMAIL=a.email)
    shutil.copytree(os.path.join(a.parent, ".git"), os.path.join(a.out, ".git"))
    git(a.out, "add", "-A", env=env)
    git(a.out, "commit", "-q", "-m", a.message, env=env)
    return 0
if __name__ == "__main__":
    sys.exit(main())
X
TOOL_FILE=${DC_RELEASE_TEST_TOOL:-$T/export_tool.py}
printf '# fixture word list\nzebrafish-internal\n' > "$T/strings.txt"
cat > "$S/systemctl" <<'X'
#!/usr/bin/env bash
echo "systemctl $*" >> "$STUB_LOG"
X
cat > "$S/export_dies" <<'X'
#!/usr/bin/env bash
echo "Traceback: the stand-in export died"
exit 1
X
chmod +x "$S/"*
export DC_RELEASE_GIT="$S/git" DC_RELEASE_CURL="$S/curl" DC_RELEASE_SYSTEMCTL="$S/systemctl"
REL="$VPS/release_from_git.sh"

# route METHOD PATH JSON [CODE]: what the stand-in API answers
route() {
    local key
    key=$(printf '%s %s' "$1" "$2" | tr -c 'A-Za-z0-9' '_')
    printf '%s' "$3" > "$T/api/$key.json"
    if [ $# -ge 4 ]; then printf '%s' "$4" > "$T/api/$key.code"; else rm -f "$T/api/$key.code"; fi
}
RUNS="actions/runs?head_sha=SHA&per_page=50"
runs() { # runs CI_CONCLUSION [RELEASE_TAG RELEASE_CONCLUSION]
    local rel=""
    [ $# -lt 2 ] || rel=",{\"id\":21,\"name\":\"release\",\"head_branch\":\"$2\",\"status\":\"completed\",\"conclusion\":\"$3\",\"html_url\":\"https://github.com/argallo/DeCombat/actions/runs/21\"}"
    route GET "$RUNS" "{\"workflow_runs\":[{\"id\":9,\"name\":\"ci\",\"head_branch\":\"master\",\"status\":\"completed\",\"conclusion\":\"failure\",\"html_url\":\"old\"},{\"id\":11,\"name\":\"ci\",\"head_branch\":\"master\",\"status\":\"completed\",\"conclusion\":\"$1\",\"html_url\":\"https://github.com/argallo/DeCombat/actions/runs/11\"},{\"id\":12,\"name\":\"release\",\"head_branch\":\"master\",\"status\":\"completed\",\"conclusion\":\"failure\",\"html_url\":\"x\"}$rel]}"
}
jobs_ok() { route GET "actions/runs/$1/jobs?per_page=100" '{"jobs":[{"name":"test (windows-x86_64)","conclusion":"success","html_url":"j1"},{"name":"lint (fmt, clippy, clean-room gate)","conclusion":"success","html_url":"j2"}]}'; }
jobs_bad() { route GET "actions/runs/$1/jobs?per_page=100" "{\"jobs\":[{\"name\":\"test (windows-x86_64)\",\"conclusion\":\"success\",\"html_url\":\"j1\"},{\"name\":\"$2\",\"conclusion\":\"failure\",\"html_url\":\"https://github.com/argallo/DeCombat/actions/runs/$1/job/77\"}]}"; }
rel() { : > "$STUB_LOG"; : > "$STUB_LOG.git"; bash "$REL" "$@" > "$T/out" 2>&1 < /dev/null; rc=$?; }
phase_is() { test "$(json "$ST" 'd["phase"]')" = "$1"; }
request() { printf '%s' "$2" > "$R/requests/$1.json"; }

# ---- release_setup.sh
SETUP="$VPS/release_setup.sh"
SEED=(--last-tag v0.5.0-beta6 --private-sha "${A:0:12}" --public-sha "${PUB6:0:7}")
out=$(bash "$SETUP" --export-tool "$TOOL_FILE" --private-strings "$T/strings.txt" "${SEED[@]}" 2>&1 < /dev/null)
expect "setup without a token refuses" test $? -ne 0
expect "and says so" test -n "$(printf '%s' "$out" | grep 'no token')"
expect "and writes no release.conf" test ! -f "$D/release.conf"
: > "$STUB_LOG"
out=$(GITHUB_TOKEN=$TOKEN bash "$SETUP" --export-tool "$TOOL_FILE" --private-strings "$T/strings.txt" "${SEED[@]}" 2>&1 < /dev/null)
rc=$?
expect "setup succeeds ($out)" test "$rc" -eq 0
expect "setup never prints the token" test -z "$(printf '%s' "$out" | grep -F "$TOKEN")"
expect "release.conf holds the token" has "$D/release.conf" "GITHUB_TOKEN=$TOKEN"
expect "and the public repository" has "$D/release.conf" "PUBLIC_REPO=argallo/DeCombat"
expect "the request folder exists" test -d "$R/requests"
expect "the export tool is installed" test -f "$R/tool/export_public.py"
expect "with its word list" has "$R/tool/private_strings.txt" "zebrafish-internal"
expect "the public clone is at the public master" test "$(git -C "$D/public" rev-parse HEAD)" = "$PUB6"
expect "state.json is seeded" test "$(json "$SS" '[d["last_tag"], d["last_released_private_sha"], d["auto_prepare"]]')" = "[\"v0.5.0-beta6\", \"${A:0:12}\", 1]"
expect "status.json starts idle" phase_is idle
expect "the scripts are installed for the units" test -f "$DC_ROOT/tools/release_from_git.sh"
expect "the lobby may write the request folder" has "$DC_SYSTEMD_DIR/decombat-lobby.service.d/21-release.conf" "ReadWritePaths=-$R/requests"
expect "the path unit watches it" has "$DC_SYSTEMD_DIR/decombat-release.path" "DirectoryNotEmpty=$R/requests"
expect "the release service runs the installed script" has "$DC_SYSTEMD_DIR/decombat-release.service" "$DC_ROOT/tools/release_from_git.sh run"
expect "the timer runs daily at 06:00 UTC" has "$DC_SYSTEMD_DIR/decombat-release-check.timer" "OnCalendar=*-*-* 06:00:00 UTC"
expect "and starts the daily job" has "$DC_SYSTEMD_DIR/decombat-release-check.service" "release_from_git.sh daily"
expect "setup enables the timer and the path unit" has "$STUB_LOG" "enable --now decombat-release-check.timer decombat-release.path"
json "$SS" 'd' > "$T/state0"
out=$(bash "$SETUP" --last-tag v0.1.0-beta1 --private-sha 0000000 --public-sha 0000000 2>&1 < /dev/null)
expect "a second setup keeps the token" has "$D/release.conf" "GITHUB_TOKEN=$TOKEN"
expect "and keeps state.json" test "$(json "$SS" 'd')" = "$(cat "$T/state0")"

# ---- check
rel check
expect "check succeeds ($(cat "$T/out"))" test "$rc" -eq 0
expect "nothing new on the released commit" test "$(json "$ST" 'd["message"]')" = "nothing new since v0.5.0-beta6"
expect "commits_since 0" test "$(json "$ST" 'd["commits_since"]')" = 0
B=$(commit "Merge cli-fix: the client draws the scope (CLI-1700)")
C=$(commit "Merge dsv-fix: the server counts the score (DSV-700)")
Dz=$(commit "Merge ops-x: a zebrafish-internal note (OPS-99)")
E=$(commit "Merge lg-x: the lobby lists rooms (LG-500); thanks @someone")
rel check
expect "check sees the new commits" test "$(json "$ST" 'd["message"]')" = "4 commits since v0.5.0-beta6"
expect "and stays idle" phase_is idle
expect "and names the next tag" test "$(json "$ST" 'd["next_tag"]')" = "v0.6.0-beta7"
expect "and the mirror's master" test "$(json "$ST" 'd["private_sha"]')" = "$E"

# ---- prepare and tag refusals before anything is ready
rel tag
expect "tag from idle is refused" has "$ST" "nothing is ready to tag"
expect "and pushes no tag" test -z "$(git --git-dir="$P" tag)"
mv "$D/release.conf" "$T/conf.bak"
rel prepare
expect "prepare without the token fails" test "$rc" -ne 0
expect "with phase failed, no token" test "$(json "$ST" '[d["phase"], d["message"][:8]]')" = '["failed", "no token"]'
mv "$T/conf.bak" "$D/release.conf"
mv "$R/tool/export_public.py" "$T/tool.bak"
rel prepare
expect "prepare without the export tool fails" has "$ST" "no export tool"
mv "$T/tool.bak" "$R/tool/export_public.py"
expect "nothing was pushed" test "$(pub_master)" = "$PUB6"

# ---- an export the scan refuses: nothing is pushed
F=$(commit "Merge leak: a file with a listed word" leak.txt "the zebrafish-internal plan")
rel prepare
expect "a scan refusal fails the prepare" test "$rc" -ne 0
expect "and says the scan refused" has "$ST" "the export scan refused"
expect "with the summary" test "$(json "$ST" '"refusals=1" in d["message"]')" = true
expect "and pushes nothing" test "$(pub_master)" = "$PUB6"
G=$(git -C "$O" rm -q leak.txt && git -C "$O" commit -q -m "Merge unleak: the file goes again" && git -C "$O" rev-parse HEAD)
DC_RELEASE_EXPORT="$S/export_dies" rel prepare
expect "an export tool that dies fails the prepare" has "$ST" "the export tool stopped"

# ---- prepare: export, push, ci green -> ready
runs success
jobs_ok 11
rel prepare
expect "prepare succeeds ($(tail -n 3 "$T/out"))" test "$rc" -eq 0
expect "phase ready" phase_is ready
expect "ok true" test "$(json "$ST" 'd["ok"]')" = true
PS7=$(pub_master)
expect "the public master moved" test "$PS7" != "$PUB6"
expect "to a child of beta6's commit" test "$(git --git-dir="$P" rev-parse "$PS7~1")" = "$PUB6"
expect "status names the public commit" test "$(json "$ST" 'd["public_sha"]')" = "$PS7"
expect "and the private one" test "$(json "$ST" 'd["private_sha"]')" = "$G"
expect "and the tag to come" test "$(json "$ST" 'd["next_tag"]')" = "v0.6.0-beta7"
expect "and the ci run" test "$(json "$ST" '[d["ci"]["conclusion"], d["ci"]["url"]]')" = '["success", "https://github.com/argallo/DeCombat/actions/runs/11"]'
MSG=$(git --git-dir="$P" log -1 --format=%s "$PS7")
expect "the commit message is Beta 7 with the subjects ($MSG)" test "${MSG:0:64}" = "Beta 7: the client draws the scope (CLI-1700); the server counts"
expect "without the Merge prefix" test -z "$(printf '%s' "$MSG" | grep 'Merge ')"
expect "without a subject the scan would refuse" test -z "$(printf '%s' "$MSG" | grep zebrafish)"
expect "the message is cut" test "${#MSG}" -le 208
expect "the export commit is DeCombat's" test "$(git --git-dir="$P" log -1 --format='%an <%ae>' "$PS7")" = "DeCombat <contact@decombat.com>"
expect "the API saw the token in its header" has "$STUB_LOG" "auth=ok"
expect "never on curl's command line" lacks "$STUB_LOG" "argv_token=True"
expect "the history has the outcome" has "$R/history.jsonl" "ready to tag v0.6.0-beta7"
rel prepare
expect "a second prepare of the same commit is refused" has "$ST" "already prepared as v0.6.0-beta7"
expect "and the phase stays ready" phase_is ready
expect "and nothing is pushed" test "$(pub_master)" = "$PS7"
DC_RELEASE_RATE_PREPARE=600 rel prepare
expect "a prepare refused for the same commit stays ready" phase_is ready
rel check
expect "a check keeps the ready phase" phase_is ready
rel daily
expect "the daily job does not prepare over a ready release" has "$T/out" "ready to tag: no prepare"

# ---- tag: only while the public master is the prepared commit
git -C "$T/pubwork" fetch -q origin && git -C "$T/pubwork" reset -q --hard origin/master
printf 'by hand\n' >> "$T/pubwork/README"
git -C "$T/pubwork" commit -qam "a push by hand" && git -C "$T/pubwork" push -q origin HEAD:refs/heads/master
rel tag
expect "a moved public master stops the tag" has "$ST" "moved to"
expect "phase failed" phase_is failed
expect "and no tag is pushed" test -z "$(git --git-dir="$P" tag)"
rel tag
expect "a tag after that is refused" has "$ST" "the phase is failed, not ready"
git -C "$T/pubwork" push -q -f origin "$PS7:refs/heads/master"
rel prepare
expect "prepare again with the commit still on the public master looks at ci again" phase_is ready
expect "without a new export" test "$(pub_master)" = "$PS7"

# ---- tag -> release run -> published, with the notes
runs success v0.6.0-beta7 success
jobs_ok 21
route GET "releases/tags/v0.6.0-beta7" '{"id":99,"html_url":"https://github.com/argallo/DeCombat/releases/tag/v0.6.0-beta7"}'
route PATCH "releases/99" '{"id":99}'
rel tag
expect "tag succeeds ($(tail -n 3 "$T/out"))" test "$rc" -eq 0
expect "phase published" phase_is published
expect "with the release page" test "$(json "$ST" 'd["release_url"]')" = "https://github.com/argallo/DeCombat/releases/tag/v0.6.0-beta7"
expect "the tag is on the public commit" test "$(git --git-dir="$P" rev-parse "v0.6.0-beta7^{commit}")" = "$PS7"
expect "an annotated tag" test "$(git --git-dir="$P" cat-file -t v0.6.0-beta7)" = tag
expect "the notes were set" has "$STUB_LOG" "curl PATCH releases/99"
grep '^DATA ' "$STUB_LOG" | sed 's/^DATA //' > "$T/patch.json"
expect "as a pre-release" test "$(json "$T/patch.json" 'd["prerelease"]')" = true
json "$T/patch.json" 'd["body"]' > "$T/notes.md"
expect "notes header" test "$(head -n 1 "$T/notes.md")" = "# DeCombat v0.6.0-beta7"
expect "notes update line" has "$T/notes.md" "A beta6 or beta5 client offers this update itself at login (Update now); older clients need a fresh download of the Windows zip from this page."
expect "notes group the client" has "$T/notes.md" "## Client"
expect "with its bullet" has "$T/notes.md" "- the client draws the scope (CLI-1700)"
expect "the match servers" has "$T/notes.md" "- the server counts the score (DSV-700)"
expect "lobby and servers" has "$T/notes.md" "## Lobby and servers"
expect "no Merge prefix" lacks "$T/notes.md" "Merge "
expect "no subject the scan would refuse" lacks "$T/notes.md" "zebrafish"
expect "but a count of them" has "$T/notes.md" "further internal change"
expect "no @mention" lacks "$T/notes.md" "@someone"
expect "the SmartScreen note" has "$T/notes.md" "Windows still warns about the unsigned exe (SmartScreen \"More info > Run anyway\"; a Defender flag is a false positive). Check the zip's hash against SHA256SUMS on this page."
expect "state moves to beta7" test "$(json "$SS" '[d["last_tag"], d["last_released_private_sha"], d["last_public_sha"], d["prepared"]]')" = "[\"v0.6.0-beta7\", \"$G\", \"$PS7\", null]"
expect "the history has it" has "$R/history.jsonl" "published v0.6.0-beta7"
rel check
expect "afterwards nothing is new" test "$(json "$ST" '[d["phase"], d["message"]]')" = '["idle", "nothing new since v0.6.0-beta7"]'
rel tag
expect "a second tag is refused" has "$ST" "nothing is ready to tag"
rel notes "$A" "$G"
expect "notes prints the body" has "$T/out" "## Match servers"
expect "with the next tag by default" test "$(head -n 1 "$T/out")" = "# DeCombat v0.7.0-beta8"

# ---- ci failure -> failed with the job names; ci fixed -> prepare again looks at ci again
H=$(commit "Merge ren-x: the sky is blue (REN-5)")
runs failure
jobs_bad 11 "test (macos-aarch64)"
rel prepare
expect "a red ci fails the prepare" test "$rc" -ne 0
expect "naming the failed job" has "$ST" "test (macos-aarch64) https://github.com/argallo/DeCombat/actions/runs/11/job/77"
expect "phase failed" test "$(json "$ST" '[d["phase"], d["ok"]]')" = '["failed", false]'
PS8=$(pub_master)
rel tag
expect "tag after a red ci is refused" has "$ST" "nothing is ready to tag"
runs success
jobs_ok 11
rel prepare
expect "after a re-run on GitHub, prepare again makes it ready" phase_is ready
expect "with the same public commit" test "$(pub_master)" = "$PS8"
route GET "$RUNS" '{"workflow_runs":[]}'
DC_RELEASE_RATE_TAG=600 rel tag
expect "a tag right after the last is refused" has "$ST" "wait 600 s"
expect "and stays ready" phase_is ready

# ---- a release run that fails; a tag again resumes on the existing tag
runs success v0.7.0-beta8 failure
jobs_bad 21 "publish the GitHub Release"
rel tag
expect "a failed release run fails" test "$rc" -ne 0
expect "naming its job" has "$ST" "publish the GitHub Release"
expect "and how to retry" has "$ST" "delete the tag v0.7.0-beta8"
expect "the tag was pushed" test "$(git --git-dir="$P" rev-parse "v0.7.0-beta8^{commit}")" = "$PS8"
runs success v0.7.0-beta8 success
jobs_ok 21
route GET "releases/tags/v0.7.0-beta8" '{"id":100,"html_url":"https://github.com/argallo/DeCombat/releases/tag/v0.7.0-beta8"}'
route PATCH "releases/100" '{"id":100}'
rel tag
expect "tag again after a failed release run publishes" phase_is published
expect "on the tag already there" has "$R/logs/$(ls -t "$R/logs" | head -n 1)" "already on"
expect "beta8 is the last tag" test "$(json "$SS" 'd["last_tag"]')" = "v0.7.0-beta8"
route GET "$RUNS" '{"workflow_runs":[]}'

# ---- requests through run
I=$(commit "Merge gmd-x: a mode (GMD-3)")
request 1700000000-check "{\"action\":\"check\",\"by\":\"admin web \$(touch $T/pwned)\"}"
rel run
expect "a check request runs a check" test "$(json "$ST" 'd["message"]')" = "1 commit since v0.7.0-beta8"
expect "the request is gone" test -z "$(ls -A "$R/requests")"
expect "its text never runs" test ! -e "$T/pwned"
request 1700000001-check '{"action":"check"}'
DC_RELEASE_RATE_CHECK=300 rel run
expect "a check within 30 s is ignored" has "$T/out" "check request ignored"
request 1700000002-prepare '{"action":"tag","by":"x"}'
rel run
expect "an action that is not the name's is refused" has "$ST" "request refused: action must be"
expect "and pushes nothing" test "$(pub_master)" = "$PS8"
request 1700000003-prepare '["prepare"]'
rel run
expect "no JSON object is refused" has "$ST" "not a JSON object"
request 1700000004-prepare "{\"action\":\"prepare\",\"by\":\"$(head -c 5000 /dev/zero | tr '\0' x)\"}"
rel run
expect "an oversized request is refused" has "$ST" "over 4096 bytes"
echo junk > "$R/requests/notes.txt"
echo '{"action":"deploy"}' > "$R/requests/1700000005-deploy.json"
echo '{"action":"prepare"}' > "$R/requests/.1700000006-prepare.json"
ln -s "$D/release.conf" "$R/requests/1700000007-prepare.json" 2>/dev/null || true
rel run
expect "stray files are no request" test "$(pub_master)" = "$PS8"
expect "and are removed" test -z "$(ls -A "$R/requests")"
expect "the request folder never sees the token" lacks "$T/out" "$TOKEN"
runs success
jobs_ok 11
request 1700000008-check '{"action":"check"}'
request 1700000009-prepare '{"action":"prepare","by":"the admin web"}'
rel run
expect "the newest request wins" has "$T/out" "superseded by 1700000009-prepare.json"
expect "a prepare request prepares" phase_is ready
expect "the requester is written back" test "$(json "$ST" 'd["by"]')" = "the admin web"
PS9=$(pub_master)
expect "and pushed" test "$PS9" != "$PS8"
DC_RELEASE_RATE_PREPARE=600 rel prepare
expect "a prepare is refused within 10 min" has "$ST" "refused: ${I:0:8} is already prepared"
J=$(commit "Merge more: one more (CLI-9)")
DC_RELEASE_RATE_PREPARE=600 rel prepare
expect "a prepare of a new commit within 10 min is refused" has "$ST" "wait 600 s between prepares"
expect "and pushes nothing" test "$(pub_master)" = "$PS9"
expect "and the release stays ready" phase_is ready

# ---- an interrupted job is written off; the daily job
"$PY" -c 'import json,sys; p=sys.argv[1]; d=json.load(open(p)); d.update(phase="ci_running", ok=None); json.dump(d, open(p,"w"))' "$ST"
request 1700000010-check '{"action":"check"}'
rel run
expect "an interrupted job is marked failed" has "$ST" "interrupted"
"$PY" -c 'import json,sys; p=sys.argv[1]; d=json.load(open(p)); d["auto_prepare"]=0; json.dump(d, open(p,"w"))' "$SS"
rel daily
expect "daily with auto_prepare 0 only checks" test "$(pub_master)" = "$PS9"
expect "and says so" has "$T/out" "auto_prepare is off"
"$PY" -c 'import json,sys; p=sys.argv[1]; d=json.load(open(p)); d["auto_prepare"]=1; json.dump(d, open(p,"w"))' "$SS"
rel daily
expect "daily with auto_prepare 1 prepares the new commit" phase_is ready
expect "from the timer" test "$(json "$ST" 'd["by"]')" = "the daily timer"
expect "a new public commit" test "$(pub_master)" != "$PS9"

# ---- the token: never in a log, the status, the history, the public clone, git's or curl's command lines
expect "the askpass helper holds no token" lacks "$R/work/askpass.sh" "$TOKEN"
expect "and answers the password from the environment" test "$(DC_RELEASE_TOKEN=$TOKEN sh "$R/work/askpass.sh" "Password for x: ")" = "$TOKEN"
expect "and the user name" test "$(sh "$R/work/askpass.sh" "Username for x: ")" = "x-access-token"
leaks=$(grep -rlF -- "$TOKEN" "$R" "$D/public" "$STUB_LOG" "$STUB_LOG.git" 2>/dev/null)
expect "no file but release.conf holds the token (${leaks})" test -z "$leaks"
expect "git never had it on its command line" lacks "$STUB_LOG.git" "$TOKEN"

out=$(bash "$REL" status 2>&1)
expect "status prints status.json" test -n "$(printf '%s' "$out" | grep '"phase"')"
for k in phase updated private_sha public_sha last_tag next_tag commits_since ci release_url message log_tail ok by; do
    expect "status.json has $k" test "$(json "$ST" "\"$k\" in d")" = true
done
expect "log_tail holds at most 20 lines" test "$(json "$ST" 'len(d["log_tail"]) <= 20')" = true

printf 'passed %d, failed %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
