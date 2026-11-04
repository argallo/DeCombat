#!/usr/bin/env bash
# ship_servers_test.sh: tests of tools/ship_servers.sh with stand-ins for ssh, git and both push scripts; it touches no
# server. Run: bash tools/ship_servers_test.sh  (exit 0 = all passed)
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
SCRIPT="$HERE/ship_servers.sh"
T=$(mktemp -d "${TMPDIR:-/tmp}/dc-ship-test.XXXXXX")
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/bin"
fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fails=$((fails + 1)); }
expect() { # expect NAME CONDITION...
  name=$1; shift
  if "$@"; then ok "$name"; else bad "$name"; fi
}
has() { grep -qF -- "$2" "$1"; }
lacks() { ! grep -qF -- "$2" "$1"; }

cat > "$T/conf" <<C
VPS_HOST=vps.example
HOST_ADDR=home.example
HOST_USER=hostuser
KEY=/dev/null
C
cat > "$T/toml.good" <<'C'
# written by push.sh
name = "us-west"
lobby = "lobby.example:10005"
fingerprint = "ABC123"
token = "secret-token-of-at-least-24-chars"
dserver_exe = "/opt/decombat/bin/dc-dserver"
port_range = "27000-27005"
dserver_args = "-tick 20 -verbose"
C
cat > "$T/bin/git" <<'S'
#!/usr/bin/env bash
case "$*" in
  *"rev-parse --short HEAD"*) echo abc1234 ;;
  *"rev-parse --abbrev-ref HEAD"*) echo some-branch ;;
  *"status --porcelain"*) printf '%s' "${STUB_DIRTY:-}" ;;
  *"branch -r --contains"*) echo origin/some-branch ;;
esac
S
cat > "$T/ssh" <<'S'
#!/usr/bin/env bash
for last; do :; done
echo "ssh $*" >> "$STUB_LOG"
case "$last" in
  *"cat /opt/decombat/config/host.toml"*) [ -f "$STUB_TOML" ] && cat "$STUB_TOML" || exit 1 ;;
  true) exit "${STUB_VPS_DOWN:-0}" ;;
  *COMMIT.txt*) printf 'commit: %s (built now)\nactive\n' "${STUB_LIVE:-abc1234}" ;;
esac
S
cat > "$T/vps_push" <<'S'
#!/usr/bin/env bash
printf 'VPS_PUSH' >> "$STUB_LOG"; printf ' [%s]' "$@" >> "$STUB_LOG"; echo >> "$STUB_LOG"
exit "${STUB_VPS_FAIL:-0}"
S
cat > "$T/host_push" <<'S'
#!/usr/bin/env bash
printf 'HOST_PUSH' >> "$STUB_LOG"; printf ' [%s]' "$@" >> "$STUB_LOG"; echo >> "$STUB_LOG"
exit "${STUB_HOST_FAIL:-0}"
S
chmod +x "$T/bin/git" "$T/ssh" "$T/vps_push" "$T/host_push"
export PATH="$T/bin:$PATH" DC_SHIP_SSH="$T/ssh" DC_SHIP_VPS_PUSH="$T/vps_push" DC_SHIP_HOST_PUSH="$T/host_push"
export STUB_LOG="$T/log" STUB_TOML="$T/toml.good"
run() { # run ARGS...: output in $T/out, calls in $T/log, exit code in $rc
  : > "$T/log"
  "$SCRIPT" --conf "$T/conf" "$@" > "$T/out" 2>&1 < /dev/null
  rc=$?
}
zero() { [ "$rc" -eq 0 ]; }
nonzero() { [ "$rc" -ne 0 ]; }
order_ok() { [ "$(grep -n VPS_PUSH "$T/log" | cut -d: -f1)" -lt "$(grep -n HOST_PUSH "$T/log" | cut -d: -f1)" ]; }

run --dry-run
expect "dry run succeeds" zero
expect "dry run deploys nothing" lacks "$T/log" "_PUSH"
expect "dry run shows the host settings" has "$T/out" "lobby.example:10005"
expect "the plan never shows the token" lacks "$T/out" "secret-token"

run
expect "no terminal and no --yes refuses" nonzero
expect "the refusal deploys nothing" lacks "$T/log" "_PUSH"

run --yes
expect "a full run succeeds" zero
expect "the VPS goes first" order_ok
expect "the VPS builds" has "$T/log" "VPS_PUSH [--host] [vps.example] [--user] [root] [--key] [/dev/null] [--build]"
expect "the host keeps its token" has "$T/log" "[--token] [secret-token-of-at-least-24-chars]"
expect "the host keeps its fingerprint and lobby" has "$T/log" "[--lobby] [lobby.example:10005] [--fingerprint] [ABC123]"
expect "the host keeps its ports" has "$T/log" "[--ports] [27000-27005]"
expect "the host keeps its server arguments" has "$T/log" "[--dserver-args] [-tick 20 -verbose]"
expect "the host update passes no ROM" lacks "$T/log" "--rom"
expect "the output never shows the token" lacks "$T/out" "secret-token"
expect "it reports the live commit" has "$T/out" "done: commit abc1234 is live"

run --yes --vps-only
expect "--vps-only succeeds" zero
expect "--vps-only leaves the host alone" lacks "$T/log" "HOST_PUSH"
expect "--vps-only does not read host.toml" lacks "$T/log" "host.toml"

run --yes --host-only
expect "--host-only succeeds" zero
expect "--host-only leaves the VPS alone" lacks "$T/log" "VPS_PUSH"

run --vps-only --host-only
expect "both only flags refuse" nonzero

STUB_DIRTY=" M crates/x.rs" run --yes
expect "uncommitted server code refuses" nonzero
expect "that refusal deploys nothing" lacks "$T/log" "_PUSH"
STUB_DIRTY=" M crates/x.rs" run --yes --allow-dirty
expect "--allow-dirty deploys" has "$T/log" "VPS_PUSH"

sed '/^token/d' "$T/toml.good" > "$T/toml.notoken"
STUB_TOML="$T/toml.notoken" run --yes
expect "a host.toml without a token refuses" nonzero
expect "that refusal deploys nothing, not even the VPS" lacks "$T/log" "_PUSH"
STUB_TOML="$T/no-such-file" run --yes
expect "an unreadable host.toml refuses" nonzero
expect "that refusal deploys nothing either" lacks "$T/log" "_PUSH"

STUB_VPS_DOWN=1 run --yes
expect "an unreachable VPS refuses" nonzero
expect "and touches neither server" lacks "$T/log" "_PUSH"

STUB_VPS_FAIL=1 run --yes
expect "a failed VPS deploy fails" nonzero
expect "and leaves the host alone" lacks "$T/log" "HOST_PUSH"

STUB_HOST_FAIL=1 run --yes
expect "a failed host update fails" nonzero
expect "and says the VPS is already updated" has "$T/out" "the VPS is already on the new build"

STUB_LIVE=fffffff run --yes
expect "a server on another commit fails the check" nonzero
expect "and is named" has "$T/out" "NOT as expected"

"$SCRIPT" --conf "$T/no-such-conf" --yes > "$T/out" 2>&1 < /dev/null; rc=$?
expect "a missing settings file refuses" nonzero

bash -n "$SCRIPT" && ok "syntax" || bad "syntax"
[ "$fails" -eq 0 ] && echo "all passed" || echo "$fails failed"
exit "$fails"
