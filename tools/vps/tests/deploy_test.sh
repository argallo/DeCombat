#!/usr/bin/env bash
# deploy_test.sh: tests of tools/vps/deploy_from_git.sh and deploy_setup.sh (docs/DEPLOY_FROM_ADMIN.md) on a temp root,
# with a real git origin and mirror, the real tools/package_servers.sh, and stand-ins for cargo, pgrep, systemctl and
# deploy_remote.sh. Touches no server. Runs under Git Bash on Windows and on Linux: bash tools/vps/tests/deploy_test.sh
# Provenance: ORIGINAL
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VPS=$(cd "$HERE/.." && pwd)
REPO=$(cd "$VPS/../.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/dc-deploy-test.XXXXXX")
trap 'rm -rf "${T:?}"' EXIT

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL: %s\n' "$*"; }
expect() { local d=$1; shift; if "$@"; then ok; else bad "$d"; fi; }
has() { grep -qF -- "$2" "$1" 2>/dev/null; }
lacks() { ! grep -qF -- "$2" "$1" 2>/dev/null; }
json() { "$PY" -c 'import json,sys; d=json.load(open(sys.argv[1],encoding="utf-8")); v=eval(sys.argv[2],{"d":d}); print(json.dumps(v) if not isinstance(v,str) else v)' "$1" "$2"; }

PY=""
for c in python3 python; do if "$c" -c "pass" >/dev/null 2>&1; then PY=$c; break; fi; done
[ -n "$PY" ] || { echo "no python: skipped"; exit 0; }
command -v git >/dev/null 2>&1 || { echo "no git: skipped"; exit 0; }

for f in "$VPS/deploy_from_git.sh" "$VPS/deploy_setup.sh" "$HERE/deploy_test.sh"; do
    expect "bash -n $(basename "$f")" bash -n "$f"
done

export DC_TEST=1 DC_ROOT="$T/opt" DC_SYSTEMD_DIR="$T/systemd" DC_PYTHON="$PY" DC_PACKAGE_STRIP=true
export DC_DEPLOY_SETTLE=0 DC_DEPLOY_MATCH_POLL=0 STUB_LOG="$T/stub.log"
# the rate limit has its own test below
export DC_DEPLOY_RATE_ANY=0 DC_DEPLOY_RATE_SAME=0
# a git without the machine's settings (Git for Windows turns line ends into CRLF)
: > "$T/gitconfig"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL="$T/gitconfig"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com
unset DC_ADMIN DC_INSTANCE DC_UNIT
D="$DC_ROOT/deploy"

# ---- the origin: a repository that holds what the packer reads
O="$T/origin"
mkdir -p "$O/tools" "$O/examples"
cp "$REPO/tools/package_servers.sh" "$O/tools/"
cp -r "$REPO/tools/vps" "$O/tools/vps"
rm -rf "$O/tools/vps/tests"
cp -r "$REPO/tools/host" "$O/tools/host"
for f in server-vps.toml decombat-lobby.service decombat-lobby@.service decombat-bots.service decombat-host.service \
    decombat.env decombat-hardening.conf decombat-backup.service decombat-backup.timer decombat-health.service \
    decombat-health.timer decombat-live-games.service decombat-live-games.timer backup.conf alerts.conf; do
    cp "$REPO/examples/$f" "$O/examples/"
done
commit() { echo "$1" > "$O/README"; git -C "$O" add -A >/dev/null; git -C "$O" commit -q -m "$1"; git -C "$O" rev-parse HEAD; }
git -C "$O" init -q -b master
A=$(commit "first: the running build")
B=$(commit "second: a fix")
C=$(commit "third: a \"quoted\" subject")

# ---- stand-ins
mkdir -p "$T/stubs"
cat > "$T/stubs/cargo" <<'S'
#!/usr/bin/env bash
echo "cargo $* DC_GIT_COMMIT=$DC_GIT_COMMIT" >> "$STUB_LOG"
[ -z "${STUB_CARGO_FAIL:-}" ] || { echo "error[E0001]: the stub build failed"; exit 101; }
mkdir -p target/release
for b in dc-lobby-server dc-dserver dc-server-rom dc-config dc-botclient dc-host; do
    printf '#!/bin/sh\necho %s %s\n' "$b" "$DC_GIT_COMMIT" > "target/release/$b"
    chmod +x "target/release/$b"
done
echo "    Finished release"
S
cat > "$T/stubs/pgrep" <<'S'
#!/usr/bin/env bash
[ -n "${STUB_MATCHES:-}" ]
S
cat > "$T/stubs/remote" <<'S'
#!/usr/bin/env bash
echo "REMOTE $*" >> "$STUB_LOG"
[ -z "${STUB_REMOTE_FAIL:-}" ] || { echo "NOT running: decombat-lobby"; exit 1; }
printf 'commit: %s\n' "$4" > "$DC_ROOT/bin/COMMIT.txt"
S
cat > "$T/stubs/systemctl" <<'S'
#!/usr/bin/env bash
echo "systemctl $*" >> "$STUB_LOG"
S
chmod +x "$T/stubs/"*
export DC_DEPLOY_PGREP="$T/stubs/pgrep" DC_DEPLOY_REMOTE="$T/stubs/remote" DC_DEPLOY_SYSTEMCTL="$T/stubs/systemctl"

