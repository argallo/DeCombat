#!/usr/bin/env bash
# update_test.sh: tests of tools/host/update.sh and tools/vps/deploy_remote.sh on a temp root, with fake tarballs made by
# tools/package_servers.sh and stand-ins for curl, systemctl and pgrep; it touches no server. Run: bash tools/host/update_test.sh
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
SCRIPT="$HERE/update.sh"
T=$(mktemp -d "${TMPDIR:-/tmp}/dc-update-test.XXXXXX")
trap 'rm -rf "${T:?}"' EXIT
fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fails=$((fails + 1)); }
expect() { name=$1; shift; if "$@"; then ok "$name"; else bad "$name"; fi; }
has() { grep -qF -- "$2" "$1"; }
lacks() { ! grep -qF -- "$2" "$1"; }
state_is() { grep -qx "state=$1" "$ROOT/update/status"; }
installed_is() { head -n 1 "$ROOT/bin/COMMIT.txt" | grep -q "^commit: $1"; }

OLD=1111111111111111111111111111111111111111
NEW=2222222222222222222222222222222222222222
BAD=3333333333333333333333333333333333333333
ROOT="$T/root"
SITE="$T/site"     # what the fake curl serves, by file name
mkdir -p "$SITE" "$T/stubs" "$T/bin_new" "$T/bin_nohost"

# ---- fake binaries: each says which build it is; dc-host --version fails when told to
fake_bin() { # fake_bin DIR LABEL
  for b in dc-lobby-server dc-dserver dc-server-rom dc-botclient dc-config dc-host; do
    printf '#!/usr/bin/env bash\n# build %s\n[ -z "${STUB_BIN_FAIL:-}" ] || exit 1\nexit 0\n' "$2" > "$1/$b"
    chmod +x "$1/$b"
  done
}
fake_bin "$T/bin_new" new
export DC_PACKAGE_STRIP=true
make_tgz() { # make_tgz BIN_DIR NAME COMMIT: the real packer
  (cd "$T" && bash "$REPO/tools/package_servers.sh" --bin-dir "$1" --name "$2" --out "$SITE" --commit "$3" >/dev/null)
}
pointer() { # pointer COMMIT TARBALL [SHA]
  printf 'commit=%s\ntarball=%s\nsha256=%s\nref=test\n' "$1" "$2" "${3:-$(sha256sum "$SITE/$2" | cut -d' ' -f1)}" > "$SITE/desired.txt"
}
TGZ=decombat-servers-linux-x86_64-22222222.tar.gz
make_tgz "$T/bin_new" decombat-servers-linux-x86_64-22222222 "$NEW"

# ---- stand-ins
cat > "$T/stubs/curl" <<'S'
#!/usr/bin/env bash
out=""; url=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out=$2; shift 2 ;;
    --proto|--proto-redir|--connect-timeout|--max-time|--retry) shift 2 ;;
    https://*) url=$1; shift ;;
    *) shift ;;
  esac
done
echo "curl $url" >> "$STUB_LOG"
case "$url" in https://releases.example/x/*) ;; *) exit 22 ;; esac
[ -z "${STUB_NET_DOWN:-}" ] || exit 7
f="$STUB_SITE/${url##*/}"
[ -f "$f" ] && cp "$f" "$out"
S
cat > "$T/stubs/systemctl" <<'S'
#!/usr/bin/env bash
echo "systemctl $*" >> "$STUB_LOG"
case "$1" in
  stop) exit "${STUB_STOP_FAIL:-0}" ;;
  start) exit 0 ;;
  is-active)
    # a new build that does not stay up: the second start (the rollback) comes after this check
    exit "${STUB_NOT_ACTIVE:-0}" ;;
esac
exit 0
S
cat > "$T/stubs/pgrep" <<'S'
#!/usr/bin/env bash
[ -n "${STUB_MATCHES:-}" ]
S
chmod +x "$T/stubs/"*
export STUB_LOG="$T/log" STUB_SITE="$SITE" DC_ROOT="$ROOT" DC_UPDATE_CURL="$T/stubs/curl" DC_UPDATE_SYSTEMCTL="$T/stubs/systemctl"
export DC_UPDATE_PGREP="$T/stubs/pgrep" DC_UPDATE_SETTLE=0 DC_UPDATE_ELF_CHECK=0

