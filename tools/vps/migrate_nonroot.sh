#!/usr/bin/env bash
# migrate_nonroot.sh: one-time move of a running VPS away from root SSH (docs/OPS.md "Non-root operation"). Every step prints
# what it would do unless --apply is given. NOT RUN ON A LINUX VPS YET; tests: tools/vps/tests/migrate_test.sh.
# DC-DEFAULT: OPS-22 (proposed), see DC_DEFAULTS.md
#
#   bash migrate_nonroot.sh check                                             what is true of this box now (changes nothing)
#   sudo bash migrate_nonroot.sh users --deploy-key FILE [--admin-key FILE] [--apply]
#   sudo bash migrate_nonroot.sh harden [--restart] [--apply]
#   sudo bash migrate_nonroot.sh firewall [--apply]
#   sudo bash migrate_nonroot.sh lock-ssh [--rollback-minutes N] [--apply]    FROM A SESSION OF THE NEW ADMIN USER
#   sudo bash migrate_nonroot.sh confirm                                      FROM A NEW SESSION, within N minutes
#   sudo bash migrate_nonroot.sh rollback-ssh
#
# The order keeps SSH access: root login stays on until the new admin user has PROVEN it can log in and use sudo, and
# lock-ssh refuses to run from anything but such a session. lock-ssh arms a timer first: unless `confirm` runs from a new
# session within N minutes (default 10) the SSH change undoes itself, so a mistake locks nobody out. `reload`, never
# `restart`: the sessions already open stay open. It never enables ufw and never removes a firewall rule.
#
# users     decombat-admin (full sudo, key login; for you), decombat-deploy (limited sudoers, key login; for the workflow).
#           --admin-key defaults to root's authorized_keys. The sudoers file passes `visudo -c` before it is installed.
# harden    the systemd drop-in examples/decombat-hardening.conf for the lobby, instance and bot units. Takes effect at the
#           next restart; --restart does it now, which ends running matches. Undo: delete the 10-hardening.conf files
#           and `systemctl daemon-reload`.
# firewall  compares the ports of every instance with `ufw status`; --apply adds the missing rules (OpenSSH first).
# lock-ssh  /etc/ssh/sshd_config.d/00-decombat.conf: PermitRootLogin no, PasswordAuthentication no, checked by `sshd -t`.
#
# What the deploy user can and cannot do: it runs deploy_remote.sh and restarts or reads the DeCombat units, and has no
# shell as root. But a deploy installs what the tarball holds, so whoever holds that key can still ship code that runs
# on the next deploy: treat DC_VPS_SSH_KEY as a production secret.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

DC_ETC=${DC_ETC:-/etc}
DC_HOME_BASE=${DC_HOME_BASE:-/home}
ADMIN_USER=${DC_ADMIN_USER:-decombat-admin}
DEPLOY_USER=${DC_DEPLOY_USER:-decombat-deploy}
SSH_DROP="$DC_ETC/ssh/sshd_config.d/00-decombat.conf"
SUDOERS="$DC_ETC/sudoers.d/decombat"
TIMER=decombat-ssh-rollback

STEP=${1:-}
[ $# -eq 0 ] || shift
APPLY=0; DEPLOY_KEY=""; ADMIN_KEY=""; RESTART=0; MINUTES=10
while [ $# -gt 0 ]; do
    case "$1" in
        --apply) APPLY=1; shift ;;
        --deploy-key) DEPLOY_KEY=${2:?--deploy-key needs a file}; shift 2 ;;
        --admin-key) ADMIN_KEY=${2:?--admin-key needs a file}; shift 2 ;;
        --restart) RESTART=1; shift ;;
        --rollback-minutes) MINUTES=${2:?--rollback-minutes needs a number}; shift 2 ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done

plan() { say "  [plan] $*"; }
done_() { say "  [done] $*"; }
run() { # DESCRIPTION COMMAND...: runs only with --apply
    local d=$1
    shift
    if [ "$APPLY" -eq 1 ]; then
        "$@"
        done_ "$d"
    else
        plan "$d"
    fi
}
mode_line() {
    if [ "$APPLY" -eq 1 ]; then say "applying"; else say "dry run: nothing changes (--apply does it)"; fi
}

valid_keys() { # FILE: one or more public keys, nothing else
    [ -s "$1" ] || return 1
    ssh-keygen -lf "$1" >/dev/null 2>&1
}

