# SERVER.md: running a DeCombat server on its own

How to start, watch and stop `dc-lobby-server` without a client, as an operator. Firewall and router set-up for
players outside your network: [HOSTING.md](../HOSTING.md). Rules referred to here: LG-130 (graceful stop), LG-131
(admin endpoint), LG-132 (a room closes when its last player leaves), LG-185 / LG-186 (network view) in
[DC_DEFAULTS.md](../DC_DEFAULTS.md).

## 1. What you need

- `dc-lobby-server.exe` and `dc-dserver.exe` from one build, in one folder (`target\release` of
  `cargo build --release -p dc-lobby-server -p dc-dserver`).
- Your own 2009 Combat Arms client folder (the ROM). The server only reads it; nothing of the game is shipped.
- A `server.toml`: copy [examples/server.toml](../examples/server.toml) next to the exes and edit `[paths] rom`.
  Every other key of the full reference `crates/dc-config/server.example.toml` may be added.

The minimal flag set is `--config`, `--dserver` (start a game server per room) and `--dserver-exe` (the game server
is not found on its own). Everything else comes from the toml or has a built-in default:

| what | toml key | flag | default |
|---|---|---|---|
| ROM folder | `paths.rom` | env `CA_ROM` | none (needed for maps and modes) |
| database | `paths.db` | `--db` | `<exe dir>\data\ca.db` |
| log file | `paths.log_dir` (writes `dc-lobby-server.log`) | `--log FILE` | console only |
| listen address | `server.bind`, `server.public` | `--bind`, `--public` | 127.0.0.1 |
| address told to clients | `server.advertise_ip` | `--lobbyip`, `--gameip` | 127.0.0.1 |
| auth / lobby / clan TCP ports | `server.auth_port` / `lobby_port` / `clan_port` | `--port` / `--lobbyport` / `--clanport` | 10000 / 10002 / 10003 |
| clan web and mod downloads (TCP) | `server.http_port`, `server.http_enabled` | `--clanweb` | auth + 8080 |
| first room's game port (UDP) | `server.game_port` | `--gameport` | 27888 |
| further rooms' game ports (UDP) | `dserver.port_range` | none | 29901-29999 |
| admin endpoint | none | `--admin 127.0.0.1:10090` | off |
| stop grace | none | `--stop-grace SECS` | 10 |
| accounts | `auth.auto_create` | `--add-account NAME:PASSWORD`, `--no-auto-create` | auto-create on |

Precedence: flag > `DC_<SECTION>__<KEY>` environment variable > server.toml > `<db dir>\lobby.json` > built-in.
Relative paths in the toml are taken from the folder the server is started in.

## 2. Check the set-up

```
dc-config check server.toml --fs --rom "C:\Games\CombatArms"
dc-lobby-server doctor --config server.toml
```

`dc-config check` validates the toml (keys, values, ports, the mod set; `--fs` also checks that the folders exist: the server creates the database folder on its first start, so before that `--fs` reports it missing). `doctor`
resolves the same flags and environment as a real start and prints one OK / WARN / FAIL row per check: config,
listener ports free, ROM identification and table counts, the database (opened read-only), mods. Exit 0 = no FAIL.

## 3. Start

In a console (Ctrl+C stops it, section 5):

```
cd C:\DeCombat
dc-lobby-server.exe --config server.toml --dserver --dserver-exe C:\DeCombat\dc-dserver.exe --admin 127.0.0.1:10090
```

Or detached, with the helper script (hidden window, output in `logs\lobby_<time>.out` next to the toml, returns once
the admin endpoint answers):

```
powershell -NoProfile -ExecutionPolicy Bypass -File tools\server_start.ps1 -BinDir C:\DeCombat -Config C:\DeCombat\server.toml
```

Script options: `-Admin ADDR:PORT` (default 127.0.0.1:10090), `-Token SECRET`, `-WorkDir DIR` (default: the toml's
folder), `-Extra "--add-account alice:pw --stop-grace 20"`. It refuses to start a second server on the same admin
address.

