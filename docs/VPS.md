# VPS.md: the DeCombat servers on a Linux VPS

Goal: the lobby and the game servers run on a rented Linux VPS, so
the host's own client joins as a remote player like everybody else.

**First real run: 2026-10-30 from a Mac** (docs/MACOS.md). Ubuntu 24.04 on a 2 vCPU / 4 GB VPS, deployed with
`tools/vps/push.sh` (the unix counterpart of `push.ps1`, build-on-the-VPS path: rsync of the checkout, rustup,
`cargo build --release` in 2.5 minutes, the artifact layout packed on the VPS). What the scripts got right on the
first Linux run: bootstrap, install (binaries and ROM), the unit, status, bots.sh. What needed a fix or a detour:
- the server ROM archive must hold `server-rom.json` at its top (`tar -czf x.tgz -C <rom folder> .`, as push.ps1
  and push.sh do), not the folder itself;
- `/opt/decombat/tools/bootstrap.sh` (the installed copy) cannot run again: it looks for `examples/` next to
  `tools/vps/`, which only the upload stage has; run it from `~/decombat-upload/tools/vps/bootstrap.sh`;
- the provider's Ubuntu image had `ufw` active with only OpenSSH allowed, so nothing reached the lobby until
  `bootstrap.sh --apply-firewall`; check `sudo ufw status` first thing when "nothing connects";
- the `--login-password` headless walk of dc-client doubled the password (the box was pre-filled and the pilot typed
  it again): fixed in dc-client's main; the bot client and a by-hand login were never affected.

- the second deploy the same day (master with the TLS login, LG-220..226) was one `push.sh --host <VPS_IP> --build
  --yes`: graceful stop of a running bot match, new binaries, the self-signed certificate made at the first start,
  fingerprint in the log and from `dc-lobby-server tls --db ...`; a client that trusts on first use logged in. Note
  the web port: the example config's `http_port = 0` now means auth + 8080 (18080, the firewall table of HOSTING.md),
  while an earlier bootstrap opened 10080 in `ufw`; `sudo ufw allow 18080/tcp` (and the provider firewall) after
  such an update, or set `http_port` explicitly.

Earlier note (2026-10-29): nothing here had run on Linux yet. The scripts, the config and the unit were written on Windows and checked
there only as far as Windows allows: `bash -n` on every script, a PowerShell parse and a `-DryRun` of `push.ps1`,
a stdlib check of the workflow, the Linux cross-check `DC_TARGET=linux python cargo_env.py cargo check`, and crate
tests that parse `examples/server-vps.toml` and the unit's `ExecStart` line (dc-config
`hosting_examples_parse_and_validate`, dc-lobby-server `cli::tests::vps_unit_and_example_config_resolve_to_the_public_setup`).
The first real run is on the VPS; open an issue for what breaks.

Rules referred to: BOT-82 / BOT-83 (bots on the server ROM, bot passwords), LG-187 (loopback bots), LG-30 (per-peer announced address), LG-31 (`--public`), LG-126 (the sibling `dc-dserver`),
LG-130 (graceful stop), LG-131 (admin endpoint), LG-140..146 (accounts), LG-170..175 (game masters), DSV-370..372
(server ROM) in [DC_DEFAULTS.md](../DC_DEFAULTS.md). Background: [SERVER.md](SERVER.md) (operating the lobby),
[SERVER_ROM.md](SERVER_ROM.md), [HOSTING.md](../HOSTING.md) (ports, accounts, the friend pack).

## What goes where

| file | on the VPS | from |
|---|---|---|
| `dc-lobby-server`, `dc-dserver`, `dc-server-rom`, `dc-config`, `dc-botclient`, `COMMIT.txt` | `/opt/decombat/bin` | the CI artifact (or a build on the VPS) |
| server ROM (about 920 MB) | `/opt/decombat/rom` | `dc-server-rom` on the Windows machine that has the 2009 client |
| `server.toml`, `decombat.env` | `/opt/decombat/config` | [examples/server-vps.toml](../examples/server-vps.toml), [examples/decombat.env](../examples/decombat.env) |
| `ca.db` (accounts, characters, items) | `/opt/decombat/data` | created on the first start |
| `dc-lobby-server.log`, `dserver/dserver_c<ch>_r<room>_g<game>.log` | `/opt/decombat/logs` | written by the servers |
| `bootstrap.sh`, `install.sh`, `rom_check.sh`, `status.sh`, `bots.sh` | `/opt/decombat/tools` | [tools/vps](../tools/vps) |
| systemd unit `decombat-lobby` | `/etc/systemd/system/decombat-lobby.service` | [examples/decombat-lobby.service](../examples/decombat-lobby.service) |
| systemd unit `decombat-bots` (optional, not enabled) | `/etc/systemd/system/decombat-bots.service` | [examples/decombat-bots.service](../examples/decombat-bots.service) |
| bot logs | `/opt/decombat/logs/bots/<name>.out`, `<name>.events.jsonl` | `bots.sh` |
| a further lobby instance `<name>` (optional) | `/opt/decombat/instances/<name>/{config/server.toml, decombat.env, data, logs, mods}` | `tools/vps/instance.sh <name> create` (section "Several instances on one box") |
| systemd template `decombat-lobby@` (one unit per further instance) | `/etc/systemd/system/decombat-lobby@.service` | [examples/decombat-lobby@.service](../examples/decombat-lobby@.service) |