fresh() { # a host as tools/host/push.sh leaves it, on the old build
  rm -rf "${ROOT:?}"
  mkdir -p "$ROOT/bin" "$ROOT/config" "$ROOT/rom" "$ROOT/logs"
  fake_bin "$ROOT/bin" old
  echo "commit: ${OLD:0:7} (built by hand)" > "$ROOT/bin/COMMIT.txt"
  printf 'name = "h"\ntoken = "secret-token-of-at-least-24-chars"\nrom = "%s/rom"\n' "$ROOT" > "$ROOT/config/host.toml"
  echo 'POINTER_URL=https://releases.example/x/desired.txt' > "$ROOT/config/update.conf"
  echo rom-file > "$ROOT/rom/some.rez"
  echo '{}' > "$ROOT/rom/server-rom.json"
  CONFIG_SUM=$(cat "$ROOT/config/host.toml" "$ROOT/config/update.conf" "$ROOT/rom/some.rez" | sha256sum)
}
config_kept() { [ "$(cat "$ROOT/config/host.toml" "$ROOT/config/update.conf" "$ROOT/rom/some.rez" | sha256sum)" = "$CONFIG_SUM" ]; }
binary_is() { grep -q "# build $1" "$ROOT/bin/$2"; }
run() { : > "$T/log"; bash "$SCRIPT" "$@" > "$T/out" 2>&1 < /dev/null; rc=$?; }
zero() { [ "$rc" -eq 0 ]; }
nonzero() { [ "$rc" -ne 0 ]; }
stop_before_start() { [ "$(grep -n 'systemctl stop' "$T/log" | cut -d: -f1)" -lt "$(grep -n 'systemctl start' "$T/log" | head -n 1 | cut -d: -f1)" ]; }

# ---- the good path
fresh; pointer "$NEW" "$TGZ"
run
expect "an update succeeds" zero
expect "it says so" state_is updated
expect "the new commit is recorded" installed_is "$NEW"
expect "the new dc-host is in place" binary_is new dc-host
expect "the new dc-dserver is in place" binary_is new dc-dserver
expect "the agent stops before it starts" stop_before_start
expect "host.toml, update.conf and the ROM are untouched" config_kept
expect "a lobby binary is not installed on a host" binary_is old dc-lobby-server
expect "the old build is kept for a rollback" has "$ROOT/update/prev/dc-host" "# build old"
expect "the updater refreshes itself" test -f "$ROOT/tools/host/update.sh"
expect "the work folder is gone" test -z "$(ls -d "$ROOT"/update/work.* 2>/dev/null)"
expect "the output never shows the token" lacks "$T/out" "secret-token"
expect "the status never shows the token" lacks "$ROOT/update/status" "secret-token"

run
expect "a second check succeeds" zero
expect "and is up to date" state_is up-to-date
expect "and does not stop the agent" lacks "$T/log" "systemctl stop"
expect "and does not download the tarball" lacks "$T/log" "$TGZ"

fresh; echo "commit: ${NEW:0:7} (built by hand)" > "$ROOT/bin/COMMIT.txt"; pointer "$NEW" "$TGZ"
run
expect "a short id of the same commit is up to date" state_is up-to-date

# ---- matches
fresh; pointer "$NEW" "$TGZ"
STUB_MATCHES=1 run
expect "a running match is no failure" zero
expect "the host waits" state_is waiting
expect "the agent keeps running" lacks "$T/log" "systemctl stop"
expect "nothing is downloaded meanwhile" lacks "$T/log" "$TGZ"
expect "the old build stays" binary_is old dc-host
run
expect "after the match it updates" state_is updated
fresh; pointer "$NEW" "$TGZ"
STUB_MATCHES=1 run --force
expect "--force updates during a match" state_is updated

# ---- verification
fresh; pointer "$NEW" "$TGZ" 0000000000000000000000000000000000000000000000000000000000000000
run
expect "a wrong SHA-256 fails" nonzero
expect "and says so" has "$T/out" "the pointer says"
expect "and installs nothing" binary_is old dc-host
expect "and never stops the agent" lacks "$T/log" "systemctl stop"
expect "and is tried again later (not remembered as a bad build)" test ! -e "$ROOT/update/failed_commit"

