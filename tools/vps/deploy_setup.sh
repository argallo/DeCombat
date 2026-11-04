#!/usr/bin/env bash
# deploy_setup.sh: once, as root on the VPS: deploys from the admin dashboard (docs/DEPLOY_FROM_ADMIN.md).
# Provenance: ORIGINAL (DeCombat operator tooling). WRITTEN ON WINDOWS, NOT RUN ON LINUX YET.
#
#   sudo bash deploy_setup.sh [--repo git@github.com:OWNER/REPO.git] [--branch master] [--required-instances "modded"]
#   sudo bash deploy_setup.sh --check       after the deploy key is on GitHub: one fetch, then the units' state
#
# Makes, and keeps on a second run:
#   /opt/decombat/deploy/key/id_ed25519     a read-only deploy key made here (root only); the public half is printed:
#                                           add it under the repository's Settings > Deploy keys, write access OFF
#   /opt/decombat/deploy/key/known_hosts    GitHub's ed25519 host key (checked against GitHub's published fingerprint)
#   /opt/decombat/deploy/sign/host_update_ed25519   the key that signs the build offered to the game hosts (root only);
#                                           the printed `decombat-deploy ssh-ed25519 ...` line goes to each host's
#                                           `update.sh --setup --from-lobby --signer "<line>"`
#   /opt/decombat/src.git                   the bare mirror of the branch (root)
#   /opt/decombat/deploy/                   root:decombat 0750: deploy.conf, available.json, status.json, history.jsonl,
#                                           logs/, host/ (the build the host agents are offered), work/ (root only)
#   /opt/decombat/deploy/requests/          decombat 0700: the one place the lobby writes (a deploy request)
#   decombat-lobby.service.d/20-deploy.conf ReadWritePaths for that folder (the hardened lobby may write nothing else)
#   decombat-src-fetch.{service,timer}      every 5 min: fetch, list the new commits (available.json)
#   decombat-deploy.{path,service}          a request starts deploy_from_git.sh run
# and copies deploy_from_git.sh, deploy_setup.sh and common.sh to /opt/decombat/tools (install.sh refreshes them at every deploy).
# Needs git, python3, a rust toolchain for root (rustup, as the build on the VPS of docs/VPS.md) and build-essential.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

D=${DC_DEPLOY_DIR:-$DC_ROOT/deploy}
MIRROR=${DC_SRC_GIT:-$DC_ROOT/src.git}
SYSTEMCTL=${DC_DEPLOY_SYSTEMCTL:-systemctl}
GIT=${DC_DEPLOY_GIT:-git}
REPO=""
BRANCH=""
REQUIRED=""
CHECK=0
# GitHub's published ed25519 host key (docs.github.com "GitHub's SSH key fingerprints"):
# SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU
GITHUB_ED25519="github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl"
GITHUB_FP="SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU"

while [ $# -gt 0 ]; do
    case "$1" in
        --repo) [ $# -ge 2 ] || die "--repo needs a value"; REPO=$2; shift 2 ;;
        --branch) [ $# -ge 2 ] || die "--branch needs a value"; BRANCH=$2; shift 2 ;;
        --required-instances) [ $# -ge 2 ] || die "--required-instances needs a value"; REQUIRED=$2; shift 2 ;;
        --check) CHECK=1; shift ;;
        -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
        *) die "unknown option: $1 (--help)" ;;
    esac
done
need_root

if [ $CHECK -eq 1 ]; then
    bash "$DC_ROOT/tools/deploy_from_git.sh" fetch || die "the fetch failed: is the deploy key added on GitHub (Settings > Deploy keys)?"
    "$SYSTEMCTL" --no-pager status decombat-src-fetch.timer decombat-deploy.path 2>/dev/null | sed -n '1,12p' || true
    say "ready: the dashboard's Updates tab lists the new commits"
    exit 0
fi