The log's last start-up lines are `admin endpoint on http://127.0.0.1:10090 ...`, `dc-lobby-server: auth 10000 lobby
10002 ...` and `running; Ctrl+C stops the server ...`.

## 4. Status

```
curl http://127.0.0.1:10090/status
```

(`Invoke-RestMethod http://127.0.0.1:10090/status` in PowerShell.) JSON: `uptime_secs`, `lobby_logins` since the
start, `connections`, `online`, `db`, `rom`, `address` (bind, advertised addresses, ports), `game_servers`, and per
room `id` (`<channel>-<no>`), `title`, `state` (waiting / countdown / playing), `mode`, `map_index`, `players`,
`bots`, `max_players`, `game` (id, UDP port, pid, ready) or null.

The endpoint is loopback only. To reach it from another machine start with `--admin 0.0.0.0:10090 --admin-public
--admin-token SECRET`; every request then carries the token (`-H "X-Admin-Token: SECRET"`, `-H "Authorization:
Bearer SECRET"` or `?token=SECRET`). A token can be set on a loopback endpoint too.

## 4a. Network

Three tools show how the players' connections are doing, so a change (a new host, a VPS, a client or server update)
can be judged by numbers instead of by feel.

- **Scoreboard ping (CLI-1360).** The in-match scoreboard (Tab) shows each player's ping in a column left of the
  kills. The value is the one the game server puts in its ping table every 5 s (S2C 0xa2,
  the 2009 message; DSV-410: the server-measured round trip of the player's link). The 2009 board keeps the value but
  draws no column; the column is DeCombat's.
- **`GET /net` (LG-185).** Every player of every running match with the newest 10 s window its game server measured:

  ```
  curl http://127.0.0.1:10090/net
  powershell -NoProfile -ExecutionPolicy Bypass -File tools\server_net.ps1 [-Watch] [-Every 10]
  ```

  JSON `games` (running matches) and `players`, each with `game`, `room`, `client`, `account`, `nick`,
  `tcp_age_secs` (age of the player's lobby connection), `report_age_secs` (null until the first window arrived),
  `window_secs`, `rtt_ms` (`mean`, `min`, `max` of the window's samples, `samples`), `input_jitter_ms`, `input_rate`
  (player updates per second), `longest_gap_ms`, `resends`, `flow_drops`, `datagrams_in`, `datagrams_out`. The game
  server sends the window over the pipe every 10 s (`@net`, `specs/DSERVER_PIPE_addendum_net.md`). Same access rules
  as `/status` (loopback, or the token). `tools\server_net.ps1` prints it as a table, once or every 10 s.
- **`tools/net_report.py` (after a session).** Reads the logs a match left behind and prints one row per player:

  ```
  python -I tools/net_report.py <capture session folder>
  python -I tools/net_report.py --dserver dserver_c1_r0_g1.log friend/dc-client.net.jsonl host/dc-client.net.jsonl
  python -I tools/net_report.py --compare <session A> <session B>
  ```

  Server view (from the dserver log's `client N: link 10 s:` lines): round trip median / p95 / max, input jitter,
  longest input gap, resends and resend % (the loss proxy of server -> client reliable traffic), flow drops. Client
  view (from each `dc-client.net.jsonl` given): the client's own round-trip estimate, its resends, received
  duplicates, datagram rates, and from its `remote_interp` lines how smoothly it drew the OTHER players (holds and
  late samples per minute, arrival jitter q95, playout delay). `--compare` prints the change per player between two
  sessions. Ask remote players for their `logs\dc-client.net.jsonl` after a match; the dserver logs are in
  `--dserver-logdir`.

What is good: on loopback the round trip is 0-2 ms; a WAN player typically 30-150 ms. Watch the p95 and max rather
than the median (the friend match of 2026-10-29: median 116 ms, p95 276, max 1044), a resend % above 1-2, input
jitter above about 15 ms (the client then draws that player with a longer delay, CLI-1330) and holds per minute
above 0.

## 4b. Security

What crosses the network, and how (DC_DEFAULTS LG-220 .. LG-226, CLI-1470 .. CLI-1475; all of it is our own design,
the 2009 client had no encryption):

- **Encrypted (TLS 1.3):** the auth port (login name and password, nickname creation, server pick) and the web port
  (registration page, clan pages, mod downloads). Same port numbers as before: each port tells a TLS handshake from a
  2009 frame by its first bytes.
- **Plain:** the lobby port and the clan port (no password; they carry the session key from the server pick) and the
  game's UDP traffic.
- **Hashed:** passwords in the DB are Argon2id hashes (LG-140); the server never logs a password or the TLS key.
- **Who must use TLS:** on a public lobby (`--public`, or a non-loopback `--bind`) every non-loopback peer. A plain
  login from outside is closed before a frame is read; a plain browser request is redirected to `https://` (a form
  post is refused). Loopback peers (your own client and bots) may stay plain. `--tls loopback` requires TLS from every
  peer (the tests use it). `--plain-auth-public` lets the real 2009 client log in from outside
  without encryption: only on a network you trust; the lobby warns at every start.
- **Mail:** the password-recovery mails go out over SMTP with STARTTLS (port 587) or TLS (port 465); the SMTP
  password is never printed (`dc-config dump` shows `***`), recipients are logged masked (`r***@example.org`), a code
  only reaches the log in the dry run.
- **Same-host proxy:** A reverse proxy, tunnel or port forwarder on the same host as the lobby makes remote players look like loopback (the lobby judges the socket peer), so their passwords would cross the public leg in clear text. If one runs there, start the lobby with `--tls loopback`; the lobby logs this at start-up when `--public` is on and `--tls` is not `loopback`.

**Password recovery by e-mail** (LG-240 .. LG-245). New accounts give an e-mail address (registration page and the
game's dialog); `account add --email` / `account set-email` set it for others. Set up a mail account the lobby may send
from (your provider's SMTP, or a transactional mail service) in `server.toml`:

```toml
[mail]
smtp_host = "smtp.example.org"   # empty = recovery off
smtp_port = 587                  # 587 STARTTLS (required), 465 TLS from the first byte
user = "noreply@example.org"     # SMTP login
# password: put it in the environment as DC_MAIL__PASSWORD, not in the file
from = "DeCombat <noreply@example.org>"
subject_prefix = "[DeCombat]"
dry_run = false                  # true: write the mails into the lobby log, send nothing
```

The flags `--mail-smtp-host`, `--mail-smtp-port`, `--mail-user`, `--mail-password`, `--mail-from`,
`--mail-subject-prefix` and `--mail-dry-run` override them. The start-up log says `mail: <host>:<port> (STARTTLS)`,
`mail: dry run` or `mail: not set up`. Mails are sent by one background thread: a slow or unreachable mail server
delays only the mail (its failure is one `mail: ... failed` log line), never the lobby.

The flow, on the web port (JSON with `Accept: application/json`, else HTML pages): `GET /recover` says whether mail
is set up; `POST /recover/password` (`name`, `email`) mails an 8-character code when both match, and always answers
"If the account and the address match, a code was sent."; `POST /recover/password/confirm` (`name`, `code`, `pw`,
`pw2`) checks the password rules, then the code (any case, 15 minutes, 5 tries, one use; only its SHA-256 is
stored), and sets the new password; `POST /recover/id` (`email`) mails the login names that use the address. At most
3 codes per account and 3 login-ID mails per address an hour; the registration page's limit of 5 requests per
address in 10 minutes applies too.

**The certificate.** The first start makes a self-signed certificate and key next to the DB (`tls_cert.pem`,
`tls_key.pem`; another folder with `--tls-dir`). The key file is owner-only on Linux; keep it out of backups you
share and never commit it. Every start logs the fingerprint:

```
TLS: certificate <dir>\tls_cert.pem (loaded); SHA-256 fingerprint 92:01:28:...:13:C1
TLS: players pin this server with: --server-fingerprint 92:01:28:...:13:C1
```

`dc-lobby-server tls --db <ca.db>` prints the same fingerprint (and makes the pair if there is none yet).

**Replacing it.** Stop the lobby, put your own pair into the TLS folder and start it again. A CA-issued pair (for
example Let's Encrypt on a VPS with a DNS name) goes in as `fullchain.pem` + `privkey.pem` (used first when both
exist); any other pair replaces `tls_cert.pem` + `tls_key.pem`. Deleting both files makes a new self-signed pair at
the next start: every player who pinned or remembered the old one then sees the identity box below until they use the
new fingerprint, so give it to them first.

**How players trust the server.** dc-client checks the certificate in this order: the fingerprint given with
`--server-fingerprint` (or `DC_SERVER_FINGERPRINT`; the Windows friend pack's `pack.json` carries it when built with
`tools/friend_pack_win.py --fingerprint <fp>` or `--server-cert <tls_cert.pem>`); a CA-issued certificate valid for the name
typed in `--lobby`; else the first certificate seen for that address is remembered in the player's profile
(`%APPDATA%\DeCombat\known_servers.txt`). A later different certificate is refused with the box "The server's
identity has changed. Ask the host for the new fingerprint and start the game with --server-fingerprint." (the
console shows both fingerprints); the game goes on only with the new fingerprint on its command line. A server
without TLS is refused, except on 127.0.0.1 (old test servers).

## 5. Stop

Any of these runs the same graceful stop (LG-130):

- Ctrl+C or Ctrl+Break in the server's console, or closing the console window.
- `curl -X POST http://127.0.0.1:10090/stop`
- `powershell -NoProfile -ExecutionPolicy Bypass -File tools\server_stop.ps1` (`-Admin`, `-Token`, `-Wait SECS`;
  returns when the server is gone).

The server then refuses new connections, sends every running game server `quit` (the match ends and its result is
stored), waits up to `--stop-grace` seconds for the game servers, kills what is left, closes the client connections,
checkpoints and closes the database and exits with status 0. The log ends with `stopping: ...` lines and `stopped`.
Killing the process (Task Manager, `taskkill /F`) skips all of that; its game servers then stop on their own when
their control pipe closes.

To end one room's match and send its members back to the lobby without stopping the server:

```
curl -X POST http://127.0.0.1:10090/rooms/1-0/close
```

A room also closes on its own when its last player leaves (LG-132): bots (`Bot<digits>` names, `--test-bot-account`)
are sent back to the lobby and the room's game port is freed.

## 6. Keep it running

These commands are written out for you to run; nothing here runs them.

Scheduled task at logon (runs in your session, no admin rights needed):

```
schtasks /Create /TN "DeCombat server" /SC ONLOGON /RL LIMITED /TR "powershell -NoProfile -ExecutionPolicy Bypass -File C:\DeCombat\tools\server_start.ps1 -BinDir C:\DeCombat -Config C:\DeCombat\server.toml"
schtasks /Run /TN "DeCombat server"
schtasks /Delete /TN "DeCombat server" /F
```

Stop it with `server_stop.ps1` (ending the task does not stop the detached server).

Windows service with NSSM (elevated prompt; NSSM sends Ctrl+C on a service stop and waits `AppStopMethodConsole`
milliseconds, so keep that above `--stop-grace`):

```
nssm install DeCombat C:\DeCombat\dc-lobby-server.exe --config C:\DeCombat\server.toml --dserver --dserver-exe C:\DeCombat\dc-dserver.exe --admin 127.0.0.1:10090
nssm set DeCombat AppDirectory C:\DeCombat
nssm set DeCombat AppStdout C:\DeCombat\logs\service.out
nssm set DeCombat AppStderr C:\DeCombat\logs\service.out
nssm set DeCombat AppStopMethodConsole 20000
nssm set DeCombat Start SERVICE_AUTO_START
nssm start DeCombat
nssm stop DeCombat
nssm remove DeCombat confirm
```

Windows service with WinSW: put `WinSW-x64.exe` next to the exes as `DeCombat.exe` with this `DeCombat.xml`, then
`DeCombat.exe install`, `DeCombat.exe start`, `DeCombat.exe stop`, `DeCombat.exe uninstall` (elevated). The stop
command asks the admin endpoint; check the element names against the WinSW version you use.

```
<service>
  <id>DeCombat</id>
  <name>DeCombat server</name>
  <executable>%BASE%\dc-lobby-server.exe</executable>
  <arguments>--config "%BASE%\server.toml" --dserver --dserver-exe "%BASE%\dc-dserver.exe" --admin 127.0.0.1:10090</arguments>
  <workingdirectory>%BASE%</workingdirectory>
  <stopexecutable>curl.exe</stopexecutable>
  <stoparguments>-s -X POST http://127.0.0.1:10090/stop</stoparguments>
  <stoptimeout>30 sec</stoptimeout>
  <log mode="roll"/>
</service>
```

A service runs as another account (LocalSystem by default): the ROM folder, the database folder and the log folder
must be readable / writable for it, and its paths must be absolute or relative to `AppDirectory` /
`workingdirectory`. The firewall rules of HOSTING.md name the program path, so they keep working.

## 7. Logs

- `paths.log_dir` (or `--log FILE`): the lobby log, appended; the same lines go to the console. The detached script
  also keeps the console output in `logs\lobby_<time>.out` / `.err`.
- `--dserver-logdir DIR`: one `dserver_c<channel>_r<room>_g<game>.log` per started game server, and their working
  folder; `--dserver-verbose` copies the game servers' console lines into the lobby log.
- `--pktlog FILE`: hex dump of every lobby packet (debugging only; large).
- `<db dir>\server_list.json`: the server-list document (name, address, ports, mod set and its hash).

## 8. Backup of the database

The database is one SQLite file in WAL mode. While the server runs, take a consistent copy with the SQLite shell
(the server keeps running; readers do not block it):

```
sqlite3 C:\DeCombat\server\ca.db ".backup C:\DeCombat\backup\ca-2026-10-29.db"
```

Without `sqlite3.exe`, Python's built-in module does the same:

```
python -c "import sqlite3; s=sqlite3.connect(r'C:\DeCombat\server\ca.db'); d=sqlite3.connect(r'C:\DeCombat\backup\ca.db'); s.backup(d); d.close()"
```

After a graceful stop the file is complete on its own (no `-wal` file is left) and a plain file copy is a backup. To
restore, stop the server and put the copy in place of `ca.db` (delete any `ca.db-wal` / `ca.db-shm` next to it).

## 9. Linux

**A VPS step by step (provision, firewall, CI artifact, server ROM upload, first start, accounts, clients):
[VPS.md](VPS.md)**, with the scripts in `tools/vps/` and the config `examples/server-vps.toml`. This section is the
background.

**Nothing here has run on Linux.** The `linux-check` lane (2026-10-29) proves only that the server binaries
compile for `x86_64-unknown-linux-gnu`: on Windows, `DC_TARGET=linux python cargo_env.py cargo check -p
dc-lobby-server -p dc-dserver -p dc-rez -p dc-botclient --all-targets` is green (check-only: no Linux linker here,
nothing is linked or tested). The first run on a VPS is the real test; open an issue for what breaks.

What is already portable in the code: the stop handler (SIGINT / SIGTERM run the LG-130 graceful stop,
`crates/dc-lobby-server/src/lifecycle.rs`), the game servers start in their own process group (`process_group(0)`,
the unix side of the Windows `CREATE_NEW_PROCESS_GROUP`), the server ROM's archive lookups ignore letter case
(SERVER_ROM.md), the hang dumps are a no-op off Windows.

Build natively on the Linux host (no `cargo_env.py` there; it is Windows-only), from the `rust/` folder of the repo:

```sh
sudo apt install build-essential          # a C compiler for the bundled SQLite and Lua; nothing else is native
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # rustup, stable toolchain
cargo build --release -p dc-lobby-server -p dc-dserver -p dc-rez -p dc-config
```

The binaries are `target/release/dc-lobby-server`, `dc-dserver`, `dc-server-rom` (from dc-rez) and `dc-config`, with
no `.exe` suffix. Copy them to `/opt/decombat/bin`.

The ROM: make a server ROM on the machine that has the client and copy it over, as in
[SERVER_ROM.md](SERVER_ROM.md) "Copy it to a VPS" (`tar` + `scp`, then `dc-server-rom --verify`), to
`/opt/decombat/rom`. The whole client folder works too, but is five times larger.

Configuration: [examples/server-vps.toml](../examples/server-vps.toml) as `/opt/decombat/config/server.toml`
(`tools/vps/bootstrap.sh` copies it); [examples/decombat.env](../examples/decombat.env) as
`/opt/decombat/config/decombat.env` carries the advertised address (`DC_SERVER__ADVERTISE_IP`) and the Linux paths
(`DC_PATHS__ROM`, `DC_PATHS__DB`, `DC_PATHS__LOG_DIR`, which also make the Windows paths of examples/server.toml
usable). Without `--dserver-exe` the lobby starts the `dc-dserver` next to itself (LG-126). Check with
`dc-config check server.toml --fs` and `dc-lobby-server doctor --config server.toml`.

By hand, in a terminal (Ctrl+C or `kill -TERM <pid>` stops it gracefully):

```sh
cd /opt/decombat && set -a && . config/decombat.env && set +a
bin/dc-lobby-server --config config/server.toml --public --dserver "--dserver-args=-bindip 0.0.0.0" --admin 127.0.0.1:10090
```

As a service: [examples/decombat-lobby.service](../examples/decombat-lobby.service) (`tools/vps/bootstrap.sh`
installs it; the system user `decombat` runs it). systemd stops it with `SIGTERM` to the lobby only
(`KillMode=mixed`), so the lobby ends each match through the pipe; `TimeoutStopSec=30` stays above
`--stop-grace 10` plus the connection and database close. Status, stop and room close also work through the admin
endpoint (sections 4 and 5, `curl` on the host). The ports to open are the TCP ports and the UDP range of section 1;
the provider's firewall and the host's own (`ufw`) both need them.

## 10. Community maps (vanilla++)

Community maps ride the Vanilla++ server entry of a vanilla lobby, with no second instance:
a map pack is a curated pack, one `<sha256>.zip` in `paths.mods_dir` and one line in the entry's `mods = [...]`
(LG-313). The recipe is [docs/VPS.md](VPS.md) "Several instances on one box" > "vanilla++", the lines
[examples/server-vanilla-plus.toml](../examples/server-vanilla-plus.toml). A remote entry's agent needs `packs=1` in its
hello ([specs/HOST_AGENT.md](../specs/HOST_AGENT.md)). `GET /entry_mods/<entry id>.json` on the web port lists the maps
the entry offers (`community_map_base`, `maps`). On this machine `dc-mapkit play <project>` starts that shape with the
project's own export (a vanilla lobby with the entry "Vanilla++", game server and client on 127.0.0.1, logs under the
project's `build/play/`; DC_DEFAULTS.md MAP-67 .. MAP-69, MAP-89).

A modded lobby can offer maps too, with `[mods] policy = "modded"` and the map mods in its `[[mods.set]]`; `GET /status`
on its admin endpoint lists them under `community_maps`, and `dc-mapkit play <project> --modded` starts one.