make_user() { # NAME KEYFILE
    local name=$1 keyfile=$2 home="$DC_HOME_BASE/$1"
    if id "$name" >/dev/null 2>&1; then
        say "  user $name exists"
    else
        run "create user $name, login shell /bin/bash, no password" useradd --create-home --home-dir "$home" --shell /bin/bash "$name"
    fi
    if [ "$APPLY" -eq 1 ]; then
        dir_mode 0700 "$home/.ssh"
        # appended, never replaced: a second run keeps what the owner added
        touch "$home/.ssh/authorized_keys"
        while IFS= read -r k; do
            [ -n "$k" ] || continue
            grep -qxF -- "$k" "$home/.ssh/authorized_keys" || printf '%s\n' "$k" >> "$home/.ssh/authorized_keys"
        done < "$keyfile"
        chmod 0600 "$home/.ssh/authorized_keys"
        if [ "$DC_TEST" != 1 ]; then chown -R "$name:$name" "$home/.ssh"; fi
        done_ "keys of $keyfile in $home/.ssh/authorized_keys"
    else
        plan "add the keys of $keyfile to $home/.ssh/authorized_keys"
    fi
}

sudoers_text() {
    local u=$DEPLOY_USER sc=/usr/bin/systemctl
    cat <<EOF
# DeCombat, written by tools/vps/migrate_nonroot.sh. The owner's login:
$ADMIN_USER ALL=(ALL:ALL) NOPASSWD: ALL
# The deploy workflow (.github/workflows/deploy-servers.yml, tools/vps/deploy_remote.sh): root-owned scripts only.
$u ALL=(root) NOPASSWD: /usr/bin/bash $DC_ROOT/tools/deploy_remote.sh install $DC_HOME_BASE/$u/decombat-upload/*
$u ALL=(root) NOPASSWD: /usr/bin/bash $DC_ROOT/tools/deploy_remote.sh hosts *
$u ALL=(root) NOPASSWD: /usr/bin/bash $DC_ROOT/tools/status.sh, /usr/bin/bash $DC_ROOT/tools/status.sh *
$u ALL=(root) NOPASSWD: $sc restart decombat-lobby, $sc restart decombat-lobby@*
$u ALL=(root) NOPASSWD: $sc status decombat-lobby, $sc status decombat-lobby@*
EOF
}

step_check() {
    say "== users"
    for u in "$ADMIN_USER" "$DEPLOY_USER"; do
        if id "$u" >/dev/null 2>&1; then say "  $u exists"; else say "  $u MISSING (step: users)"; fi
    done
    say "== units: who they run as (User= of the installed unit)"
    for f in "$DC_SYSTEMD_DIR"/decombat-*.service; do
        [ -f "$f" ] || continue
        say "  $(basename "$f"): $(sed -n 's/^User=//p' "$f" | head -n 1)"
    done
    say "== hardening drop-ins"
    for u in decombat-lobby.service decombat-lobby@.service decombat-bots.service; do
        if [ -f "$DC_SYSTEMD_DIR/$u.d/10-hardening.conf" ]; then say "  $u: yes"; else say "  $u: no (step: harden)"; fi
    done
    say "== sshd"
    if [ -f "$SSH_DROP" ]; then say "  $SSH_DROP present"; else say "  no $SSH_DROP yet (step: lock-ssh)"; fi
    if command -v sshd >/dev/null 2>&1; then
        sshd -T 2>/dev/null | grep -E '^(permitrootlogin|passwordauthentication) ' | sed 's/^/  /' || say "  (sshd -T needs root)"
    fi
    say "== firewall"
    step_firewall
}

step_users() {
    need_root
    mode_line
    [ -n "$DEPLOY_KEY" ] || die "--deploy-key FILE: the public key made for the workflow (docs/RELEASES.md)"
    valid_keys "$DEPLOY_KEY" || die "$DEPLOY_KEY holds no public key"
    [ -n "$ADMIN_KEY" ] || ADMIN_KEY="${DC_ROOT_KEYS:-/root/.ssh/authorized_keys}"
    valid_keys "$ADMIN_KEY" || die "$ADMIN_KEY holds no public key (--admin-key FILE)"
    grep -Eq '^[[:space:]]*(@includedir|#includedir)[[:space:]]+/etc/sudoers.d' "$DC_ETC/sudoers" \
        || die "$DC_ETC/sudoers does not read /etc/sudoers.d: nothing changed"
    make_user "$ADMIN_USER" "$ADMIN_KEY"
    make_user "$DEPLOY_USER" "$DEPLOY_KEY"
    if [ "$APPLY" -eq 1 ]; then
        dir_mode 0700 "$DC_HOME_BASE/$DEPLOY_USER/decombat-upload"
        if [ "$DC_TEST" != 1 ]; then chown "$DEPLOY_USER:$DEPLOY_USER" "$DC_HOME_BASE/$DEPLOY_USER/decombat-upload"; fi
    fi
    # the deploy script must be on the box, root-owned, for the sudoers rule to mean anything
    if [ "$APPLY" -eq 1 ] && [ "$HERE" != "$DC_ROOT/tools" ]; then
        for f in "$HERE"/*.sh "$HERE"/*.py; do
            install -m 0755 "$f" "$DC_ROOT/tools/$(basename "$f")"
            sed -i 's/\r$//' "$DC_ROOT/tools/$(basename "$f")"
        done
        done_ "these scripts copied to $DC_ROOT/tools"
    fi
    local tmp
    tmp=$(mktemp "${TMPDIR:-/tmp}/decombat-sudoers.XXXXXX")
    sudoers_text > "$tmp"
    if ! visudo -cf "$tmp" >/dev/null; then
        rm -f "$tmp"
        die "the sudoers text does not pass visudo: nothing installed"
    fi
    if [ "$APPLY" -eq 1 ]; then
        install -m 0440 "$tmp" "$SUDOERS"
        done_ "$SUDOERS installed, checked by visudo"
    else
        plan "install $SUDOERS (passes visudo):"
        sed 's/^/         /' "$tmp"
    fi
    rm -f "$tmp"
    say ""
    say "next, from your own machine, in a NEW terminal, with this root session left open:"
    say "  ssh -i <your key> $ADMIN_USER@<VPS_IP> 'sudo -n id -un'     must print root"
    say "  ssh -i <deploy key> $DEPLOY_USER@<VPS_IP> 'sudo -n bash $DC_ROOT/tools/status.sh --lines 1 | head -n 3'"
}

step_harden() {
    need_root
    mode_line
    local src=""
    for d in "$HERE/../../examples" "$HERE/../examples" "$HERE/examples"; do
        [ -f "$d/decombat-hardening.conf" ] && src="$d/decombat-hardening.conf" && break
    done
    [ -n "$src" ] || die "examples/decombat-hardening.conf not found next to this script"
    for u in decombat-lobby.service decombat-lobby@.service decombat-bots.service; do
        if [ "$APPLY" -eq 1 ]; then
            dir_mode 0755 "$DC_SYSTEMD_DIR/$u.d"
            install -m 0644 "$src" "$DC_SYSTEMD_DIR/$u.d/10-hardening.conf"
            sed -i 's/\r$//' "$DC_SYSTEMD_DIR/$u.d/10-hardening.conf"
            done_ "$u.d/10-hardening.conf"
        else
            plan "install $u.d/10-hardening.conf"
        fi
    done
    run "systemctl daemon-reload" systemctl daemon-reload
    if [ "$RESTART" -eq 1 ]; then
        for n in $(list_instances || true); do
            instance_paths "$n"
            if systemctl is-enabled --quiet "$INST_UNIT" 2>/dev/null; then
                run "restart $INST_UNIT (running matches end)" systemctl restart "$INST_UNIT"
            fi
        done
    else
        say "  takes effect at the next restart; after it open a room and start a match: the game servers run under it too"
    fi
}

step_firewall() {
    local want=("OpenSSH") n p r active=0 rules=""
    for n in $(list_instances || true); do
        instance_ports "$n"
        for p in $P_AUTH $P_LOBBY $P_CLAN $P_HTTP; do want+=("$p/tcp"); done
        for r in $P_UDP; do want+=("${r/-/:}/udp"); done
    done
    say "  open in the provider's firewall AND ufw (10090-range admin ports stay closed: loopback only):"
    say "  also TCP auth+5 (10005) when [hosts] agents are configured"
    if command -v ufw >/dev/null 2>&1; then
        rules=$(ufw status 2>/dev/null || true)
        printf '%s\n' "$rules" | grep -q 'Status: active' && active=1
    fi
    for r in $(printf '%s\n' "${want[@]}" | sort -u); do
        if [ "$active" -eq 1 ] && ! printf '%s\n' "$rules" | grep -qF -- "$r"; then
            say "    $r  MISSING in ufw"
            if [ "$APPLY" -eq 1 ] && [ "$STEP" = firewall ]; then
                need_root
                ufw allow "$r"
                done_ "ufw allow $r"
            fi
        else
            say "    $r"
        fi
    done
    [ "$active" -eq 1 ] || say "  ufw is not active here; the provider's firewall is the one that counts (never enabled from here)"
}

step_lock_ssh() {
    need_root
    mode_line
    # the proof: this very session came in as the new admin user over SSH and got root through sudo
    if [ "${SUDO_USER:-}" != "$ADMIN_USER" ] || [ -z "${SSH_CONNECTION:-}${SSH_CLIENT:-}" ]; then
        die "run this from an SSH session of $ADMIN_USER (sudo bash ...): that proves the new login works. SUDO_USER='${SUDO_USER:-}'"
    fi
    case "$MINUTES" in ''|*[!0-9]*|0) die "--rollback-minutes must be at least 1" ;; esac
    valid_keys "$DC_HOME_BASE/$ADMIN_USER/.ssh/authorized_keys" || die "$ADMIN_USER has no authorized key"
    sudo -l -U "$ADMIN_USER" 2>/dev/null | grep -q 'NOPASSWD: ALL' || die "$ADMIN_USER has no full sudo: nothing changed"
    grep -Eq '^[[:space:]]*Include[[:space:]]+/etc/ssh/sshd_config.d/' "$DC_ETC/ssh/sshd_config" \
        || die "$DC_ETC/ssh/sshd_config does not read sshd_config.d: nothing changed, edit it by hand"
    # a firewall that would block the new session's port would have blocked this one too: not checked again here
    local me
    me=$(realpath "$0")
    run "arm the rollback: in $MINUTES minutes the SSH change undoes itself unless 'confirm' runs" \
        systemd-run --unit="$TIMER" --on-active="${MINUTES}m" /bin/bash "$me" rollback-ssh
    if [ "$APPLY" -eq 1 ]; then
        dir_mode 0755 "$(dirname "$SSH_DROP")"
        printf '# DeCombat: tools/vps/migrate_nonroot.sh lock-ssh\nPermitRootLogin no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n' > "$SSH_DROP"
        chmod 0644 "$SSH_DROP"
        if ! sshd -t; then
            rm -f "$SSH_DROP"
            systemctl stop "$TIMER.timer" 2>/dev/null || true
            die "sshd -t refused the configuration: removed again, nothing changed"
        fi
        systemctl reload ssh || systemctl reload sshd
        done_ "$SSH_DROP written, sshd reloaded; this session stays open"
        sshd -T 2>/dev/null | grep -E '^(permitrootlogin|passwordauthentication) ' | sed 's/^/  now: /' || true
        say ""
        say "now, in a NEW terminal: ssh $ADMIN_USER@<VPS_IP>, then: sudo bash $DC_ROOT/tools/migrate_nonroot.sh confirm"
        say "no confirm within $MINUTES minutes: root login comes back by itself"
    else
        plan "write $SSH_DROP (PermitRootLogin no, PasswordAuthentication no), check with sshd -t, reload sshd"
    fi
}

step_confirm() {
    need_root
    [ -f "$SSH_DROP" ] || die "lock-ssh has not run: nothing to confirm"
    systemctl stop "$TIMER.timer" 2>/dev/null || die "cannot stop the rollback timer: it may have fired; check $SSH_DROP"
    systemctl reset-failed "$TIMER.service" 2>/dev/null || true
    say "rollback timer stopped; root login stays off."
    say "next: GitHub secrets DC_VPS_USER=$DEPLOY_USER and the deploy key (docs/RELEASES.md); your own scripts that use root@ log in as $ADMIN_USER."
}

step_rollback() {
    need_root
    rm -f "$SSH_DROP"
    systemctl reload ssh 2>/dev/null || systemctl reload sshd
    say "removed $SSH_DROP, sshd reloaded: the settings before lock-ssh are back"
}

case "$STEP" in
    check) step_check ;;
    users) step_users ;;
    harden) step_harden ;;
    firewall) mode_line; step_firewall ;;
    lock-ssh) step_lock_ssh ;;
    confirm) step_confirm ;;
    rollback-ssh) step_rollback ;;
    -h|--help|"") sed -n '2,30p' "$0" ;;
    *) die "unknown step: $STEP (see --help)" ;;
esac
