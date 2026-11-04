#!/usr/bin/env bash
# bootstrap.sh: prepare an Ubuntu 22.04 / 24.04 VPS for a DeCombat server (docs/VPS.md). Idempotent: run it again
# after an update of the scripts. WRITTEN ON WINDOWS, NOT RUN ON LINUX YET.
#
#   sudo bash bootstrap.sh [--public-ip ADDRESS] [--apply-firewall]
#
# - creates the system user `decombat` (no login shell) and /opt/decombat/{bin,rom,data,logs,config,tools}
# - copies examples/server-vps.toml -> /opt/decombat/config/server.toml and examples/decombat.env ->
#   /opt/decombat/config/decombat.env, only when they do not exist yet (your edits are kept)
# - --public-ip ADDRESS: writes DC_SERVER__ADVERTISE_IP=ADDRESS into decombat.env (the address clients are told)
# - installs examples/decombat-lobby.service as /etc/systemd/system/decombat-lobby.service and the instance
#   template examples/decombat-lobby@.service (daemon-reload; the
#   unit is NOT enabled or started here: docs/VPS.md "First start")
# - copies these scripts to /opt/decombat/tools
# - prints the ufw rules for the DeCombat ports; --apply-firewall adds them (plus OpenSSH) but never enables ufw
# Nothing here holds an address, a key or a password: they come from the command line.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

PUBLIC_IP=""
APPLY_FW=0
while [ $# -gt 0 ]; do
    case "$1" in
        --public-ip) [ $# -ge 2 ] || die "--public-ip needs an address"; PUBLIC_IP=$2; shift 2 ;;
        --public-ip=*) PUBLIC_IP=${1#*=}; shift ;;
        --apply-firewall) APPLY_FW=1; shift ;;
        -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done

need_root

EXAMPLES=""
for d in "$HERE/../../examples" "$HERE/../examples" "$HERE/examples"; do
    if [ -f "$d/server-vps.toml" ]; then EXAMPLES=$(cd "$d" && pwd); break; fi
done
[ -n "$EXAMPLES" ] || die "examples/server-vps.toml not found (expected <dir>/tools/vps next to <dir>/examples)"

if [ -n "$PUBLIC_IP" ]; then
    # a dotted IPv4 address, not loopback, not unspecified (LG-31 refuses those anyway)
    if ! printf '%s' "$PUBLIC_IP" | grep -Eq '^([0-9]{1,3}\.){3}[0-9]{1,3}$'; then
        die "--public-ip '$PUBLIC_IP' is not a dotted IPv4 address"
    fi
    case "$PUBLIC_IP" in
        127.*|0.0.0.0) die "--public-ip must be the VPS public address, not $PUBLIC_IP" ;;
        10.*|192.168.*|172.1[6-9].*|172.2[0-9].*|172.3[01].*)
            say "warning: $PUBLIC_IP is a private address; players outside that network cannot reach it" ;;
    esac
fi

# 1. commands the scripts use (all in a default Ubuntu image; checked so a minimal image says what is missing)
missing=""
for c in systemctl tar curl install useradd sha256sum od; do
    command -v "$c" >/dev/null 2>&1 || missing="$missing $c"
done
if [ -n "$missing" ]; then
    say "missing commands:$missing"
    say "  apt-get update && apt-get install -y curl tar coreutils passwd systemd"
    exit 1
fi

# 2. the service user
if id "$DC_USER" >/dev/null 2>&1; then
    say "user $DC_USER exists"
else
    useradd --system --user-group --home-dir "$DC_ROOT" --no-create-home --shell /usr/sbin/nologin "$DC_USER"
    say "created system user $DC_USER"
fi

# 3. the folders: binaries, tools, config and ROM root-owned (the service only reads them), data and logs its own
install -d -m 0755 -o root -g root "$DC_ROOT" "$DC_ROOT/bin" "$DC_ROOT/tools"
install -d -m 0750 -o root -g "$DC_USER" "$DC_ROOT/config" "$DC_ROOT/rom"
install -d -m 0750 -o "$DC_USER" -g "$DC_USER" "$DC_ROOT/data" "$DC_ROOT/data/mods" "$DC_ROOT/logs" \
    "$DC_ROOT/logs/dserver"