The running VPS (installed 2026-10-29, before `tick_hz` existed in the example) had `tick_hz = 30` added by hand under
`[dserver]` on 2026-10-31 (docs/NET_LATENCY.md option 1; the installed default was 10 Hz). A fresh install from
`examples/server-vps.toml` already carries the line. Check the next match: its dserver log header says `tick 30 Hz`.

The service runs as the system user `decombat` (no login shell). It may write only `data` and `logs`; binaries,
config and ROM belong to root and are read-only for it. Backups, alerts, the move away from root's SSH login and the launch-day runbook: [OPS.md](OPS.md).

The unit starts `dc-lobby-server --config /opt/decombat/config/server.toml --public --check-password
--no-auto-create --register-open "--dserver-args=-bindip 0.0.0.0" --dserver --dserver-logdir /opt/decombat/logs/dserver
--admin 127.0.0.1:10090 --stop-grace 10`: listens on every interface, tells remote players the address from
`DC_SERVER__ADVERTISE_IP` (refuses to start while that is loopback, LG-31), checks passwords, refuses unknown names,
serves the invite-only registration page on TCP 18080, starts the `dc-dserver` next to itself per match (LG-126)
bound to every interface (`-bindip 0.0.0.0`: on a provider that maps the public address onto a private one, binding
the public address itself would fail), and keeps the operator endpoint on loopback.

## Order of operations

`<VPS_IP>` is the VPS public IPv4 address, `<KEY>` your SSH private key file, `<CA>` your 2009 client folder.
Nothing of these is stored in the repository; every script takes them as parameters.

### 1. Provision

Ubuntu 22.04 or 24.04, x86_64, 2 vCPU, 4 GB RAM, 20 GB disk; SSH login with a key (`root`, or a user with sudo).
Note the public IPv4 address.

### 2. Provider firewall

Open inbound, from anywhere:

| what | protocol | port |
|---|---|---|
| SSH | TCP | 22 |
| auth (login, server list); TLS | TCP | 10000 |
| lobby | TCP | 10002 |
| clan lobby | TCP | 10003 |
| registration page, clan pages, diagnostic report upload; TLS (https) | TCP | 18080 |
| connection test echo (docs/DIAGNOSTICS.md §4; `sudo ufw allow 18080/udp`, the bootstrap does not add it yet) | UDP | 18080 |
| game servers (first room 27015, further rooms 27016-27100) | UDP | 27015-27100 |

Do not open 10090 (the operator endpoint listens on loopback only; reach it with an SSH tunnel, section 9).

### 3. Get the binaries

Either:

- **CI artifact** (recommended; `deploy-servers.yml` does all of this for you once the VPS is set up, docs/RELEASES.md):
  GitHub, the repository's Actions tab, workflow `release` (it runs on pushes to master, or "Run workflow"). When the job
  `servers (linux x86_64)` is green, download the artifact
  `decombat-servers-linux-x86_64-<commit>` from the run's summary page. It is a zip holding
  `decombat-servers-linux-x86_64-<commit>.tar.gz`; unzip it on Windows and keep the `.tar.gz` (it keeps the
  executable bits). Inside: `bin/` (stripped release builds), `tools/vps/`, `examples/`, `COMMIT.txt`,
  `SHA256SUMS`. The job builds on Ubuntu 22.04, so the binaries run on 22.04 and 24.04. With the GitHub CLI:
  `gh run download <run id> -n decombat-servers-linux-x86_64-<commit>`.
- **Build on the VPS**: `sudo apt install build-essential git`, rustup (`curl --proto '=https' --tlsv1.2 -sSf
  https://sh.rustup.rs | sh`), clone the repository, then in `rust/`
  `cargo build --release -p dc-lobby-server -p dc-dserver -p dc-rez -p dc-botclient -p dc-config`. A release build
  needs about 2-4 GB of RAM and some minutes on 2 vCPU; copy `target/release/{dc-lobby-server,dc-dserver,
  dc-server-rom,dc-config,dc-botclient}` to `/opt/decombat/bin` (by hand, as root) instead of `install.sh <tarball>`.

There is no cross-build from Windows (no Linux linker or sysroot here; the Windows `DC_TARGET=linux` lane only
checks that the code compiles).

### 4. Build the server ROM on Windows

On the machine with the 2009 client (docs/SERVER_ROM.md):

