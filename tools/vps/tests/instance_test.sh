#!/usr/bin/env bash
# instance_test.sh: tests of tools/vps/instance.sh and the instance helpers of common.sh on a temporary root
# (DC_ROOT, DC_SYSTEMD_DIR, DC_TEST=1: no root, no chown, no systemctl). Runs under Git Bash on Windows and on Linux:
#   bash tools/vps/tests/instance_test.sh
# Also `bash -n` on every tools/vps script. Exit 0 = all passed.
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VPS=$(cd "$HERE/.." && pwd)
REPO=$(cd "$VPS/../.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/dc-instance-test.XXXXXX")
trap 'rm -rf "$T"' EXIT

export DC_TEST=1 DC_ROOT="$T/opt" DC_SYSTEMD_DIR="$T/systemd"
unset DC_ADMIN DC_BOT_LOBBY DC_INSTANCE DC_UNIT

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL: %s\n' "$*"; }
check() { # DESCRIPTION COMMAND...
    local d=$1
    shift
    if "$@"; then ok; else bad "$d"; fi
}
eq() { # DESCRIPTION GOT WANT
    if [ "$2" = "$3" ]; then ok; else bad "$1: got '$2', want '$3'"; fi
}
has() { # DESCRIPTION FILE-OR-TEXT PATTERN (fixed string)
    if printf '%s\n' "$2" | grep -qF -- "$3"; then ok; else bad "$1: no '$3'"; fi
}