say "folders under $DC_ROOT ready"

# 4. config: copied once; later runs keep the operator's edits
if [ ! -f "$DC_ROOT/config/server.toml" ]; then
    install -m 0640 -o root -g "$DC_USER" "$EXAMPLES/server-vps.toml" "$DC_ROOT/config/server.toml"
    say "wrote $DC_ROOT/config/server.toml (from examples/server-vps.toml)"
else
    say "kept $DC_ROOT/config/server.toml"
fi
ENVF="$DC_ROOT/config/decombat.env"
if [ ! -f "$ENVF" ]; then
    install -m 0640 -o root -g "$DC_USER" "$EXAMPLES/decombat.env" "$ENVF"
    say "wrote $ENVF (from examples/decombat.env)"
else
    say "kept $ENVF"
fi
# the files may come from Windows: drop carriage returns (systemd and the TOML reader would keep them)
sed -i 's/\r$//' "$DC_ROOT/config/server.toml" "$ENVF"
if [ -n "$PUBLIC_IP" ]; then
    if grep -Eq '^#?[[:space:]]*DC_SERVER__ADVERTISE_IP=' "$ENVF"; then
        sed -i -E "s|^#?[[:space:]]*DC_SERVER__ADVERTISE_IP=.*|DC_SERVER__ADVERTISE_IP=$PUBLIC_IP|" "$ENVF"
    else
        printf 'DC_SERVER__ADVERTISE_IP=%s\n' "$PUBLIC_IP" >> "$ENVF"
    fi
    say "advertised address set in $ENVF"
fi
if ! grep -Eq '^DC_SERVER__ADVERTISE_IP=[0-9]' "$ENVF"; then
    say "NOTE: no DC_SERVER__ADVERTISE_IP in $ENVF yet: run again with --public-ip <VPS public IPv4>;"
    say "      until then the lobby refuses to start (--public with a loopback address, LG-31)"
fi

# 5. the scripts themselves, so status.sh / rom_check.sh / install.sh are on the host
for f in common.sh bootstrap.sh install.sh rom_check.sh status.sh bots.sh instance.sh backup.sh restore.sh healthcheck.sh \
    migrate_nonroot.sh deploy_remote.sh deploy_hosts.py; do
    if [ -f "$HERE/$f" ] && [ "$HERE/$f" != "$DC_ROOT/tools/$f" ]; then
        install -m 0755 -o root -g root "$HERE/$f" "$DC_ROOT/tools/$f"
        sed -i 's/\r$//' "$DC_ROOT/tools/$f"
    fi
done
# what instance.sh create reads when it runs from $DC_ROOT/tools
install -d -m 0755 -o root -g root "$DC_ROOT/tools/examples"
for f in server-vps.toml decombat-lobby@.service; do
    if [ -f "$EXAMPLES/$f" ]; then
        install -m 0644 -o root -g root "$EXAMPLES/$f" "$DC_ROOT/tools/examples/$f"
        sed -i 's/\r$//' "$DC_ROOT/tools/examples/$f"
    fi
done
say "scripts in $DC_ROOT/tools"

# 6. the systemd unit (always refreshed; put local changes in a drop-in: systemctl edit decombat-lobby)
UNIT_DST="/etc/systemd/system/$DC_UNIT.service"
install -m 0644 -o root -g root "$EXAMPLES/decombat-lobby.service" "$UNIT_DST"
sed -i 's/\r$//' "$UNIT_DST"
systemctl daemon-reload
say "installed $UNIT_DST (not enabled here; docs/VPS.md 'First start': systemctl enable --now $DC_UNIT)"
# the template of further lobby instances (docs/VPS.md "Several instances on one box"; tools/vps/instance.sh)
if [ -f "$EXAMPLES/decombat-lobby@.service" ]; then
    install -m 0644 -o root -g root "$EXAMPLES/decombat-lobby@.service" "$DC_SYSTEMD_DIR/decombat-lobby@.service"
    sed -i 's/\r$//' "$DC_SYSTEMD_DIR/decombat-lobby@.service"
    systemctl daemon-reload
    say "installed $DC_SYSTEMD_DIR/decombat-lobby@.service (instances: tools/vps/instance.sh)"
