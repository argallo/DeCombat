#!/usr/bin/env bash
# healthcheck.sh: is this box serving? From decombat-health.timer every 2 minutes, as root (docs/OPS.md "Alerts"). A check
# that changes state posts to the webhook of alerts.conf (Discord style, {"content": ...}). NOT RUN ON A LINUX VPS YET;
# tests: tools/vps/tests/healthcheck_test.sh.
# DC-DEFAULT: OPS-21 (proposed), see DC_DEFAULTS.md
#
#   sudo bash healthcheck.sh [--config FILE] [--print]
#
# For every enabled lobby instance: the unit is active, the auth port answers a TLS handshake, the web port answers over
# https, the certificate is not about to expire, and the loopback admin /status answers with each game host agent online
# (except HOSTS_OPTIONAL). For the box: decombat-bots active when enabled, the disk below DISK_MAX_PCT, a backup younger
# than BACKUP_MAX_HOURS. With a lobby that has the operator monitor (MON-5): its /ops/alerts checks, a match with
# OPS_SILENT_DROPS or more silent drops, OPS_ERRORS problem events or OPS_CRASHES crashed game servers within
# OPS_WINDOW_MIN minutes, and a game host on another build than the lobby (0 turns a check off). A failure counts after FAIL_AFTER runs in a row (a deploy's restart is no alarm); recovery
# posts at once. A post that fails is tried again next run. PING_URL (optional) is fetched at every run: a dead-man
# service that hears nothing tells you when the whole box is gone. --print shows the results and posts nothing.
# Exit 0: everything fine. 1: something failed.
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

CONF=${DC_ALERTS_CONF:-$DC_ROOT/config/alerts.conf}
PRINT=0
while [ $# -gt 0 ]; do
    case "$1" in
        --config) CONF=${2:?--config needs a file}; DC_ALERTS_CONF=$CONF; shift 2 ;;
        --print) PRINT=1; shift ;;
        -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done
need_root

cfg() { env_get "$CONF" "$1"; }
DISK_MAX=$(cfg DISK_MAX_PCT); DISK_MAX=${DISK_MAX:-90}
BACKUP_HOURS=$(cfg BACKUP_MAX_HOURS); BACKUP_HOURS=${BACKUP_HOURS:-30}
BACKUP_DIR=$(cfg BACKUP_DIR); BACKUP_DIR=${BACKUP_DIR:-$DC_ROOT/backups}
FAIL_AFTER=$(cfg FAIL_AFTER); FAIL_AFTER=${FAIL_AFTER:-2}
OPTIONAL=$(cfg HOSTS_OPTIONAL)
LABEL=$(cfg LABEL); LABEL=${LABEL:-$(hostname 2>/dev/null || echo server)}
STATE=$(cfg STATE_FILE); STATE=${STATE:-$DC_ROOT/logs/health.state}
PING=$(cfg PING_URL)
OPS_WINDOW=$(cfg OPS_WINDOW_MIN); OPS_WINDOW=${OPS_WINDOW:-10}
OPS_SILENT=$(cfg OPS_SILENT_DROPS); OPS_SILENT=${OPS_SILENT:-3}
OPS_ERRORS=$(cfg OPS_ERRORS); OPS_ERRORS=${OPS_ERRORS:-30}
OPS_CRASHES=$(cfg OPS_CRASHES); OPS_CRASHES=${OPS_CRASHES:-2}

RESULTS=""
record() { # NAME ok|fail TEXT
    RESULTS="$RESULTS$1"$'\t'"$2"$'\t'"$3"$'\n'
}

check_tls() { # NAME PORT
    if timeout 8 openssl s_client -connect "127.0.0.1:$2" -servername decombat </dev/null 2>/dev/null \
        | grep -q 'BEGIN CERTIFICATE'; then
        record "$1" ok "TLS answers on $2"
    else
        record "$1" fail "no TLS answer on port $2"
    fi
}

check_web() { # NAME PORT
    local code
    code=$(curl -sk --max-time 6 -o /dev/null -w '%{http_code}' "https://127.0.0.1:$2/server_list.json" 2>/dev/null || true)
    case "$code" in
        2??|3??|4??) record "$1" ok "https answers on $2 ($code)" ;;
        *) record "$1" fail "no https answer on port $2 (${code:-none})" ;;
    esac
}

