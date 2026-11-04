#!/usr/bin/env bash
# push.sh: deploy the DeCombat servers to the VPS from macOS / Linux (docs/VPS.md): the unix counterpart of
# push.ps1. First run from a Mac on 2026-10-30 (docs/MACOS.md). Uses ssh, scp, rsync and tar.
#
#   tools/vps/push.sh --host IP [--key FILE] [--user NAME] [--port N] [--bootstrap --public-ip IP]
#                     [--tarball FILE | --build] [--rom DIR|FILE.tgz] [--apply-firewall] [--yes]
#
# --bootstrap     run bootstrap.sh on the VPS first (user decombat, /opt/decombat, unit installed not enabled);
#                 --public-ip IP is the address clients are told (DC_SERVER__ADVERTISE_IP)
# --tarball FILE  the CI artifact decombat-servers-linux-x86_64-<commit>.tar.gz, installed with install.sh
# --build         no CI artifact: rsync this checkout to ~/decombat-src on the VPS (no target/, rom/, .git),
#                 install rustup + build-essential there when missing, `cargo build --release` the five server
#                 binaries, pack them in the artifact layout and install that (minutes on 2 vCPU)
# --rom X         a server ROM folder (dc-server-rom output; packed here with tar) or an already packed .tgz
# --apply-firewall  bootstrap.sh adds the ufw rules (never enables ufw; the provider firewall is the one that counts)
# --yes           no question before the ROM upload
# Nothing here holds an address, a key or a password: they are parameters.
set -euo pipefail

HOST=""; KEY="$HOME/.ssh/id_ed25519"; USER_NAME="root"; PORT=22; BOOTSTRAP=0; PUBLIC_IP=""; TARBALL=""; BUILD=0
ROM=""; APPLY_FW=0; YES=0
while [ $# -gt 0 ]; do
  case "$1" in
    --host) HOST=$2; shift 2 ;;
    --key) KEY=$2; shift 2 ;;
    --user) USER_NAME=$2; shift 2 ;;
    --port) PORT=$2; shift 2 ;;
    --bootstrap) BOOTSTRAP=1; shift ;;
    --public-ip) PUBLIC_IP=$2; shift 2 ;;
    --tarball) TARBALL=$2; shift 2 ;;
    --build) BUILD=1; shift ;;
    --rom) ROM=$2; shift 2 ;;
    --apply-firewall) APPLY_FW=1; shift ;;
    --yes) YES=1; shift ;;
    -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
done
[ -n "$HOST" ] || { echo "--host is required (--help)" >&2; exit 2; }
[ -f "$KEY" ] || { echo "no key file: $KEY" >&2; exit 2; }
[ $BOOTSTRAP -eq 0 ] || [ -n "$PUBLIC_IP" ] || { echo "--bootstrap needs --public-ip" >&2; exit 2; }
[ -z "$TARBALL" ] || [ $BUILD -eq 0 ] || { echo "--tarball or --build, not both" >&2; exit 2; }

HERE="$(cd "$(dirname "$0")/../.." && pwd)"
TARGET="$USER_NAME@$HOST"
SSH=(ssh -i "$KEY" -p "$PORT" -o StrictHostKeyChecking=accept-new -o BatchMode=yes)
SCP=(scp -q -i "$KEY" -P "$PORT" -o StrictHostKeyChecking=accept-new)
SUDO=""; [ "$USER_NAME" = "root" ] || SUDO="sudo"
STAGE="decombat-upload"

say() { printf '%s\n' "$*"; }

# ---- scripts and examples (bootstrap.sh and install.sh read them from the stage)
say "uploading tools/vps and examples to $TARGET:~/$STAGE ..."
"${SSH[@]}" "$TARGET" "rm -rf ~/$STAGE/tools ~/$STAGE/examples && mkdir -p ~/$STAGE/tools"
"${SCP[@]}" -r "$HERE/tools/vps" "$TARGET:$STAGE/tools/vps"
"${SCP[@]}" -r "$HERE/examples" "$TARGET:$STAGE/examples"

# ---- binaries
PKG_REMOTE=""
if [ -n "$TARBALL" ]; then
  [ -f "$TARBALL" ] || { echo "no such file: $TARBALL" >&2; exit 2; }
  PKG_REMOTE="$STAGE/$(basename "$TARBALL")"
  say "uploading $(basename "$TARBALL") ..."
  "${SCP[@]}" "$TARBALL" "$TARGET:$PKG_REMOTE"