command -v "$GIT" >/dev/null 2>&1 || die "git is missing: apt-get install git"
command -v "$DC_PYTHON" >/dev/null 2>&1 || die "$DC_PYTHON is missing"
[ "$DC_TEST" = 1 ] || id "$DC_USER" >/dev/null 2>&1 || die "no user $DC_USER: run bootstrap.sh first"
[ -d "$DC_ROOT/bin" ] || die "no $DC_ROOT/bin: run bootstrap.sh first"

# ---- folders: the lobby reads the state, writes only requests/
dir_mode 0750 "$D" "$D/logs" "$D/host"
dir_mode 0700 "$D/key" "$D/sign" "$D/work" "$D/requests"
if [ "$DC_TEST" != 1 ]; then
    chown root:"$DC_USER" "$D" "$D/logs" "$D/host"
    chown root:root "$D/key" "$D/sign" "$D/work"
    chown "$DC_USER:$DC_USER" "$D/requests"
fi

# ---- deploy.conf: written once, then the operator's
if [ ! -f "$D/deploy.conf" ]; then
    {
        echo "# deploy_from_git.sh settings (docs/DEPLOY_FROM_ADMIN.md); read as KEY=value, never run"
        echo "REPO=${REPO:-git@github.com:argallo/DeCombat.git}"
        echo "BRANCH=${BRANCH:-master}"
        echo "# instances whose unit must be enabled, or the deploy stops before any change (deploy_remote.sh)"
        echo "REQUIRED_INSTANCES=$REQUIRED"
        echo "# minutes a deploy without \"end running matches\" waits for the matches to end"
        echo "MATCH_WAIT_MIN=15"
        echo "# cargo -j; empty = all cores (the build runs at nice 10)"
        echo "BUILD_JOBS="
        echo "# the cargo binary; empty = the one on root's PATH, else /root/.cargo/bin/cargo"
        echo "CARGO="
    } > "$D/deploy.conf"
    chmod 0640 "$D/deploy.conf"
    [ "$DC_TEST" = 1 ] || chown root:"$DC_USER" "$D/deploy.conf"
    say "wrote $D/deploy.conf"
else
    say "keeping $D/deploy.conf"
    [ -z "$REPO$BRANCH$REQUIRED" ] || say "  (--repo / --branch / --required-instances apply to a new deploy.conf only: edit it)"