fresh; pointer "$BAD" "$TGZ"
run
expect "a tarball of another commit fails" nonzero
expect "and installs nothing" binary_is old dc-host

fresh; printf 'commit=nope\ntarball=../../evil.tar.gz\nsha256=zz\n' > "$SITE/desired.txt"
run
expect "a malformed pointer fails" nonzero
expect "and fetches nothing else" lacks "$T/log" "evil"
expect "and installs nothing" binary_is old dc-host

fresh; pointer "$NEW" "$TGZ"
STUB_NET_DOWN=1 run
expect "no network fails softly" nonzero
expect "and leaves the agent alone" lacks "$T/log" "systemctl stop"

# the tarball's own binaries list
mkdir -p "$T/tamper"; tar -xzf "$SITE/$TGZ" -C "$T/tamper"
echo extra >> "$T/tamper/decombat-servers-linux-x86_64-22222222/bin/dc-host"
TAMPER=decombat-servers-linux-x86_64-tamper.tar.gz
mv "$T/tamper/decombat-servers-linux-x86_64-22222222" "$T/tamper/decombat-servers-linux-x86_64-tamper"
tar -czf "$SITE/$TAMPER" -C "$T/tamper" decombat-servers-linux-x86_64-tamper
fresh; pointer "$NEW" "$TAMPER"
run
expect "a binary that fails SHA256SUMS fails" nonzero
expect "and says so" has "$T/out" "SHA256SUMS"
expect "and installs nothing" binary_is old dc-host

# a tarball with entries outside its top folder
mkdir -p "$T/escape/decombat-servers-linux-x86_64-escape" "$T/escape/other"; echo x > "$T/escape/other/f"; echo y > "$T/escape/decombat-servers-linux-x86_64-escape/g"
ESC=decombat-servers-linux-x86_64-escape.tar.gz
tar -czf "$SITE/$ESC" -C "$T/escape" decombat-servers-linux-x86_64-escape other
fresh; pointer "$NEW" "$ESC"
run
expect "entries outside the top folder fail" nonzero
expect "and are named" has "$T/out" "outside"

# ---- a build that cannot run or does not stay up
fresh; pointer "$NEW" "$TGZ"
STUB_BIN_FAIL=1 run
expect "a dc-host that does not run here fails" nonzero
expect "and never stops the agent" lacks "$T/log" "systemctl stop"
expect "and is remembered" test -f "$ROOT/update/failed_commit"
run
expect "the remembered commit is not fetched again" lacks "$T/log" "$TGZ"
run --force
expect "--force tries it again" state_is updated
expect "and a success clears the memory" test ! -e "$ROOT/update/failed_commit"

fresh; pointer "$NEW" "$TGZ"
STUB_NOT_ACTIVE=1 run
expect "an agent that does not stay up fails" nonzero
expect "the old build is back" binary_is old dc-host
expect "the old dc-dserver too" binary_is old dc-dserver
expect "the old commit is recorded" installed_is "${OLD:0:7}"
expect "the agent is started again" test "$(grep -c 'systemctl start' "$T/log")" -eq 2
expect "the commit is remembered" has "$ROOT/update/failed_commit" "$NEW"
expect "config stays" config_kept

fresh; pointer "$NEW" "$TGZ"
STUB_STOP_FAIL=1 run
expect "an agent that will not stop fails" nonzero
expect "and nothing is swapped" binary_is old dc-host

# ---- hold, status, missing pieces
fresh; pointer "$NEW" "$TGZ"; mkdir -p "$ROOT/update"; touch "$ROOT/update/hold"
run
expect "a hold is no failure" zero
expect "a hold is reported" state_is held
expect "a hold keeps the by-hand build" binary_is old dc-host
expect "a hold does not read the pointer" lacks "$T/log" "desired.txt"
run --release-hold
run
expect "after --release-hold it updates" state_is updated
run --status
expect "--status prints the outcome" has "$T/out" "state=updated"

