# OPS.md: running the public servers

The runbook for launch day and after. Set-up is in [VPS.md](VPS.md), deploying through GitHub in [RELEASES.md](RELEASES.md), the
lobby's flags in [SERVER.md](SERVER.md). `<VPS_IP>` and the keys are yours; none of them are in this repository.

**What has been run.** The backup, restore, health check and migration scripts passed their tests on a Windows machine under Git
Bash, with stand-ins for `systemctl`, `sshd`, `ufw`, `rclone` and the webhook, and real SQLite and gpg. None has run on a Linux VPS,
and neither have their systemd units or `decombat-hardening.conf`. Do the first run of each by hand, in the order of section 1,
while you can watch it.

Rules: OPS-20 (backups), OPS-21 (alerts), OPS-22 (non-root), OPS-23 (load test), MON-1..MON-5 (monitoring), LG-326..325, CLI-1651 in
[DC_DEFAULTS.md](../DC_DEFAULTS.md).

## 1. One-time set-up before going public

In this order; each step keeps the service up unless it says otherwise.

1. **Deploy this build.** It brings the scripts, the timers (not enabled) and the example configs to `/opt/decombat/tools`.
2. **Backups**, section 7.
3. **Alerts**, section 8, including the external check.
4. **Non-root**, section 9.
5. **Client versions**: after the first tagged release set `[client] latest`, section 6.

## 2. A look at things

```
sudo bash /opt/decombat/tools/status.sh                     # build, unit, sockets, /status, /net, end of the logs
sudo bash /opt/decombat/tools/status.sh --instance modded
sudo bash /opt/decombat/tools/healthcheck.sh --print        # every health check, right now; posts nothing
curl -s http://127.0.0.1:10090/status                       # on the VPS only: the admin endpoint is loopback
journalctl -u decombat-lobby -f
tail -f /opt/decombat/logs/dc-lobby-server.log
```

Who played where, how their links were and what went wrong: the dashboard and `dc-lobby-server ops`, section 15.

## 3. Restart, stop

```
sudo systemctl restart decombat-lobby            # graceful: matches end and are stored; players are dropped for seconds
sudo systemctl restart decombat-lobby@modded
sudo systemctl reset-failed decombat-lobby       # first, when systemd gave up after five failed starts
curl -s -X POST http://127.0.0.1:10090/stop      # graceful stop; systemd does not restart a clean exit
curl -s -X POST http://127.0.0.1:10090/rooms/1-5/close   # end one room's match and close it: <channel>-<room no>
```

A restart forgets the login lockouts and rate limits (they are in memory) and ends every game on the hosts too (RELEASES.md).

## 4. Deploy

GitHub: Actions > deploy-servers > Run workflow, `ref` = tag or commit, `targets` = `all`, then approve `production`. Details and what a
half-finished rollout looks like: RELEASES.md. By hand, from a machine with the tarball:

```
scp decombat-servers-linux-x86_64-<commit>.tar.gz decombat-admin@<VPS_IP>:
ssh decombat-admin@<VPS_IP> sudo bash /opt/decombat/tools/install.sh decombat-servers-linux-x86_64-<commit>.tar.gz
```

Between matches if you can: every lobby instance restarts. `tools/vps/push.sh --host <VPS_IP> --user decombat-admin --build --yes` builds on
the VPS instead.

## 5. Rollback

The same deploy with the older `ref`. By hand: `install.sh` with the older tarball. **The database is not rolled back**, and a newer
build may have added columns to it. An older build usually runs with them; when it does not start (`journalctl -u decombat-lobby`), restore
the backup from before the deploy (section 7) and accept losing what happened since. Take one by hand before any deploy that changes the
schema: `sudo systemctl start decombat-backup.service`.

## 6. Telling players about a new client

`/opt/decombat/config/server.toml`, then a restart:

```toml
[client]
latest = "0.3.0"             # older clients show "DeCombat 0.3.0 is out. You have 0.2.1." once, and go on
min_version = "0.2.0"        # older clients show why they cannot log in, and do not
update_url = "https://github.com/argallo/DeCombat/releases/latest"   # what "Open page" opens (this is the default)

[client.download]            # optional: direct zips for "Update now"; empty = the client finds the newest release itself
windows = "https://github.com/argallo/DeCombat/releases/download/v0.3.0/DeCombat-win-v0.3.0.zip"
macos = "https://github.com/argallo/DeCombat/releases/download/v0.3.0/DeCombat-mac-v0.3.0.zip"
```

