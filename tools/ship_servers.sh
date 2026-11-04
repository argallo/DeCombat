#!/usr/bin/env bash
# ship_servers.sh: one command that builds this checkout and deploys it to every server: the VPS lobby
# (tools/vps/push.sh --build) and the remote game host (tools/host/push.sh). macOS / Linux. docs/VPS.md, docs/HOST.md.
#
#   tools/ship_servers.sh [--dry-run] [--yes] [--vps-only | --host-only] [--allow-dirty] [--conf FILE]
#
# --dry-run      check everything and print the plan; deploys nothing (it does read the host's settings over ssh)
# --yes          no question before deploying (needed when not run in a terminal)
# --vps-only     only the VPS; --host-only: only the game host
# --allow-dirty  deploy although crates/, the manifests, examples/, tools/vps or tools/host have uncommitted changes
# --conf FILE    the settings file (default ~/.config/decombat/ship_servers.conf, or $DC_SHIP_CONF):
#                  VPS_HOST=<address>       VPS_USER=root (optional)
#                  HOST_ADDR=<address>      HOST_USER=<ssh user>
#                  KEY=<private key file> (optional, default ~/.ssh/id_ed25519)
#
# The game host's name, lobby, fingerprint, token, port range and server arguments are read from its own
# /opt/decombat/config/host.toml and passed back unchanged, so an update needs no secret on this machine and wipes
# no setting. The first install of a host still goes through tools/host/push.sh by hand (it needs --rom).
# The VPS deploy restarts the lobby; the host update ends the matches running on that host. Run between matches.
# Afterwards both machines are asked which commit they run. Nothing here holds an address, a key or a secret.
set -uo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
CONF="${DC_SHIP_CONF:-$HOME/.config/decombat/ship_servers.conf}"
DRY=0; YES=0; DO_VPS=1; DO_HOST=1; DIRTY_OK=0
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY=1; shift ;;
    --yes) YES=1; shift ;;
    --vps-only) DO_HOST=0; shift ;;
    --host-only) DO_VPS=0; shift ;;
    --allow-dirty) DIRTY_OK=1; shift ;;
    --conf) CONF="$2"; shift 2 ;;
    -h|--help) sed -n 2,20p "$0"; exit 0 ;;
    *) echo "unknown option $1 (--help)" >&2; exit 2 ;;
  esac
done
[ $DO_VPS -eq 1 ] || [ $DO_HOST -eq 1 ] || { echo "--vps-only and --host-only together deploy nothing" >&2; exit 2; }

die() { echo "ship_servers: $*" >&2; exit 1; }
say() { printf '%s\n' "$*"; }

[ -f "$CONF" ] || die "no settings file at $CONF (--help lists what goes in it)"
VPS_HOST=""; VPS_USER="root"; HOST_ADDR=""; HOST_USER=""; KEY="$HOME/.ssh/id_ed25519"
# shellcheck disable=SC1090
. "$CONF"
[ $DO_VPS -eq 0 ] || [ -n "$VPS_HOST" ] || die "VPS_HOST is not set in $CONF"
[ $DO_HOST -eq 0 ] || { [ -n "$HOST_ADDR" ] && [ -n "$HOST_USER" ]; } || die "HOST_ADDR and HOST_USER must be set in $CONF"

# the seams the test script replaces
SSH_BIN="${DC_SHIP_SSH:-ssh}"
VPS_PUSH="${DC_SHIP_VPS_PUSH:-$HERE/tools/vps/push.sh}"
HOST_PUSH="${DC_SHIP_HOST_PUSH:-$HERE/tools/host/push.sh}"
remote() { # remote TARGET COMMAND
  "$SSH_BIN" -i "$KEY" -o BatchMode=yes -o ConnectTimeout=10 "$1" "$2"
}

# ---- the checkout: the servers report this commit, so it has to say what they run
COMMIT=$(git -C "$HERE" rev-parse --short HEAD) || die "not a git checkout: $HERE"
BRANCH=$(git -C "$HERE" rev-parse --abbrev-ref HEAD)
CHANGED=$(git -C "$HERE" status --porcelain -- crates Cargo.toml Cargo.lock examples tools/vps tools/host)
if [ -n "$CHANGED" ] && [ $DIRTY_OK -eq 0 ]; then
  say "$CHANGED" >&2
  die "uncommitted changes would go to the servers under commit $COMMIT: commit them, or pass --allow-dirty"
