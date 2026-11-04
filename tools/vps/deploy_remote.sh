#!/usr/bin/env bash
# deploy_remote.sh: the VPS side of .github/workflows/deploy-servers.yml, as root (docs/RELEASES.md).
#
#   deploy_remote.sh install TARBALL SHA256 COMMIT [REQUIRED_INSTANCE...]
#   deploy_remote.sh hosts COMMIT [WAIT_SECONDS]
#
# install: checks the tarball, refuses before any change when a REQUIRED_INSTANCE's unit is not enabled, runs the
#   tarball's own install.sh (which restarts every enabled lobby instance), then proves each unit restarted and is
#   active, prints which commit each runs, and fails when the firewall rules differ from before (nothing here or in
#   install.sh edits them; the web ports stay as bootstrap.sh and the operator left them).
# hosts: asks the default lobby's admin endpoint which build each game host agent reports, for up to WAIT_SECONDS.
#   A host on another build is reported, not failed: it updates when its matches end (docs/HOST.md "Updates").
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"
INSTALL=${DC_DEPLOY_INSTALL:-$HERE/install.sh}
WAIT_UP=${DC_DEPLOY_WAIT:-30}
POLL=${DC_DEPLOY_POLL:-3}

stamp() { systemctl show -p ActiveEnterTimestampMonotonic --value "$1" 2>/dev/null; }
rules() { command -v ufw >/dev/null 2>&1 && ufw status numbered 2>/dev/null | sha256sum | cut -d' ' -f1; }

install_build() {
  local tgz=$1 want=$2 commit=$3; shift 3
  local got unit n units="" before_rules failed=0 stamps="" installed before waited ok
  got=$(sha256sum "$tgz" | cut -d' ' -f1)
  [ "$got" = "$want" ] || die "the tarball's SHA-256 is $got, expected $want; nothing changed"
  for n in "$@"; do
    valid_instance_name "$n" || die "not an instance name: $n"
    systemctl is-enabled --quiet "decombat-lobby@$n" || die "decombat-lobby@$n is not enabled on this box; nothing changed"
  done
  # every enabled lobby instance: install.sh restarts these same units
  for n in default $(list_instances | grep -vx default || true); do
    instance_paths "$n"
    unit=$INST_UNIT
    if systemctl is-enabled --quiet "$unit" 2>/dev/null; then units="$units $unit"; fi
  done
  [ -n "$units" ] || die "no enabled lobby unit on this box; nothing changed"
  before_rules=$(rules || true)
  for unit in $units; do stamps="$stamps $unit=$(stamp "$unit")"; done
  bash "$INSTALL" "$tgz" || die "install.sh failed; some units may still run the old build"
  installed=$(sed -n 's/^commit: \([0-9a-f]*\).*/\1/p' "$DC_ROOT/bin/COMMIT.txt" | head -n 1)
  [ "$installed" = "$commit" ] || die "$DC_ROOT/bin/COMMIT.txt says $installed, expected $commit"
  for unit in $units; do
    # shellcheck disable=SC2086
    before=$(printf '%s\n' $stamps | sed -n "s/^$unit=//p")
    waited=0; ok=0
    while [ "$waited" -le "$WAIT_UP" ]; do
      if systemctl is-active --quiet "$unit" && [ "$(stamp "$unit")" != "$before" ]; then ok=1; break; fi
      waited=$((waited + 3)); sleep "$POLL"
    done
    if [ $ok -eq 1 ]; then
      say "running: $unit commit $commit active"
    else
      say "NOT running: $unit did not restart onto commit $commit ($(systemctl is-active "$unit" 2>/dev/null))"
      failed=1
    fi
  done
  if [ "$(rules || true)" != "$before_rules" ]; then
    say "the firewall rules changed during this deploy: check them"
    failed=1
  fi
  [ $failed -eq 0 ] || die "the deploy did not finish cleanly"
}

report_hosts() {
  local commit=$1 wait=${2:-0} short body deadline token out rc
  short=${commit:0:8}
  instance_select default
  token=$(env_get "$INST_ENV" DC_ADMIN_TOKEN)
  deadline=$(( $(date +%s) + wait ))
  while :; do
    body=$(curl -sS --max-time 5 ${token:+-H "X-Admin-Token: $token"} "http://$DC_ADMIN/status" 2>/dev/null || true)
    out=$(printf '%s' "$body" | python3 "$HERE/deploy_hosts.py" "$short")
    rc=$?
    { [ $rc -ne 0 ] && [ "$(date +%s)" -lt "$deadline" ]; } || break
    sleep "${DC_DEPLOY_HOST_POLL:-20}"
  done
  printf '%s\n' "$out"
  [ $rc -eq 0 ] || say "not every host is on $short yet: a host updates at its next check with no match running (update.sh --status on it says why)"
}

need_root
case "${1:-}" in
  install) [ $# -ge 4 ] || die "install TARBALL SHA256 COMMIT [INSTANCE...]"; shift; install_build "$@" ;;
  hosts) [ $# -ge 2 ] || die "hosts COMMIT [WAIT_SECONDS]"; shift; report_hosts "$@" ;;
  -h|--help) sed -n '2,12p' "$0" ;;
  *) die "install or hosts (--help)" ;;
esac
