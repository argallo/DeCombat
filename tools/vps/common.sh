# common.sh: shared settings of the DeCombat VPS scripts (docs/VPS.md). Sourced, not run. WRITTEN ON WINDOWS,
# NOT RUN ON LINUX YET.
# shellcheck shell=bash

DC_ROOT=${DC_ROOT:-/opt/decombat}
DC_USER=${DC_USER:-decombat}
DC_UNIT=${DC_UNIT:-decombat-lobby}
# set by the caller (environment or --admin): instance_select keeps it instead of the instance's env file value
_DC_ADMIN_GIVEN=${DC_ADMIN:+1}
_DC_BOT_LOBBY_GIVEN=${DC_BOT_LOBBY:+1}
DC_ADMIN=${DC_ADMIN:-127.0.0.1:10090}
DC_SYSTEMD_DIR=${DC_SYSTEMD_DIR:-/etc/systemd/system}
# the lobby instance the scripts act on (docs/VPS.md "Several instances on one box"): `default` is the one in
# $DC_ROOT itself (unit decombat-lobby), any other name lives in $DC_ROOT/instances/<name> (unit decombat-lobby@<name>)
DC_INSTANCE=${DC_INSTANCE:-default}
# DC_TEST=1: the shell tests (tools/vps/tests): no root check, no chown, no systemctl
DC_TEST=${DC_TEST:-0}
# the binaries a server tarball carries; the first two are required
DC_BINS_REQUIRED="dc-lobby-server dc-dserver"
DC_BINS_OPTIONAL="dc-server-rom dc-config dc-botclient dc-host"
# the ports of rust/HOSTING.md
DC_TCP_PORTS="10000 10002 10003 18080"
DC_UDP_RANGE="27015:27100"

# the interpreter of the small Python pieces (SQLite, JSON); Ubuntu has python3
DC_PYTHON=${DC_PYTHON:-python3}

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

need_root() {
    [ "$DC_TEST" = 1 ] && return 0
    [ "$(id -u)" -eq 0 ] || die "run as root (sudo bash $0 ...)"
}

# true when the file starts with the ELF magic (a Linux binary, not a Windows .exe or a text file)
is_elf() {
    [ "$(head -c 4 "$1" | od -An -tx1 | tr -d ' \n')" = "7f454c46" ]
}

# dir_mode MODE DIR...: the folders exist with that mode (a failed chmod only counts outside the tests: Git Bash on Windows)
dir_mode() {
    local m=$1 d
    shift
    for d in "$@"; do
        mkdir -p "$d"
        chmod "$m" "$d" 2>/dev/null || [ "$DC_TEST" = 1 ]
    done
}


# ---- backups and alerts (backup.sh, restore.sh, healthcheck.sh)

# runs a shell function of these scripts as the service user. setpriv, not sudo or runuser: no PAM, and it works under
# NoNewPrivileges. The function's own interpreter calls see the same environment.
as_service_user() {
    if [ "$DC_TEST" = 1 ] || [ "$(id -u)" -ne 0 ]; then
        "$@"
        return
    fi
    local fn=$1
    shift
    setpriv --reuid="$DC_USER" --regid="$DC_USER" --init-groups env DC_PYTHON="$DC_PYTHON" DC_TEST=1 \
        bash -c ". '$DC_ROOT/tools/common.sh'; $fn \"\$@\"" -- "$@"
}

# sqlite_snapshot SRC DST: a consistent copy of a live database through SQLite's online backup API, as one file (no
# -wal), checked. Prints "<tables> tables, <rows> rows, <accounts> accounts"; exit 1 with the reason otherwise.
sqlite_snapshot() {
    "$DC_PYTHON" - "$1" "$2" <<'PY'
import pathlib, sqlite3, sys
src, dst = sys.argv[1:3]
try:
    a = sqlite3.connect(pathlib.Path(src).resolve().as_uri() + "?mode=ro", uri=True, timeout=30)
    b = sqlite3.connect(dst)
    a.backup(b)
    a.close()
    b.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    b.execute("PRAGMA journal_mode=DELETE")
    b.commit()
    b.close()
except sqlite3.Error as e:
    print(f"sqlite: {e}")
    sys.exit(1)
PY
    [ $? -eq 0 ] || return 1
    sqlite_check "$2"
}

