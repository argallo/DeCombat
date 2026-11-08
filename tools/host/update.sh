#!/usr/bin/env bash
# update.sh: a game host updates itself (docs/HOST.md "Updates"). A home host takes no inbound connection and no CI
# runner, so it pulls: a systemd timer runs this every few minutes as root. It reads the desired-version pointer, and
# when that differs from what is installed and no match runs here it takes the tarball, verifies it, swaps the
# binaries, restarts the agent and writes the outcome. The pointer and tarball come from one of two sources:
#   - SOURCE=url (default): POINTER_URL, the release the workflow deploy-servers.yml publishes (a public repository);
#   - SOURCE=lobby: the build the lobby offers its agents over their control connection (docs/DEPLOY_FROM_ADMIN.md):
#     the agent dc-host writes it to INCOMING (default /opt/decombat/incoming) as desired.txt and the tarball. Nothing
#     of it is trusted until its signature verifies against the deploy key pinned in config/update_signers at the
#     setup (ssh-keygen -Y verify, namespace decombat-host-update) and its sequence number is above the last one
#     installed; in this mode the tarball's own update.sh is never installed (a new updater comes with --setup only).
#
#   update.sh                      one check (what the timer runs)
#   update.sh --force              install now, though matches are running or this commit failed before
#   update.sh --status             print the last outcome
#   update.sh --release-hold       resume after a by-hand deploy (tools/host/push.sh sets the hold)
#   update.sh --setup --pointer-url URL     first time, as root: this script, update.conf and the timer
#   update.sh --setup --from-lobby --signer "decombat-deploy ssh-ed25519 AAAA..." [--agent-user NAME]
#                                  the same with SOURCE=lobby (replaces an update.conf of the URL source); the signer
#                                  line is what tools/vps/deploy_setup.sh prints
# Both setups make the paths root runs from root-owned (/opt/decombat, bin, config, tools, update, rom; host.toml
# root:<agent> 0640) and leave only logs, packs and incoming to the agent's user; a check refuses to run as root while
# any of them is writable by someone else.
#
# Touches only bin/{dc-host,dc-dserver,dc-server-rom}, bin/COMMIT.txt, tools/host/ and update/ under /opt/decombat.
# config/host.toml (the token), the ROM, packs and logs are never written. A build that does not start is rolled back,
# remembered in update/failed_commit, and not tried again until the pointer moves or --force is given.
# Exit 0: up to date, updated, held, or waiting for matches to end. Exit 1: something failed (update/status says what).
set -uo pipefail

ROOT=${DC_ROOT:-/opt/decombat}
UNIT=${DC_UPDATE_UNIT:-decombat-host}
SYSTEMD_DIR=${DC_SYSTEMD_DIR:-/etc/systemd/system}
# the seams tools/host/update_test.sh replaces
CURL=${DC_UPDATE_CURL:-curl}
SYSTEMCTL=${DC_UPDATE_SYSTEMCTL:-systemctl}
PGREP=${DC_UPDATE_PGREP:-pgrep}
SETTLE=${DC_UPDATE_SETTLE:-8}
ELF_CHECK=${DC_UPDATE_ELF_CHECK:-1}
CONF=$ROOT/config/update.conf
STATE=$ROOT/update
BINS_REQUIRED="dc-host dc-dserver"
BINS="$BINS_REQUIRED dc-server-rom"
SETPRIV=${DC_UPDATE_SETPRIV:-setpriv}
SSHKEYGEN=${DC_UPDATE_SSHKEYGEN:-ssh-keygen}
SIGNERS=$ROOT/config/update_signers
SIGN_NS=decombat-host-update
SIGN_ID=decombat-deploy
# the owner the root-run paths must have (the tests run as themselves)
ROOT_UID=${DC_UPDATE_ROOT_UID:-0}

