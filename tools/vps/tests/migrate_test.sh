#!/usr/bin/env bash
# migrate_test.sh: tests of tools/vps/migrate_nonroot.sh on a temporary root with stand-ins for every system command; above all
# that a dry run changes nothing and that the SSH step cannot run before the new login is proven.
#   bash tools/vps/tests/migrate_test.sh
# Runs under Git Bash on Windows and on Linux. Exit 0 = all passed.
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VPS=$(cd "$HERE/.." && pwd)
REPO=$(cd "$VPS/../.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/dc-migrate-test.XXXXXX")
trap 'rm -rf "$T"' EXIT

export DC_TEST=1 DC_ROOT="$T/opt" DC_SYSTEMD_DIR="$T/systemd" DC_ETC="$T/etc" DC_HOME_BASE="$T/home" DC_ROOT_KEYS="$T/root_keys"
unset DC_ADMIN DC_BOT_LOBBY DC_INSTANCE DC_UNIT SUDO_USER SSH_CONNECTION SSH_CLIENT

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL: %s\n' "$*"; }
check() { local d=$1; shift; if "$@"; then ok; else bad "$d"; fi; }
has() { if printf '%s\n' "$2" | grep -qF -- "$3"; then ok; else bad "$1: no '$3' in: $2"; fi; }
hasnt() { if printf '%s\n' "$2" | grep -qF -- "$3"; then bad "$1: '$3' in: $2"; else ok; fi; }

mkdir -p "$T/bin" "$T/opt/config" "$T/opt/tools" "$T/opt/data" "$T/systemd" "$T/etc/sudoers.d" "$T/etc/ssh/sshd_config.d" "$T/home" "$T/w/tools/vps" "$T/w/examples"
log() { :; }
cat > "$T/bin/stub" <<'S'
#!/usr/bin/env bash
n=$(basename "$0")
echo "$n $*" >> "$STUB_LOG"
case "$n" in
  id)
    # `id NAME`: a user exists when its home does, or it was created in this run
    for last; do :; done
    case "$last" in decombat-*) [ -d "$DC_HOME_BASE/$last" ] ;; *) exit 0 ;; esac ;;
  useradd) for last; do :; done; mkdir -p "$DC_HOME_BASE/$last" ;;
  ssh-keygen) grep -q '^ssh-' "$2" ;;
  visudo) [ -z "${STUB_VISUDO_BAD:-}" ] ;;
  sshd)
    case "$1" in
      -t) [ -z "${STUB_SSHD_BAD:-}" ] ;;
      -T) echo "permitrootlogin no"; echo "passwordauthentication no" ;;
    esac ;;
  sudo) echo "(ALL : ALL) NOPASSWD: ALL" ;;
  systemctl) case "$1" in is-enabled) exit 0 ;; esac ;;
  ufw)
    case "$1" in
      status) printf 'Status: active\n22/tcp ALLOW\nOpenSSH ALLOW\n10000/tcp ALLOW\n' ;;
    esac ;;
esac
S
chmod +x "$T/bin/stub"
for c in id useradd ssh-keygen visudo sshd sudo systemctl systemd-run ufw; do cp "$T/bin/stub" "$T/bin/$c"; done
export PATH="$T/bin:$PATH" STUB_LOG="$T/stub.log"
for s in migrate_nonroot common; do tr -d '\r' < "$VPS/$s.sh" > "$T/w/tools/vps/$s.sh"; done
tr -d '\r' < "$REPO/examples/decombat-hardening.conf" > "$T/w/examples/decombat-hardening.conf"
M="$T/w/tools/vps/migrate_nonroot.sh"
touch "$T/w/tools/vps/deploy_hosts.py"

echo "ssh-ed25519 AAAAdeploy deploy" > "$T/deploy.pub"
echo "ssh-ed25519 AAAAroot owner" > "$T/root_keys"
echo "#includedir /etc/sudoers.d" > "$T/etc/sudoers"
echo "Include /etc/ssh/sshd_config.d/*.conf" > "$T/etc/ssh/sshd_config"
printf '[server]\nauth_port = 10000\ngame_port = 27015\n[dserver]\nport_range = "27016-27100"\n' > "$T/opt/config/server.toml"
for u in decombat-lobby.service decombat-bots.service; do printf 'User=decombat\n' > "$T/systemd/$u"; done

snapshot() { (cd "$T" && find etc home systemd opt -type f -o -type d | sort; find etc home systemd -type f -exec cksum {} +) 2>/dev/null; }