# sqlite_check FILE: opens a database, integrity_check, counts: the same line as sqlite_snapshot.
sqlite_check() {
    "$DC_PYTHON" - "$1" <<'PY'
import pathlib, sqlite3, sys
try:
    c = sqlite3.connect(pathlib.Path(sys.argv[1]).resolve().as_uri() + "?mode=ro", uri=True)
    r = c.execute("PRAGMA integrity_check").fetchone()[0]
    if r != "ok":
        print(f"integrity_check: {r}")
        sys.exit(1)
    tables = [n for (n,) in c.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")]
    if not tables:
        print("no tables")
        sys.exit(1)
    rows = sum(c.execute(f'SELECT count(*) FROM "{t}"').fetchone()[0] for t in tables)
    accounts = c.execute("SELECT count(*) FROM accounts").fetchone()[0] if "accounts" in tables else 0
    print(f"{len(tables)} tables, {rows} rows, {accounts} accounts")
except sqlite3.Error as e:
    print(f"sqlite: {e}")
    sys.exit(1)
PY
}

# alert_post TEXT: to WEBHOOK_URL of alerts.conf ({"content": ...}, Discord style; 2000 characters at most) and/or, with
# EMAIL_TO, as a mail through the default lobby's own [mail] settings (server.toml: smtp_host, smtp_port, user, from; the
# password from DC_MAIL__PASSWORD of decombat.env, else [mail] password). Returns 0 with neither configured, 1 when one
# that is configured fails. The webhook address goes through curl's stdin config and the mail password through the
# environment, so neither is on a command line. DC_ALERT_MAIL_FILE (tests) writes the mail there instead of sending it.
# DC-DEFAULT: OPS-24 (proposed) alerts by mail, see DC_DEFAULTS.md
alert_post() {
    local conf=${DC_ALERTS_CONF:-$DC_ROOT/config/alerts.conf} url body to rc=0
    url=$(env_get "$conf" WEBHOOK_URL)
    if [ -n "$url" ]; then
        body=$(printf '%s' "$1" | "$DC_PYTHON" -c 'import json,sys; print(json.dumps({"content": sys.stdin.read()[:1900]}))') || return 1
        printf 'url = "%s"\n' "$url" | curl -fsS --max-time 15 -o /dev/null -H 'Content-Type: application/json' -d "$body" -K - || rc=1
    fi
    to=$(env_get "$conf" EMAIL_TO)
    if [ -n "$to" ]; then
        alert_mail "$to" "$1" || rc=1
    fi
    return $rc
}

# alert_mail TO TEXT: one plain mail, subject "[DeCombat alert] <first line>" (see alert_post).
alert_mail() {
    local toml=${DC_ALERT_MAIL_TOML:-$DC_ROOT/config/server.toml} envf=${DC_ALERT_MAIL_ENV:-$DC_ROOT/config/decombat.env} pw
    pw=$(env_get "$envf" DC_MAIL__PASSWORD 2>/dev/null || true)
    ALERT_TEXT="$2" ALERT_TO="$1" ALERT_TOML="$toml" ALERT_PW="$pw" ALERT_FILE="${DC_ALERT_MAIL_FILE:-}" \
        "$DC_PYTHON" - <<'PY'
import os, smtplib, ssl, sys, tomllib
from email.message import EmailMessage
text = os.environ.get("ALERT_TEXT", "")[:20000]
try:
    with open(os.environ["ALERT_TOML"], "rb") as f:
        mail = tomllib.load(f).get("mail", {})
except (OSError, tomllib.TOMLDecodeError) as e:
    print(f"alert mail: cannot read the [mail] settings: {e}", file=sys.stderr)
    sys.exit(1)
host, port = mail.get("smtp_host", ""), int(mail.get("smtp_port", 587) or 587)
sender, user = mail.get("from", ""), mail.get("user", "")
password = os.environ.get("ALERT_PW") or mail.get("password", "")
to = [a.strip() for a in os.environ["ALERT_TO"].split(",") if a.strip()]
if not (host and sender and to):
    print("alert mail: [mail] smtp_host / from or EMAIL_TO is missing", file=sys.stderr)
    sys.exit(1)
m = EmailMessage()
first = (text.strip().splitlines() or ["alert"])[0][:120]
m["Subject"] = "[DeCombat alert] " + first
m["From"], m["To"] = sender, ", ".join(to)
m.set_content(text)
if os.environ.get("ALERT_FILE"):
    with open(os.environ["ALERT_FILE"], "a", encoding="utf-8") as f:
        f.write(m.as_string() + "\n")
    sys.exit(0)
try:
    ctx = ssl.create_default_context()
    if port == 465:
        s = smtplib.SMTP_SSL(host, port, timeout=20, context=ctx)
    else:
        s = smtplib.SMTP(host, port, timeout=20)
        s.starttls(context=ctx)
    with s:
        if user:
            s.login(user, password)
        s.send_message(m)
except (OSError, smtplib.SMTPException) as e:
    print(f"alert mail: {e.__class__.__name__}: {e}", file=sys.stderr)
    sys.exit(1)
PY
}

# ---- lobby instances (docs/VPS.md "Several instances on one box")

# a name: lower-case letter first, then a-z 0-9 _ -, at most 32 characters; `default` is the instance in $DC_ROOT
valid_instance_name() {
    case "$1" in
        default) return 0 ;;
        ''|*[!a-z0-9_-]*|[!a-z]*) return 1 ;;
    esac
    [ "${#1}" -le 32 ]
}