check_cert() { # NAME DIR
    local c="$2/fullchain.pem"
    [ -f "$c" ] || c="$2/tls_cert.pem"
    [ -f "$c" ] || return 0
    if openssl x509 -in "$c" -noout -checkend 1209600 >/dev/null 2>&1; then
        record "$1" ok "certificate valid for more than 14 days"
    else
        record "$1" fail "certificate $c expires within 14 days"
    fi
}

check_admin() { # NAME ADDR ENVFILE
    local token body out
    token=$(env_get "$3" DC_ADMIN_TOKEN)
    body=$(curl -sS --max-time 5 ${token:+-H "X-Admin-Token: $token"} "http://$2/status" 2>/dev/null || true)
    out=$(printf '%s' "$body" | "$DC_PYTHON" -c '
import json, sys
try:
    d = json.load(sys.stdin)
except ValueError:
    print("noanswer")
    sys.exit(0)
skip = set(sys.argv[1].split())
print(" ".join(h.get("name", "?") for h in d.get("hosts", []) if not h.get("online") and h.get("name") not in skip) or "fine")
' "$OPTIONAL" 2>/dev/null || echo noanswer)
    case "$out" in
        fine) record "$1" ok "admin /status answers, game hosts online" ;;
        noanswer|"") record "$1" fail "admin /status on $2 does not answer" ;;
        *) record "$1" fail "game host offline: $out" ;;
    esac
}

check_ops() { # NAME ADDR ENVFILE: the monitor's own checks (MON-5); a lobby without /ops/alerts adds none
    local token body name status text
    token=$(env_get "$3" DC_ADMIN_TOKEN)
    body=$(curl -sS --max-time 5 ${token:+-H "X-Admin-Token: $token"}         "http://$2/ops/alerts?window=$OPS_WINDOW&silent=$OPS_SILENT&errors=$OPS_ERRORS&crashes=$OPS_CRASHES" 2>/dev/null || true)
    while IFS=$'	' read -r name status text; do
        [ -n "$name" ] && record "$1.$name" "$status" "$text"
    done < <(printf '%s' "$body" | "$DC_PYTHON" -c '
import json, sys
try:
    d = json.load(sys.stdin)
except ValueError:
    sys.exit(0)
if not isinstance(d, list):
    sys.exit(0)
for c in d:
    name = str(c.get("name", "?"))
    if name.startswith("host."):
        continue  # offline hosts: check_admin, which knows HOSTS_OPTIONAL
    text = " ".join(str(c.get("text", "")).split())
    print("%s	%s	%s" % ("".join(ch for ch in name if ch.isalnum() or ch in "._-"), "ok" if c.get("ok") else "fail", text))
' 2>/dev/null)
}

# ---- the checks
ANY=0
for n in $(list_instances || true); do
    instance_paths "$n"
    unit=$INST_UNIT envf=$INST_ENV
    systemctl is-enabled --quiet "$unit" 2>/dev/null || continue
    ANY=1
    if systemctl is-active --quiet "$unit"; then
        record "$n.unit" ok "$unit active"
    else
        record "$n.unit" fail "$unit is $(systemctl is-active "$unit" 2>/dev/null || true)"
    fi
    instance_ports "$n"
    instance_paths "$n"
    db=$(conf_get "$n" paths db); db=${db:-$INST_DATA/ca.db}
    a=$(env_get "$envf" DC_ADMIN); a=${a:-127.0.0.1:$P_ADMIN}
    check_tls "$n.auth" "$P_AUTH"
    check_web "$n.web" "$P_HTTP"
    check_cert "$n.cert" "$(dirname "$db")"
    check_admin "$n.admin" "$a" "$envf"
    check_ops "$n.ops" "$a" "$envf"
done
[ "$ANY" -eq 1 ] || record box.units fail "no enabled lobby unit"
if systemctl is-enabled --quiet decombat-bots 2>/dev/null; then
    if systemctl is-active --quiet decombat-bots; then record box.bots ok "decombat-bots active"
    else record box.bots fail "decombat-bots is not active"; fi
fi
pct=$(df -P "$DC_ROOT" 2>/dev/null | awk 'NR==2{print $5+0}')
if [ -z "$pct" ]; then record box.disk fail "df gave nothing for $DC_ROOT"
elif [ "$pct" -ge "$DISK_MAX" ]; then record box.disk fail "disk ${pct}% full (limit ${DISK_MAX}%)"
else record box.disk ok "disk ${pct}% full"; fi
if [ "$BACKUP_HOURS" != 0 ]; then
    if [ ! -f "$BACKUP_DIR/last_ok" ]; then
        record box.backup fail "no finished backup yet ($BACKUP_DIR/last_ok)"
    else
        age=$(( ($(date +%s) - $(stat -c %Y "$BACKUP_DIR/last_ok")) / 3600 ))
        if [ "$age" -ge "$BACKUP_HOURS" ]; then record box.backup fail "the last backup is $age hours old"
        else record box.backup ok "last backup $age hours ago"; fi
    fi
fi

if [ "$PRINT" -eq 1 ]; then
    printf '%s' "$RESULTS" | awk -F'\t' '{printf "%-14s %-5s %s\n", $1, $2, $3}'
    case "$RESULTS" in *$'\t'fail$'\t'*) exit 1 ;; *) exit 0 ;; esac