fresh; rm "$ROOT/config/update.conf"
run
expect "no update.conf fails" nonzero
fresh; echo 'POINTER_URL=http://releases.example/x/desired.txt' > "$ROOT/config/update.conf"
run
expect "a plain http pointer is refused" nonzero
expect "and not read" lacks "$T/log" "curl"
fresh; rm "$ROOT/bin/dc-host"
run
expect "a machine without dc-host fails" nonzero
expect "and points at the first install" has "$T/out" "push.sh"

# ---- --setup
fresh; rm "$ROOT/config/update.conf"; mkdir -p "$T/units"
DC_TEST=1 DC_SYSTEMD_DIR="$T/units" run --setup --pointer-url https://releases.example/x/desired.txt
expect "--setup succeeds" zero
expect "it writes the config" has "$ROOT/config/update.conf" "POINTER_URL=https://releases.example/x/desired.txt"
expect "it writes the timer" test -f "$T/units/decombat-host-update.timer"
expect "the service runs the installed script" has "$T/units/decombat-host-update.service" "$ROOT/tools/host/update.sh"
expect "it enables the timer" has "$T/log" "enable --now decombat-host-update.timer"
echo 'POINTER_URL=https://mine.example/y/desired.txt' > "$ROOT/config/update.conf"
DC_TEST=1 DC_SYSTEMD_DIR="$T/units" run --setup --pointer-url https://releases.example/x/desired.txt
expect "--setup keeps an existing config" has "$ROOT/config/update.conf" "mine.example"
DC_TEST=1 DC_SYSTEMD_DIR="$T/units" run --setup --pointer-url http://releases.example/x/desired.txt
expect "--setup refuses a plain http pointer" nonzero
expect "the tarball carries the updater" test -n "$(tar -tzf "$SITE/$TGZ" | grep tools/host/update.sh)"
expect "the tarball carries no host.toml" test -z "$(tar -tzf "$SITE/$TGZ" | grep -E 'host\.toml|\.db$')"

# ---- SOURCE=lobby: the build the lobby offered, as the agent left it in incoming/ (docs/DEPLOY_FROM_ADMIN.md, HA-2)
# the deploy key of tools/vps/deploy_setup.sh, and a second one an attacker holds
mkdir -p "$T/keys"
ssh-keygen -q -t ed25519 -N "" -C deploy -f "$T/keys/deploy"
ssh-keygen -q -t ed25519 -N "" -C other -f "$T/keys/other"
SIGNER="decombat-deploy $(cut -d' ' -f1,2 "$T/keys/deploy.pub")"
lobby_fresh() {
  fresh
  printf 'SOURCE=lobby\nINCOMING=%s/incoming\n' "$ROOT" > "$ROOT/config/update.conf"
  printf '%s\n' "$SIGNER" > "$ROOT/config/update_signers"
  mkdir -p "$ROOT/incoming"
}
offer() { # offer COMMIT TARBALL [SEQ] [KEY] [SHA]: what dc-host writes after a transfer, signed as deploy_from_git.sh signs
  local seq=${3:-1} key=${4:-$T/keys/deploy} sha=${5:-$(sha256sum "$SITE/$2" | cut -d' ' -f1)} size
  size=$(wc -c < "$SITE/$2" | tr -d ' ')
  cp "$SITE/$2" "$ROOT/incoming/$2"
  printf 'decombat-host-update 1\ncommit=%s\ntarball=%s\nsha256=%s\nsize=%s\nseq=%s\n' "$1" "$2" "$sha" "$size" "$seq" > "$T/offer.msg"
  rm -f "$T/offer.msg.sig"
  ssh-keygen -q -Y sign -f "$key" -n decombat-host-update "$T/offer.msg" 2>/dev/null
  printf 'commit=%s\ntarball=%s\nsha256=%s\nsize=%s\nseq=%s\nsig=%s\n' "$1" "$2" "$sha" "$size" "$seq" \
    "$(base64 < "$T/offer.msg.sig" | tr -d '\n\r')" > "$ROOT/incoming/desired.txt"
}
lobby_fresh
run
expect "lobby source without an offer is no failure" zero
expect "and says no build was offered" has "$ROOT/update/status" "offered no build"
expect "and reads nothing from the network" lacks "$T/log" "curl"
lobby_fresh; offer "$NEW" "$TGZ" 5
run
expect "lobby source: a signed offer is installed" state_is updated
expect "lobby source: the new commit is recorded" installed_is "$NEW"
expect "lobby source: no network" lacks "$T/log" "curl"
expect "lobby source: the offer is used up" test ! -e "$ROOT/incoming/desired.txt"
expect "lobby source: its sequence number is kept" has "$ROOT/update/lobby_seq" "5"
expect "lobby source: the tarball's updater is never installed" test ! -e "$ROOT/tools/host/update.sh"
run
expect "lobby source: then up to date" state_is up-to-date