# instance_paths NAME: INST_DIR INST_CONF INST_ENV INST_DATA INST_LOGS INST_MODS INST_UNIT of that instance
instance_paths() {
    if [ "$1" = default ]; then
        INST_DIR=$DC_ROOT
        INST_CONF=$DC_ROOT/config/server.toml
        INST_ENV=$DC_ROOT/config/decombat.env
        INST_DATA=$DC_ROOT/data
        INST_LOGS=$DC_ROOT/logs
        INST_MODS=$DC_ROOT/data/mods
        INST_UNIT=decombat-lobby
    else
        INST_DIR=$DC_ROOT/instances/$1
        INST_CONF=$INST_DIR/config/server.toml
        INST_ENV=$INST_DIR/decombat.env
        INST_DATA=$INST_DIR/data
        INST_LOGS=$INST_DIR/logs
        INST_MODS=$INST_DIR/mods
        INST_UNIT=decombat-lobby@$1
    fi
}

# the names of the instances on this box: `default` when $DC_ROOT/config/server.toml exists, then each
# $DC_ROOT/instances/<name> that holds a config/server.toml
list_instances() {
    [ -f "$DC_ROOT/config/server.toml" ] && echo default
    local d
    for d in "$DC_ROOT"/instances/*/; do
        [ -f "${d}config/server.toml" ] || continue
        d=${d%/}
        echo "${d##*/}"
    done
}

# env_get FILE VAR: the value of the last uncommented VAR=... line (empty when none or unreadable)
env_get() {
    [ -r "$1" ] || return 0
    sed -n "s/^[[:space:]]*$2=//p" "$1" | tr -d '\r' | tail -n 1 | sed -e 's/^"//' -e 's/"$//'
}