fi

# ---- what changed: the state file holds NAME <tab> ok|fail|told <tab> runs failed in a row
OLD=""
[ ! -f "$STATE" ] || OLD=$(cat "$STATE")
NEW=""
POSTS=""
FAILED=0
while IFS=$'\t' read -r name status text; do
    [ -n "$name" ] || continue
    prev=$(printf '%s\n' "$OLD" | awk -F'\t' -v n="$name" '$1==n{print $2 "\t" $3}')
    pstat=${prev%%$'\t'*}
    runs=${prev##*$'\t'}
    case "$runs" in ''|*[!0-9]*) runs=0 ;; esac
    if [ "$status" = ok ]; then
        if [ "$pstat" = told ]; then
            POSTS="$POSTS[OK] $LABEL: $text"$'\n'
        fi
        NEW="$NEW$name"$'\t'ok$'\t'0$'\n'
    else
        FAILED=1
        runs=$((runs + 1))
        if [ "$pstat" = told ]; then
            NEW="$NEW$name"$'\t'told$'\t'$runs$'\n'
        elif [ "$runs" -ge "$FAIL_AFTER" ]; then
            POSTS="$POSTS[DOWN] $LABEL: $text"$'\n'
            NEW="$NEW$name"$'\t'told$'\t'$runs$'\n'
        else
            NEW="$NEW$name"$'\t'fail$'\t'$runs$'\n'
        fi
    fi
done <<< "$RESULTS"
# a check that no longer runs (an instance disabled) and had been told about ends with a line of its own
while IFS=$'\t' read -r name pstat _; do
    [ -n "$name" ] && [ "$pstat" = told ] || continue
    printf '%s' "$RESULTS" | awk -F'\t' -v n="$name" '$1==n{f=1} END{exit !f}' \
        || POSTS="$POSTS[OK] $LABEL: $name is no longer checked"$'\n'
done <<< "$OLD"

if [ -n "$POSTS" ]; then
    if alert_post "$POSTS"; then
        printf '%s' "$POSTS"
    else
        printf 'healthcheck: the webhook post failed; trying again next run:\n%s' "$POSTS" >&2
        # keep the old state, so the same change is seen again; the fail counts stay where they were
        NEW=$OLD
        [ -z "$NEW" ] || NEW="$NEW"$'\n'
    fi
fi
mkdir -p "$(dirname "$STATE")"
printf '%s' "$NEW" > "$STATE.tmp" && mv -f "$STATE.tmp" "$STATE"
if [ -n "$PING" ]; then
    printf 'url = "%s"\n' "$PING" | curl -fsS --max-time 10 -o /dev/null -K - 2>/dev/null \
        || say "healthcheck: PING_URL did not answer"
fi
exit "$FAILED"