# ---- deploy_setup.sh
mkdir -p "$DC_ROOT/bin" "$DC_ROOT/tools" "$DC_SYSTEMD_DIR"
printf 'commit: %s (built by hand)\n' "${A:0:8}" > "$DC_ROOT/bin/COMMIT.txt"
: > "$STUB_LOG"
out=$(bash "$VPS/deploy_setup.sh" --repo "git@github.com:example/repo.git" --required-instances "" 2>&1)
rc=$?
expect "setup succeeds ($out)" test "$rc" -eq 0
expect "setup makes the request folder" test -d "$D/requests"
expect "setup makes the deploy key" test -f "$D/key/id_ed25519"
expect "setup prints the public key" test -n "$(printf '%s' "$out" | grep -F "$(cut -d' ' -f2 "$D/key/id_ed25519.pub")")"
expect "setup never prints the private key" test -z "$(printf '%s' "$out" | grep -F 'PRIVATE KEY')"
expect "setup pins GitHub's host key" has "$D/key/known_hosts" "github.com ssh-ed25519 "
expect "setup writes deploy.conf" has "$D/deploy.conf" "REPO=git@github.com:example/repo.git"
expect "setup makes the bare mirror" test -f "$DC_ROOT/src.git/HEAD"
expect "the mirror's origin is the repository" test "$(git --git-dir="$DC_ROOT/src.git" remote get-url origin)" = "git@github.com:example/repo.git"
expect "setup installs the script the units run" test -f "$DC_ROOT/tools/deploy_from_git.sh"
expect "the lobby may write the request folder" has "$DC_SYSTEMD_DIR/decombat-lobby.service.d/20-deploy.conf" "ReadWritePaths=-$D/requests"
expect "the path unit watches the request folder" has "$DC_SYSTEMD_DIR/decombat-deploy.path" "DirectoryNotEmpty=$D/requests"
expect "the deploy service runs the installed script" has "$DC_SYSTEMD_DIR/decombat-deploy.service" "$DC_ROOT/tools/deploy_from_git.sh run"
expect "the fetch timer runs every 5 min" has "$DC_SYSTEMD_DIR/decombat-src-fetch.timer" "OnUnitActiveSec=5min"
expect "setup enables the timer and the path unit" has "$STUB_LOG" "enable --now decombat-src-fetch.timer decombat-deploy.path"
expect "setup makes the host offer signing key" test -f "$D/sign/host_update_ed25519"
SIGNER=$(printf '%s\n' "$out" | grep '^decombat-deploy ssh-ed25519 ' | head -n 1)
expect "setup prints the signer line for the hosts" test -n "$SIGNER"
expect "the signer line is the signing key's" test "$SIGNER" = "decombat-deploy $(cut -d' ' -f1,2 "$D/sign/host_update_ed25519.pub")"
sed -i 's/^MATCH_WAIT_MIN=.*/MATCH_WAIT_MIN=0/' "$D/deploy.conf"
key_before=$(cat "$D/key/id_ed25519.pub")
out=$(bash "$VPS/deploy_setup.sh" --repo "git@github.com:other/repo.git" 2>&1)
expect "a second setup keeps deploy.conf" has "$D/deploy.conf" "MATCH_WAIT_MIN=0"
expect "a second setup keeps the key" test "$(cat "$D/key/id_ed25519.pub")" = "$key_before"
out=$(bash "$VPS/deploy_setup.sh" --repo "https://github.com/example/repo.git" 2>&1)
expect "a second setup does not apply a new --repo" has "$D/deploy.conf" "REPO=git@github.com:example/repo.git"

# the tests fetch from the local origin (no key: plain git)
rm -f "$D/key/id_ed25519"
git --git-dir="$DC_ROOT/src.git" remote set-url origin "$O"
export PATH="$T/stubs:$PATH"
DEPLOY="$VPS/deploy_from_git.sh"
# without the deploy key a fetch refuses rather than use root's own ssh identity
out=$(bash "$DEPLOY" fetch 2>&1)
expect "no deploy key: the fetch refuses" test $? -ne 0
expect "and says why" test -n "$(printf '%s' "$out" | grep 'fetch failed: no deploy key')"
export DC_DEPLOY_TEST_NO_KEY=1

# ---- fetch: what is new
out=$(bash "$DEPLOY" fetch 2>&1)
expect "fetch succeeds ($out)" test $? -eq 0
AV="$D/available.json"
expect "available.json names master" test "$(json "$AV" 'd["master"]')" = "$C"
expect "it knows the running build" test "$(json "$AV" 'd["running_found"]')" = "true"
expect "it lists the two new commits, newest first" test "$(json "$AV" '[c["sha"] for c in d["new"]]')" = "[\"$C\", \"$B\"]"
expect "with subjects" test "$(json "$AV" 'd["new"][0]["subject"]')" = 'third: a "quoted" subject'
expect "and their count" test "$(json "$AV" 'd["new_count"]')" = 2
expect "the running build is the first of the older ones" test "$(json "$AV" 'd["older"][0]["sha"]')" = "$A"

