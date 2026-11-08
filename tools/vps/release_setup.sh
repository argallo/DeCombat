#!/usr/bin/env bash
# release_setup.sh: once, as root on the VPS, after deploy_setup.sh: beta releases from the admin dashboard and a daily
# timer (docs/DEPLOY_FROM_ADMIN.md "Releases from the dashboard"). Provenance: ORIGINAL (DeCombat operator tooling).
# WRITTEN ON WINDOWS, NOT RUN ON LINUX YET.
#
#   sudo GITHUB_TOKEN=... bash release_setup.sh --export-tool FILE --private-strings FILE \
#        --last-tag v0.5.0-beta6 --private-sha 791809ca1697 --public-sha 2ad7eec [--public-repo OWNER/REPO] [--reseed]
#   sudo bash release_setup.sh --check       the units' state and status.json
#
# Without GITHUB_TOKEN in the environment it asks for the token (not echoed) unless release.conf has one already.
# Makes, and keeps on a second run:
#   /opt/decombat/deploy/release.conf        root 0600: GITHUB_TOKEN= (fine-grained: the public repository only,
#                                            Contents read and write, Actions read), PUBLIC_REPO=
#   /opt/decombat/deploy/release/            root:decombat 0750: state.json (root only), status.json, history.jsonl, logs/
#   /opt/decombat/deploy/release/requests/   decombat 0700: the one place the lobby writes (a release request)
#   /opt/decombat/deploy/release/tool/       root 0700: the export tool and its word list (both from the files given;
#                                            neither is in this repository)
#   /opt/decombat/deploy/public              root 0700: a clone of the public repository at its master
#   decombat-lobby.service.d/21-release.conf ReadWritePaths for the request folder
#   decombat-release-check.{service,timer}   daily 06:00 UTC: release_from_git.sh daily (check, then prepare when
#                                            something is new and state.json has auto_prepare 1)
#   decombat-release.{path,service}          a request starts release_from_git.sh run
# state.json is seeded from --last-tag / --private-sha / --public-sha on the first run (or with --reseed).
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

D=${DC_DEPLOY_DIR:-$DC_ROOT/deploy}
R=$D/release
SYSTEMCTL=${DC_RELEASE_SYSTEMCTL:-systemctl}
GIT=${DC_RELEASE_GIT:-git}
TOOL_SRC=""
STRINGS_SRC=""
LAST_TAG=""
PRIV=""
PUBSHA=""
SLUG=""
RESEED=0
CHECK=0
TAG_RE='^v[0-9]{1,4}\.[0-9]{1,4}\.[0-9]{1,4}-beta[0-9]{1,4}$'

while [ $# -gt 0 ]; do
    case "$1" in
        --export-tool) [ $# -ge 2 ] || die "--export-tool needs a file"; TOOL_SRC=$2; shift 2 ;;
        --private-strings) [ $# -ge 2 ] || die "--private-strings needs a file"; STRINGS_SRC=$2; shift 2 ;;
        --last-tag) [ $# -ge 2 ] || die "--last-tag needs a value"; LAST_TAG=$2; shift 2 ;;
        --private-sha) [ $# -ge 2 ] || die "--private-sha needs a value"; PRIV=$2; shift 2 ;;
        --public-sha) [ $# -ge 2 ] || die "--public-sha needs a value"; PUBSHA=$2; shift 2 ;;
        --public-repo) [ $# -ge 2 ] || die "--public-repo needs OWNER/REPO"; SLUG=$2; shift 2 ;;
        --reseed) RESEED=1; shift ;;
        --check) CHECK=1; shift ;;
        -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
        *) die "unknown option: $1 (--help)" ;;
    esac
done
need_root

if [ $CHECK -eq 1 ]; then
    "$SYSTEMCTL" --no-pager status decombat-release-check.timer decombat-release.path 2>/dev/null | sed -n '1,12p' || true
    bash "$DC_ROOT/tools/release_from_git.sh" status
    exit 0
fi