fi
# the bots' unit (docs/VPS.md "Bots"): installed, never enabled here
if [ -f "$EXAMPLES/decombat-bots.service" ]; then
    install -m 0644 -o root -g root "$EXAMPLES/decombat-bots.service" /etc/systemd/system/decombat-bots.service
    sed -i 's/\r$//' /etc/systemd/system/decombat-bots.service
    systemctl daemon-reload
    say "installed /etc/systemd/system/decombat-bots.service (not enabled; docs/VPS.md 'Bots')"
fi

# OPS-22: the hardening of the service units, as a drop-in (examples/decombat-hardening.conf). OPS-20 / OPS-21: backup and
# health timers and their configs, installed, never enabled here (docs/OPS.md)
if [ -f "$EXAMPLES/decombat-hardening.conf" ]; then
    for u in decombat-lobby.service decombat-lobby@.service decombat-bots.service; do
        install -d -m 0755 "$DC_SYSTEMD_DIR/$u.d"
        install -m 0644 -o root -g root "$EXAMPLES/decombat-hardening.conf" "$DC_SYSTEMD_DIR/$u.d/10-hardening.conf"
        sed -i 's/\r$//' "$DC_SYSTEMD_DIR/$u.d/10-hardening.conf"
    done
fi
for u in decombat-backup.service decombat-backup.timer decombat-health.service decombat-health.timer; do
    [ -f "$EXAMPLES/$u" ] || continue
    install -m 0644 -o root -g root "$EXAMPLES/$u" "$DC_SYSTEMD_DIR/$u"
    sed -i 's/\r$//' "$DC_SYSTEMD_DIR/$u"
done
for f in backup.conf alerts.conf; do
    if [ -f "$EXAMPLES/$f" ] && [ ! -f "$DC_ROOT/config/$f" ]; then
        install -m 0640 -o root -g root "$EXAMPLES/$f" "$DC_ROOT/config/$f"
        sed -i 's/\r$//' "$DC_ROOT/config/$f"
    fi
done
systemctl daemon-reload
say "hardening drop-ins, backup and health timers installed (timers not enabled: docs/OPS.md)"

# 7. firewall: the operator owns it; print, apply only on request, never enable ufw from here
UFW_RULES=("allow OpenSSH")
for p in $DC_TCP_PORTS; do UFW_RULES+=("allow $p/tcp"); done
UFW_RULES+=("allow $DC_UDP_RANGE/udp")
say ""
say "firewall rules for DeCombat (TCP $DC_TCP_PORTS, UDP $DC_UDP_RANGE); the provider's firewall needs the same:"
for r in "${UFW_RULES[@]}"; do say "  ufw $r"; done
if [ "$APPLY_FW" -eq 1 ]; then
    command -v ufw >/dev/null 2>&1 || die "ufw is not installed (apt-get install -y ufw); rules not applied"
    for r in "${UFW_RULES[@]}"; do
        # shellcheck disable=SC2086
        ufw $r
    done
    if ufw status | grep -q 'Status: active'; then
        say "ufw rules added (ufw is active)"
    else
        say "ufw rules added; ufw is INACTIVE and stays so (enable it yourself with 'ufw enable' once SSH is allowed)"
    fi
else
    say "  (not applied; run again with --apply-firewall to add them)"
fi

# 8. journald
say ""
say "logs: journalctl -u $DC_UNIT -f   |   $DC_ROOT/logs/dc-lobby-server.log   |   $DC_ROOT/logs/dserver/"
say "to cap the journal on a small disk: SystemMaxUse=500M in /etc/systemd/journald.conf, then"
say "  systemctl restart systemd-journald"
say ""
say "next: install.sh <server tarball>, then install.sh --rom <server-rom.tgz> (docs/VPS.md)"