# HIGH-1: a relaying lobby (or the agent) cannot forge, alter or replay an offer
signed_refused() { # DESCRIPTION
  run
  expect "$1: refused" nonzero
  expect "$1: nothing installed" binary_is old dc-host
  expect "$1: the agent keeps running" lacks "$T/log" "systemctl stop"
  expect "$1: the offer is dropped" test ! -e "$ROOT/incoming/desired.txt"
}
lobby_fresh; offer "$NEW" "$TGZ" 5 "$T/keys/other"
signed_refused "an offer signed by another key"
expect "and says why" has "$ROOT/update/status" "signature does not verify"
lobby_fresh; offer "$NEW" "$TGZ" 5; sed -i 's/^seq=5$/seq=6/' "$ROOT/incoming/desired.txt"
signed_refused "a sequence number changed after signing"
lobby_fresh; offer "$NEW" "$TGZ" 5; grep -v '^sig=' "$ROOT/incoming/desired.txt" > "$T/d" && mv "$T/d" "$ROOT/incoming/desired.txt"
signed_refused "an offer without a signature"
lobby_fresh; offer "$NEW" "$TGZ" 5; rm "$ROOT/config/update_signers"
run
expect "no pinned signer fails" nonzero
expect "and installs nothing (no signer)" binary_is old dc-host
# another tarball under a genuine signature for other bytes
lobby_fresh; offer "$NEW" "$TGZ" 5 "$T/keys/deploy" 0000000000000000000000000000000000000000000000000000000000000000
run
expect "bytes other than the signed SHA-256 fail" nonzero
expect "and install nothing (sha)" binary_is old dc-host
# a downgrade: a genuine older offer replayed after a newer one was taken
lobby_fresh; mkdir -p "$ROOT/update"; echo 7 > "$ROOT/update/lobby_seq"
offer "$NEW" "$TGZ" 7
signed_refused "a replayed offer (same sequence number)"
expect "and says so" has "$ROOT/update/status" "not above the last taken 7"
lobby_fresh; mkdir -p "$ROOT/update"; echo 7 > "$ROOT/update/lobby_seq"; offer "$NEW" "$TGZ" 3
signed_refused "an older offer (lower sequence number)"
lobby_fresh; mkdir -p "$ROOT/update"; echo 7 > "$ROOT/update/lobby_seq"; offer "$NEW" "$TGZ" 8
run
expect "a newer sequence number is taken" state_is updated
# an offer from before a by-hand install would take the host back: dropped
lobby_fresh; offer "$NEW" "$TGZ"; touch -d '2 minutes ago' "$ROOT/incoming/desired.txt"
run
expect "lobby source: a stale offer is no failure" zero
expect "lobby source: and is dropped" test ! -e "$ROOT/incoming/desired.txt"
expect "lobby source: and installs nothing" binary_is old dc-host
lobby_fresh; offer "$NEW" "$TGZ"; rm "$ROOT/incoming/$TGZ"; ln -s "$SITE/$TGZ" "$ROOT/incoming/$TGZ" 2>/dev/null
if [ -L "$ROOT/incoming/$TGZ" ]; then
  run
  expect "lobby source: a linked tarball is refused" nonzero
  expect "lobby source: and nothing installed" binary_is old dc-host