printf 'commit: 0123456789abcdef (built by hand)\n' > "$DC_ROOT/bin/COMMIT.txt"
bash "$DEPLOY" fetch > /dev/null 2>&1
expect "an unknown running build lists master's latest" test "$(json "$AV" '[d["running_found"], len(d["new"]), d["new_count"]]')" = "[false, 3, null]"
printf 'commit: %s (built by hand)\n' "${A:0:8}" > "$DC_ROOT/bin/COMMIT.txt"

git --git-dir="$DC_ROOT/src.git" remote set-url origin "$T/nowhere"
bash "$DEPLOY" fetch > /dev/null 2>&1
expect "a failed fetch is a failure" test $? -ne 0
expect "and says so in available.json" test "$(json "$AV" 'd["ok"]')" = "false"
expect "but keeps the list" test "$(json "$AV" 'len(d["new"])')" = 2
git --git-dir="$DC_ROOT/src.git" remote set-url origin "$O"

# ---- run
request() { # request NAME JSON
    printf '%s' "$2" > "$D/requests/$1.json"
}
run() { : > "$STUB_LOG"; bash "$DEPLOY" run > "$T/out" 2>&1 < /dev/null; rc=$?; }
ST="$D/status.json"
phase_is() { test "$(json "$ST" 'd["phase"]')" = "$1"; }

request 1700000000-0000000a "{\"sha\":\"${C:0:8}\",\"targets\":[\"vps\",\"hosts\"],\"end_matches\":false,\"by\":\"admin web \$(touch $T/pwned)\"}"
run
expect "a deploy succeeds ($(cat "$T/out"))" test "$rc" -eq 0
expect "it is done" phase_is done
expect "and ok" test "$(json "$ST" 'd["ok"]')" = "true"
expect "status names the full commit" test "$(json "$ST" 'd["sha"]')" = "$C"
expect "and its subject" test "$(json "$ST" 'd["subject"]')" = 'third: a "quoted" subject'
expect "the request is gone" test -z "$(ls "$D/requests")"
expect "the build had the short commit" has "$STUB_LOG" "DC_GIT_COMMIT=${C:0:8}"
expect "the VPS set is built as deploy_all.sh builds it" has "$STUB_LOG" "-p dc-lobby-server -p dc-dserver -p dc-rez -p dc-botclient -p dc-config"
expect "and the host agent" has "$STUB_LOG" "-p dc-host --bin dc-host"
TGZ="$D/work/out/decombat-servers-linux-x86_64-${C:0:8}.tar.gz"
SUM=$(sha256sum "$TGZ" | cut -d' ' -f1)
expect "deploy_remote.sh installs the tarball with its hash and commit" has "$STUB_LOG" "REMOTE install $TGZ $SUM $C"
expect "the tarball has the six binaries" test "$(tar -tzf "$TGZ" | grep -c '/bin/dc-')" -eq 6
expect "its COMMIT.txt is the full commit" test "$(tar -xzOf "$TGZ" "decombat-servers-linux-x86_64-${C:0:8}/COMMIT.txt" | head -n 1)" = "commit: $C"
expect "the hosts are offered the build" has "$D/host/desired.txt" "commit=$C"
expect "with its tarball and hash" has "$D/host/desired.txt" "sha256=$SUM"
expect "the offered tarball is the installed one" test "$(sha256sum "$D/host/decombat-servers-linux-x86_64-${C:0:8}.tar.gz" | cut -d' ' -f1)" = "$SUM"
expect "the history has the deploy" has "$D/history.jsonl" "\"ok\": true"
expect "one history line" test "$(wc -l < "$D/history.jsonl")" -eq 1
expect "the requester's text is never run" test ! -e "$T/pwned"
expect "and is kept as text" test "$(json "$ST" 'd["by"]')" = "admin web \$(touch $T/pwned)"
expect "the log tail is in the status" test "$(json "$ST" 'len(d["log_tail"]) > 0')" = "true"
expect "available.json now starts from the new build" test "$(json "$AV" '[d["running_found"], d["new_count"]]')" = "[true, 0]"

# ---- refusals: nothing of the request runs
refused() { # refused DESCRIPTION JSON
    request 1700000001-0000000b "$2"
    run
    expect "$1: exit 0" test "$rc" -eq 0
    expect "$1: refused" has "$ST" "request refused"
    expect "$1: nothing installed" lacks "$STUB_LOG" "REMOTE"
    expect "$1: nothing built" lacks "$STUB_LOG" "cargo"
}
refused "a bad sha" '{"sha":"zzzz; rm -rf /","targets":["vps"]}'
refused "a target that does not exist" "{\"sha\":\"${C:0:8}\",\"targets\":[\"vps\",\"everything\"]}"
refused "end_matches that is no boolean" "{\"sha\":\"${C:0:8}\",\"end_matches\":\"yes\"}"
refused "no JSON object" '["x"]'
refused "an oversized request" "{\"sha\":\"${C:0:8}\",\"by\":\"$(head -c 5000 /dev/zero | tr '\0' x)\"}"

