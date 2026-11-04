#!/usr/bin/env bash
# instance.sh: several DeCombat lobby instances on one box (docs/VPS.md "Several instances on one box"). Run on the
# VPS as root. WRITTEN ON WINDOWS, NOT RUN ON LINUX YET (tools/vps/tests/instance_test.sh runs it on a temp root).
#
#   sudo bash instance.sh NAME create --auth-port N --game-ports A-B [--policy vanilla|modded] [--admin-port P]
#                         [--server-name TEXT] [--public-ip ADDRESS]
#                         [--identity-issue ID | --identity-issuer ID --identity-key KEY | --identity-from INSTANCE]
#   sudo bash instance.sh NAME start | stop | status [status.sh flags] | remove [--purge]
#   bash instance.sh list
#
# An instance is one lobby with its own ports, DB, logs, mods and mod policy: /opt/decombat/instances/NAME/
# {config/server.toml, decombat.env, data/, logs/, mods/}, unit decombat-lobby@NAME. The binaries, the server ROM and
# the user `decombat` are shared. `default` is the instance in /opt/decombat itself (unit decombat-lobby): it can be
# started, stopped and looked at here, not created or removed.
# create   writes the folder from examples/server-vps.toml: TCP auth N, lobby N+2, clan N+3, web N+8080, UDP A (first
#          room) and A+1-B (further rooms), the DB / logs / mods paths, [mods] policy (default vanilla), a loopback
#          admin port (the first free from 10091, or --admin-port), and [identity]: --identity-issue ID makes it the
#          issuer, --identity-issuer + --identity-key accept another issuer's tokens, --identity-from INSTANCE reads
#          both from an issuing instance on this box (its issuer id and `dc-lobby-server identity --db` output).
#          Refuses ports another instance uses (their server.toml / decombat.env). Prints the ufw lines to add; it
#          never runs ufw. The advertised address: --public-ip, else the default instance's DC_SERVER__ADVERTISE_IP.
# start    systemctl enable --now the unit;  stop: systemctl stop (graceful: matches end and are stored);
# status   status.sh for the instance;  remove: stop + disable, the folder moves to /opt/decombat/removed-instances/
#          (--purge deletes it, DB included).
# Environment: DC_ROOT (default /opt/decombat), DC_SYSTEMD_DIR (/etc/systemd/system), DC_TEST=1 (tests: no root, no
# chown, no systemctl), DC_LOBBY_BIN (the dc-lobby-server for --identity-from; default $DC_ROOT/bin/dc-lobby-server).
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

usage() { sed -n '2,27p' "$0"; exit "${1:-0}"; }

NAME=${1:-}
case "$NAME" in ''|-h|--help) usage 0 ;; esac
if [ "$NAME" = list ]; then
    for n in $(list_instances); do
        instance_ports "$n"
        instance_paths "$n"
        printf '%-12s unit %-24s auth %s lobby %s clan %s web %s admin %s udp %s  %s\n' "$n" "$INST_UNIT" \
            "$P_AUTH" "$P_LOBBY" "$P_CLAN" "$P_HTTP" "$P_ADMIN" "$P_UDP" "$INST_DIR"
    done
    exit 0
fi
valid_instance_name "$NAME" || die "not an instance name: '$NAME' (a lower-case letter first, then a-z 0-9 _ -, at most 32)"
CMD=${2:-}
[ -n "$CMD" ] || usage 2
shift 2

sysctl_do() {
    if [ "$DC_TEST" = 1 ]; then say "(test) systemctl $*"; else systemctl "$@"; fi
}

# install -d / install with mode and owner; under DC_TEST plain mkdir / cp (Git Bash cannot chmod everything)
mkdir_owned() {   # MODE OWNER GROUP DIR...
    local mode=$1 owner=$2 group=$3
    shift 3
    if [ "$DC_TEST" = 1 ]; then mkdir -p "$@"; else install -d -m "$mode" -o "$owner" -g "$group" "$@"; fi
}
file_owned() {    # MODE OWNER GROUP SRC DST
    if [ "$DC_TEST" = 1 ]; then cp "$4" "$5"; else install -m "$1" -o "$2" -g "$3" "$4" "$5"; fi
}

# examples/ next to tools/vps (the upload stage, a checkout) or the copy install.sh keeps in $DC_ROOT/tools/examples
find_examples() {
    local d
    for d in "$HERE/../../examples" "$HERE/../examples" "$HERE/examples"; do
        if [ -f "$d/server-vps.toml" ]; then (cd "$d" && pwd); return 0; fi
    done
    return 1
}