`sudo -u decombat /opt/decombat/bin/dc-config check /opt/decombat/config/server.toml` checks the numbers and that the
links are `https://`. Clients take a `[client.download]` zip only from the official release download
(`https://github.com/argallo/DeCombat/releases/download/<tag>/<file>`): any other link is ignored (the client looks the
release up on GitHub instead), and `dc-config check` and the lobby's start log warn about it. A client asks `GET /client_version.json` on the web port before its login. Only a build from a `v*`
tag knows its version: other builds (a friend zip from master, a local build) and clients older than this feature never
ask. So `min_version` cannot turn away a modified or old client: it is a courtesy that replaces a strange failure with a
clear message, not access control. Raise it only after the release is out, or nobody can log in until they can download it.

Both boxes have buttons: **Open page** opens `update_url` in the player's browser; **Update now** (Windows) downloads the
zip (the `[client.download]` link, or the newest release on GitHub), checks it against the release's `SHA256SUMS`,
replaces the game's files and starts the game again; on macOS **Download** saves the zip in Downloads and shows it in
Finder. A direct link must point at a release asset (`.../releases/download/<tag>/<file>`) so the client finds the
`SHA256SUMS` next to it; set it only once the release is published. Docs: docs/RELEASES.md "How a client updates itself".

## 7. Backups (`tools/vps/backup.sh`)

A consistent SQLite copy of every instance's database (the online backup API, while the lobby runs), its config and its TLS and identity
keys, in one tar.gz encrypted to your public key before it has a name. The VPS can write backups and cannot read them.

**Set-up.** On your own machine, never on the VPS:

```
age-keygen -o decombat-backup.key      # keep this file somewhere safe and NOT on the VPS: without it the backups are noise
age-keygen -y decombat-backup.key      # prints the public line, age1...
```

(gpg works too: `gpg --export --armor <id> > backup-public.asc`, uploaded, and `GPG_RECIPIENT_FILE`.) On the VPS:

```
sudo apt-get install -y age rclone python3
sudo rclone config --config /opt/decombat/config/rclone.conf     # any off-site remote: another provider's bucket, not this VPS's
sudo chmod 0640 /opt/decombat/config/rclone.conf
sudoedit /opt/decombat/config/backup.conf                        # AGE_RECIPIENT, RCLONE_REMOTE; from examples/backup.conf
sudo bash /opt/decombat/tools/backup.sh --dry-run
sudo bash /opt/decombat/tools/backup.sh                          # the first real one, watched
sudo bash /opt/decombat/tools/restore.sh --list                  # it is local and on the remote
sudo systemctl enable --now decombat-backup.timer                # every 6 hours
systemctl list-timers 'decombat-*'
```

Then **restore it once for real**, section 7a, on your own machine: a backup nobody has read is a hope.

A failed run exits 1, shows in `journalctl -u decombat-backup`, posts to the webhook, and leaves `last_ok` alone, so the health
check also complains once the last good one is too old. Mods and the ROM are not in the backup: the ROM is yours to rebuild, the mods
come from their packages. Size: `KEEP` copies locally on the same disk, so watch the disk check.

### 7a. Restore

Nothing changes on the box without `--apply`:

```
# read a backup: decrypts, opens each database, prints its rows
sudo bash /opt/decombat/tools/restore.sh --from-remote decombat-<time>.tar.gz.age --key decombat-backup.key
# put it back: stops the unit, moves the old database aside to <data>/before-restore-<time>/, starts the unit
sudo bash /opt/decombat/tools/restore.sh --from-remote decombat-<time>.tar.gz.age --key decombat-backup.key --apply --instance default
```