```
cd rust
set DC_TARGET=x86_64
python cargo_env.py cargo build --release -p dc-rez --bins
target\release\dc-server-rom.exe "<CA>" "C:\DeCombat\server-rom"
target\release\dc-server-rom.exe --verify "C:\DeCombat\server-rom"
```

About 920 MB, 2828 files (916.4 MB; since BOT-82 it keeps the first-person weapon models, 266 MB, for the bots). `--only-worlds NAME,NAME` makes it smaller (rooms on a left-out world fail at their start).

### 5. Bootstrap, upload, install (one command from Windows)

```
powershell -NoProfile -ExecutionPolicy Bypass -File tools\vps\push.ps1 -Host <VPS_IP> -Key <KEY> -Bootstrap -PublicAddress <VPS_IP> -Tarball <decombat-servers-linux-x86_64-<commit>.tar.gz> -Rom C:\DeCombat\server-rom
```

What it does (`-DryRun` prints the commands without running them):

1. uploads `tools/vps/*.sh` and the three example files to `~/decombat-upload` on the VPS (line ends converted to
   Unix), then the tarball, then the server ROM packed with `tar.exe` (it shows the size and asks first; `-Yes`
   skips the question);
2. `bootstrap.sh --public-ip <VPS_IP>`: user `decombat`, `/opt/decombat/{bin,rom,data,logs,config,tools}`, copies the
   config once (later runs keep your edits), sets `DC_SERVER__ADVERTISE_IP` in `config/decombat.env`, installs the
   systemd unit (not enabled), prints the `ufw` rules (`-ApplyFirewall` adds them and `allow OpenSSH`; it never
   enables ufw: the provider firewall of step 2 is the one that matters);
3. `install.sh <tarball> --rom server-rom.tgz`: binaries to `/opt/decombat/bin` (ELF check, `COMMIT.txt` with the
   install time and the tarball's SHA-256), the ROM unpacked beside the old one, checked with `rom_check.sh`
   (`dc-server-rom --verify`: every file against the SHA-256 in `server-rom.json`), then swapped in; the uploaded
   ROM archive is deleted afterwards;
4. `status.sh`.

`-User <name>` for a non-root login (the scripts then run with `sudo`, which may ask for the password; `ssh -t`),
`-Port <n>` for another SSH port. On later updates only `-Tarball` is needed: `install.sh` restarts the service
when it is enabled (graceful stop: running matches end and their results are stored).

By hand on the VPS instead: unpack the tarball, then `sudo bash <dir>/tools/vps/bootstrap.sh --public-ip <VPS_IP>`,
`sudo bash <dir>/tools/vps/install.sh <tarball>`, `sudo bash /opt/decombat/tools/install.sh --rom <server-rom.tgz>`.

### 6. First start

```
sudo bash /opt/decombat/tools/rom_check.sh
sudo -u decombat /opt/decombat/bin/dc-config check /opt/decombat/config/server.toml --fs
sudo systemctl enable --now decombat-lobby
sudo bash /opt/decombat/tools/status.sh
```

The `dc-config check` warning about a loopback `advertise_ip` is expected: the address comes from `decombat.env`.
Good signs in the status: unit `active (running)`, `GET /status` answers with `address` showing `<VPS_IP>`, the log
has `admin endpoint on http://127.0.0.1:10090`, `registration page on port 18080` and `dc-lobby-server: auth 10000
lobby 10002 ...`.

### 7. The first GM account and invite codes

Always run the account CLI as `decombat` (a run as root would leave database files the service cannot write):

```
sudo -u decombat /opt/decombat/bin/dc-lobby-server account --db /opt/decombat/data/ca.db add <YOU> --invite
sudo -u decombat /opt/decombat/bin/dc-lobby-server account --db /opt/decombat/data/ca.db gm <YOU> --on
sudo -u decombat /opt/decombat/bin/dc-lobby-server account --db /opt/decombat/data/ca.db invite --count 5 --days 7
sudo -u decombat /opt/decombat/bin/dc-lobby-server account --db /opt/decombat/data/ca.db list
# equip items on an existing account (permanent; slot from the ROM for a bare number; counts from its next login):
sudo -u decombat /opt/decombat/bin/dc-lobby-server account --db /opt/decombat/data/ca.db loadout <NAME> 27
# a fresh random GP primary + secondary of the ROM shop (a bot kit, LG-260; another draw every run):
sudo -u decombat /opt/decombat/bin/dc-lobby-server account --db /opt/decombat/data/ca.db loadout <NAME> random --rom /opt/decombat/rom
```

The first start makes the lobby's TLS certificate next to the DB (`/opt/decombat/data/tls_cert.pem`, key
`tls_key.pem`, owner-only) and logs its fingerprint; `sudo -u decombat /opt/decombat/bin/dc-lobby-server tls --db
/opt/decombat/data/ca.db` prints it. With a DNS name you may drop a CA-issued pair in as `fullchain.pem` +
`privkey.pem` (docs/SERVER.md section 4b).