# a request that is not a regular file of the expected name is removed unread
ln -s "$D/deploy.conf" "$D/requests/1700000002-0000000c.json" 2>/dev/null || cp "$D/deploy.conf" "$D/requests/x"
echo junk > "$D/requests/notes.txt"
run
expect "a stray file is no deploy" lacks "$STUB_LOG" "cargo"
expect "and is removed" test -z "$(ls -A "$D/requests")"

# a commit that is not on master (master was rewound after the mirror saw it)
git -C "$O" checkout -q -b side "$B"
S=$(commit "side: never on master")
git -C "$O" checkout -q master
request 1700000003-0000000d "{\"sha\":\"$S\"}"
run
expect "a commit of another branch fails" test "$rc" -ne 0
expect "and says so" has "$ST" "is not a commit of the repository"
expect "and installs nothing" lacks "$STUB_LOG" "REMOTE"
git -C "$O" reset -q --hard "$B"
request 1700000004-0000000e "{\"sha\":\"$C\"}"
run
expect "a commit the mirror has but master lost fails" has "$ST" "is not on master"
expect "and builds nothing" lacks "$STUB_LOG" "cargo"
git -C "$O" reset -q --hard "$C"

# ---- matches
request 1700000005-0000000f "{\"sha\":\"$B\",\"targets\":[\"vps\"]}"
STUB_MATCHES=1 run
expect "running matches stop a deploy without end_matches" test "$rc" -ne 0
expect "and it says so" has "$ST" "matches were still running"
expect "after the build, before any install" lacks "$STUB_LOG" "REMOTE"
request 1700000006-00000010 "{\"sha\":\"$B\",\"targets\":[\"vps\"],\"end_matches\":true}"
STUB_MATCHES=1 run
expect "end_matches deploys during a match" phase_is done
expect "and installs" has "$STUB_LOG" "REMOTE install"
expect "vps alone does not move the hosts' build" has "$D/host/desired.txt" "commit=$C"

# ---- hosts alone, two requests, a failed build, a failed install
request 1700000007-00000011 "{\"sha\":\"$A\",\"targets\":[\"vps\"]}"
request 1700000008-00000012 "{\"sha\":\"$B\",\"targets\":[\"hosts\"]}"
run
expect "the newest request wins" test "$(json "$ST" 'd["id"]')" = "1700000008-00000012"
expect "the older one is superseded" has "$T/out" "superseded"
expect "hosts alone installs nothing on the VPS" lacks "$STUB_LOG" "REMOTE"
expect "hosts alone moves the hosts' build" has "$D/host/desired.txt" "commit=$B"
expect "two builds stay in host/" test "$(ls "$D"/host/*.tar.gz | wc -l)" -eq 2

request 1700000009-00000013 "{\"sha\":\"$C\"}"
STUB_CARGO_FAIL=1 run
expect "a failed build fails" test "$rc" -ne 0
expect "and is marked failed" test "$(json "$ST" '[d["phase"], d["ok"]]')" = '["failed", false]'
expect "with the compiler's line in the tail" test "$(json "$ST" 'any("stub build failed" in l for l in d["log_tail"])')" = "true"
expect "and installs nothing" lacks "$STUB_LOG" "REMOTE"

request 1700000010-00000014 "{\"sha\":\"$C\",\"targets\":[\"vps\",\"hosts\"]}"
STUB_REMOTE_FAIL=1 run
expect "a failed install fails" test "$(json "$ST" 'd["ok"]')" = "false"
expect "and does not offer the build to the hosts" has "$D/host/desired.txt" "commit=$B"

# an interrupted deploy is written off at the next start
"$PY" -c 'import json,sys; p=sys.argv[1]; d=json.load(open(p)); d.update(phase="building", ok=None, finished=None); json.dump(d, open(p,"w"), indent=1)' "$ST"
run
expect "an interrupted deploy is marked failed" has "$ST" "interrupted"

out=$(bash "$DEPLOY" status 2>&1)
expect "status prints status.json" test -n "$(printf '%s' "$out" | grep '"phase"')"

# ---- LOW-1..3: odd entries in requests/ are removed unread, so the path unit cannot loop on them
: > "$STUB_LOG"
echo x > "$D/requests/..x"
echo x > "$D/requests/.hidden.json"
if mkfifo "$D/requests/1700000020-00000020.json" 2>/dev/null; then
    timeout 20 bash "$DEPLOY" run > "$T/out" 2>&1 < /dev/null
    expect "a FIFO request does not hang the run" test $? -ne 124
    expect "and is removed" test ! -e "$D/requests/1700000020-00000020.json"
fi
NL="$D/requests/1700000021-00000021.json
x"
if printf '{"sha":"%s"}' "$C" > "$NL" 2>/dev/null; then
    run
    expect "a name with a newline is not a request" lacks "$STUB_LOG" "cargo"