Upload the private key only for the restore and delete it after (`shred -u`), or decrypt on your own machine with
`age -d -i decombat-backup.key x.age > x.tar.gz`, upload the `.tar.gz` and pass that with no `--key`. `--with-keys` also restores the TLS and
identity keys: on a **new VPS** you want both `--with-keys` and `--with-config`, because a new TLS key changes the fingerprint every client
has pinned and a new identity key ends every token. On a new VPS: `bootstrap.sh`, `install.sh`, the ROM (VPS.md sections 4-5), then the
restore; a new address also means a new `advertise_ip` and new client packs. The old database stays in `before-restore-...`
with its accounts and hashes: delete it when you are sure.

## 8. Alerts (`tools/vps/healthcheck.sh`)

Every 2 minutes: each enabled lobby unit active, its auth port answering TLS, its web port answering https, its certificate not within 14 days
of expiry, its admin `/status` answering with each game host online, the bots' unit if enabled, the disk, a backup from the last 30 hours.
A change posts to a Discord-style webhook; a failure is told after two runs in a row, so a deploy's restart stays quiet.

```
sudoedit /opt/decombat/config/alerts.conf                # WEBHOOK_URL, from examples/alerts.conf
sudo bash /opt/decombat/tools/healthcheck.sh --print
sudo systemctl enable --now decombat-health.timer
```

With a lobby that has the monitor (section 15) it also asks the lobby's `/ops/alerts`: a match with `OPS_SILENT_DROPS` or more players
dropped silently, `OPS_ERRORS` problem events or `OPS_CRASHES` crashed game servers within `OPS_WINDOW_MIN` minutes, and a game host on
another build than the lobby (MON-5; 0 turns one off). These clear by themselves once the window has passed.

Test it: `sudo systemctl stop decombat-lobby`, a message within about 4 minutes, start it, a message.

**When the whole VPS dies** none of that runs. Choose an outside check (these were true when written, check their free tiers):

