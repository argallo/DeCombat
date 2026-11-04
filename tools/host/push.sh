#!/usr/bin/env bash
# push.sh: deploy a DeCombat remote game host (specs/HOST_AGENT.md, docs/HOST.md) to a Linux machine you can ssh
# into with a key, from macOS / Linux. The machine builds `dc-dserver` and `dc-host` itself (rustup is installed
# for the user when missing), the binaries go to /opt/decombat/bin, the agent's config to
# /opt/decombat/config/host.toml, the server ROM to /opt/decombat/rom, and a systemd unit runs the agent as the
# ssh user. First written 2026-10-31 for the user's home machine ("US West").
#
#   tools/host/push.sh --host IP --user NAME [--key FILE] [--port N] --name AGENT --lobby HOST:PORT
#                      --fingerprint SHA256 --token SECRET [--rom DIR|FILE.tgz] [--ports LO-HI]
#                      [--public-address IP] [--dserver-args "..."] [--no-build]
#
# --name           the agent's name: the `host` of a [[lobby.servers]] entry and a [[hosts.agents]] entry
# --lobby          the lobby's host port (hosts.port, default auth_port + 5 = 10005)
# --fingerprint    the lobby certificate's SHA-256 (the client's pin; `dc-lobby-server tls --db ...` prints it)
# --token          the shared secret of that [[hosts.agents]] entry (at least 24 characters)
# --rom            a server ROM folder (dc-server-rom output) or its .tgz; uploaded and unpacked once (884 MB)
# --ports          the UDP range the router forwards to the machine (default 27888-27899: 12 matches at once)
# --public-address noted in the summary only: the advertised address lives in the lobby's [[hosts.agents]] entry
# --no-build       skip the build (reinstall config / ROM / unit only)
# The sudo rights the machine needs (a sudoers drop-in; nothing else runs as root): mkdir -p /opt/decombat*,
# chown -R ... /opt/decombat*, install, tee /etc/systemd/system/*, systemctl, apt-get.
# A by-hand build on a machine with the updater (tools/host/update.sh) sets its hold, so the timer does not put the
# desired build back over it; `update.sh --release-hold` on the machine resumes.
# Nothing here holds an address, a key or a secret: they are parameters, and the secret goes only into host.toml
# on the machine (mode 0600).
set -euo pipefail
HOST=""; USER_NAME=""; KEY="$HOME/.ssh/id_ed25519"; PORT=22; NAME=""; LOBBY=""; FP=""; TOKEN=""; ROM=""
PORTS="27888-27899"; PUBLIC=""; DARGS=""; BUILD=1
while [ $# -gt 0 ]; do
  case "$1" in
    --host) HOST="$2"; shift 2 ;;
    --user) USER_NAME="$2"; shift 2 ;;
    --key) KEY="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    --lobby) LOBBY="$2"; shift 2 ;;
    --fingerprint) FP="$2"; shift 2 ;;
    --token) TOKEN="$2"; shift 2 ;;
    --rom) ROM="$2"; shift 2 ;;
    --ports) PORTS="$2"; shift 2 ;;
    --public-address) PUBLIC="$2"; shift 2 ;;
    --dserver-args) DARGS="$2"; shift 2 ;;
    --no-build) BUILD=0; shift ;;
    -h|--help) sed -n 2,24p "$0"; exit 0 ;;
    *) echo "unknown option $1 (--help)" >&2; exit 2 ;;
  esac
done
for v in HOST USER_NAME NAME LOBBY FP TOKEN; do
  [ -n "${!v}" ] || { echo "--$(echo "$v" | tr '[:upper:]_' '[:lower:]-' | sed 's/user-name/user/') is required (--help)" >&2; exit 2; }
done
[ -f "$KEY" ] || { echo "no key file: $KEY" >&2; exit 2; }
[ ${#TOKEN} -ge 24 ] || { echo "--token must be at least 24 characters" >&2; exit 2; }
case "$PORTS" in *-*) ;; *) echo "--ports must be LO-HI" >&2; exit 2 ;; esac

HERE="$(cd "$(dirname "$0")/../.." && pwd)"
TARGET="$USER_NAME@$HOST"
SSH=(ssh -i "$KEY" -p "$PORT" -o StrictHostKeyChecking=accept-new -o BatchMode=yes)
SCP=(scp -q -i "$KEY" -P "$PORT" -o StrictHostKeyChecking=accept-new)
COMMIT=$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo unknown)
ROOT=/opt/decombat
say() { printf '%s\n' "$*"; }

# ---- folders (root once; then the user's)
say "== folders under $ROOT on $TARGET"
"${SSH[@]}" "$TARGET" "sudo -n /usr/bin/mkdir -p $ROOT/bin $ROOT/config $ROOT/logs $ROOT/rom && sudo -n /usr/bin/chown -R $USER_NAME:$USER_NAME $ROOT"