`add <YOU> --invite` prints a code bound to that name: open `https://<VPS_IP>:18080/register` (the browser warns
about the self-signed certificate: compare its SHA-256 fingerprint with the log line), enter the name, a new
password (one you use nowhere else) and the code. `invite --count 5` makes codes for friends. The full CLI
(`reset-password`, `disable`, `enable`): HOSTING.md "Accounts". A GM's privilege applies from its next login.

### 7a. Identity: one login for every lobby (optional)

Our VPS lobby can be the identity authority (LG-68 / LG-69): a player registers there once, and any lobby that trusts
it (a community host, a modded server) lets the player in without ever seeing the password. Each lobby keeps its own
character and progression for the player.

Issuing (on the VPS): in `server.toml` set `[identity] issue = true` and `issuer = "<a short id, e.g. decombat.vps>"`
(or start with `--identity --identity-issuer <id>`). The first start makes the key pair
`/opt/decombat/data/identity_key.pem` (owner-only, back it up; anyone with it can mint logins) and logs the public key;
`sudo -u decombat /opt/decombat/bin/dc-lobby-server identity --db /opt/decombat/data/ca.db` prints it. Tokens are
issued by `POST https://<VPS_IP>:18080/identity/token` with `{"login":"...","password":"..."}` and last
`token_hours` (24). The VPS lobby itself also accepts its own tokens.

On Windows the key files (`identity_key.pem`, the TLS key) are written with the data folder's inherited access list and no ACL of their own: the host must restrict that folder to the service account (unix gets mode 0600).

Accepting (on a community host): `[identity] issuer = "<the authority's id>"` and `public_key = "<the printed key>"`
(or `--identity-issuer <id> --identity-key <key>`). A token login makes a local account with the token's login name
the first time and finds it again later; a local account that already holds that name is not taken over (the login
is refused and logged). Password logins of the host's own accounts keep working. `dc-config check` refuses an
`issuer` without a `public_key` and the other way round.

Client side (CLI-1610): `dc-client --menu lobby` asks the authority for the token itself. By default the authority is
the lobby it was started with (`--lobby <VPS_IP>:10000` -> `https://<VPS_IP>:18080`, pinned with the same
`--server-fingerprint`), so the friend pack works unchanged once the VPS issues; a lobby that does not issue (404)
gets the plain password login as before. For a community lobby start the client with
`--lobby <host>:10000 --identity <VPS_IP>:18080 --identity-fingerprint <VPS fp>`: the password goes only to the VPS,
the community lobby gets `tok:<token>`. The token is kept in the profile folder (`identity_token.json`, until it
expires), so the next start logs in without the password; a refused token is dropped and the password asked again.
Register / Find ID / Find password on the login screen go to the authority. `--identity off` logs in with a local
account of the lobby itself (its own password).

### 8. Clients

- **Players**: `python tools/friend_pack_win.py --client <dc-client.exe> --server <VPS_IP> --fingerprint <fp>` (or
  `tools/friend_pack_mac.sh --server <VPS_IP> --fingerprint <fp>` on a Mac) builds the zip preset to the VPS, pinned to
  the lobby's certificate, with the registration steps in its README. Give each player the zip and one invite code.