# ---- dry runs change nothing
before=$(snapshot)
out=$(bash "$M" users --deploy-key "$T/deploy.pub" 2>&1); rc=$?
check "users dry run ok: $out" test "$rc" -eq 0
has "says it is a dry run" "$out" "dry run"
has "shows the sudoers" "$out" "deploy_remote.sh install"
hasnt "the deploy user's rule is not everything" "$(printf '%s\n' "$out" | grep 'decombat-deploy ALL')" "NOPASSWD: ALL"
out=$(bash "$M" harden 2>&1); rc=$?
check "harden dry run ok: $out" test "$rc" -eq 0
out=$(bash "$M" firewall 2>&1); rc=$?
check "firewall dry run ok" test "$rc" -eq 0
out=$(SUDO_USER=decombat-admin SSH_CONNECTION=x bash "$M" lock-ssh 2>&1)
out=$(bash "$M" check 2>&1); rc=$?
check "check ok: $out" test "$rc" -eq 0
has "check names who the units run as" "$out" "decombat-lobby.service: decombat"
check "the dry runs changed nothing" test "$before" = "$(snapshot)"
if grep -Eq '^(useradd|ufw allow|systemd-run|systemctl (reload|restart|daemon))' "$T/stub.log"; then bad "a dry run ran: $(cat "$T/stub.log")"; else ok; fi

# ---- the SSH step refuses before the new login is proven, in every way
: > "$T/stub.log"
out=$(bash "$M" lock-ssh --apply 2>&1); rc=$?
check "lock-ssh as root without sudo: refused" test "$rc" -ne 0
has "says why" "$out" "that proves the new login works"
out=$(SUDO_USER=root SSH_CONNECTION=x bash "$M" lock-ssh --apply 2>&1); rc=$?
check "lock-ssh via sudo from root: refused" test "$rc" -ne 0
out=$(SUDO_USER=decombat-admin bash "$M" lock-ssh --apply 2>&1); rc=$?
check "lock-ssh on a console, not over SSH: refused" test "$rc" -ne 0
out=$(SUDO_USER=decombat-admin SSH_CONNECTION=x bash "$M" lock-ssh --apply 2>&1); rc=$?
check "lock-ssh before the admin has a key: refused" test "$rc" -ne 0
check "no SSH change so far" test ! -f "$T/etc/ssh/sshd_config.d/00-decombat.conf"

# ---- users, with refusals first
out=$(bash "$M" users --apply 2>&1); rc=$?
check "no deploy key: refused" test "$rc" -ne 0
echo "not a key" > "$T/nokey"
out=$(bash "$M" users --deploy-key "$T/nokey" --apply 2>&1); rc=$?
check "a file with no key: refused" test "$rc" -ne 0
out=$(STUB_VISUDO_BAD=1 bash "$M" users --deploy-key "$T/deploy.pub" --apply 2>&1); rc=$?
check "visudo refuses: refused" test "$rc" -ne 0
check "visudo refuses: nothing installed" test ! -f "$T/etc/sudoers.d/decombat"
out=$(bash "$M" users --deploy-key "$T/deploy.pub" --apply 2>&1); rc=$?
check "users ok: $out" test "$rc" -eq 0
check "sudoers installed" test -f "$T/etc/sudoers.d/decombat"
has "the root key went to the admin" "$(cat "$T/home/decombat-admin/.ssh/authorized_keys")" "AAAAroot"
has "the deploy key went to the deployer" "$(cat "$T/home/decombat-deploy/.ssh/authorized_keys")" "AAAAdeploy"
hasnt "the root key is not the deployer's" "$(cat "$T/home/decombat-deploy/.ssh/authorized_keys")" "AAAAroot"
sudo_text=$(cat "$T/etc/sudoers.d/decombat")
has "the sudoers names the root-owned script" "$sudo_text" "/usr/bin/bash $T/opt/tools/deploy_remote.sh install"
hasnt "the sudoers allows nothing from the upload folder as a script" "$sudo_text" "bash $T/home"
out=$(bash "$M" users --deploy-key "$T/deploy.pub" --apply 2>&1); rc=$?
check "a second run is fine" test "$rc" -eq 0
check "a second run adds no duplicate key" test "$(grep -c AAAAdeploy "$T/home/decombat-deploy/.ssh/authorized_keys")" = 1