# ---- bash -n on every script
for f in "$VPS"/*.sh "$HERE"/*.sh; do
    check "bash -n $(basename "$f")" bash -n "$f"
done

# ---- the default instance: the layout of bootstrap.sh, issuing identity tokens
mkdir -p "$DC_ROOT/config" "$DC_ROOT/data" "$DC_ROOT/logs" "$DC_ROOT/bin" "$DC_SYSTEMD_DIR"
{
    tr -d '\r' < "$REPO/examples/server-vps.toml"
    printf '\n[identity]\nissue = true\nissuer = "decombat.vps"\n'
} > "$DC_ROOT/config/server.toml"
tr -d '\r' < "$REPO/examples/decombat.env" > "$DC_ROOT/config/decombat.env"
tr -d '\r' < "$REPO/examples/decombat-lobby.service" > "$DC_SYSTEMD_DIR/decombat-lobby.service"

# a stand-in dc-lobby-server: `identity --db X` prints a public key and records its arguments
FAKE_KEY="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
printf '#!/usr/bin/env bash\necho "$*" > "%s/fake.args"\necho "made a new identity key pair x" >&2\necho %s\n' \
    "$T" "$FAKE_KEY" > "$T/fake-lobby"
chmod +x "$T/fake-lobby"
export DC_LOBBY_BIN="$T/fake-lobby"

run_instance() { bash "$VPS/instance.sh" "$@" 2>&1; }

# ---- create `modded`
out=$(run_instance modded create --auth-port 11000 --game-ports 27101-27186 --policy modded --identity-from default)
rc=$?
eq "create modded exit code ($out)" "$rc" 0
I="$DC_ROOT/instances/modded"
for d in config data logs logs/dserver mods; do check "dir $d" test -d "$I/$d"; done
check "server.toml" test -f "$I/config/server.toml"
check "decombat.env" test -f "$I/decombat.env"

# shellcheck source=../common.sh
. "$VPS/common.sh"
C="$I/config/server.toml"
eq "server.name" "$(toml_get "$C" server name)" "DeCombat modded"
eq "server.auth_port" "$(toml_get "$C" server auth_port)" 11000
eq "server.lobby_port" "$(toml_get "$C" server lobby_port)" 11002
eq "server.clan_port" "$(toml_get "$C" server clan_port)" 11003
eq "server.http_port" "$(toml_get "$C" server http_port)" 19080
eq "server.game_port" "$(toml_get "$C" server game_port)" 27101
eq "server.public" "$(toml_get "$C" server public)" true
eq "dserver.port_range" "$(toml_get "$C" dserver port_range)" 27102-27186
eq "dserver.tick_hz kept" "$(toml_get "$C" dserver tick_hz)" 30
eq "paths.rom (shared)" "$(toml_get "$C" paths rom)" "$DC_ROOT/rom"
eq "paths.db" "$(toml_get "$C" paths db)" "$I/data/ca.db"
eq "paths.log_dir" "$(toml_get "$C" paths log_dir)" "$I/logs"
eq "paths.mods_dir" "$(toml_get "$C" paths mods_dir)" "$I/mods"
eq "auth.auto_create" "$(toml_get "$C" auth auto_create)" false
eq "mods.policy" "$(toml_get "$C" mods policy)" modded
eq "identity.issuer (from default)" "$(toml_get "$C" identity issuer)" decombat.vps
eq "identity.public_key (from default)" "$(toml_get "$C" identity public_key)" "$FAKE_KEY"
eq "identity.issue unset on the accepting instance" "$(toml_get "$C" identity issue)" ""
eq "one [identity] table" "$(grep -c '^\[identity\]' "$C")" 1
eq "one [mods] table" "$(grep -c '^\[mods\]' "$C")" 1
eq "identity key read from the default DB" "$(cat "$T/fake.args")" "identity --db /opt/decombat/data/ca.db"
E="$I/decombat.env"
eq "env DC_ADMIN (first free from 10091)" "$(env_get "$E" DC_ADMIN)" 127.0.0.1:10091
eq "env DC_BOT_LOBBY" "$(env_get "$E" DC_BOT_LOBBY)" 127.0.0.1:11000
eq "env DC_PATHS__DB" "$(env_get "$E" DC_PATHS__DB)" "$I/data/ca.db"
eq "env no advertised address (none in the default env)" "$(env_get "$E" DC_SERVER__ADVERTISE_IP)" ""
has "ufw tcp line" "$out" "ufw allow 11000/tcp"
has "ufw web line" "$out" "ufw allow 19080/tcp"
has "ufw udp line" "$out" "ufw allow 27101:27186/udp"
has "server list entry" "$out" "lobby = \""
has "server list port" "$out" ":11000\""
has "server list policy" "$out" "policy = \"modded\""
has "advertise note" "$out" "no advertised address"
has "mods note" "$out" "[[mods.set]]"

# ---- the template unit (installed by create because the temp systemd dir had none)
U="$DC_SYSTEMD_DIR/decombat-lobby@.service"
check "template unit installed" test -f "$U"
check "template unit equals examples/" cmp -s "$U" <(tr -d '\r' < "$REPO/examples/decombat-lobby@.service")
ut=$(cat "$U")
has "unit EnvironmentFile" "$ut" "EnvironmentFile=/opt/decombat/instances/%i/decombat.env"
has "unit WorkingDirectory" "$ut" "WorkingDirectory=/opt/decombat/instances/%i"
has "unit --config" "$ut" "--config /opt/decombat/instances/%i/config/server.toml"
has "unit --dserver-logdir" "$ut" "--dserver-logdir /opt/decombat/instances/%i/logs/dserver"
has "unit --admin from the env file" "$ut" '--admin ${DC_ADMIN}'
has "unit shared binary" "$ut" "ExecStart=/opt/decombat/bin/dc-lobby-server "
has "unit graceful stop" "$ut" "KillMode=mixed"
has "unit ReadWritePaths" "$ut" "ReadWritePaths=/opt/decombat/instances/%i/data /opt/decombat/instances/%i/logs /opt/decombat/instances/%i/mods"
# the flags after --config are the default unit's, apart from the per-instance paths and the admin address
strip() { grep '^ExecStart=' "$1" | sed -E 's|/opt/decombat/instances/%i/|/opt/decombat/|; s|/opt/decombat/instances/%i/logs|/opt/decombat/logs|; s|--admin [^ ]+||'; }
eq "unit flags = the default unit's" "$(strip "$U")" "$(strip "$DC_SYSTEMD_DIR/decombat-lobby.service" | sed 's|/opt/decombat/config/|/opt/decombat/config/|')"

# ---- port clashes: refused, nothing written
out=$(run_instance clash1 create --auth-port 10000 --game-ports 28000-28010)
check "clash with default auth 10000 refused" test $? -ne 0
has "clash1 message" "$out" "TCP 10000 (instance default)"
check "clash1 folder not made" test ! -e "$DC_ROOT/instances/clash1"
out=$(run_instance clash2 create --auth-port 12000 --game-ports 27180-27200)
check "UDP overlap with modded refused" test $? -ne 0
has "clash2 message" "$out" "UDP 27102-27186 (instance modded)"
out=$(run_instance clash3 create --auth-port 11001 --game-ports 28000-28010)
check "TCP 11003 (auth + 2) clash refused" test $? -ne 0
has "clash3 message" "$out" "TCP 11003 (instance modded)"
out=$(run_instance clash4 create --auth-port 12000 --game-ports 28000-28010 --admin-port 10090)
check "admin port of the default refused" test $? -ne 0
has "clash4 message" "$out" "TCP 10090 (instance default)"
out=$(run_instance clash5 create --auth-port 12000 --game-ports 27000-27015)
check "default game_port 27015 refused" test $? -ne 0
has "clash5 message" "$out" "(instance default)"
for n in clash2 clash3 clash4 clash5; do check "$n folder not made" test ! -e "$DC_ROOT/instances/$n"; done

# ---- bad arguments
out=$(run_instance Bad create --auth-port 12000 --game-ports 28000-28010)
check "upper-case name refused" test $? -ne 0
out=$(run_instance default create --auth-port 12000 --game-ports 28000-28010)
check "create default refused" test $? -ne 0
out=$(run_instance x1 create --auth-port 12000 --game-ports 28010-28000)
check "A > B refused" test $? -ne 0
out=$(run_instance x2 create --auth-port 12000 --game-ports 28000-28010 --policy weird)
check "unknown policy refused" test $? -ne 0
out=$(run_instance x3 create --auth-port 12000 --game-ports 28000-28010 --identity-issuer a.b)
check "issuer without key refused" test $? -ne 0
out=$(run_instance modded create --auth-port 13000 --game-ports 29000-29010)
check "existing instance refused" test $? -ne 0

# ---- a third instance: vanilla, its own issuer, next admin port
out=$(run_instance third create --auth-port 12000 --game-ports 27187-27200 --identity-issue third.box --server-name "Third box")
eq "create third exit code ($out)" "$?" 0
C3="$DC_ROOT/instances/third/config/server.toml"
eq "third mods.policy" "$(toml_get "$C3" mods policy)" vanilla
eq "third identity.issue" "$(toml_get "$C3" identity issue)" true
eq "third identity.issuer" "$(toml_get "$C3" identity issuer)" third.box
eq "third name" "$(toml_get "$C3" server name)" "Third box"
eq "third admin" "$(env_get "$DC_ROOT/instances/third/decombat.env" DC_ADMIN)" 127.0.0.1:10092
check "no [[mods.set]] hint on a vanilla instance" bash -c "! grep -q 'mods.set' '$C3'"

# ---- list
out=$(run_instance list)
eq "list: three instances" "$(printf '%s\n' "$out" | wc -l | tr -d ' ')" 3
has "list default" "$out" "decombat-lobby "
has "list modded" "$out" "decombat-lobby@modded"

# ---- instance_select: what status.sh and bots.sh use
sel() { (unset DC_ADMIN DC_BOT_LOBBY; . "$VPS/common.sh"; instance_select "$1"; echo "$DC_UNIT|$DC_ADMIN|$DC_BOT_LOBBY|$INST_LOGS|$DC_TCP_PORTS|$DC_UDP_RANGE"); }
eq "select default (no-arg behaviour)" "$(sel default)" "decombat-lobby|127.0.0.1:10090|127.0.0.1:10000|$DC_ROOT/logs|10000 10002 10003 18080|27015:27100"
eq "select modded" "$(sel modded)" "decombat-lobby@modded|127.0.0.1:10091|127.0.0.1:11000|$I/logs|11000 11002 11003 19080|27101:27186"
eq "an explicit DC_ADMIN wins" "$(DC_ADMIN=127.0.0.1:9 bash -c ". '$VPS/common.sh'; instance_select modded; echo \$DC_ADMIN")" 127.0.0.1:9

# ---- start / stop / remove (systemctl stubbed by DC_TEST)
out=$(run_instance modded start)
has "start enables the template instance" "$out" "systemctl enable --now decombat-lobby@modded"
out=$(run_instance default stop)
has "stop of the default unit" "$out" "systemctl stop decombat-lobby"
out=$(run_instance third remove)
check "remove keeps the folder aside" bash -c "ls -d '$DC_ROOT'/removed-instances/third.* >/dev/null"
check "remove: folder gone from instances" test ! -e "$DC_ROOT/instances/third"
out=$(run_instance list)
eq "list after remove" "$(printf '%s\n' "$out" | wc -l | tr -d ' ')" 2

printf 'instance_test: %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