say() { printf '%s\n' "$*"; }
installed_commit() { sed -n 's/^commit: \([0-9a-f]*\).*/\1/p' "$ROOT/bin/COMMIT.txt" 2>/dev/null | head -n 1; }
# report STATE DETAIL: one line for the journal, one file for --status
report() {
  say "update: $1: $2"
  mkdir -p "$STATE"
  {
    echo "state=$1"
    echo "detail=$2"
    echo "desired=${DESIRED:-}"
    echo "installed=$(installed_commit)"
    echo "time=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } > "$STATE/status.tmp" && mv -f "$STATE/status.tmp" "$STATE/status"
}
# a by-hand build records a short id, a CI build the full one
same_commit() { [ "${#1}" -ge 7 ] && case "$2" in "$1"*) return 0 ;; *) return 1 ;; esac; }
matches_running() { "$PGREP" -x dc-dserver >/dev/null 2>&1; }
is_elf() { [ "$ELF_CHECK" = 0 ] || [ "$(head -c 4 "$1" | od -An -tx1 | tr -d ' \n')" = 7f454c46 ]; }
# conf_get FILE KEY: read as data, never sourced
conf_get() { sed -n "s/^$2=//p" "$1" 2>/dev/null | tr -d '\r' | head -n 1; }
# as_owner CMD...: a new build's own binaries run as the agent's user (AGENT_USER of update.conf), never as root
as_owner() {
  if [ "$(id -u)" -ne 0 ]; then "$@"; return; fi
  local u
  u=$(conf_get "$CONF" AGENT_USER)
  [ -n "$u" ] && [ "$(id -u "$u" 2>/dev/null)" != 0 ] && id "$u" >/dev/null 2>&1 || return 1
  "$SETPRIV" --reuid="$(id -u "$u")" --regid="$(id -g "$u")" --clear-groups -- "$@"
}
# owned_by_root PATH: owned by ROOT_UID and writable by nobody else (no link)
owned_by_root() {
  [ ! -L "$1" ] || return 1
  [ -e "$1" ] || return 0
  local m
  m=$(stat -c '%u %a' "$1") || return 1
  [ "${m%% *}" = "$ROOT_UID" ] || return 1
  case "${m##* }" in *[2367]?|*[2367]) return 1 ;; esac
  return 0
}
# layout_ok: every path root reads code or config from, or writes to, is root's alone
layout_ok() {
  local d
  for d in "$ROOT" "$ROOT/bin" "$ROOT/config" "$ROOT/tools" "$ROOT/tools/host" "$STATE" "$CONF" "$SIGNERS" \
    "$ROOT/tools/host/update.sh" "$ROOT/bin/dc-host" "$ROOT/bin/dc-dserver" "$ROOT/bin/dc-server-rom" "$ROOT/bin/COMMIT.txt"; do
    owned_by_root "$d" || { say "update: $d is not root's alone (owner or mode): run update.sh --setup again as root (docs/HOST.md)"; return 1; }
  done
}
# offer_message COMMIT TARBALL SHA256 SIZE SEQ: the bytes tools/vps/deploy_from_git.sh signs
offer_message() {
  printf 'decombat-host-update 1\ncommit=%s\ntarball=%s\nsha256=%s\nsize=%s\nseq=%s\n' "$@"
}
# secure_layout AGENT: the root-run paths to root, the agent's own folders to the agent (both setups)
secure_layout() {
  local a=$1 g d
  g=$(id -gn "$a")
  mkdir -p "$ROOT/tools/host" "$STATE" "$ROOT/logs" "$ROOT/logs/content" "$ROOT/packs" "$ROOT/incoming"
  chown root:root "$ROOT" && chmod 0755 "$ROOT"
  for d in bin config tools update rom; do
    [ -e "$ROOT/$d" ] || continue
    chown -R -h root:root "$ROOT/$d" && chmod -R go-w "$ROOT/$d"
  done
  [ ! -f "$ROOT/config/host.toml" ] || { chown root:"$g" "$ROOT/config/host.toml" && chmod 0640 "$ROOT/config/host.toml"; }
  for d in logs packs incoming; do chown -R -h "$a:$g" "$ROOT/$d"; done
}