fi
if [ -z "$(git -C "$HERE" branch -r --contains HEAD 2>/dev/null)" ]; then
  say "warning: commit $COMMIT is not on GitHub yet; push it so the commit the servers report can be found"
fi

# ---- the game host's settings, before anything changes
H_NAME=""; H_LOBBY=""; H_FP=""; H_TOKEN=""; H_PORTS=""; H_ARGS=""
toml_value() { # toml_value KEY: the quoted value of a key in $HOST_TOML
  printf '%s\n' "$HOST_TOML" | sed -n "s/^$1 = \"\(.*\)\"\$/\1/p" | head -n 1
}
if [ $DO_HOST -eq 1 ]; then
  HOST_TOML=$(remote "$HOST_USER@$HOST_ADDR" 'cat /opt/decombat/config/host.toml') \
    || die "could not read /opt/decombat/config/host.toml on $HOST_ADDR (a first install goes through tools/host/push.sh)"
  H_NAME=$(toml_value name); H_LOBBY=$(toml_value lobby); H_FP=$(toml_value fingerprint)
  H_TOKEN=$(toml_value token); H_PORTS=$(toml_value port_range); H_ARGS=$(toml_value dserver_args)
  HOST_TOML=""
  for v in H_NAME H_LOBBY H_FP H_TOKEN H_PORTS; do
    [ -n "${!v}" ] || die "$HOST_ADDR's host.toml has no value for $v; nothing was deployed"
  done
fi
if [ $DO_VPS -eq 1 ]; then
  remote "$VPS_USER@$VPS_HOST" true || die "could not reach $VPS_USER@$VPS_HOST; nothing was deployed"
fi

# ---- the plan (never the token)
say "shipping commit $COMMIT ($BRANCH)"
[ -z "$CHANGED" ] || say "  with uncommitted changes (--allow-dirty)"
[ $DO_VPS -eq 0 ] || say "  VPS   $VPS_USER@$VPS_HOST: build there, install, restart the lobby"
[ $DO_HOST -eq 0 ] || say "  host  $HOST_USER@$HOST_ADDR: agent $H_NAME, lobby $H_LOBBY, ports $H_PORTS, server args '${H_ARGS}': build there, restart the agent (ends its matches)"
if [ $DRY -eq 1 ]; then say "dry run: nothing deployed"; exit 0; fi
if [ $YES -eq 0 ]; then
  [ -t 0 ] || die "not a terminal: pass --yes to deploy"
  printf 'Deploy now? [y/N] '
  read -r answer
  case "$answer" in y|Y|yes) ;; *) say "nothing deployed"; exit 0 ;; esac
fi

# ---- deploy: the lobby first, so a new agent never meets an older lobby
if [ $DO_VPS -eq 1 ]; then
  say "== VPS"
  "$VPS_PUSH" --host "$VPS_HOST" --user "$VPS_USER" --key "$KEY" --build --yes \
    || die "the VPS deploy failed; the host was not touched"
fi
if [ $DO_HOST -eq 1 ]; then
  say "== game host"
  "$HOST_PUSH" --host "$HOST_ADDR" --user "$HOST_USER" --key "$KEY" --name "$H_NAME" --lobby "$H_LOBBY" \
    --fingerprint "$H_FP" --token "$H_TOKEN" --ports "$H_PORTS" --dserver-args "$H_ARGS" \
    || die "the host update failed$([ $DO_VPS -eq 1 ] && echo '; the VPS is already on the new build')"
fi

# ---- ask both machines what they run
say "== check"
FAILED=0
check() { # check LABEL TARGET UNIT
  got=$(remote "$2" "head -n 1 /opt/decombat/bin/COMMIT.txt; systemctl is-active $3" 2>/dev/null)
  case "$got" in
    "commit: $COMMIT "*active*) say "  $1: running $COMMIT, $3 active" ;;
    *) say "  $1: NOT as expected: $got"; FAILED=1 ;;
  esac
}
[ $DO_VPS -eq 0 ] || check VPS "$VPS_USER@$VPS_HOST" decombat-lobby
[ $DO_HOST -eq 0 ] || check host "$HOST_USER@$HOST_ADDR" decombat-host
[ $FAILED -eq 0 ] || die "a server is not on commit $COMMIT or its service is not active"
say "done: commit $COMMIT is live"