# ---- build on the machine
if [ $BUILD -eq 1 ]; then
  say "== rsync of the checkout to $TARGET:~/decombat-src"
  rsync -az --delete -e "ssh -i $KEY -p $PORT -o BatchMode=yes" \
    --exclude target --exclude rom --exclude local_play --exclude .git --exclude 'out*' --exclude 'crash_*' \
    --exclude web --exclude build --exclude .DS_Store "$HERE/" "$TARGET:~/decombat-src/"
  say "== building dc-dserver, dc-host, dc-server-rom on the machine (commit $COMMIT; minutes the first time)"
  "${SSH[@]}" "$TARGET" "set -e
    command -v cc >/dev/null || { sudo -n /usr/bin/apt-get -qq update >/dev/null; sudo -n /usr/bin/apt-get -qq install -y build-essential >/dev/null; }
    [ -x ~/.cargo/bin/cargo ] || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable >/dev/null 2>&1
    . ~/.cargo/env
    cd ~/decombat-src
    DC_GIT_COMMIT=$COMMIT cargo build --release -p dc-dserver -p dc-host -p dc-rez --bin dc-dserver --bin dc-host --bin dc-server-rom 2>&1 | grep -E '^error|Finished' || true
    for b in dc-dserver dc-host dc-server-rom; do [ -x target/release/\$b ] || { echo \"build failed: no target/release/\$b\" >&2; exit 1; }; install -m 0755 target/release/\$b $ROOT/bin/.\$b.new && mv -f $ROOT/bin/.\$b.new $ROOT/bin/\$b; done
    echo \"commit: $COMMIT (built \$(date -u +%Y-%m-%dT%H:%M:%SZ))\" > $ROOT/bin/COMMIT.txt
    $ROOT/bin/dc-host --version || true
    if [ -f $ROOT/config/update.conf ]; then mkdir -p $ROOT/update && touch $ROOT/update/hold && echo 'the updater is on hold for this by-hand build: $ROOT/tools/host/update.sh --release-hold resumes it'; fi"
fi

# ---- the server ROM
if [ -n "$ROM" ]; then
  if [ -d "$ROM" ]; then
    [ -f "$ROM/server-rom.json" ] || { echo "$ROM is not a server ROM folder (no server-rom.json)" >&2; exit 2; }
    TGZ="$(mktemp -d)/server-rom.tgz"; say "== packing $ROM"; tar -czf "$TGZ" -C "$ROM" .
  else
    TGZ="$ROM"
  fi
  say "== uploading the server ROM ($(du -h "$TGZ" | cut -f1))"
  "${SCP[@]}" "$TGZ" "$TARGET:/tmp/server-rom.tgz"
  "${SSH[@]}" "$TARGET" "set -e; rm -rf $ROOT/rom.new && mkdir -p $ROOT/rom.new && tar -xzf /tmp/server-rom.tgz -C $ROOT/rom.new && rm -f /tmp/server-rom.tgz
    $ROOT/bin/dc-server-rom --verify $ROOT/rom.new >/dev/null && rm -rf $ROOT/rom.old && { [ ! -d $ROOT/rom ] || mv $ROOT/rom $ROOT/rom.old; } && mv $ROOT/rom.new $ROOT/rom && rm -rf $ROOT/rom.old && echo 'server ROM installed'"
fi

# ---- host.toml (the secret only here, mode 0600)
say "== writing $ROOT/config/host.toml"
"${SSH[@]}" "$TARGET" "umask 077; cat > $ROOT/config/host.toml <<TOML
# DeCombat remote game host (specs/HOST_AGENT.md); written by tools/host/push.sh on $(date -u +%Y-%m-%dT%H:%M:%SZ)
name = \"$NAME\"
lobby = \"$LOBBY\"
fingerprint = \"$FP\"
token = \"$TOKEN\"
dserver_exe = \"$ROOT/bin/dc-dserver\"
rom = \"$ROOT/rom\"
port_range = \"$PORTS\"
bind_ip = \"0.0.0.0\"
log_dir = \"$ROOT/logs\"
dserver_args = \"$DARGS\"
TOML
chmod 0600 $ROOT/config/host.toml"

# ---- the unit: runs as the ssh user
say "== systemd unit decombat-host.service"
"${SSH[@]}" "$TARGET" "printf '%s\n' '[Unit]' 'Description=DeCombat remote game host (dc-host)' 'After=network-online.target' 'Wants=network-online.target' '' '[Service]' 'Type=simple' 'User=$USER_NAME' 'WorkingDirectory=$ROOT/logs' 'ExecStart=$ROOT/bin/dc-host --config $ROOT/config/host.toml' 'Restart=always' 'RestartSec=3' 'Environment=RUST_BACKTRACE=1' '' '[Install]' 'WantedBy=multi-user.target' | sudo -n /usr/bin/tee /etc/systemd/system/decombat-host.service >/dev/null
  sudo -n /usr/bin/systemctl daemon-reload && sudo -n /usr/bin/systemctl enable --now decombat-host.service && sudo -n /usr/bin/systemctl restart decombat-host.service && sleep 2 && sudo -n /usr/bin/systemctl --no-pager --lines=8 status decombat-host.service || true"
# ---- a host whose updater takes the lobby's builds keeps the paths root runs from root's (update.sh --setup
# --from-lobby made them so; the first step above handed them to this user for the copy); only logs, packs and incoming
# stay this user's, host.toml readable by its group (docs/DEPLOY_FROM_ADMIN.md, "The home game host")
"${SSH[@]}" "$TARGET" "if grep -qx 'SOURCE=lobby' $ROOT/config/update.conf 2>/dev/null; then
  chmod 0640 $ROOT/config/host.toml && sudo -n /usr/bin/mkdir -p $ROOT/logs $ROOT/packs $ROOT/incoming \
  && sudo -n /usr/bin/chown -R root:root $ROOT && sudo -n /usr/bin/chown -R $USER_NAME:$USER_NAME $ROOT/logs $ROOT/packs $ROOT/incoming \
  && sudo -n /usr/bin/chown -R root:$USER_NAME $ROOT/config/host.toml && echo 'root-owned layout restored (update.sh lobby mode)'; fi"
say "done: journalctl -u decombat-host -f on the machine shows the agent's log"