fi
# an entry that is not a file or a folder (a link made beside the binaries)
mkdir -p "$T/linky"; tar -xzf "$SITE/$TGZ" -C "$T/linky"
LNK=decombat-servers-linux-x86_64-linky.tar.gz
mv "$T/linky/decombat-servers-linux-x86_64-22222222" "$T/linky/decombat-servers-linux-x86_64-linky"
if ln -s /etc/passwd "$T/linky/decombat-servers-linux-x86_64-linky/bin/extra" 2>/dev/null && [ -L "$T/linky/decombat-servers-linux-x86_64-linky/bin/extra" ]; then
  tar -czf "$SITE/$LNK" -C "$T/linky" decombat-servers-linux-x86_64-linky
  lobby_fresh; offer "$NEW" "$LNK"
  run
  expect "a tarball with a link entry is refused" nonzero
  expect "and says so" has "$T/out" "neither a file nor a folder"
fi
# HIGH-2: a root run refuses a layout someone else can write (checked here as the test user)
lobby_fresh; offer "$NEW" "$TGZ" 9
chmod 0777 "$ROOT/bin"
if [ "$(stat -c %a "$ROOT/bin")" = 777 ]; then
  DC_UPDATE_CHECK_LAYOUT=1 DC_UPDATE_ROOT_UID=$(id -u) run
  expect "a group- or world-writable bin/ is refused" nonzero
  expect "and named" has "$T/out" "is not root's alone"
  expect "and nothing is installed (layout)" binary_is old dc-host
fi
chmod 0755 "$ROOT/bin"
lobby_fresh; offer "$NEW" "$TGZ" 9
ln -s "$ROOT/config" "$ROOT/update" 2>/dev/null
if [ -L "$ROOT/update" ]; then
  DC_UPDATE_CHECK_LAYOUT=1 DC_UPDATE_ROOT_UID=$(id -u) run
  expect "a linked update/ is refused" nonzero
fi
lobby_fresh; offer "$NEW" "$TGZ" 9
DC_UPDATE_CHECK_LAYOUT=1 DC_UPDATE_ROOT_UID=$(id -u) run
expect "a layout of the right owner passes the check" state_is updated

fresh; mkdir -p "$T/units"
DC_TEST=1 DC_SYSTEMD_DIR="$T/units" run --setup --from-lobby
expect "--setup --from-lobby needs the signer" nonzero
DC_TEST=1 DC_SYSTEMD_DIR="$T/units" run --setup --from-lobby --signer "decombat-deploy ssh-rsa AAAA"
expect "--setup --from-lobby refuses another kind of signer" nonzero
DC_TEST=1 DC_SYSTEMD_DIR="$T/units" run --setup --from-lobby --signer "$SIGNER"
expect "--setup --from-lobby succeeds" zero
expect "it switches the source" has "$ROOT/config/update.conf" "SOURCE=lobby"
expect "it records the agent's user" has "$ROOT/config/update.conf" "AGENT_USER=$(id -un)"
expect "it pins the signer" has "$ROOT/config/update_signers" "$SIGNER"
expect "it keeps the URL config beside" has "$ROOT/config/update.conf.url" "POINTER_URL="
expect "it makes the incoming folder" test -d "$ROOT/incoming"
expect "it installs itself" test -f "$ROOT/tools/host/update.sh"
DC_TEST=1 DC_SYSTEMD_DIR="$T/units" run --setup --from-lobby
expect "a second --from-lobby keeps the pinned signer" has "$ROOT/config/update_signers" "$SIGNER"
expect "and the source" has "$ROOT/config/update.conf" "SOURCE=lobby"

# ---- tools/vps/deploy_remote.sh, with a stand-in systemctl on PATH and install.sh
DEPLOY="$REPO/tools/vps/deploy_remote.sh"
mkdir -p "$T/dbin"
cat > "$T/dbin/systemctl" <<'S'
#!/usr/bin/env bash
echo "systemctl $*" >> "$STUB_LOG"
case "$1" in
  is-enabled)
    for last; do :; done
    case " $STUB_ENABLED " in *" $last "*) exit 0 ;; esac
    exit 1 ;;
  is-active) exit 0 ;;
  show)
    for last; do :; done
    case " ${STUB_FROZEN:-} " in *" $last "*) echo 100; exit 0 ;; esac
    if [ -f "$STUB_MARK" ]; then echo 200; else echo 100; fi ;;