fi
run
expect "dot files and ..x are removed" test -z "$(ls -A "$D/requests")"
expect "and build nothing" lacks "$STUB_LOG" "cargo"

# ---- HIGH-1: the host offer is signed; tools/host/update.sh (lobby mode) takes it only with a valid signature and a
# growing sequence number. A host root on the same box, the agent's incoming/ filled from deploy/host as dc-host does.
SEQ1=$(sed -n 's/^seq=//p' "$D/host/desired.txt")
expect "the offer carries a sequence number" test -n "$SEQ1"
expect "and a signature" test -n "$(sed -n 's/^sig=//p' "$D/host/desired.txt")"
verify_offer() { # the check update.sh makes, by hand: the signed message from the fields
    local f=$1
    printf 'decombat-host-update 1\ncommit=%s\ntarball=%s\nsha256=%s\nsize=%s\nseq=%s\n' \
        "$(sed -n 's/^commit=//p' "$f")" "$(sed -n 's/^tarball=//p' "$f")" "$(sed -n 's/^sha256=//p' "$f")" \
        "$(sed -n 's/^size=//p' "$f")" "$(sed -n 's/^seq=//p' "$f")" > "$T/vmsg"
    sed -n 's/^sig=//p' "$f" | base64 -d > "$T/vsig"
    printf '%s\n' "$SIGNER" > "$T/vsigners"
    ssh-keygen -Y verify -f "$T/vsigners" -I decombat-deploy -n decombat-host-update -s "$T/vsig" < "$T/vmsg" >/dev/null 2>&1
}
expect "the signature verifies with the printed signer line" verify_offer "$D/host/desired.txt"
cp "$D/host/desired.txt" "$T/offer_old.txt"
OLD_TGZ=$(sed -n 's/^tarball=//p' "$D/host/desired.txt")
cp "$D/host/$OLD_TGZ" "$T/$OLD_TGZ"
request 1700000022-00000022 "{\"sha\":\"$C\",\"targets\":[\"hosts\"]}"
run
SEQ2=$(sed -n 's/^seq=//p' "$D/host/desired.txt")
expect "the next offer's sequence number is higher" test "$SEQ2" -gt "$SEQ1"
expect "and verifies too" verify_offer "$D/host/desired.txt"
sed 's/^size=.*/size=1/' "$D/host/desired.txt" > "$T/tampered.txt"
not() { ! "$@"; }
expect "a field changed after signing does not verify" not verify_offer "$T/tampered.txt"

H="$T/hostroot"
UPD="$REPO/tools/host/update.sh"
host_root() {
    rm -rf "$H"; mkdir -p "$H/bin" "$H/config" "$H/incoming" "$H/logs"
    for b in dc-host dc-dserver dc-server-rom; do printf '#!/bin/sh\nexit 0\n' > "$H/bin/$b"; chmod +x "$H/bin/$b"; done
    echo "commit: 1111111 (built by hand)" > "$H/bin/COMMIT.txt"
    touch -d '1 hour ago' "$H/bin/COMMIT.txt"
    printf 'name = "h"\n' > "$H/config/host.toml"
    printf 'SOURCE=lobby\nINCOMING=%s/incoming\n' "$H" > "$H/config/update.conf"
    printf '%s\n' "$SIGNER" > "$H/config/update_signers"
}
give() { # give POINTER TARBALL-DIR: what dc-host leaves in incoming/
    local tb
    tb=$(sed -n 's/^tarball=//p' "$1")
    cp "$2/$tb" "$H/incoming/$tb"
    cp "$1" "$H/incoming/desired.txt"
}
host_update() {
    DC_ROOT="$H" DC_UPDATE_SYSTEMCTL="$T/stubs/systemctl" DC_UPDATE_PGREP="$T/stubs/pgrep" DC_UPDATE_SETTLE=0 \
        DC_UPDATE_ELF_CHECK=0 bash "$UPD" > "$T/hout" 2>&1 < /dev/null
}
host_root; give "$D/host/desired.txt" "$D/host"
host_update
expect "the host installs a signed offer (end to end)" test "$(sed -n 's/^state=//p' "$H/update/status")" = updated
expect "and keeps its sequence number" test "$(cat "$H/update/lobby_seq")" = "$SEQ2"
give "$T/offer_old.txt" "$T"
touch "$H/incoming/desired.txt"
host_update
expect "a genuine older offer replayed later is refused (downgrade)" test "$(sed -n 's/^state=//p' "$H/update/status")" = failed
expect "and named as such" has "$H/update/status" "not above the last taken"
host_root; give "$T/tampered.txt" "$D/host"
host_update
expect "a tampered offer is refused" has "$H/update/status" "signature does not verify"
host_root; give "$D/host/desired.txt" "$D/host"; grep -v '^sig=' "$H/incoming/desired.txt" > "$T/nosig" && cp "$T/nosig" "$H/incoming/desired.txt"
host_update
expect "an offer without a signature is refused" has "$H/update/status" "no valid size, sequence number or signature"
host_root; give "$D/host/desired.txt" "$D/host"
ssh-keygen -q -t ed25519 -N "" -C other -f "$T/otherkey"
printf 'decombat-deploy %s\n' "$(cut -d' ' -f1,2 "$T/otherkey.pub")" > "$H/config/update_signers"
host_update
expect "an offer checked against another pinned key is refused" has "$H/update/status" "signature does not verify"