setup() {
  local url=$1 signer=${2:-} agent=${3:-} self
  [ "$(id -u)" -eq 0 ] || [ "${DC_TEST:-0}" = 1 ] || { say "run --setup as root"; exit 1; }
  [ -d "$ROOT/bin" ] && [ -f "$ROOT/config/host.toml" ] || { say "no host at $ROOT: the first install goes through tools/host/push.sh"; exit 1; }
  case "$url" in https://*/*|lobby) ;; *) say "--pointer-url must be https"; exit 1 ;; esac
  # the agent's user: given, else the one the old layout gave $ROOT/logs (tools/host/push.sh)
  [ -n "$agent" ] || agent=$(stat -c %U "$ROOT/logs" 2>/dev/null || true)
  [ -n "$agent" ] && [ "$agent" != root ] && id "$agent" >/dev/null 2>&1 || { say "name the agent's user: --agent-user NAME"; exit 1; }
  if [ "$url" = lobby ]; then
    if [ -n "$signer" ]; then
      printf '%s' "$signer" | grep -Eqx "$SIGN_ID ssh-ed25519 [A-Za-z0-9+/]+={0,2}" \
        || { say "--signer must be the line deploy_setup.sh prints: $SIGN_ID ssh-ed25519 AAAA..."; exit 1; }
    elif [ ! -f "$SIGNERS" ]; then
      say "--signer \"$SIGN_ID ssh-ed25519 ...\" is required (tools/vps/deploy_setup.sh prints it)"; exit 1
    fi
  fi
  [ "$(id -u)" -ne 0 ] || secure_layout "$agent"
  mkdir -p "$ROOT/tools/host" "$STATE" "$ROOT/config"
  self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
  if [ "$self" != "$ROOT/tools/host/update.sh" ]; then
    install -m 0755 "$0" "$ROOT/tools/host/.update.sh.new" && mv -f "$ROOT/tools/host/.update.sh.new" "$ROOT/tools/host/update.sh"
  fi
  if [ -n "$signer" ]; then
    printf '%s\n' "$signer" > "$SIGNERS.tmp" && chmod 0644 "$SIGNERS.tmp" && mv -f "$SIGNERS.tmp" "$SIGNERS"
    say "pinned the deploy signer in $SIGNERS"
  fi
  if [ "$url" = lobby ]; then
    # the owner asked for the switch: an update.conf of the URL source is kept beside as update.conf.url
    if [ -f "$CONF" ] && [ "$(conf_get "$CONF" SOURCE)" != lobby ]; then mv -f "$CONF" "$CONF.url"; fi
    if [ ! -f "$CONF" ]; then
      printf 'SOURCE=lobby\nINCOMING=%s/incoming\n' "$ROOT" > "$CONF"
      chmod 0644 "$CONF"
    fi
    mkdir -p "$ROOT/incoming"
  elif [ -f "$CONF" ]; then
    say "keeping $CONF"
  else
    printf 'POINTER_URL=%s\n' "$url" > "$CONF"
    chmod 0644 "$CONF"
  fi
  if [ "$(conf_get "$CONF" AGENT_USER)" != "$agent" ]; then
    grep -v '^AGENT_USER=' "$CONF" > "$CONF.tmp"; printf 'AGENT_USER=%s\n' "$agent" >> "$CONF.tmp"
    chmod 0644 "$CONF.tmp" && mv -f "$CONF.tmp" "$CONF"
  fi
  cat > "$SYSTEMD_DIR/decombat-host-update.service" <<UNITFILE
[Unit]
Description=DeCombat game host: pull the desired server build
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/bin/env bash $ROOT/tools/host/update.sh
TimeoutStartSec=30min
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=$ROOT
ProtectHome=yes
PrivateTmp=yes
UNITFILE
  cat > "$SYSTEMD_DIR/decombat-host-update.timer" <<UNITFILE
[Unit]
Description=DeCombat game host: check for a new server build

[Timer]
OnBootSec=2min
OnUnitActiveSec=5min
RandomizedDelaySec=60

[Install]
WantedBy=timers.target
UNITFILE
  "$SYSTEMCTL" daemon-reload && "$SYSTEMCTL" enable --now decombat-host-update.timer
  say "the timer runs; journalctl -u decombat-host-update shows each check, $ROOT/tools/host/update.sh --status the last outcome"
}