# is_port N: 1..65535
is_port() {
    case "$1" in ''|*[!0-9]*) return 1 ;; esac
    [ "$1" -ge 1 ] && [ "$1" -le 65535 ]
}

# the TCP ports and UDP intervals of every instance but NAME: lines "tcp PORT INSTANCE" / "udp LO HI INSTANCE"
others_ports() {
    local n iv p
    for n in $(list_instances); do
        [ "$n" != "$NAME" ] || continue
        instance_ports "$n"
        for p in "$P_AUTH" "$P_LOBBY" "$P_CLAN" "$P_HTTP" "$P_ADMIN"; do echo "tcp $p $n"; done
        for iv in $P_UDP; do echo "udp ${iv%-*} ${iv#*-} $n"; done
    done
}

case "$CMD" in
create)
    [ "$NAME" != default ] || die "the default instance is the one in $DC_ROOT (bootstrap.sh); pick another name"
    AUTH=""; GAME=""; POLICY=vanilla; ADMIN_PORT=""; SNAME=""; PUBLIC_IP=""
    ID_ISSUE=""; ID_ISSUER=""; ID_KEY=""; ID_FROM=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --auth-port) AUTH=${2:?--auth-port needs a port}; shift 2 ;;
            --game-ports) GAME=${2:?--game-ports needs A-B}; shift 2 ;;
            --policy) POLICY=${2:?--policy needs vanilla or modded}; shift 2 ;;
            --admin-port) ADMIN_PORT=${2:?--admin-port needs a port}; shift 2 ;;
            --server-name) SNAME=${2:?--server-name needs a text}; shift 2 ;;
            --public-ip) PUBLIC_IP=${2:?--public-ip needs an address}; shift 2 ;;
            --identity-issue) ID_ISSUE=${2:?--identity-issue needs an issuer id}; shift 2 ;;
            --identity-issuer) ID_ISSUER=${2:?--identity-issuer needs an id}; shift 2 ;;
            --identity-key) ID_KEY=${2:?--identity-key needs the public key}; shift 2 ;;
            --identity-from) ID_FROM=${2:?--identity-from needs an instance}; shift 2 ;;
            -h|--help) usage 0 ;;
            *) die "unknown argument: $1 (see --help)" ;;
        esac
    done
    need_root
    instance_paths "$NAME"
    [ ! -e "$INST_DIR" ] || die "$INST_DIR exists already (instance.sh $NAME remove first)"
    is_port "$AUTH" || die "--auth-port N is required (1..65535): '$AUTH'"
    case "$GAME" in *-*) ;; *) die "--game-ports A-B is required (UDP: A the first room, A+1..B the further rooms)" ;; esac
    G_LO=${GAME%-*}
    G_HI=${GAME#*-}
    { is_port "$G_LO" && is_port "$G_HI" && [ "$G_LO" -lt "$G_HI" ]; } || die "--game-ports '$GAME': A-B with A < B"
    case "$POLICY" in vanilla|modded) ;; *) die "--policy is vanilla or modded, not '$POLICY'" ;; esac
    P_L=$((AUTH + 2)); P_C=$((AUTH + 3)); P_H=$((AUTH + 8080))
    is_port "$P_H" || die "--auth-port $AUTH: the web port auth + 8080 = $P_H is out of range"
    case "$SNAME" in *'"'*|*\\*) die "--server-name without quotes or backslashes" ;; esac
    SNAME=${SNAME:-DeCombat $NAME}
    if [ -n "$PUBLIC_IP" ] && ! printf '%s' "$PUBLIC_IP" | grep -Eq '^([0-9]{1,3}\.){3}[0-9]{1,3}$'; then
        die "--public-ip '$PUBLIC_IP' is not a dotted IPv4 address"
    fi
    n_modes=0
    [ -z "$ID_ISSUE" ] || n_modes=$((n_modes + 1))
    [ -z "$ID_ISSUER$ID_KEY" ] || n_modes=$((n_modes + 1))
    [ -z "$ID_FROM" ] || n_modes=$((n_modes + 1))
    [ "$n_modes" -le 1 ] || die "one of --identity-issue, --identity-issuer/--identity-key, --identity-from"
    if [ -n "$ID_ISSUER$ID_KEY" ] && { [ -z "$ID_ISSUER" ] || [ -z "$ID_KEY" ]; }; then
        die "--identity-issuer and --identity-key go together"
    fi
    if [ -n "$ID_FROM" ]; then
        valid_instance_name "$ID_FROM" || die "--identity-from: not an instance name: $ID_FROM"
        instance_paths "$ID_FROM"
        [ -f "$INST_CONF" ] || die "--identity-from $ID_FROM: no $INST_CONF"
        [ "$(conf_get "$ID_FROM" identity issue)" = true ] || \
            die "--identity-from $ID_FROM: that instance does not issue tokens ([identity] issue = true, docs/VPS.md 7a)"
        ID_ISSUER=$(conf_get "$ID_FROM" identity issuer)
        [ -n "$ID_ISSUER" ] || die "--identity-from $ID_FROM: its [identity] has no issuer id"
        instance_paths "$ID_FROM"
        SRC_DB=$(conf_get "$ID_FROM" paths db)
        SRC_DB=${SRC_DB:-$INST_DATA/ca.db}
        SRC_KF=$(conf_get "$ID_FROM" identity key_file)
        LOBBY_BIN=${DC_LOBBY_BIN:-$DC_ROOT/bin/dc-lobby-server}
        [ -x "$LOBBY_BIN" ] || die "--identity-from: no $LOBBY_BIN"
        ARGS=(identity --db "$SRC_DB")
        [ -z "$SRC_KF" ] || ARGS+=(--key-file "$SRC_KF")
        if [ "$DC_TEST" = 1 ]; then
            ID_KEY=$("$LOBBY_BIN" "${ARGS[@]}")
        else
            ID_KEY=$(sudo -u "$DC_USER" "$LOBBY_BIN" "${ARGS[@]}")
        fi
        ID_KEY=$(printf '%s\n' "$ID_KEY" | tr -d '\r' | tail -n 1)
        instance_paths "$NAME"
    fi
    if [ -n "$ID_KEY" ]; then
        printf '%s' "$ID_KEY" | grep -Eq '^[A-Za-z0-9+/]{43}=$' || die "identity public key '$ID_KEY' is not 44 base64 characters"
    fi
    for id in "$ID_ISSUE" "$ID_ISSUER"; do
        [ -z "$id" ] || printf '%s' "$id" | grep -Eq '^[A-Za-z0-9._:-]{1,64}$' || die "issuer id '$id': 1..64 of A-Z a-z 0-9 . _ : -"
    done

    # ports: the new instance's own, then a clash with any other instance
    OTHERS=$(others_ports)
    instance_paths "$NAME"
    own_tcp() { [ "$1" = "$AUTH" ] || [ "$1" = "$P_L" ] || [ "$1" = "$P_C" ] || [ "$1" = "$P_H" ]; }
    if [ -z "$ADMIN_PORT" ]; then
        ADMIN_PORT=10091
        while printf '%s\n' "$OTHERS" | grep -q "^tcp $ADMIN_PORT " || own_tcp "$ADMIN_PORT"; do
            ADMIN_PORT=$((ADMIN_PORT + 1))
        done
    fi
    is_port "$ADMIN_PORT" || die "--admin-port '$ADMIN_PORT' is not a port"
    clash=""
    for p in "$AUTH" "$P_L" "$P_C" "$P_H" "$ADMIN_PORT"; do
        hit=$(printf '%s\n' "$OTHERS" | awk -v p="$p" '$1 == "tcp" && $2 == p { print $3; exit }')
        [ -z "$hit" ] || clash="$clash TCP $p (instance $hit);"
    done
    ! own_tcp "$ADMIN_PORT" || clash="$clash the admin port $ADMIN_PORT is one of the instance's own;"
    hit=$(printf '%s\n' "$OTHERS" | awk -v lo="$G_LO" -v hi="$G_HI" \
        '$1 == "udp" && $2 + 0 <= hi + 0 && lo + 0 <= $3 + 0 { print "UDP " $2 "-" $3 " (instance " $4 ")"; exit }')
    [ -z "$hit" ] || clash="$clash $hit;"
    [ -z "$clash" ] || die "port clash with another instance:$clash pick other ports (instance.sh list)"

    EXAMPLES=$(find_examples) || die "examples/server-vps.toml not found (run ~/decombat-upload/tools/vps/instance.sh, or install.sh a tarball first: $DC_ROOT/tools/examples)"

    # the folder: config root-owned (read-only for the service); data, logs and mods belong to the service
    mkdir_owned 0755 root root "$DC_ROOT/instances" "$INST_DIR"
    mkdir_owned 0750 root "$DC_USER" "$INST_DIR/config"
    mkdir_owned 0750 "$DC_USER" "$DC_USER" "$INST_DATA" "$INST_LOGS" "$INST_LOGS/dserver" "$INST_MODS"

    WORK=$(mktemp -d "${TMPDIR:-/tmp}/decombat-instance.XXXXXX")
    trap 'rm -rf "$WORK"' EXIT
    {
        echo "# instance '$NAME': written by tools/vps/instance.sh create on $(date -u +%Y-%m-%dT%H:%M:%SZ) from"
        echo "# examples/server-vps.toml; unit decombat-lobby@$NAME. Ports, paths, [mods] and [identity] are this"
        echo "# instance's; check it with: $DC_ROOT/bin/dc-config check $INST_CONF --fs"
        tr -d '\r' < "$EXAMPLES/server-vps.toml" | sed '/^# Identity tokens/,$d' | sed -E \
            -e "s|^name = .*|name = \"$SNAME\"|" \
            -e "s|^auth_port = .*|auth_port = $AUTH|" \
            -e "s|^lobby_port = .*|lobby_port = $P_L|" \
            -e "s|^clan_port = .*|clan_port = $P_C|" \
            -e "s|^http_port = .*|http_port = $P_H|" \
            -e "s|^game_port = .*|game_port = $G_LO|" \
            -e "s|^port_range = .*|port_range = \"$((G_LO + 1))-$G_HI\"|" \
            -e "s|^rom = .*|rom = \"$DC_ROOT/rom\"|" \
            -e "s|^db = .*|db = \"$INST_DATA/ca.db\"|" \
            -e "s|^log_dir = .*|log_dir = \"$INST_LOGS\"|" \
            -e "s|^mods_dir = .*|mods_dir = \"$INST_MODS\"|"
        echo ""
        echo "[mods]"
        echo "policy = \"$POLICY\""
        if [ "$POLICY" = modded ]; then
            echo "# a modded lobby needs at least one mod (dc-config check refuses an empty set); the zips go to $INST_MODS"
            echo "# [[mods.set]]"
            echo "# id = \"<mod id>\""
            echo "# version = \"<version>\""
            echo "# sha256 = \"<64 lowercase hex of the zip>\""
        fi
        echo ""
        if [ -n "$ID_ISSUE" ]; then
            echo "[identity]                      # this instance issues identity tokens (docs/VPS.md 7a)"
            echo "issue = true"
            echo "issuer = \"$ID_ISSUE\""
            echo "token_hours = 24"
            echo "key_file = \"\"                   # empty = identity_key.pem next to the DB"
        elif [ -n "$ID_ISSUER" ]; then
            echo "[identity]                      # accepts the tokens of issuer '$ID_ISSUER'${ID_FROM:+ (instance $ID_FROM)}"
            echo "issuer = \"$ID_ISSUER\""
            echo "public_key = \"$ID_KEY\""
        else
            echo "# no [identity]: standalone accounts only (docs/VPS.md 7a; instance.sh create --identity-from default)"
        fi
    } > "$WORK/server.toml"
    file_owned 0640 root "$DC_USER" "$WORK/server.toml" "$INST_CONF"

    ADV=$PUBLIC_IP
    [ -n "$ADV" ] || ADV=$(env_get "$DC_ROOT/config/decombat.env" DC_SERVER__ADVERTISE_IP)
    {
        echo "# Environment of the lobby instance '$NAME' (decombat-lobby@$NAME, EnvironmentFile=), written by"
        echo "# tools/vps/instance.sh create. DC_<SECTION>__<KEY> overrides server.toml. Not config (no __):"
        echo "#   DC_ADMIN      the loopback operator endpoint: the unit passes it as --admin, status.sh asks it"
        echo "#   DC_BOT_LOBBY  where bots.sh --instance $NAME logs its bots in (this instance's auth port)"
        if [ -n "$ADV" ]; then echo "DC_SERVER__ADVERTISE_IP=$ADV"; else echo "#DC_SERVER__ADVERTISE_IP=<VPS public IPv4 address>"; fi
        echo "DC_PATHS__ROM=$DC_ROOT/rom"
        echo "DC_PATHS__DB=$INST_DATA/ca.db"
        echo "DC_PATHS__LOG_DIR=$INST_LOGS"
        echo "DC_ADMIN=127.0.0.1:$ADMIN_PORT"
        echo "DC_BOT_LOBBY=127.0.0.1:$AUTH"
        echo "RUST_BACKTRACE=1"
    } > "$WORK/decombat.env"
    file_owned 0640 root "$DC_USER" "$WORK/decombat.env" "$INST_ENV"

    # the template unit, when bootstrap.sh / install.sh have not put it there yet
    if [ ! -f "$DC_SYSTEMD_DIR/decombat-lobby@.service" ] && [ -f "$EXAMPLES/decombat-lobby@.service" ]; then
        mkdir -p "$DC_SYSTEMD_DIR"
        tr -d '\r' < "$EXAMPLES/decombat-lobby@.service" > "$WORK/unit"
        file_owned 0644 root root "$WORK/unit" "$DC_SYSTEMD_DIR/decombat-lobby@.service"
        sysctl_do daemon-reload
        say "installed $DC_SYSTEMD_DIR/decombat-lobby@.service"
    fi

    say "instance '$NAME' created in $INST_DIR (policy $POLICY)"
    say "  TCP auth $AUTH, lobby $P_L, clan $P_C, web $P_H; UDP $G_LO-$G_HI; admin 127.0.0.1:$ADMIN_PORT (loopback only)"
    [ -z "$ID_ISSUE" ] || say "  identity: issues tokens as '$ID_ISSUE'"
    [ -z "$ID_ISSUER" ] || say "  identity: accepts the tokens of '$ID_ISSUER'"
    [ -n "$ADV" ] || say "  NOTE: no advertised address: set DC_SERVER__ADVERTISE_IP in $INST_ENV (or create with --public-ip)"
    [ "$POLICY" != modded ] || say "  NOTE: add the [[mods.set]] entries (and the zips in $INST_MODS) before the first start"
    say ""
    say "firewall: add these, and the same in the provider's firewall (instance.sh never runs ufw):"
    for p in "$AUTH" "$P_L" "$P_C" "$P_H"; do say "  ufw allow $p/tcp"; done
    say "  ufw allow $G_LO:$G_HI/udp"
    say ""
    say "start: sudo bash $DC_ROOT/tools/instance.sh $NAME start   (= systemctl enable --now decombat-lobby@$NAME)"
    say ""
    say "server list (LG-272): to list this instance under the default instance's servers after the login, add to"
    say "$DC_ROOT/config/server.toml and restart decombat-lobby (players never see the address):"
    say "  [[lobby.servers]]"
    say "  name = \"$SNAME\""
    say "  region = \"\"                    # shown next to the name, e.g. \"US Central\""
    say "  lobby = \"${ADV:-<VPS public IPv4 address>}:$AUTH\""
    say "  policy = \"$POLICY\""
    say "  fingerprint = \"\"               # after the first start: sudo -u $DC_USER $DC_ROOT/bin/dc-lobby-server tls --db $INST_DATA/ca.db"
    ;;