command -v "$GIT" >/dev/null 2>&1 || die "git is missing: apt-get install git"
command -v "$DC_PYTHON" >/dev/null 2>&1 || die "$DC_PYTHON is missing"
[ "$DC_TEST" = 1 ] || id "$DC_USER" >/dev/null 2>&1 || die "no user $DC_USER: run bootstrap.sh first"
[ -d "$D/requests" ] && [ -d "${DC_SRC_GIT:-$DC_ROOT/src.git}" ] || die "no deploy folder or mirror: run deploy_setup.sh first"
[ -z "$SLUG" ] || [[ $SLUG =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || die "--public-repo must be OWNER/REPO"

# ---- folders: the lobby reads the status, writes only requests/
dir_mode 0750 "$R" "$R/logs"
dir_mode 0700 "$R/requests" "$R/work" "$R/tool"
if [ "$DC_TEST" != 1 ]; then
    chown root:"$DC_USER" "$R" "$R/logs"
    chown root:root "$R/work" "$R/tool"
    chown "$DC_USER:$DC_USER" "$R/requests"
fi

# ---- release.conf: the token, never echoed; kept on a second run unless a new one comes in the environment
TOKEN=${GITHUB_TOKEN:-}
unset GITHUB_TOKEN
OLD_TOKEN=""
OLD_SLUG=""
if [ -f "$D/release.conf" ]; then
    OLD_TOKEN=$(sed -n 's/^GITHUB_TOKEN=//p' "$D/release.conf" | tr -d '\r' | tail -n 1)
    OLD_SLUG=$(sed -n 's/^PUBLIC_REPO=//p' "$D/release.conf" | tr -d '\r' | tail -n 1)
fi
if [ -z "$TOKEN" ] && [ -z "$OLD_TOKEN" ] && [ -t 0 ]; then
    printf 'GitHub token for the public repository (not shown): ' >&2
    IFS= read -rs TOKEN || true
    printf '\n' >&2
fi
[ -n "$TOKEN" ] || TOKEN=$OLD_TOKEN
[ -n "$TOKEN" ] || die "no token: GITHUB_TOKEN=... in the environment, or run this from a terminal to be asked"
[[ $TOKEN =~ ^[A-Za-z0-9_]{20,255}$ ]] || die "the token has an unexpected shape (letters, digits and _ only)"
SLUG=${SLUG:-${OLD_SLUG:-argallo/DeCombat}}
umask 077
{
    echo "# release_from_git.sh settings (docs/DEPLOY_FROM_ADMIN.md); read as KEY=value, never run; root only"
    echo "# a fine-grained token: the public repository only; Contents read and write, Actions read"
    printf 'GITHUB_TOKEN=%s\n' "$TOKEN"
    printf 'PUBLIC_REPO=%s\n' "$SLUG"
} > "$D/release.conf.tmp"
chmod 0600 "$D/release.conf.tmp"
[ "$DC_TEST" = 1 ] || chown root:root "$D/release.conf.tmp"
mv -f "$D/release.conf.tmp" "$D/release.conf"
umask 022
TOKEN=""
say "wrote $D/release.conf (root 0600)"

# ---- the export tool and its word list (root only; the release job refuses to prepare without either)
if [ -n "$TOOL_SRC" ]; then
    [ -f "$TOOL_SRC" ] || die "no such file: $TOOL_SRC"
    install -m 0600 "$TOOL_SRC" "$R/tool/export_public.py"
    sed -i 's/\r$//' "$R/tool/export_public.py"
    say "installed the export tool in $R/tool"
fi
if [ -n "$STRINGS_SRC" ]; then
    [ -f "$STRINGS_SRC" ] || die "no such file: $STRINGS_SRC"
    install -m 0600 "$STRINGS_SRC" "$R/tool/private_strings.txt"
    sed -i 's/\r$//' "$R/tool/private_strings.txt"
    say "installed the scan word list in $R/tool"
fi
[ -f "$R/tool/export_public.py" ] || say "warning: no export tool yet (--export-tool FILE): prepare refuses until it is there"
[ -f "$R/tool/private_strings.txt" ] || say "warning: no scan word list yet (--private-strings FILE): prepare refuses until it is there"

# ---- the public clone (anonymous https: the repository is public)
PUBLIC_URL=${DC_RELEASE_PUBLIC_URL:-https://github.com/$SLUG.git}
if [ ! -d "$D/public/.git" ]; then
    rm -rf "$D/public"
    "$GIT" clone --quiet "$PUBLIC_URL" "$D/public" || die "could not clone $PUBLIC_URL"
    say "cloned the public repository into $D/public"
else
    "$GIT" -C "$D/public" remote set-url origin "$PUBLIC_URL"
    "$GIT" -C "$D/public" fetch --quiet origin || die "could not fetch $PUBLIC_URL"
fi
chmod 0700 "$D/public" 2>/dev/null || [ "$DC_TEST" = 1 ]

# ---- state.json: the last release, seeded once
if [ ! -f "$R/state.json" ] || [ $RESEED -eq 1 ]; then
    [[ $LAST_TAG =~ $TAG_RE ]] || die "--last-tag must be v<MAJOR>.<MINOR>.<PATCH>-beta<N> (the newest published beta)"
    [[ $PRIV =~ ^[0-9a-f]{7,40}$ ]] || die "--private-sha must be the hex commit of the working repository that beta was made from"
    [[ $PUBSHA =~ ^[0-9a-f]{7,40}$ ]] || die "--public-sha must be the hex commit of the public repository that beta tags"
    "$DC_PYTHON" -c '
import json, sys
json.dump({"last_tag": sys.argv[2], "last_released_private_sha": sys.argv[3], "last_public_sha": sys.argv[4],
           "auto_prepare": 1, "prepared": None}, open(sys.argv[1], "w", encoding="utf-8"), indent=1)
' "$R/state.json" "$LAST_TAG" "$PRIV" "$PUBSHA"
    chmod 0600 "$R/state.json" 2>/dev/null || true
    "$DC_PYTHON" -c '
import json, sys, time
json.dump({"phase": "idle", "updated": int(time.time()), "private_sha": "", "public_sha": sys.argv[3], "last_tag": sys.argv[2],
           "next_tag": "", "commits_since": 0, "ci": {"status": "", "conclusion": "", "url": ""}, "release_url": "",
           "message": "set up; the last release is " + sys.argv[2], "log_tail": [], "ok": None, "by": ""},
          open(sys.argv[1], "w", encoding="utf-8"), indent=1)
' "$R/status.json" "$LAST_TAG" "$PUBSHA"
    chmod 0640 "$R/status.json" 2>/dev/null || true
    [ "$DC_TEST" = 1 ] || chgrp "$DC_USER" "$R/status.json"
    say "seeded $R/state.json: last release $LAST_TAG"
else
    say "keeping $R/state.json"
    [ -z "$LAST_TAG$PRIV$PUBSHA" ] || say "  (--last-tag / --private-sha / --public-sha apply to a new state.json only: --reseed)"
fi

# ---- the scripts the units run
for f in release_from_git.sh release_setup.sh common.sh; do
    [ -f "$HERE/$f" ] || die "no $HERE/$f"
    [ "$HERE/$f" -ef "$DC_ROOT/tools/$f" ] && continue
    mkdir -p "$DC_ROOT/tools"
    install -m 0755 "$HERE/$f" "$DC_ROOT/tools/$f"
    sed -i 's/\r$//' "$DC_ROOT/tools/$f"
done

# ---- units
mkdir -p "$DC_SYSTEMD_DIR/$DC_UNIT.service.d"
cat > "$DC_SYSTEMD_DIR/$DC_UNIT.service.d/21-release.conf" <<UNIT
# release_setup.sh: the lobby may drop a release request here (docs/DEPLOY_FROM_ADMIN.md); "-": no error without it
[Service]
ReadWritePaths=-$R/requests
UNIT
cat > "$DC_SYSTEMD_DIR/decombat-release-check.service" <<UNIT
[Unit]
Description=DeCombat: the daily release check (and prepare when something is new)
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
Environment=HOME=/root
ExecStart=/usr/bin/env bash $DC_ROOT/tools/release_from_git.sh daily
TimeoutStartSec=150min
Nice=10
UNIT
cat > "$DC_SYSTEMD_DIR/decombat-release-check.timer" <<UNIT
[Unit]
Description=DeCombat: look for a new beta every day at 06:00 UTC

[Timer]
OnCalendar=*-*-* 06:00:00 UTC
Persistent=true
RandomizedDelaySec=60

[Install]
WantedBy=timers.target
UNIT
cat > "$DC_SYSTEMD_DIR/decombat-release.path" <<UNIT
[Unit]
Description=DeCombat: a release request from the dashboard starts the release job

[Path]
DirectoryNotEmpty=$R/requests

[Install]
WantedBy=paths.target
UNIT
cat > "$DC_SYSTEMD_DIR/decombat-release.service" <<UNIT
[Unit]
Description=DeCombat: a release step requested from the dashboard (check, prepare, tag)
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
Environment=HOME=/root
ExecStart=/usr/bin/env bash $DC_ROOT/tools/release_from_git.sh run
TimeoutStartSec=150min
UNIT
"$SYSTEMCTL" daemon-reload
"$SYSTEMCTL" enable --now decombat-release-check.timer decombat-release.path

say ""
say "Ready. The daily check runs at 06:00 UTC; by hand: sudo bash $DC_ROOT/tools/release_from_git.sh check"
say "The lobby needs one restart to see the drop-in (the next deploy does it): sudo systemctl restart $DC_UNIT"
say "Automatic prepare after the daily check: \"auto_prepare\": 1 in $R/state.json (0 turns it off)."