FORCE=0
case "${1:-}" in
  --help|-h) sed -n '2,28p' "$0"; exit 0 ;;
  --status) cat "$STATE/status" 2>/dev/null || say "no check yet"; exit 0 ;;
  --release-hold) rm -f "$STATE/hold"; say "hold released"; exit 0 ;;
  --force) FORCE=1 ;;
  --setup)
    shift
    MODE="" SIGNER="" AGENT=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --from-lobby) MODE=lobby; shift ;;
        --pointer-url) [ -n "${2:-}" ] || { say "--pointer-url URL"; exit 2; }; MODE=$2; shift 2 ;;
        --signer) [ -n "${2:-}" ] || { say "--signer LINE"; exit 2; }; SIGNER=$2; shift 2 ;;
        --agent-user) [ -n "${2:-}" ] || { say "--agent-user NAME"; exit 2; }; AGENT=$2; shift 2 ;;
        *) say "unknown setup option $1 (--help)"; exit 2 ;;
      esac
    done
    [ -n "$MODE" ] || { say "--setup --pointer-url URL, or --setup --from-lobby --signer LINE"; exit 2; }
    setup "$MODE" "$SIGNER" "$AGENT"; exit 0 ;;
  '') ;;
  *) say "unknown option $1 (--help)"; exit 2 ;;
esac

# root writes nothing (not even its status) under a path someone else could have replaced
if [ "$(id -u)" -eq 0 ] || [ "${DC_UPDATE_CHECK_LAYOUT:-0}" = 1 ]; then
  layout_ok || exit 1
fi
mkdir -p "$STATE"
if command -v flock >/dev/null 2>&1; then
  exec 9> "$STATE/lock"
  flock -n 9 || { say "update: another check is running"; exit 0; }
fi
DESIRED=""
[ -x "$ROOT/bin/dc-host" ] || { report failed "no $ROOT/bin/dc-host: the first install goes through tools/host/push.sh"; exit 1; }
if [ -f "$STATE/hold" ] && [ $FORCE -eq 0 ]; then
  report held "a by-hand deploy holds this host on its build (update.sh --release-hold resumes)"
  exit 0
fi

SOURCE=$(conf_get "$CONF" SOURCE)
if [ "$SOURCE" = lobby ]; then
  INCOMING=$(conf_get "$CONF" INCOMING)
  INCOMING=${INCOMING:-$ROOT/incoming}
  URL=$INCOMING/desired.txt
  [ -f "$URL" ] || { report up-to-date "the lobby has offered no build yet ($URL)"; exit 0; }
  # an offer older than the installed build (a by-hand deploy since) is stale: it would take the host back
  if [ $FORCE -eq 0 ] && [ -f "$ROOT/bin/COMMIT.txt" ] && [ "$URL" -ot "$ROOT/bin/COMMIT.txt" ]; then
    rm -f "$URL"
    report up-to-date "the offered build is older than the installed one (installed by hand since); offer dropped"
    exit 0
  fi
else
  URL=$(conf_get "$CONF" POINTER_URL)
  case "$URL" in
    https://*/*) ;;
    *) report failed "no https POINTER_URL in $CONF"; exit 1 ;;
  esac