start)
    need_root
    instance_select "$NAME"
    [ -f "$INST_CONF" ] || die "no $INST_CONF"
    sysctl_do enable --now "$DC_UNIT"
    say "$DC_UNIT enabled and started; look at it: bash $DC_ROOT/tools/instance.sh $NAME status"
    ;;
stop)
    need_root
    instance_select "$NAME"
    say "stopping $DC_UNIT (graceful: running matches end and are stored) ..."
    sysctl_do stop "$DC_UNIT"
    ;;
status)
    exec bash "$HERE/status.sh" --instance "$NAME" "$@"
    ;;
remove)
    [ "$NAME" != default ] || die "the default instance is not removed here"
    PURGE=0
    [ "${1:-}" != --purge ] || PURGE=1
    need_root
    instance_select "$NAME"
    if [ "$DC_TEST" != 1 ]; then
        systemctl stop "$DC_UNIT" 2>/dev/null || true
        systemctl disable "$DC_UNIT" 2>/dev/null || true
    fi
    if [ "$PURGE" -eq 1 ]; then
        rm -rf "$INST_DIR"
        say "instance '$NAME' removed, its folder (DB included) deleted"
    else
        dest="$DC_ROOT/removed-instances/$NAME.$(date -u +%Y%m%dT%H%M%SZ)"
        mkdir -p "$DC_ROOT/removed-instances"
        mv "$INST_DIR" "$dest"
        say "instance '$NAME' removed; its folder (DB, logs) is kept in $dest"
    fi
    say "close its ports in ufw and in the provider's firewall yourself"
    ;;
*)
    die "unknown command: $CMD (create | start | stop | status | remove; see --help)"
    ;;
esac