# a deploy without the signing key cannot offer anything to the hosts
mv "$D/sign/host_update_ed25519" "$T/signkey.bak"
request 1700000023-00000023 "{\"sha\":\"$B\",\"targets\":[\"hosts\"]}"
run
expect "no signing key: the hosts are offered nothing" has "$ST" "no signing key"
mv "$T/signkey.bak" "$D/sign/host_update_ed25519"

# ---- LOW-5: root's own rate limit (a lobby can drop request files at will) and the log pruning
request 1700000024-00000024 "{\"sha\":\"$B\",\"targets\":[\"hosts\"]}"
run
expect "a deploy before the rate limit test" phase_is done
request 1700000025-00000025 "{\"sha\":\"$C\",\"targets\":[\"hosts\"]}"
DC_DEPLOY_RATE_ANY=60 run
expect "a deploy within 60 s of the last is refused" has "$ST" "wait 60 s between deploys"
expect "and builds nothing" lacks "$STUB_LOG" "cargo"
request 1700000026-00000026 "{\"sha\":\"$B\",\"targets\":[\"hosts\"]}"
DC_DEPLOY_RATE_SAME=300 run
expect "the same commit within 5 min is refused" has "$ST" "the same commit waits 300 s"
request 1700000027-00000027 "{\"sha\":\"$A\",\"targets\":[\"hosts\"]}"
DC_DEPLOY_RATE_SAME=300 run
expect "another commit is not held by the same-commit limit" phase_is done
expect "refusals by the limit stay out of the history" test -z "$(grep 'wait 60 s\|waits 300 s' "$D/history.jsonl")"
for i in $(seq 1 60); do : > "$D/logs/old-$i.log"; touch -d "$((i + 100)) minutes ago" "$D/logs/old-$i.log"; done
request 1700000028-00000028 "{\"sha\":\"$B\",\"targets\":[\"hosts\"]}"
run
expect "the logs are pruned to the newest 50" test "$(ls "$D"/logs/*.log | wc -l)" -le 51

# ---- LG-402: the dashboard's "Check now" (a fetch request: one fetch, never a deploy, rate limited)
D0=$(commit "fifth: new since the dashboard looked")
: > "$STUB_LOG"
rm -f "$D/status.json"
printf '{"by":"the admin web $(touch %s/pwned2)","at":1}' "$T" > "$D/requests/fetch-1700000100.json"
DC_DEPLOY_RATE_FETCH=0 bash "$DEPLOY" run > "$T/out" 2>&1 < /dev/null
expect "a fetch request is answered with a fetch" test "$(json "$AV" 'd["master"]')" = "$D0"
expect "the request is removed" test -z "$(ls "$D/requests")"
expect "it never builds or installs" lacks "$STUB_LOG" "cargo"
expect "and starts no deploy" test ! -f "$D/status.json"
expect "its content never reaches a shell" test ! -e "$T/pwned2"
D1=$(commit "sixth: too soon")
printf '{}' > "$D/requests/fetch-1700000101.json"
DC_DEPLOY_RATE_FETCH=300 bash "$DEPLOY" run > "$T/out" 2>&1 < /dev/null
expect "a second check within the limit is ignored" test "$(json "$AV" 'd["master"]')" = "$D0"
expect "and says so" has "$T/out" "check request ignored"
expect "and is removed too" test -z "$(ls "$D/requests")"
printf '{}' > "$D/requests/fetch-abc.json"
printf '{}' > "$D/requests/fetch-1700000102.json.bak"
DC_DEPLOY_RATE_FETCH=0 bash "$DEPLOY" run > "$T/out" 2>&1 < /dev/null
expect "a badly named fetch file is no check" test "$(json "$AV" 'd["master"]')" = "$D0"
expect "and is cleared" test -z "$(ls "$D/requests")"
request 1700000103-00000103 "{\"sha\":\"$D1\",\"targets\":[\"hosts\"]}"
printf '{}' > "$D/requests/fetch-1700000103.json"
run
expect "a deploy request beside a check request deploys" phase_is done
expect "and leaves no request behind" test -z "$(ls "$D/requests")"