- **You (the host's own client, now a remote player)**: the same package, or
  `dc-client --menu lobby --lobby <VPS_IP>:10000 --name <YOU> --rom <client folder>`.
- **Bots**: section 8a.

### 8a. Bots

`dc-botclient` runs on the server ROM (BOT-82: it reads the worlds, the tables and the first-person weapon models'
clip lengths, all in the server ROM) and takes the account password (BOT-83: `--login-password <pw>` or the
environment variable `DC_BOT_LOGIN_PASSWORD`).

The lobby can also run the bots itself, controlled from the dashboard's Bots tab: `[bots] managed = true`,
docs/BOTS.md "Bots the lobby runs". It replaces `bots.sh`; stop `bots.sh` before you switch.

**On the VPS** (recommended: the bots sit next to the servers, no extra internet hop). A bot that logs in through
loopback needs no account and no password: on a `--public` lobby loopback logins skip the account gate (LG-32,
LG-187), so `--check-password --no-auto-create` stays on for everybody else. Two ways:

```
sudo bash /opt/decombat/tools/bots.sh start 3            # VpsBot1..3 in the background, as decombat
sudo bash /opt/decombat/tools/bots.sh status             # which run, the end of their logs
sudo bash /opt/decombat/tools/bots.sh stop
```

or the unit (`bootstrap.sh` installs it, not enabled): `sudo systemctl enable --now decombat-bots` (3 bots, after the
lobby, at boot). Another count or prefix: `sudo systemctl edit decombat-bots` with
`[Service]` / `ExecStart=` / `ExecStart=/bin/bash /opt/decombat/tools/bots.sh run 2 --prefix VpsBot`. Use one of the
two, not both with the same prefix.

Each bot logs in at `127.0.0.1:10000` (a new bot account gets a random GP primary and secondary and a random face,
LG-260: start new names, or a new prefix, to re-roll), joins the first listed room (`--room any`: open a room, the bots come in) and
keeps to rooms with a player (BOT-84, `--filler <prefix>` from `bots.sh`'s `--prefix`): when the last player leaves a
room, or the running match, the bots leave too and wait for the next room with a player,
hunts and shoots back (`--shooter --hunt --skill 30-65`: each bot gets its own level from its name, BOT-91; `BOT_SKILL=lo-hi`
or a number in the environment changes it), keeps its seat for every next match (`--matches 0`) and starts
again 10 s after it exits (the room closed, the lobby restarted). `--args "<dc-botclient flags>"` adds flags, for
example `--args "--skill 70"`. Logs: `/opt/decombat/logs/bots/<name>.out` and `<name>.events.jsonl`. They read the
server ROM in `/opt/decombat/rom`.

**From the Windows PC** against the VPS: each bot needs an account there with a password, made once on the VPS:

```
sudo -u decombat /opt/decombat/bin/dc-lobby-server account --db /opt/decombat/data/ca.db add PcBot1 --password <bot pw>
sudo -u decombat /opt/decombat/bin/dc-lobby-server account --db /opt/decombat/data/ca.db add PcBot2 --password <bot pw>
```

then one bot by hand: `dc-botclient.exe --lobby <VPS_IP>:10000 --name PcBot1 --login-password <bot pw> --room any --shooter
--hunt --matches 0 --rom "<CA>"`.

### 9. When it fails

- `sudo journalctl -u decombat-lobby -n 100 --no-pager` (`-f` to follow): start-up refusals land here, for example
  `config: --public with --lobbyip 127.0.0.1 ...` (no `DC_SERVER__ADVERTISE_IP` in `/opt/decombat/config/decombat.env`:
  run `bootstrap.sh --public-ip <VPS_IP>` again, then `sudo systemctl restart decombat-lobby`). After five failed
  starts in 5 minutes systemd stops trying: `sudo systemctl reset-failed decombat-lobby` before the next start.
- `/opt/decombat/logs/dc-lobby-server.log`: logins (`login lockout` lines), rooms, game server starts and results.
- `/opt/decombat/logs/dserver/dserver_c<ch>_r<room>_g<game>.log`: one per match; the `client N: link 10 s: rtt ...,
  input rate ..., resends ...` lines and `client N: link summary` describe each remote player's connection
  (`status.sh` prints the latest). The client side: `logs\dc-client.net.jsonl`, `"ev":"remote_interp"` lines.
- The operator endpoint from Windows: `ssh -i <KEY> -L 10090:127.0.0.1:10090 root@<VPS_IP>`, then
  `curl http://127.0.0.1:10090/status` on Windows (SERVER.md sections 4 and 5; `POST /stop`, `POST
  /rooms/<ch>-<no>/close`).
- Nothing connects: the provider firewall (step 2) and, if enabled, `sudo ufw status`; `sudo ss -lntup | grep dc-`
  shows what listens. Login works but the match never loads: UDP 27015-27100 is closed somewhere, or the advertised
  address is wrong (`GET /status` `address`).
- `rom_check.sh` fails: the upload is incomplete or changed; push the ROM again (`push.ps1 -Rom ...`).

## Several instances on one box

A modded server is its own lobby instance (one mod set and one policy per lobby), and one
VPS runs several of them. Each instance has its own ports, DB (accounts, characters, progression), logs and mods
folder; the binaries (`/opt/decombat/bin`), the server ROM (`/opt/decombat/rom`) and the user `decombat` are shared.
The existing setup is the instance named `default` and stays exactly where it is (`/opt/decombat/config`, `data`,
`logs`, unit `decombat-lobby`). Every further instance lives in `/opt/decombat/instances/<name>/` and runs as
`decombat-lobby@<name>` (the template [examples/decombat-lobby@.service](../examples/decombat-lobby@.service):
the same flags and the same graceful stop as the default unit, the paths of the instance, `--admin` from the
instance's `decombat.env`). Example: the reference server `vanilla` is the default instance (TCP 10000 / 10002 /
10003 / 18080, UDP 27015-27100, admin 127.0.0.1:10090, the identity issuer) and `modded` a second one:

```
sudo bash /opt/decombat/tools/instance.sh modded create --auth-port 11000 --game-ports 27101-27186 --policy modded --identity-from default --server-name "DeCombat Modded"
```

This writes `/opt/decombat/instances/modded/config/server.toml` from `examples/server-vps.toml` with TCP auth 11000,
lobby 11002, clan 11003, web 19080 (auth + 8080), UDP 27101 (the first room) and 27102-27186 (further rooms),
`db = /opt/decombat/instances/modded/data/ca.db`, `log_dir`, `mods_dir = .../modded/mods`, `[mods] policy =
"modded"`, and `decombat.env` with `DC_ADMIN=127.0.0.1:10091` (the first free loopback port from 10091;
`--admin-port P` picks one) and `DC_BOT_LOBBY=127.0.0.1:11000`; the advertised address is copied from the default
instance's `DC_SERVER__ADVERTISE_IP` (or `--public-ip <VPS_IP>`). `create` refuses a TCP or UDP port that another
instance already uses (it reads their `server.toml` and `decombat.env`; `instance.sh list` shows them all) and a name
that exists. A modded instance needs its `[[mods.set]]` entries (and the zips in its `mods/`) before the first start:
`dc-config check` refuses `policy = "modded"` with an empty set, and so does the lobby.

Identity hand-over (section 7a): the default instance issues (`[identity] issue = true`, `issuer =
"decombat.vps"`); `--identity-from default` reads its issuer id from its `server.toml` and its public key from
`sudo -u decombat /opt/decombat/bin/dc-lobby-server identity --db /opt/decombat/data/ca.db`, and writes
`[identity] issuer = "decombat.vps"` and `public_key = "<44 characters>"` into the new instance: a player registered
on `vanilla` logs in on `modded` with the same token and gets a separate character there. `create` refuses
`--identity-from` when the source instance does not issue. By hand: `--identity-issuer <id> --identity-key <key>`;
an instance that issues on its own: `--identity-issue <id>`; none of them: standalone accounts.

Server list (LG-272, CLI-1622: mods come after the login, in the server list): players see
the `modded` instance under the vanilla instance's own servers ("US Central", "US West", ...) once the vanilla
instance lists it. `create` prints the lines; add them to the DEFAULT instance's `/opt/decombat/config/server.toml`
(next to its own `[[lobby.servers]]` entries, which stay as they are) and restart `decombat-lobby`:

```toml
[[lobby.servers]]
name = "Hacking Mod (US Central)"
region = ""
lobby = "<VPS_IP>:11000"        # the modded instance's auth port; data only, players never see it
policy = "modded"
fingerprint = ""                # the modded instance's certificate after its first start:
                                # sudo -u decombat /opt/decombat/bin/dc-lobby-server tls --db /opt/decombat/instances/modded/data/ca.db
```

An entry with `lobby` is no server of the vanilla lobby (no id, no channels, no game host): the lobby sends it to our
clients in its own push before the 2009 server list (0x3f02, `dc_wire::moddedlist`), which a 2009 client ignores.
The row shows a MODDED badge, the name and the region; the panel on the right lists its mods (when the community
document knows the server) and has the Mods button. Picking the row logs out of the vanilla lobby, reads and loads the
modded set (with the consent prompt for unverified mods), and logs into the modded instance with the identity token
of the vanilla instance (so `--identity-from default` is required, see below); the server list again returns to the
vanilla lobby with the token. An empty `fingerprint` means trust on first use for that instance's own certificate.

Firewall: `create` prints the lines and never runs `ufw` itself; add them, and the same in the provider's firewall:

```
sudo ufw allow 11000/tcp
sudo ufw allow 11002/tcp
sudo ufw allow 11003/tcp
sudo ufw allow 19080/tcp
sudo ufw allow 27101:27186/udp
```

Then:

```
sudo bash /opt/decombat/tools/instance.sh modded start     # systemctl enable --now decombat-lobby@modded
sudo bash /opt/decombat/tools/instance.sh modded status    # status.sh --instance modded
sudo bash /opt/decombat/tools/instance.sh modded stop      # graceful stop (matches end and are stored)
sudo bash /opt/decombat/tools/instance.sh modded remove    # stop, disable, folder to /opt/decombat/removed-instances
journalctl -u decombat-lobby@modded -f
```

`install.sh` installs both unit files and afterwards restarts every enabled instance, one after the other, each
with the graceful stop (the default `decombat-lobby` and each `decombat-lobby@<name>` with a folder in
`/opt/decombat/instances`). The tools take the instance from `--instance <name>` or `DC_INSTANCE=<name>`:
`status.sh --instance modded`, `bots.sh start 3 --instance modded` (logs in at the instance's `DC_BOT_LOBBY`, logs in
`/opt/decombat/instances/modded/logs/bots`). The variables of an instance's `decombat.env` that are not config
(no `__`): `DC_ADMIN` (its loopback operator endpoint: the unit passes it as `--admin`, `status.sh` asks it; for
`POST /stop` through an SSH tunnel use that port, for example `-L 10091:127.0.0.1:10091`) and `DC_BOT_LOBBY`. Without
`--instance` every script acts on the default instance as before. The `decombat-bots` unit serves the default
instance; bots for another instance: `bots.sh start N --instance <name>`.

Bots on a modded instance (LG-271): every login carries the client's mod-set hash and content hash, and a modded
lobby refuses a login whose hashes differ from its own (log line `client <addr>: mod set mismatch (client …, server
…)` or `mod content mismatch`). `dc-botclient` sends no hashes unless told, which only a vanilla lobby takes, so pass
the instance's hashes through `--args`: the mod-set hash is `mod_set_hash` in the instance's
`data/server_list.json` (also the start log line `mods: policy modded, … mod-set hash <hash>`), the content hash is
in the start log line `mods: content: … hash <hash>` (no such line: no content, pass `0`):

```bash
H=$(grep -o '"mod_set_hash": *"[0-9a-f]*"' /opt/decombat/instances/modded/data/server_list.json | grep -o '[0-9a-f]\{64\}')
sudo bash /opt/decombat/tools/bots.sh start 3 --instance modded --args "--mod-set-hash $H --content-hash 0"
```

A bot loads no mods: on an instance whose set patches weapons it plays with the ROM table while the game server uses
the patched one, so give it the content hash only for runs where that does not matter.

Server list: the two instances are two entries with the same address and different ports (`<VPS_IP>:10000`
vanilla, `<VPS_IP>:11000` modded). The client's server browser lists both once the community JSON
(HOSTING_AND_MODS.md section 8, decision 3) carries both entries; until then a player picks one with
`dc-client --lobby <VPS_IP>:11000`. Both share the TLS
fingerprint only if they share the key pair: each instance makes its own `tls_key.pem` next to its DB, so give the
players the modded instance's fingerprint too (`dc-lobby-server tls --db /opt/decombat/instances/modded/data/ca.db`).

### vanilla++: community maps on the Vanilla++ entry

Community maps need no instance of their own. They ride the Vanilla++ server entry of the vanilla lobby: a map made with `dc-mapkit` (docs/MAPKIT_AGENTS.md) is a curated pack like any
other (LG-313, docs/MODDING_GUIDE.md section 12), the entry's channels offer its maps, and the entry's game servers load
the world. Same lobby, DB, accounts and ports; the lines are in
[examples/server-vanilla-plus.toml](../examples/server-vanilla-plus.toml). Step by step, on the default instance:

```
# 1. the package: the zip a map's author exported (`dc-mapkit export <dir> --zip`, or `dc-modkit pack` on the exported
#    folder; the repository ships examples/mods/starter_map as a ready example), in the lobby's mods folder
#    (`paths.mods_dir`) under its SHA-256
sha256sum starter_map-0.1.0.zip
sudo -u decombat cp starter_map-0.1.0.zip /opt/decombat/data/mods/<sha256>.zip

#    From a pack repository's release instead (docs/PACKS_FROM_REPOSITORIES.md): one command fetches the tagged
#    release's zips, checks each against SHA256SUMS and the declared hash, installs them under their hash and prints
#    the rows; a branch name is refused, a re-run changes nothing, server.toml is not touched
sudo bash /opt/decombat/tools/packs_sync.sh argallo/DeCombat-mods v1 [--only variant_prism_scar,case_reel]
#    Or declare the release once in server.toml and press Sync packs on the dashboard's Updates tab
#    (docs/DEPLOY_FROM_ADMIN.md); `dc-config check --fs` then names the command for a missing zip:
#    [[packs.sources]]
#    repo = "argallo/DeCombat-mods"
#    tag = "v1"

# 2. the lines, from the review list (the map's row in community/vanilla_plus.json, MODDING_GUIDE.md section 12)
dc-config vanilla-plus-lines community/vanilla_plus.json

# 3. paste them into the Vanilla++ entry in /opt/decombat/config/server.toml; the entry's other lines stay as they are
[[lobby.servers]]
name = "Vanilla++"
capacity = 1000
host = "us-west"
mods = [
  { id = "starter_map", version = "0.1.0", sha256 = "<sha256>" },
]

# 4. the game host: a remote entry's agent must say `packs=1` and `contentlines=1` in its hello (specs/HOST_AGENT.md); an older agent is
#    updated with the same tools/host/push.sh line that installed it, without --rom (docs/HOST.md), between matches

# 5. check, restart, look (the entry's id is its position among the [[lobby.servers]] entries, from 1; 18080 is the
#    web port, auth + 8080)
sudo -u decombat /opt/decombat/bin/dc-config check /opt/decombat/config/server.toml --fs
sudo systemctl restart decombat-lobby
curl -s http://127.0.0.1:18080/entry_mods/<entry id>.json
```

The document lists the entry's set, `community_map_base` (the index of its first map, after the ROM's rows, MAP-35) and
`maps`, one record per map. A pack that did not load is in the start log (`mods: ... curated pack '<id>' left out: ...`,
LG-313) and the server runs without it; a record whose world file is missing, or whose mod is not `shared`, is left out
of `maps` with a log line. `dc-config check ... --fs` names a zip that is not in the mods folder.

Players pick the entry in the server list. The client loads the set at the pick from the lobby's web port, with no
consent prompt (CLI-1643), and the waiting room on that entry lists the maps after the ROM's; other entries do not see
them. A room on such a map starts its game server with the world from the pack: on the lobby's own machine directly, on
a remote host after the agent has the pack (cached across starts). The lobby refuses a community room on a host whose
hello lacks `packs=1` and logs why. More maps: more zips and `mods` lines; 0x311 carries 30 (map, mode) pairs at most
(MAP-35).

Players' own cosmetic mods (a crosshair such as `drawn_crosshair`, kind `client_visual`) are refused on a vanilla
server. The Vanilla++ entry lets them load with one more line in the entry, after `mods` (LG-426; `dc-config check`
refuses it on an entry without `mods`, on another lobby's entry and on a modded lobby):

```
client_visual_allowed = true
```

The lobby must run a build that knows the key before the line goes in: an older `dc-lobby-server` refuses the unknown
key and does not start. The set hash and the pick's claim do not change, so clients need no update; an older client
keeps joining the entry and only its local mods stay off.

Players may also pick their field of view (Options, Graphic tab, "Field of View", 70 to 120, the 4:3-horizontal angle;
90 is the 2009 view) on this entry. The lobby publishes the range in the entry's set document as `client_fov: [min, max]`
(LG-429, CLI-1916); without a key it is `[70, 120]`, so a Vanilla++ entry needs no line for it. `client_fov = [90, 90]` in
the entry locks the row to the 2009 view; `dc-config check` takes 60 to 130 with the minimum not above the maximum, and
refuses the key on an entry without `mods`. As with `client_visual_allowed`, the lobby must run a build that knows the
key before a line goes in; the key is no part of the set hash, so clients need no update, and an older client keeps its
2009 view.

A separate modded lobby still offers maps the generic way: `[mods] policy = "modded"` with the map mods in `[[mods.set]]`
(the steps at the top of this section, MAP-35 .. MAP-39). It is not how Vanilla++ works; it has its own DB, ports and
mod consent prompt.

On a developer's machine the same shape comes from `dc-mapkit play <project> [--headless]` (a vanilla lobby with the
project's own export on the entry "Vanilla++", its game server and the client on 127.0.0.1; `--modded` for the modded
lobby; DC_DEFAULTS.md MAP-67 .. MAP-69, MAP-89) or `tools/vanilla_plus_mac.sh` on a Mac; the proof that the whole chain
plays is `cargo test -p dc-client --test vanilla_plus` (MAP-72).

## Live games in Discord

An instance can write a public-only snapshot of its running matches into `/var/lib/decombat-public-games/<instance>.json`
(`DC_MONITOR__PUBLIC_GAMES_FILE` in its `decombat.env`), and `tools/vps/live_games.py` with two systemd units keeps one
Discord message current from those files. Nothing is installed or enabled by a deploy: docs/LIVE_GAMES.md.

## Deploys from the dashboard

After a one-time setup as root (`tools/vps/deploy_setup.sh`, a read-only GitHub deploy key), the dashboard's Updates tab
builds and installs a commit of master on this box and offers it to the game hosts: docs/DEPLOY_FROM_ADMIN.md.

## Bug reports from Discord

A small job reads the bug-report channel of the Discord server with a read-only bot and the dashboard's Updates
tab lists the messages (`tools/vps/triage_setup.sh`, once, after you wrote `deploy/triage.conf` by hand): docs/DEPLOY_FROM_ADMIN.md.

## Security

- The operator endpoint (`--admin 127.0.0.1:10090`) is loopback only; reach it through an SSH tunnel, never open it
  in a firewall. `--admin-public` would need `--admin-token` (SERVER.md section 4); the unit does not use it.
- Registration is open (`--register-open`, the open beta since 2026-11-08, LG-433; `--register` alone asks for an
  invite code), unknown names are refused (`--no-auto-create`) and passwords are
  checked (`--check-password`, Argon2id in the DB). Five failed logins lock a name or an address for 60 s.
- The login (auth port 10000) and the registration page (18080) are TLS-encrypted; the lobby refuses plain logins
  from outside (docs/SERVER.md section 4b). Keep `tls_key.pem` private; if you replace or lose the pair, give the
  players the new fingerprint. The lobby and clan ports and the game traffic stay plain (no password on them).
- Password recovery mails need the `[mail]` keys in `server.toml` (docs/SERVER.md section 4b); put the SMTP
  password into `config/decombat.env` as `DC_MAIL__PASSWORD=...`, not into the file.
- The server ROM is the game's own data from your own 2009 client: it goes to your VPS only, never into the
  repository, a CI artifact, a friend pack or a public download. The CI artifact holds only our binaries and scripts.
- SSH: key login only (disable password login in `/etc/ssh/sshd_config`: `PasswordAuthentication no`), keep the key
  file off the repository, and keep the VPS updated (`sudo apt update && sudo apt upgrade`; unattended-upgrades is on
  by default on Ubuntu).
- The scripts and the repository hold no address, key or password; they are parameters.