esac
S
cat > "$T/fake_install.sh" <<'S'
#!/usr/bin/env bash
echo "INSTALL $*" >> "$STUB_LOG"
[ -z "${STUB_INSTALL_FAIL:-}" ] || exit 1
touch "$STUB_MARK"
echo "commit: $STUB_COMMIT" > "$DC_ROOT/bin/COMMIT.txt"
S
chmod +x "$T/dbin/systemctl"
dfresh() {
  rm -rf "${T:?}/droot"
  mkdir -p "$T/droot/bin" "$T/droot/config" "$T/droot/instances/modded/config"
  touch "$T/droot/config/server.toml" "$T/droot/instances/modded/config/server.toml"
  rm -f "$T/mark"
}
drun() {
  : > "$T/log"
  ( export PATH="$T/dbin:$PATH" DC_ROOT="$T/droot" DC_TEST=1 DC_DEPLOY_INSTALL="$T/fake_install.sh" STUB_MARK="$T/mark" STUB_COMMIT="$NEW" \
      DC_DEPLOY_WAIT=0 DC_DEPLOY_POLL=0
    bash "$DEPLOY" "$@" ) > "$T/out" 2>&1 < /dev/null
  rc=$?
}
SUM=$(sha256sum "$SITE/$TGZ" | cut -d' ' -f1)

dfresh; STUB_ENABLED="decombat-lobby decombat-lobby@modded" drun install "$SITE/$TGZ" "$SUM" "$NEW" modded
expect "a VPS deploy succeeds" zero
expect "it reports the default lobby's commit" has "$T/out" "running: decombat-lobby commit $NEW active"
expect "it reports the modded lobby's commit" has "$T/out" "running: decombat-lobby@modded commit $NEW active"

dfresh; STUB_ENABLED="decombat-lobby" drun install "$SITE/$TGZ" "$SUM" "$NEW" modded
expect "a disabled modded instance refuses" nonzero
expect "before anything changes" lacks "$T/log" "INSTALL"

dfresh; STUB_ENABLED="decombat-lobby decombat-lobby@modded" drun install "$SITE/$TGZ" 00 "$NEW" modded
expect "a wrong tarball hash refuses" nonzero
expect "before anything changes (hash)" lacks "$T/log" "INSTALL"

dfresh; STUB_ENABLED="decombat-lobby decombat-lobby@modded" STUB_FROZEN="decombat-lobby@modded" drun install "$SITE/$TGZ" "$SUM" "$NEW" modded
expect "a modded lobby that did not restart fails" nonzero
expect "and is named" has "$T/out" "NOT running: decombat-lobby@modded"

dfresh; STUB_ENABLED="decombat-lobby decombat-lobby@modded" STUB_INSTALL_FAIL=1 drun install "$SITE/$TGZ" "$SUM" "$NEW" modded
expect "a failed install.sh fails the deploy" nonzero

PY=""; for c in python3 python; do if "$c" -c "pass" >/dev/null 2>&1; then PY=$c; break; fi; done
if [ -n "$PY" ]; then
  hosts() { printf '%s' "$1" | "$PY" "$REPO/tools/vps/deploy_hosts.py" 22222222 > "$T/out" 2>&1; rc=$?; }
  hosts '{"hosts":[{"name":"w","online":true,"version":"22222222","games":0}]}'
  expect "a host on the desired build passes" zero
  expect "and is reported" has "$T/out" "host w: on 22222222 (desired), 0 game(s) running"
  hosts '{"hosts":[{"name":"w","online":true,"version":"1111111","games":2}]}'
  expect "a host on an older build is pending" test "$rc" -eq 1
  expect "and is reported as such" has "$T/out" "(not yet), 2 game(s)"
  hosts '{"hosts":[{"name":"w","online":false,"games":0}]}'
  expect "an offline host is pending" test "$rc" -eq 1
  hosts 'not json'
  expect "no answer from the lobby is its own case" test "$rc" -eq 2
fi

for f in "$SCRIPT" "$DEPLOY" "$REPO/tools/package_servers.sh"; do bash -n "$f" && ok "syntax $(basename "$f")" || bad "syntax $(basename "$f")"; done
[ "$fails" -eq 0 ] && echo "all passed" || echo "$fails failed"
exit "$fails"