fi
REPO=$(sed -n 's/^REPO=//p' "$D/deploy.conf" | tail -n 1)
BRANCH=$(sed -n 's/^BRANCH=//p' "$D/deploy.conf" | tail -n 1)
case "$REPO" in git@github.com:*/*.git|ssh://git@github.com/*) ;; *) die "REPO must be a GitHub ssh address (git@github.com:OWNER/REPO.git)" ;; esac
case "$BRANCH" in ''|*[!A-Za-z0-9._/-]*|-*) die "BRANCH must be a plain branch name" ;; esac

# ---- the deploy key and GitHub's host key
if [ ! -f "$D/key/id_ed25519" ]; then
    ssh-keygen -q -t ed25519 -N "" -C "decombat-deploy@$(hostname 2>/dev/null || echo vps)" -f "$D/key/id_ed25519"
    say "made the deploy key $D/key/id_ed25519"
fi
chmod 0600 "$D/key/id_ed25519"
printf '%s\n' "$GITHUB_ED25519" > "$D/key/known_hosts"
got=$(ssh-keygen -lf "$D/key/known_hosts" | awk '{print $2}')
[ "$got" = "$GITHUB_FP" ] || die "the pinned GitHub host key does not have GitHub's published fingerprint ($got)"

# ---- the key that signs the hosts' offers (deploy_from_git.sh sign_offer); never leaves this folder
if [ ! -f "$D/sign/host_update_ed25519" ]; then
    ssh-keygen -q -t ed25519 -N "" -C "decombat-host-update@$(hostname 2>/dev/null || echo vps)" -f "$D/sign/host_update_ed25519"
    say "made the host offer signing key $D/sign/host_update_ed25519"
fi
chmod 0600 "$D/sign/host_update_ed25519"

# ---- the mirror
if [ ! -d "$MIRROR" ]; then
    "$GIT" init --quiet --bare "$MIRROR"
    say "made the mirror $MIRROR"
fi
if "$GIT" --git-dir="$MIRROR" remote get-url origin >/dev/null 2>&1; then
    "$GIT" --git-dir="$MIRROR" remote set-url origin "$REPO"
else
    "$GIT" --git-dir="$MIRROR" remote add origin "$REPO"
fi

# ---- the scripts the units run
for f in deploy_from_git.sh deploy_setup.sh common.sh; do
    [ -f "$HERE/$f" ] || die "no $HERE/$f"
    [ "$HERE/$f" -ef "$DC_ROOT/tools/$f" ] && continue
    mkdir -p "$DC_ROOT/tools"
    install -m 0755 "$HERE/$f" "$DC_ROOT/tools/$f"
    sed -i 's/\r$//' "$DC_ROOT/tools/$f"
done

# ---- units
mkdir -p "$DC_SYSTEMD_DIR/$DC_UNIT.service.d"
cat > "$DC_SYSTEMD_DIR/$DC_UNIT.service.d/20-deploy.conf" <<UNIT
# deploy_setup.sh: the lobby may drop a deploy request here (docs/DEPLOY_FROM_ADMIN.md); "-": no error without it
[Service]
ReadWritePaths=-$D/requests
UNIT
cat > "$DC_SYSTEMD_DIR/decombat-src-fetch.service" <<UNIT
[Unit]
Description=DeCombat: fetch $BRANCH into the deploy mirror and list the new commits
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/bin/env bash $DC_ROOT/tools/deploy_from_git.sh fetch
TimeoutStartSec=5min
Nice=10
UNIT
cat > "$DC_SYSTEMD_DIR/decombat-src-fetch.timer" <<UNIT
[Unit]
Description=DeCombat: look for new commits every 5 minutes

[Timer]
OnBootSec=2min
OnUnitActiveSec=5min
RandomizedDelaySec=30

[Install]
WantedBy=timers.target
UNIT
cat > "$DC_SYSTEMD_DIR/decombat-deploy.path" <<UNIT
[Unit]
Description=DeCombat: a deploy request from the dashboard starts a deploy

[Path]
DirectoryNotEmpty=$D/requests

[Install]
WantedBy=paths.target
UNIT
cat > "$DC_SYSTEMD_DIR/decombat-deploy.service" <<UNIT
[Unit]
Description=DeCombat: deploy a commit of $BRANCH (requested from the dashboard)
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
Environment=HOME=/root
ExecStart=/usr/bin/env bash $DC_ROOT/tools/deploy_from_git.sh run
TimeoutStartSec=120min
UNIT
"$SYSTEMCTL" daemon-reload
"$SYSTEMCTL" enable --now decombat-src-fetch.timer decombat-deploy.path

say ""
say "Add this public key on GitHub: the repository > Settings > Deploy keys > Add deploy key, title \"VPS deploy\","
say "\"Allow write access\" left OFF:"
say ""
cat "$D/key/id_ed25519.pub"
say ""
say "Each game host pins this signer line (update.sh --setup --from-lobby --signer \"<line>\", docs/DEPLOY_FROM_ADMIN.md):"
say ""
printf 'decombat-deploy %s
' "$(cut -d' ' -f1,2 "$D/sign/host_update_ed25519.pub")"
say ""
say "Then: sudo bash $DC_ROOT/tools/deploy_setup.sh --check  (or wait for the timer)."
say "The lobby needs one restart to see the drop-in (the next deploy does it): sudo systemctl restart $DC_UNIT"
say "and [deploy] dir = \"$D\" in $DC_ROOT/config/server.toml (docs/DEPLOY_FROM_ADMIN.md)."