elif [ $BUILD -eq 1 ]; then
  say "rsync of the checkout to $TARGET:~/decombat-src ..."
  rsync -az --delete -e "ssh -i $KEY -p $PORT -o BatchMode=yes" \
    --exclude target --exclude rom --exclude local_play --exclude .git --exclude 'out*' --exclude 'crash_*' \
    --exclude web --exclude .DS_Store "$HERE/" "$TARGET:~/decombat-src/"
  COMMIT=$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo unknown)
  say "building on the VPS (commit $COMMIT; minutes) ..."
  "${SSH[@]}" "$TARGET" "set -e
    export DEBIAN_FRONTEND=noninteractive
    command -v cc >/dev/null || $SUDO apt-get -qq update >/dev/null && $SUDO apt-get -qq install -y build-essential pkg-config >/dev/null
    [ -x ~/.cargo/bin/cargo ] || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable >/dev/null 2>&1
    . ~/.cargo/env
    cd ~/decombat-src
    DC_GIT_COMMIT=$COMMIT cargo build --release -p dc-lobby-server -p dc-dserver -p dc-rez -p dc-botclient -p dc-config -p dc-host 2>&1 | grep -E '^error|Finished' || true
    P=decombat-servers-linux-x86_64-$COMMIT
    rm -rf ~/\$P && mkdir -p ~/\$P/bin ~/\$P/tools
    for b in dc-lobby-server dc-dserver dc-server-rom dc-config dc-botclient dc-host; do cp target/release/\$b ~/\$P/bin/; strip ~/\$P/bin/\$b; done
    cp -r tools/vps ~/\$P/tools/vps && cp -r examples ~/\$P/examples
    echo \"commit: $COMMIT (built on the VPS \$(date -u +%Y-%m-%dT%H:%M:%SZ))\" > ~/\$P/COMMIT.txt
    (cd ~/\$P && sha256sum bin/* > SHA256SUMS)
    tar -czf ~/$STAGE/\$P.tar.gz -C ~ \$P
    ls -la ~/$STAGE/\$P.tar.gz"
  PKG_REMOTE="$STAGE/decombat-servers-linux-x86_64-$COMMIT.tar.gz"
fi

# ---- server ROM
ROM_REMOTE=""
if [ -n "$ROM" ]; then
  if [ -d "$ROM" ]; then
    [ -f "$ROM/server-rom.json" ] || { echo "$ROM is not a server ROM folder (no server-rom.json)" >&2; exit 2; }
    TGZ="$(mktemp -d)/server-rom.tgz"
    say "packing $ROM ..."
    tar -czf "$TGZ" -C "$ROM" .
  else
    TGZ="$ROM"
  fi
  SIZE=$(du -h "$TGZ" | cut -f1)
  if [ $YES -eq 0 ]; then
    read -r -p "upload the server ROM ($SIZE) to $TARGET? [y/N] " a
    case "$a" in y|Y) ;; *) echo "skipped the ROM upload"; TGZ="" ;; esac
  fi
  if [ -n "$TGZ" ]; then
    say "uploading the server ROM ($SIZE) ..."
    "${SCP[@]}" "$TGZ" "$TARGET:$STAGE/server-rom.tgz"
    ROM_REMOTE="$STAGE/server-rom.tgz"
  fi
fi

# ---- on the VPS: bootstrap, install, status
CMDS=()
if [ $BOOTSTRAP -eq 1 ]; then
  FW=""; [ $APPLY_FW -eq 0 ] || FW=" --apply-firewall"
  CMDS+=("$SUDO bash ~/$STAGE/tools/vps/bootstrap.sh --public-ip $PUBLIC_IP$FW")
fi
if [ -n "$PKG_REMOTE" ] || [ -n "$ROM_REMOTE" ]; then
  ARGS=""; [ -z "$PKG_REMOTE" ] || ARGS=" ~/$PKG_REMOTE"; [ -z "$ROM_REMOTE" ] || ARGS="$ARGS --rom ~/$ROM_REMOTE"
  CMDS+=("$SUDO bash ~/$STAGE/tools/vps/install.sh$ARGS")
  [ -z "$ROM_REMOTE" ] || CMDS+=("rm -f ~/$ROM_REMOTE")
fi
CMDS+=("$SUDO bash /opt/decombat/tools/status.sh || true")
JOINED=$(printf '%s && ' "${CMDS[@]}"); JOINED=${JOINED% && }
say "on the VPS: $JOINED"
"${SSH[@]}" -t "$TARGET" "$JOINED"