# toml_get FILE SECTION KEY: the value of KEY in [SECTION] (quotes and trailing comments dropped), empty when absent.
# Enough for the flat keys of examples/server-vps.toml; not a TOML parser.
toml_get() {
    [ -r "$1" ] || return 0
    awk -v sec="$2" -v key="$3" '
        { sub(/\r$/, "") }
        /^[[:space:]]*\[/ { s = $0; sub(/^[[:space:]]*\[+[[:space:]]*/, "", s); sub(/[[:space:]]*\].*$/, "", s); cur = s; next }
        cur == sec {
            line = $0
            if (match(line, "^[[:space:]]*" key "[[:space:]]*=")) {
                v = substr(line, RLENGTH + 1)
                if (v ~ /^[[:space:]]*"/) { sub(/^[[:space:]]*"/, "", v); sub(/".*$/, "", v) }
                else { sub(/#.*/, "", v); gsub(/^[[:space:]]+|[[:space:]]+$/, "", v) }
                val = v
            }
        }
        END { print val }' "$1"
}

# conf_get NAME SECTION KEY: the instance's DC_<SECTION>__<KEY> env file override, else its server.toml value
conf_get() {
    instance_paths "$1"
    local var v
    var="DC_$(printf '%s__%s' "$2" "$3" | tr '[:lower:]' '[:upper:]')"
    v=$(env_get "$INST_ENV" "$var")
    [ -n "$v" ] || v=$(toml_get "$INST_CONF" "$2" "$3")
    printf '%s' "$v"
}

# instance_ports NAME: P_AUTH P_LOBBY P_CLAN P_HTTP P_ADMIN (TCP) and P_UDP ("lo-hi lo-hi": the first room's game
# port and the further rooms' range) of an instance, with the lobby's own rules for 0 (lobby = auth + 2,
# clan = lobby + 1, http = auth + 8080) and the schema defaults for absent keys
instance_ports() {
    local n=$1 v range
    v=$(conf_get "$n" server auth_port); P_AUTH=${v:-10000}
    v=$(conf_get "$n" server lobby_port); P_LOBBY=${v:-0}; [ "$P_LOBBY" != 0 ] || P_LOBBY=$((P_AUTH + 2))
    v=$(conf_get "$n" server clan_port); P_CLAN=${v:-0}; [ "$P_CLAN" != 0 ] || P_CLAN=$((P_LOBBY + 1))
    v=$(conf_get "$n" server http_port); P_HTTP=${v:-0}; [ "$P_HTTP" != 0 ] || P_HTTP=$((P_AUTH + 8080))
    v=$(conf_get "$n" server game_port); v=${v:-27888}
    range=$(conf_get "$n" dserver port_range); range=${range:-29901-29999}
    P_UDP="$v-$v $range"
    instance_paths "$n"
    v=$(env_get "$INST_ENV" DC_ADMIN)
    if [ -z "$v" ] && [ "$n" = default ] && [ -r "$DC_SYSTEMD_DIR/decombat-lobby.service" ]; then
        v=$(grep -o -- '--admin [^ ]*' "$DC_SYSTEMD_DIR/decombat-lobby.service" | head -n 1 | cut -d' ' -f2 || true)
    fi
    [ -n "$v" ] || v=127.0.0.1:10090
    P_ADMIN=${v##*:}
}

# instance_select NAME: point the scripts at an instance: INST_* paths, DC_UNIT, DC_ADMIN (the env file's DC_ADMIN
# unless the caller gave one), DC_BOT_LOBBY (the env file's, else 127.0.0.1:<auth_port>), DC_TCP_PORTS, DC_UDP_RANGE
instance_select() {
    valid_instance_name "$1" || die "not an instance name: '$1' (a-z first, then a-z 0-9 _ -)"
    instance_paths "$1"
    [ "$1" = default ] || [ -d "$INST_DIR" ] || die "no instance '$1' ($INST_DIR missing; tools/vps/instance.sh $1 create)"
    DC_INSTANCE=$1
    [ "$1" = default ] || DC_UNIT=$INST_UNIT
    instance_ports "$1"
    instance_paths "$1"
    [ -n "$_DC_ADMIN_GIVEN" ] || DC_ADMIN=127.0.0.1:$P_ADMIN
    if [ -z "$_DC_BOT_LOBBY_GIVEN" ]; then
        DC_BOT_LOBBY=$(env_get "$INST_ENV" DC_BOT_LOBBY)
        DC_BOT_LOBBY=${DC_BOT_LOBBY:-127.0.0.1:$P_AUTH}
    fi
    # an unreadable config (no sudo) keeps the defaults above rather than the schema's
    if [ -r "$INST_CONF" ]; then
        DC_TCP_PORTS="$P_AUTH $P_LOBBY $P_CLAN $P_HTTP"
        local last=${P_UDP##* }
        DC_UDP_RANGE="${P_UDP%%-*}:${last#*-}"
    fi
}