- **A dead-man ping**, the better one: a free [healthchecks.io](https://healthchecks.io) check with a period of 5 minutes, its address as
  `PING_URL` in `alerts.conf`. It tells you when the pings stop: box gone, network gone, or timer dead.
- **A port monitor**: [UptimeRobot](https://uptimerobot.com) or [Better Stack](https://betterstack.com), a TCP or "port" monitor on
  `<VPS_IP>:10000`, and an https one on `https://<VPS_IP>:18080/server_list.json` with certificate checking off (the certificate is self-signed
  unless you installed a CA pair). This also sees the provider firewall, which nothing on the box can. Each probe is a line in the log.

Use both if you can, with the alert going to something other than the Discord server being watched: your phone or e-mail.

## 9. Non-root operation (`tools/vps/migrate_nonroot.sh`)

**Where it stands.** The service already runs as the system user `decombat` (no login shell) and writes only `data` and `logs`. What runs as root is
everything around it: SSH logins (`root@`, the workflow's default `DC_VPS_USER`, the operator's own deploy script), the deploy scripts, and now the backup
and health timers, which need the config and keys. The migration ends root's SSH login; root stays for `sudo`.

| login | for | sudo |
|---|---|---|
| `decombat-admin` | you | everything, no password (it has none; the key is the login) |
| `decombat-deploy` | the workflow | `deploy_remote.sh` from `/opt/decombat/tools`, `status.sh`, restart and status of the lobby units; nothing else |

**The limit of the deploy user.** It has no root shell, but a deploy installs what its tarball says, units and scripts included, as root. So its key
cannot run a command directly, but it can ship one. Keep it a production secret, behind the `production` environment's approval.

Each step prints its plan first; `--apply` does it. From a root session you **leave open** until the end:

```
sudo bash /opt/decombat/tools/migrate_nonroot.sh check
sudo bash /opt/decombat/tools/migrate_nonroot.sh users --deploy-key deploy_key.pub                  # read the plan
sudo bash /opt/decombat/tools/migrate_nonroot.sh users --deploy-key deploy_key.pub --apply
```

In a **new terminal**, the commands it printed: `ssh decombat-admin@<VPS_IP> 'sudo -n id -un'` must say `root`. Then, in that admin session:

```
sudo bash /opt/decombat/tools/migrate_nonroot.sh firewall                 # compare; --apply adds what is missing, OpenSSH first
sudo bash /opt/decombat/tools/migrate_nonroot.sh harden --apply           # the drop-ins; no restart yet
sudo systemctl restart decombat-lobby                                     # when it suits: matches end
```

**Then check a match**: open a room, start it with a bot, see `dserver_*.log` grow. A game server is a child of the unit and runs under the same limits;
if anything misbehaves, `sudo rm /etc/systemd/system/decombat-*.d/10-hardening.conf && sudo systemctl daemon-reload` and restart. Then, still in the admin session:

```
sudo bash /opt/decombat/tools/migrate_nonroot.sh lock-ssh                 # read the plan
sudo bash /opt/decombat/tools/migrate_nonroot.sh lock-ssh --apply
```

It refuses to run except from an SSH session of `decombat-admin` using sudo: that session is the proof the new login works. It arms a timer first, and in
10 minutes root login comes back by itself. In one more **new terminal**: log in as `decombat-admin`, then

```
sudo bash /opt/decombat/tools/migrate_nonroot.sh confirm
ssh root@<VPS_IP>                                                         # must now be refused
```

No confirm and the setting undoes itself; `rollback-ssh` undoes it on purpose. Provider web consoles still reach the box if all this fails.

**After it:** GitHub secret `DC_VPS_USER=decombat-deploy` and `DC_VPS_SSH_KEY` the deploy key (RELEASES.md); the workflow then calls the root-owned scripts in
`/opt/decombat/tools`. Your own scripts that say `root@` (your deploy and bot scripts, `push.sh` without `--user`) log in as
`decombat-admin` with `sudo`, or they stop working. **Firewall**: provider and ufw, TCP 22, 10000, 10002, 10003, 18080, UDP 27015-27100, plus each instance's
own (`firewall` lists them) and TCP 10005 with game hosts. Never the admin ports (10090 and up).

## 10. An abusive player

**An account.** Effective at its next login, so kick it as well when it is online:

```
sudo -u decombat /opt/decombat/bin/dc-lobby-server account --db /opt/decombat/data/ca.db disable <LOGIN>   # enable <LOGIN> lifts it
```

A GM in the game does it in place: section 11. An identity token keeps working on a community lobby that trusts the VPS until it expires (24 h): disabling only
stops that lobby's own account. Rename a bad character name: `account --db ... rename <LOGIN> <NEW NAME>`.

**An address.** `ufw` rules go first-match, so insert at the top:

```
sudo ufw insert 1 deny from 198.51.100.7
sudo ss -K dst 198.51.100.7                       # drops what is connected already
sudo ufw status numbered ; sudo ufw delete <n>    # to lift it
grep 198.51.100.7 /opt/decombat/logs/dc-lobby-server.log | tail
```

With ufw inactive use the provider's firewall. Who is who: a GM's trace (below) shows a player's address, and `connection N from <ip>` lines are in the log. An
address may be many people (a campus, CGNAT). A flood from many addresses is the provider's problem to filter: ask them. The lobby's limits, without you doing anything:
64 connections per address and 4000 in all, 120 s to log in, a lockout after 10 failed logins: section 13.

## 11. GM commands

```
sudo -u decombat /opt/decombat/bin/dc-lobby-server account --db /opt/decombat/data/ca.db gm <LOGIN> --on   # from its next login
sudo -u decombat /opt/decombat/bin/dc-lobby-server account --db /opt/decombat/data/ca.db gm <LOGIN> --off
```

In the game, a GM types `/` lines in the chat box. The command words are the 2009 client's own, read from your client folder at run time, so they are not listed in
this repository. What they do on our lobby:

| action | what happens | where |
|---|---|---|
| kick | drops the player's connection | 0x3d0 action 1, `gm.rs` |
| ban for N days | as kick, and refused at login with the banned box until then; no number or 0: permanent | 0x3d0 action 2 |
| lift a ban | | 0x3d0 action 0 |
| trace | the account's login, name, place, and address (`gm.lookup_shows_address`) | 0x3d2 |
| mute for N minutes | the server drops its chat and whispers | 0x3d4 |
| `/k NAME` or "Kick User" | out of the room | waiting room |
| observer mode | watch unseen | 0x30e |

The test commands `/notice <text>`, `/expire`, `/shopver`, `/grade <n>` and `!hisec <item>` are GM-only on a public lobby. A ticker to everybody: `/notice <text>` as a
GM. `/help` in the lobby chat lists the commands the player can use right there (the GM and test commands only for those who may use them, and the commands of the server entry's mods). A text at every login: `notice.login_ticker` and `notice.login_box` in `lobby.json` beside the database (OPS-10). `account grant`, `account loadout` and `account event` change a player's things; see `--help`.

How often a match drops a HiSec case or a Camo Crate: about one drop a match (the game server's own rule). On an entry with camos, `camo_share` in `server.toml` `[hisec]` (0..100, default 75) is the percent of those drops that are Camo Crates; the rest are HiSec cases, and 0 means no crates. In `lobby.json` the same key is `"hisec.camo_share": 75`. The lobby passes it to each game server as `-hisec camo_share=<n>`, and `dc-config check` refuses a value above 100.

## 12. Reports

A player's report goes in the `reports` table. With `mail.reports_to` and a mail server set (LG-316) it is also mailed. To read the table (`python3` is on the box; `apt install sqlite3` for a prompt):

```
sudo -u decombat python3 - <<'PY'
import sqlite3
c = sqlite3.connect("file:/opt/decombat/data/ca.db?mode=ro", uri=True)
q = """SELECT r.id, datetime(r.created,'unixepoch'), a.login, r.target_nick, r.kind, r.text, r.extra
       FROM reports r LEFT JOIN accounts a ON a.user_no = r.reporter ORDER BY r.id DESC LIMIT 30"""
for row in c.execute(q):
    print(*row, sep=" | ")
PY
```

Only read: never open the live DB for writing by hand, and never as root without `sudo -u decombat` (root-owned `-wal` files stop the service).

## 13. Limits a public player runs into

| what | limit | rule |
|---|---|---|
| connections | 64 per address on all TCP ports together, 4000 in all; loopback free | LG-326 |
| time to log in | 120 s on a connection, then closed; no limit after | LG-326 |
| failed logins | 10 per name or address in 10 minutes lock it for 5 minutes | LG-146 |
| registration, recovery | 5 requests per address in 10 minutes | LG-145, LG-241 |
| TLS handshakes in progress | capped; further connections are closed at once | LG-223 |
| frame size | 0x8000 bytes, or the connection is dropped | `dc-wire` |
| web request | 16 KB head, 5 s per read, small bodies | `clanweb.rs` |
| chat line | 120 UTF-16 units; none on how fast | `wire.rs` |

Known gaps that matter on the day: a player can take the seat of another player in a running match by knowing the game port and the name (the UDP hello is checked against
the roster by name only); no rate limit on chat or on lobby requests; the lobby and clan ports are not encrypted after the login; the password check runs inside the lobby's one
lock, so a storm of logins with real names slows everybody; no server check of movement speed. A vulnerability report comes through `SECURITY.md`.

**Never put a local relay or proxy in front of the lobby's TCP ports** (a tunnel, a WebSocket or WebRTC bridge on the box): loopback peers skip the password check on a
`--public` lobby (LG-32), so every player behind such a proxy could log in as anybody.

Seat tickets (LG-327): a client older than this feature cannot join a match on a lobby and game server that have it. Its player sees the
disconnect box and, in the room's chat, "Your DeCombat client is too old to join matches. Please download the current version." Publish the new client
before you deploy the lobby. `seat of uid N refused` in a game server's log is such a client or a stranger with that name.

Logins under a flood (LG-328, LG-329): password work has its own bounded pool. `too many password checks waiting` in the log means the queue of 256 was
full and a player was told the login failed; `over the message rate` means one connection went over its cap and its messages were dropped.

## 14. Load test

`python tools/lobby_soak.py --bin-dir target/debug --bots 400 --seconds 270 --per-second 5` starts a lobby on loopback ports with a throwaway database and one `dc-botclient`
process per bot, no window, no ROM: real TCP logins, a channel, and restarts. **It does not test matches**: no game server runs and no ROM is read. It shares one
machine and loopback with the bots, skips the password hash (loopback, no `--public`), so a real login storm costs more; and it is not over a network.
Don't run it on the VPS while people play.

Measured 2026-11-02, 32-thread Windows PC, debug build, 400 bots at 5 starts a second for 270 s: 200 connections held (all bots take channel 1, and **a channel holds 200**: `--channel-max`,
`lobby.channels` capacity; the rest were refused with "channel 1 refused (full?)" and came back), 1350 logins, the lobby at 30 % of one core and 33 MB, no panic, no rolled-back
scope, no bot crash. It says the lobby's connection handling has room at this size; it says nothing about the 2 vCPU VPS, about matches, or about hashed logins.

## 15. The dashboard and the match records (MON-1..MON-5)

The lobby keeps, for 14 days (`[monitor] retention_days`): every match with its room, map, mode, host and build, players, bots and
outcome; each player's link over the whole match (round trip mean / p95 / max, input rate, longest gap, resends, flow drops, datagrams,
largest datagram, and how it ended: `bye`, `silent`, `kicked`, `end`, ...) from the game server's `@linksum` line, local and remote hosts
alike; each 10 s link window; and per account the notable events (login, refused ready with the reason, start blocked or a member left
behind, seat ticket refused, match join / leave / silent drop, lobby disconnect during a match, report filed, crash diagnostics). A game server
older than this build sends no `@linksum`: its players get a row without link numbers.

**Open it from Windows** (one line; `-Key` defaults to `~\.ssh\decombat_vps`, the admin token is read from the VPS and never printed):

```
powershell -NoProfile -ExecutionPolicy Bypass -File tools\vps\ops_tunnel.ps1 -Host <VPS_IP>
```

It opens an SSH tunnel to the loopback admin port and the browser on `http://127.0.0.1:10090/ops`; Enter closes the tunnel. The modded
instance: `-AdminPort <its admin port>` (`status.sh --instance modded` names it) and, to keep both open, `-LocalPort 10091`. By hand:
`ssh -i <key> -N -L 10090:127.0.0.1:10090 root@<VPS_IP>`, then `http://127.0.0.1:10090/ops?token=<DC_ADMIN_TOKEN>`.

The page: **Live** (the hosts with their build and whether it is the lobby's, the matches running with each player's newest 10 s window
coloured ok / warn / bad, the rooms, the newest problems; it refreshes every 30 s, like every tab while nothing is being typed), **Problems**, **Matches** (the last 100, with each map's name as the lobby started it on that server entry (OPS-13), a link per match:
every player's whole-link numbers, a round-trip line per player, the match's events and every 10 s window), **Player** (search by character
or login: the account panel with its changes, section 16, then the matches and the events of the account; the list marks banned, disabled and
muted accounts and has an "Only banned" filter), **Alerts** (the checks of section 8 with their default thresholds), **Bots** (the bot groups the lobby runs and
where each bot is, with add / edit / start / stop / delete: docs/BOTS.md "Bots the lobby runs"). A **Beta list** link in the header opens the beta signup list (docs/BETA.md). Colours: bad = nothing
arrived (no datagram in a window, no report for 25 s, a silent end) or a round trip over 200 ms; warn = over 120 ms, a spike over 400 ms, more than
10 resends or any flow drop per 10 s. The round trip is the raw samples', so its max can be above the smoothed one of the server log's
`link summary` line; the other numbers are the same as that line's.

The dashboard answers only a loopback peer, also when the admin port was opened with `--admin-public`, only to a `Host` that is a loopback name,
and with `--admin-token` only with the token. Never forward the admin port anywhere but your own machine's loopback.

**From a browser anywhere, with a hardware key:** the admin web (`[admin_web]`, a Cloudflare Tunnel to its own loopback listener) serves the
same dashboard and beta list at `https://admin.<domain>` to a signed-in YubiKey only: docs/ADMIN_WEB.md. With `[admin_web] instances` it
serves the other lobby instances of the box too, under `/i/<name>/ops`, with the same sign-in (docs/ADMIN_WEB.md section 7). The SSH
tunnel above stays the break-glass way in.

**From an ssh session**, read-only on the database:

```
sudo -u decombat /opt/decombat/bin/dc-lobby-server ops --db /opt/decombat/data/ca.db timeline <name>    # matches and events, by character or login
sudo -u decombat /opt/decombat/bin/dc-lobby-server ops --db /opt/decombat/data/ca.db match <id> [--net]  # every player, events, 10 s windows
sudo -u decombat /opt/decombat/bin/dc-lobby-server ops --db /opt/decombat/data/ca.db matches [--limit N]
sudo -u decombat /opt/decombat/bin/dc-lobby-server ops --db /opt/decombat/data/ca.db events --problems [--limit N]
```

Times there are UTC. **Deploy order**: the lobby (it makes its new tables at its start), then the host agents' game servers (`deploy_hosts.py`):
until a host runs the new `dc-dserver`, its matches have records without link numbers. Nothing in the client changes.

**Public match board (OPS-16).** The records above are for operators. For players, a lobby can write a public-only snapshot of its running
matches (`[monitor] public_games_file`, env `DC_MONITOR__PUBLIC_GAMES_FILE`, off by default) and a separate script keeps one Discord message
up to date from it: docs/LIVE_GAMES.md.

## 16. Changing an account from the dashboard (LG-395..LG-398)

Open **Players**, pick a name. The **Account** panel shows the login, character, account number, created and last login, whether the
player is online and where (channel, room, in a match), GM and bot flags, EXP with rank, GP, NX, the ban in force (until when or for good,
the reason, who set it), the mute, whether the login is disabled, and the recovery address (masked). Beside it, the changes. Each one asks
first; each needs the dashboard's page token, so it works only from the page itself (the SSH tunnel or the admin web).

| change | what it does | can be undone |
|---|---|---|
| Give / Take NX, GP, EXP | adds or takes the amount, with a reason. At most `[gm] admin_max_nx` (100,000), `admin_max_gp` (1,000,000), `admin_max_exp` (10,000,000) in one change; a balance never goes below 0. NX is the prepaid balance the daily bonus and clan wins pay into: NX is earned in play here, this is for prizes and corrections, never for selling. An online player's game shows the new NX and GP at once (the lobby sends the balance); the shop's balance refresh asks again and gets the same. In a match, GP and EXP show when it ends. EXP given here pays no rank reward | by the opposite change |
| Ban | for hours or days, or for good, with a reason: every connection drops and each login gets the banned box with the days left | Lift the ban |
| Kick now | drops the connections (reason required); they can log in again | - |
| Loadout | the OPS-14 row: a dropdown of gun names per slot (primary, secondary, melee, throwing; the ROM's weapons and the camo variants of the loaded mod sets, from `GET /ops/guns`) writes the spec `account loadout` takes, with a typed field for ids the lists lack. Gives the items for good and equips them, the old items stay unequipped in the inventory; an online player sees them from the next login | equip the old items again |
| Mute | the server drops their chat and whispers for N minutes (at most `gm.max_mute_minutes`) | Unmute |
| Disable login | LG-142: refused at the next login; a session already open stays, so kick as well | Enable login |
| Send a password reset mail | the recovery code mail (15 minutes, once) to the address the player gave; needs `[mail]`; at most 3 an hour | - |
| Delete account | type the login to confirm. Refused for a GM account (take the flag away first: section 11) and while the player is connected (kick first) | **no** |

**What a delete removes**, in one transaction: the account and its character, items and loadouts (items, function slots, expiry notes),
inbox and notices, friends and friend requests in both directions, the recommender row, coupon uses, its megaphone lines not yet aired,
its ban, the NX reward claims (daily, clan wins, events), the identity-token link (a later sign-in with the same token makes a fresh
account), recovery codes, the bot loadout of its login, and its bot pool seat. Its clan: as when the player leaves it, the longest-standing
member leads it, or a leader alone disbands it. The beta signup row stays, still `registered`, with no account. **Kept:** the match records
and ops events (they keep the account number; the last event says "account deleted (login, character) by ..."), the GM action log
(`op_actions`), reports the player filed or that name them, the shop's sales log (the hot list counts it), a used invite code, crash
diagnostics, aired megaphone lines. A backup (section 7) is the only way back.

**Where it is logged:** every change is an event of kind `admin` on the player's timeline (Events, on the player's page) naming the change,
the amounts or length, the reason and who did it: `the admin port` (the SSH tunnel) or `the admin web, key #<n> from <address>`; also a row of
`op_actions` with `by_no` 0 and a line in the lobby log (`ops player admin: ...`). A GM's chat-box ban, kick and mute (section 11) use the
same functions and are logged as before.