# ---- harden
: > "$T/stub.log"
out=$(bash "$M" harden --apply 2>&1); rc=$?
check "harden ok: $out" test "$rc" -eq 0
for u in decombat-lobby.service decombat-lobby@.service decombat-bots.service; do
    check "$u drop-in" test -f "$T/systemd/$u.d/10-hardening.conf"
done
has "reload" "$(cat "$T/stub.log")" "systemctl daemon-reload"
hasnt "no restart without --restart" "$(cat "$T/stub.log")" "systemctl restart"
has "the hardening writes nowhere itself" "$(cat "$T/systemd/decombat-lobby.service.d/10-hardening.conf")" "ProtectSystem=strict"
out=$(bash "$M" harden --restart --apply 2>&1)
has "--restart restarts" "$(cat "$T/stub.log")" "systemctl restart decombat-lobby"

# ---- firewall: only what is missing, never an enable
: > "$T/stub.log"
out=$(bash "$M" firewall --apply 2>&1); rc=$?
check "firewall ok: $out" test "$rc" -eq 0
log=$(cat "$T/stub.log")
has "the missing lobby port" "$log" "ufw allow 10002/tcp"
has "the game ports" "$log" "ufw allow 27016:27100/udp"
hasnt "what is there stays" "$log" "ufw allow 10000/tcp"
hasnt "never enabled" "$log" "ufw enable"
hasnt "never removed" "$log" "ufw delete"

# ---- lock-ssh: sshd refuses, then the good way, in order
: > "$T/stub.log"
GOOD="SUDO_USER=decombat-admin SSH_CONNECTION=x"
out=$(env $GOOD STUB_SSHD_BAD=1 bash "$M" lock-ssh --apply 2>&1); rc=$?
check "sshd -t refuses: refused" test "$rc" -ne 0
check "sshd -t refuses: the file is gone again" test ! -f "$T/etc/ssh/sshd_config.d/00-decombat.conf"
has "sshd -t refuses: the timer is stopped" "$(cat "$T/stub.log")" "systemctl stop decombat-ssh-rollback.timer"
hasnt "sshd -t refuses: no reload" "$(cat "$T/stub.log")" "systemctl reload"
: > "$T/stub.log"
out=$(env $GOOD bash "$M" lock-ssh --rollback-minutes 7 --apply 2>&1); rc=$?
check "lock-ssh ok: $out" test "$rc" -eq 0
has "root off" "$(cat "$T/etc/ssh/sshd_config.d/00-decombat.conf")" "PermitRootLogin no"
log=$(cat "$T/stub.log")
has "the rollback is armed" "$log" "--on-active=7m"
hasnt "reload, never restart" "$log" "systemctl restart"
armed=$(grep -n 'systemd-run' "$T/stub.log" | head -n 1 | cut -d: -f1)
checked=$(grep -n '^sshd -t' "$T/stub.log" | head -n 1 | cut -d: -f1)
reloaded=$(grep -n 'systemctl reload' "$T/stub.log" | head -n 1 | cut -d: -f1)
check "the order: arm ($armed), check ($checked), reload ($reloaded)" test "$armed" -lt "$checked" -a "$checked" -lt "$reloaded"
has "tells the next step" "$out" "confirm"

# ---- the rollback brings it back; confirm stops the timer
out=$(bash "$M" rollback-ssh 2>&1); rc=$?
check "rollback ok" test "$rc" -eq 0
check "rollback removes the file" test ! -f "$T/etc/ssh/sshd_config.d/00-decombat.conf"
out=$(bash "$M" confirm 2>&1); rc=$?
check "confirm without lock-ssh: refused" test "$rc" -ne 0
out=$(env $GOOD bash "$M" lock-ssh --apply 2>&1)
: > "$T/stub.log"
out=$(bash "$M" confirm 2>&1); rc=$?
check "confirm ok: $out" test "$rc" -eq 0
has "confirm stops the timer" "$(cat "$T/stub.log")" "systemctl stop decombat-ssh-rollback.timer"
check "confirm keeps the setting" test -f "$T/etc/ssh/sshd_config.d/00-decombat.conf"

# ---- an sshd_config without the include is left alone
echo "# no include" > "$T/etc/ssh/sshd_config"
rm -f "$T/etc/ssh/sshd_config.d/00-decombat.conf"
out=$(env $GOOD bash "$M" lock-ssh --apply 2>&1); rc=$?
check "no Include: refused" test "$rc" -ne 0
check "no Include: nothing written" test ! -f "$T/etc/ssh/sshd_config.d/00-decombat.conf"

printf 'migrate_test: %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