# ---- LG-434: the dashboard's "Sync packs" (a packs request: packs_sync.sh per source into the named mods folder,
# never a deploy; the request's shape, the allowlist and the folder are checked; a refusal installs nothing)
expect "setup writes the allowlist of pack repositories" has "$D/packs.allow" "argallo/DeCombat-mods"
REL="$T/rel"
mkdir -p "$REL/example/packs/v1" "$DC_ROOT/data/mods"
"$PY" - "$REL/example/packs/v1" <<'PYEOF'
import hashlib, json, os, sys, zipfile
d = sys.argv[1]
rows, sums = [], []
for mid, ver in (("camo_a", "1.0.0"), ("map_b", "0.2.0")):
    name = f"{mid}-{ver}.zip"
    with zipfile.ZipFile(os.path.join(d, name), "w", compression=zipfile.ZIP_STORED) as z:
        z.writestr("manifest.json", json.dumps({"id": mid, "name": mid, "version": ver, "kind": "shared"}))
        z.writestr("main.lua", "return function(mod) end")
    data = open(os.path.join(d, name), "rb").read()
    sha = hashlib.sha256(data).hexdigest()
    rows.append({"id": mid, "version": ver, "sha256": sha, "size": len(data), "file": name})
    sums.append(f"{sha}  {name}")
json.dump({"version": 1, "packs": rows}, open(os.path.join(d, "packs.json"), "w"))
open(os.path.join(d, "SHA256SUMS"), "w").write("\n".join(sums) + "\n")
open(os.path.join(d, "hash_a"), "w").write(rows[0]["sha256"])
PYEOF
HA=$(cat "$REL/example/packs/v1/hash_a")
# the folder as the lobby would write it: a native path (Git Bash hands converted paths to python only as arguments)
MODS_NATIVE=$(cygpath -m "$DC_ROOT/data/mods" 2>/dev/null || printf '%s' "$DC_ROOT/data/mods")
ELSE_NATIVE=$(cygpath -m "$T/elsewhere/mods" 2>/dev/null || printf '%s' "$T/elsewhere/mods")
OTHER_NATIVE=$(cygpath -m "$DC_ROOT/data/other" 2>/dev/null || printf '%s' "$DC_ROOT/data/other")
preq() { # preq NAME JSON: a packs request
    printf '%s' "$2" > "$D/requests/packs-$1.json"
}
prun() { DC_PACKS_BASE_URL="$REL" DC_DEPLOY_RATE_PACKS=${PRATE:-0} bash "$DEPLOY" run > "$T/out" 2>&1 < /dev/null; }
pphase() { json "$D/packs.json" 'd["phase"]'; }
SRC1='"sources":[{"repo":"example/packs","tag":"v1"}]'
: > "$STUB_LOG"
rm -f "$D/status.json"
preq 1700000200 "{\"by\":\"the admin web \$(touch $T/pwned3)\",\"at\":1,\"mods_dir\":\"$MODS_NATIVE\",$SRC1}"
prun
expect "a repo that is not in packs.allow is refused" test "$(pphase)" = failed
expect "and names the allowlist" has "$D/packs.json" "is not in"
expect "and installs nothing" test ! -f "$DC_ROOT/data/mods/$HA.zip"
echo "example/packs" >> "$D/packs.allow"
preq 1700000201 "{\"by\":\"the admin web \$(touch $T/pwned3)\",\"at\":1,\"mods_dir\":\"$MODS_NATIVE\",$SRC1}"
prun
expect "an allowed repo's release installs its zips under their hash" test -f "$DC_ROOT/data/mods/$HA.zip"
expect "the request is removed" test -z "$(ls "$D/requests")"
expect "it never builds or installs a build" lacks "$STUB_LOG" "cargo"
expect "and starts no deploy" test ! -f "$D/status.json"
expect "its content never reaches a shell" test ! -e "$T/pwned3"
expect "packs.json says done" test "$(pphase)" = done
expect "packs.json names the source" test "$(json "$D/packs.json" 'd["sources"][0]')" = "example/packs v1"
expect "packs.json holds rows to paste" test "$(json "$D/packs.json" '"camo_a" in d["rows"] and "sha256" in d["rows"]')" = "true"
expect "and the log tail" test "$(json "$D/packs.json" 'len(d["log_tail"]) > 0')" = "true"
rm -f "$DC_ROOT/data/mods/$HA.zip"
# refusals: each installs nothing and says why
refused() { # refused NAME JSON WHY
    preq "$1" "$2"; prun
    expect "$3: refused" test "$(pphase)" = failed
    expect "$3: nothing installed" test ! -f "$DC_ROOT/data/mods/$HA.zip"
}
refused 1700000202 "{\"mods_dir\":\"$MODS_NATIVE\",\"sources\":[{\"repo\":\"example/packs\",\"tag\":\"main\"}]}" "a branch tag"
expect "the reason names the tag rule" has "$D/packs.json" "not a release tag"
refused 1700000203 "{\"mods_dir\":\"$MODS_NATIVE\",\"sources\":[{\"repo\":\"example/packs\",\"tag\":\"-v1\"}]}" "a tag with a leading dash"
refused 1700000204 "{\"mods_dir\":\"$MODS_NATIVE\",\"sources\":[{\"repo\":\"example/packs\",\"tag\":\"v1;id\"}]}" "a tag with a semicolon"
refused 1700000205 "{\"mods_dir\":\"$MODS_NATIVE\",\"sources\":[{\"repo\":\"example/packs\",\"tag\":\"v1\u00e9\"}]}" "a tag with a non-ascii letter"
refused 1700000206 "{\"mods_dir\":\"$MODS_NATIVE\",\"sources\":[{\"repo\":\"../../etc\",\"tag\":\"v1\"}]}" "a repo that is not owner/repo"
expect "the reason names the repo rule" has "$D/packs.json" "not a GitHub owner/repo"
NINE=$(for i in 1 2 3 4 5 6 7 8 9; do printf '{"repo":"example/packs","tag":"v%s"},' "$i"; done)
refused 1700000207 "{\"mods_dir\":\"$MODS_NATIVE\",\"sources\":[${NINE%,}]}" "nine sources"
expect "the reason names the source limit" has "$D/packs.json" "1 to 8"
mkdir -p "$T/elsewhere/mods"
refused 1700000208 "{\"mods_dir\":\"$ELSE_NATIVE\",$SRC1}" "a mods folder outside the root"
expect "the reason names the folder rule" has "$D/packs.json" "must be an existing mods folder"
expect "and the outside folder gets nothing" test -z "$(ls "$T/elsewhere/mods")"
mkdir -p "$DC_ROOT/data/other"
refused 1700000209 "{\"mods_dir\":\"$OTHER_NATIVE\",$SRC1}" "a folder under the root that is not a mods folder"
expect "and it gets nothing" test -z "$(ls "$DC_ROOT/data/other")"
"$PY" -c 'import sys; sys.stdout.write("{\"pad\":\"" + "x" * 9000 + "\"}")' > "$D/requests/packs-1700000210.json"
prun
expect "an oversized request is refused" test "$(pphase)" = failed
expect "and says so" has "$D/packs.json" "over 8192 bytes"
if ln -s "$DC_ROOT/data" "$DC_ROOT/dlink" 2>/dev/null && [ -L "$DC_ROOT/dlink" ]; then
    LINK_NATIVE=$(cygpath -m "$DC_ROOT/dlink/mods" 2>/dev/null || printf '%s' "$DC_ROOT/dlink/mods")
    refused 1700000211 "{\"mods_dir\":\"$LINK_NATIVE\",$SRC1}" "a mods path through a symbolic link"
    expect "the reason names the link rule" has "$D/packs.json" "no symbolic link"
    preq 1700000212 "{\"mods_dir\":\"$MODS_NATIVE\",$SRC1}"
    ln -s "$D/requests/packs-1700000212.json" "$D/requests/packs-1700000213.json"
    prun
    expect "a symlinked request file is removed, the real one answered" test -z "$(ls "$D/requests")"
    expect "and the real one installed" test -f "$DC_ROOT/data/mods/$HA.zip"
    rm -f "$DC_ROOT/data/mods/$HA.zip" "$DC_ROOT/dlink"