fi
WORK=$(mktemp -d "$STATE/work.XXXXXX") || { report failed "no temp folder under $STATE"; exit 1; }
trap 'rm -rf "${WORK:?}"' EXIT
if [ "$SOURCE" = lobby ]; then
  # fetch MAX OUT FILE: root's copy of the agent's file, never through a link (cp -P copies a link as a link, refused
  # after), a FIFO cannot hold it up past the time-out; every check below reads the copy
  fetch() { [ -f "$3" ] && [ ! -L "$3" ] && timeout "$1" cp -P -- "$3" "$2" && [ -f "$2" ] && [ ! -L "$2" ]; }
else
  fetch() { "$CURL" --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL --retry 2 --connect-timeout 20 --max-time "$1" -o "$2" "$3"; }
fi

# ---- the pointer: three checked fields; the download address is built here, not taken from it
fetch 60 "$WORK/desired.txt" "$URL" || { report failed "could not read the pointer $URL"; exit 1; }
DESIRED=$(conf_get "$WORK/desired.txt" commit)
TARBALL=$(conf_get "$WORK/desired.txt" tarball)
WANT_SHA=$(conf_get "$WORK/desired.txt" sha256)
valid() { printf '%s' "$2" | grep -Eqx "$1"; }
if ! valid '[0-9a-f]{40}' "$DESIRED" || ! valid 'decombat-servers-linux-x86_64-[0-9A-Za-z._-]+\.tar\.gz' "$TARBALL" || ! valid '[0-9a-f]{64}' "$WANT_SHA"; then
  DESIRED=""
  report failed "the pointer is malformed"
  exit 1
fi
SEQ=""
if [ "$SOURCE" = lobby ]; then
  # HA-2: signed by the deploy key pinned at the setup, and newer than every offer taken before
  SIZE=$(conf_get "$WORK/desired.txt" size)
  SEQ=$(conf_get "$WORK/desired.txt" seq)
  SIG=$(conf_get "$WORK/desired.txt" sig)
  sig_ok() { [ "${#1}" -ge 16 ] && [ "${#1}" -le 8192 ] && case "$1" in *[!A-Za-z0-9+/=]*) return 1 ;; esac; }
  if ! valid '[0-9]{1,12}' "$SIZE" || ! valid '[0-9]{1,18}' "$SEQ" || ! sig_ok "$SIG"; then
    rm -f "$URL"; DESIRED=""
    report failed "the offer has no valid size, sequence number or signature; dropped"
    exit 1
  fi
  [ -f "$SIGNERS" ] || { report failed "no $SIGNERS: run update.sh --setup --from-lobby --signer LINE"; exit 1; }
  offer_message "$DESIRED" "$TARBALL" "$WANT_SHA" "$SIZE" "$SEQ" > "$WORK/offer.msg"
  printf '%s' "$SIG" | base64 -d > "$WORK/offer.sig" 2>/dev/null
  if ! "$SSHKEYGEN" -Y verify -f "$SIGNERS" -I "$SIGN_ID" -n "$SIGN_NS" -s "$WORK/offer.sig" < "$WORK/offer.msg" >/dev/null 2>&1; then
    rm -f "$URL"
    report failed "the offer's signature does not verify against $SIGNERS; dropped, nothing installed"
    exit 1
  fi
  LAST_SEQ=""
  [ -f "$STATE/lobby_seq" ] && LAST_SEQ=$(tr -cd '0-9' < "$STATE/lobby_seq")
  if [ -n "$LAST_SEQ" ] && [ "$SEQ" -le "$LAST_SEQ" ]; then
    rm -f "$URL"
    report failed "the offer's sequence number $SEQ is not above the last taken $LAST_SEQ (an old or replayed offer); dropped"
    exit 1
  fi
fi
took_seq() { [ -z "$SEQ" ] || { printf '%s\n' "$SEQ" > "$STATE/lobby_seq.tmp" && mv -f "$STATE/lobby_seq.tmp" "$STATE/lobby_seq"; }; }
if same_commit "$(installed_commit)" "$DESIRED"; then
  # SOURCE=lobby: the offer is used up (the agent fetches a new one when the lobby offers another build)
  [ "$SOURCE" != lobby ] || { took_seq; rm -f "$URL"; }
  report up-to-date "running the desired build"
  exit 0
fi
if [ $FORCE -eq 0 ] && [ "$(cat "$STATE/failed_commit" 2>/dev/null)" = "$DESIRED" ]; then
  report failed "this commit failed here before; waiting for a new pointer (or --force)"
  exit 1
fi
if [ $FORCE -eq 0 ] && matches_running; then
  report waiting "a match is running; trying again at the next check"
  exit 0
fi

# ---- the download, verified twice: the tarball against the pointer, the binaries against the tarball's list
fetch 1200 "$WORK/$TARBALL" "${URL%/*}/$TARBALL" || { report failed "could not download $TARBALL"; exit 1; }
GOT_SHA=$(sha256sum "$WORK/$TARBALL" | cut -d' ' -f1)
[ "$GOT_SHA" = "$WANT_SHA" ] || { report failed "$TARBALL has SHA-256 $GOT_SHA, the pointer says $WANT_SHA; nothing installed"; exit 1; }
TOP=${TARBALL%.tar.gz}
# a CI tarball has one top folder; no entry may leave it
if tar -tzf "$WORK/$TARBALL" | grep -Fv "$TOP/" | grep -q . || tar -tzf "$WORK/$TARBALL" | grep -Eq '(^|/)\.\.(/|$)'; then
  report failed "$TARBALL holds entries outside $TOP/; nothing installed"
  exit 1
fi
# and files and folders only (no link, device or fifo)
if tar -tvzf "$WORK/$TARBALL" | cut -c1 | grep -qv '[-d]'; then
  report failed "$TARBALL holds an entry that is neither a file nor a folder; nothing installed"
  exit 1
fi
mkdir "$WORK/x" && tar -xzf "$WORK/$TARBALL" -C "$WORK/x" --no-same-owner || { report failed "could not unpack $TARBALL"; exit 1; }
PKG="$WORK/x/$TOP"
(cd "$PKG" && sha256sum -c --quiet SHA256SUMS >/dev/null 2>&1) || { report failed "$TARBALL fails its own SHA256SUMS; nothing installed"; exit 1; }
same_commit "$(sed -n 's/^commit: \([0-9a-f]*\).*/\1/p' "$PKG/COMMIT.txt" | head -n 1)" "$DESIRED" \
  || { report failed "$TARBALL is not the build of commit $DESIRED; nothing installed"; exit 1; }
refuse() { # a fault of this build, not of the moment: do not fetch it again at every check
  printf '%s\n' "$DESIRED" > "$STATE/failed_commit"
  report failed "$1"
  exit 1
}
for b in $BINS_REQUIRED; do
  [ -f "$PKG/bin/$b" ] || refuse "$TARBALL has no bin/$b; nothing installed"
done
for b in $BINS; do
  [ ! -L "$PKG/bin/$b" ] || refuse "bin/$b is a link; nothing installed"
  [ ! -f "$PKG/bin/$b" ] || is_elf "$PKG/bin/$b" || refuse "bin/$b is not a Linux binary; nothing installed"
done
# it runs on this machine's libraries, and the ROM here still fits it
as_owner "$PKG/bin/dc-host" --version >/dev/null 2>&1 || refuse "the new dc-host does not run here; nothing installed"
ROM=$(sed -n 's/^rom = "\(.*\)"$/\1/p' "$ROOT/config/host.toml" 2>/dev/null | head -n 1)
ROM=${ROM:-$ROOT/rom}
if [ -f "$ROM/server-rom.json" ] && [ -f "$PKG/bin/dc-server-rom" ]; then
  as_owner "$PKG/bin/dc-server-rom" --verify "$ROM" >/dev/null 2>&1 || refuse "the ROM at $ROM does not verify with the new build; nothing installed"