else
    echo "note: no symbolic links here, the symlink cases are skipped"
fi
# a sync that died half way is written off at the next run, and the next request proceeds
"$PY" -c 'import json,sys,time; json.dump({"phase":"running","ok":None,"finished":None,"updated":int(time.time()),"started":1,"message":"syncing","sources":[],"mods_dir":"","rows":"","log_tail":[]}, open(sys.argv[1],"w"))' "$D/packs.json"
preq 1700000214 "{\"mods_dir\":\"$MODS_NATIVE\",$SRC1}"
prun
expect "a dead sync is written off and the next request answered" test -f "$DC_ROOT/data/mods/$HA.zip"
expect "and packs.json says done again" test "$(pphase)" = done
# the rate limit counts from the last success only
preq 1700000215 "{\"mods_dir\":\"$MODS_NATIVE\",$SRC1}"
PRATE=300 prun
expect "a sync within the limit of the last finished one is ignored" has "$T/out" "pack sync request ignored"
expect "and the outcome keeps its phase with a note" test "$(pphase)" = done
expect "the note says why" has "$D/packs.json" "sync request ignored"
expect "and its request is removed" test -z "$(ls "$D/requests")"
rm -f "$DC_ROOT/data/mods/$HA.zip"
refused 1700000216 "{\"mods_dir\":\"$MODS_NATIVE\",\"sources\":[{\"repo\":\"example/packs\",\"tag\":\"main\"}]}" "a refusal after a success"
preq 1700000217 "{\"mods_dir\":\"$MODS_NATIVE\",$SRC1}"
PRATE=300 prun
expect "a refusal does not hold the next press back" test "$(pphase)" = done
printf '{}' > "$D/requests/packs-abc.json"
prun
expect "a badly named packs file is cleared" test -z "$(ls "$D/requests")"
D2=$(commit "seventh: beside a pack sync")
request 1700000218-00000218 "{\"sha\":\"$D2\",\"targets\":[\"hosts\"]}"
preq 1700000218 "{\"mods_dir\":\"$MODS_NATIVE\",$SRC1}"
prun
expect "a deploy request beside a pack sync request deploys" phase_is done
expect "and leaves the sync request for the next run" test -f "$D/requests/packs-1700000218.json"
rm -f "$DC_ROOT/data/mods/$HA.zip"
prun
expect "which answers it" test -f "$DC_ROOT/data/mods/$HA.zip"

printf 'passed %d, failed %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