fi

# ---- the swap: the agent is down meanwhile, so no match can start on a half-installed host
if [ $FORCE -eq 0 ] && matches_running; then
  report waiting "a match started during the download; trying again at the next check"
  exit 0
fi
"$SYSTEMCTL" stop "$UNIT" || { report failed "could not stop $UNIT; nothing installed"; "$SYSTEMCTL" start "$UNIT" || true; exit 1; }
PREV="$STATE/prev"
rm -rf "${STATE:?}/prev" && mkdir -p "$PREV"
for b in $BINS; do
  [ ! -f "$ROOT/bin/$b" ] || cp -p "$ROOT/bin/$b" "$PREV/$b"
done
[ ! -f "$ROOT/bin/COMMIT.txt" ] || cp -p "$ROOT/bin/COMMIT.txt" "$PREV/COMMIT.txt"
place() { # place SRC NAME MODE: a new file beside the target in root's own bin/, then a rename
  rm -f "$ROOT/bin/.$2.new" || return 1
  if [ "$(id -u)" -eq 0 ]; then
    install -m "$3" -o root -g root "$1" "$ROOT/bin/.$2.new" || return 1
  else
    install -m "$3" "$1" "$ROOT/bin/.$2.new" || return 1
  fi
  mv -f "$ROOT/bin/.$2.new" "$ROOT/bin/$2"
}
rollback() {
  for f in "$PREV"/*; do
    [ -e "$f" ] || continue
    if [ "$(basename "$f")" = COMMIT.txt ]; then place "$f" COMMIT.txt 0644; else place "$f" "$(basename "$f")" 0755; fi
  done
  "$SYSTEMCTL" start "$UNIT" || true
}
SWAPPED=1
for b in $BINS; do
  [ ! -f "$PKG/bin/$b" ] || place "$PKG/bin/$b" "$b" 0755 || SWAPPED=0
done
{
  cat "$PKG/COMMIT.txt"
  echo "installed: $(date -u +%Y-%m-%dT%H:%M:%SZ) from $TARBALL"
  echo "tarball sha256: $GOT_SHA"
} > "$WORK/COMMIT.txt"
[ $SWAPPED -eq 0 ] || place "$WORK/COMMIT.txt" COMMIT.txt 0644 || SWAPPED=0
if [ $SWAPPED -eq 0 ]; then
  rollback
  report failed "could not write to $ROOT/bin; the old build is back"
  exit 1
fi
"$SYSTEMCTL" reset-failed "$UNIT" 2>/dev/null || true
if ! "$SYSTEMCTL" start "$UNIT"; then
  rollback
  refuse "$UNIT did not start on the new build; the old build is back"
fi
sleep "$SETTLE"
if ! "$SYSTEMCTL" is-active --quiet "$UNIT"; then
  "$SYSTEMCTL" stop "$UNIT" || true
  rollback
  refuse "$UNIT did not stay up on the new build; the old build is back"
fi
# the next update runs the updater of this build (the URL source only: under SOURCE=lobby a new updater comes with the
# operator's --setup, never from a tarball that came through the agent)
if [ "$SOURCE" != lobby ] && [ -f "$PKG/tools/host/update.sh" ] && mkdir -p "$ROOT/tools/host"; then
  { rm -f "$ROOT/tools/host/.update.sh.new" && install -m 0755 "$PKG/tools/host/update.sh" "$ROOT/tools/host/.update.sh.new" \
    && mv -f "$ROOT/tools/host/.update.sh.new" "$ROOT/tools/host/update.sh"; } || say "update: could not refresh update.sh; the old one stays"
fi
rm -f "$STATE/failed_commit"
[ "$SOURCE" != lobby ] || { took_seq; rm -f "$URL"; }
report updated "now running commit $DESIRED"
