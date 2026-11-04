# HOSTING.md: hosting a DeCombat server for players outside your network

Rules: LG-30 (per-peer announced address), LG-31 (`--public`), LG-32 (account gate), CLI-810 (`--login-password`) in
DC_DEFAULTS.md.

Servers on a machine without the client folder (a VPS): make a server ROM with `dc-server-rom` (650 MB instead of
the 3.2 GB folder, no executables) and point `CA_ROM` at it, see `docs/SERVER_ROM.md`.

Servers on a Linux VPS, so the host plays as a remote player too: [docs/VPS.md](docs/VPS.md) (order of operations,
`tools/vps/` scripts, `examples/server-vps.toml`, the CI artifact).

## Start

Running a server (config file, stop and status commands, keeping it running as a service, backups):
[docs/SERVER.md](docs/SERVER.md). For players outside your network, start the lobby with these flags on top:

```
dc-lobby-server --config server.toml --dserver --dserver-exe <dc-dserver> --public --lobbyip <YOUR_PUBLIC_IP_OR_DNS_NAME> --gameip <YOUR_PUBLIC_IP_OR_DNS_NAME> --check-password --no-auto-create --add-account NAME:PASSWORD
```

- `--public` listens on 0.0.0.0; `--lobbyip` / `--gameip` = your public address. Give one `--add-account NAME:PASSWORD`
  per player, or use the account CLI below. Give every dc-dserver `-bindip 0.0.0.0` (`--dserver-args`).
- A client on the server's own machine joins through 127.0.0.1 and needs no account or password (LG-32 exempts
  loopback logins on a public server). A LAN machine is told the LAN address it reached (LG-30), so no router
  hairpin is needed; it logs in like a remote player (needs an account).
- `--public` refuses 127.0.0.1, localhost and 0.0.0.0 (LG-31): give the router's WAN address (what
  "what is my IP" shows) or a dynamic-DNS name.

## Ports

| what | protocol | port |
|---|---|---|
| auth (login, server list); TLS (LG-220) | TCP | 10000 |
| lobby | TCP | 10002 |
| clan lobby | TCP | 10003 |
| clan web pages, registration page (`--register`); TLS (https) | TCP | 18080 |
| game server, first room | UDP | 27015 |
| game servers of further rooms playing at the same time | UDP | 27016-27100 |

## One-time setup (elevated PowerShell or cmd: Run as administrator)

```
netsh advfirewall firewall add rule name="DeCombat lobby" dir=in action=allow protocol=TCP localport=10000,10002,10003,18080 program="<...>\dc-lobby-server.exe"
netsh advfirewall firewall add rule name="DeCombat game" dir=in action=allow protocol=UDP localport=27015,27016-27100 program="<...>\dc-dserver.exe"
```

Append ` remoteip=<FRIEND_IP>` to both to scope them (`--allow`); with the LG-32 account gate an unscoped rule is
fine. Remove later with `netsh advfirewall firewall delete rule name="DeCombat lobby"` (and `"DeCombat game"`).
Windows may also show its own firewall prompt the first time the servers listen on 0.0.0.0.

Router: forward each port of the table to this machine's LAN address (same port number inside and outside). UDP
27016-27100 is only needed when two rooms play at once (the whole block is Steam's client range, which campus and corporate networks usually allow). The lobby takes the further-room range from `<db dir>/lobby.json`: `{"dserver.port_range": [27016, 27100]}` (its built-in default is still 29901-29999).

## What a player needs

- Their own 2009 Combat Arms client folder (nothing of the game is shipped).
- The Windows package (`python tools/friend_pack_win.py --client <dc-client.exe> --server <host> [--fingerprint <fp>]`):
  `dc-client.exe`, `DeCombat.bat` and a README, preset to your server. The player puts their 2009 client folder next
  to `DeCombat.bat` and double-clicks it; the first start asks for the account name, the login screen takes the
  password. When the host's lobby issues identity tokens (docs/VPS.md §7a), name and password buy a token there first
  and the login carries only the token, which the player's profile keeps (`identity_token.json`) until it expires
  (`--identity off` keeps the plain password login, CLI-1610).
- The macOS player pack (`tools/friend_pack_mac.sh --server <host> [--fingerprint <fp>]`, run on a Mac with the
  client built): `DeCombat.app` (the client in an app bundle, universal when `target/x86_64-apple-darwin/release/
  dc-client` was built too; the server and the fingerprint preset) and a README. The player puts their 2009 client
  folder next to the app and double-clicks it; the first start asks for the account name, the login screen takes
  the password. Unsigned by Apple, so the first start needs "Open Anyway" in Privacy & Security. The zip goes to
  `out_player/`.
- Both packages and the server tarball are also built by GitHub Actions from one commit (`docs/RELEASES.md`,
  `.github/workflows/release.yml`), with the server address and fingerprint from the repository's variables.
- No port forwarding on their side: the game server answers each client at its observed UDP source address and
  the client sends at least every 200 ms once joined (keep-alive), so their router's mapping never expires.

## Playing alone with bots

See [docs/BOTS.md](docs/BOTS.md).

## Accounts

Rules LG-140..146 and CLI-1290 in DC_DEFAULTS.md (branch `accounts-r1`). Passwords are only checked when the lobby
runs with `--check-password`; add `--no-auto-create` so an unknown name is refused
instead of becoming a new account. Passwords are stored as Argon2id; older salted SHA-256 rows still work and are
rewritten on their next good login.

**Account CLI.** Works on the lobby's DB file, also while the lobby runs (a change counts from the next login).
The DB path is in the lobby's start-up line (`db ...`); without `--db` the CLI uses `<exe dir>/data/ca.db`.

```
dc-lobby-server account --db <ca.db> add alice --password <pw>     # make an account with a password
dc-lobby-server account --db <ca.db> add bob --invite               # account without a password + a code for bob
dc-lobby-server account --db <ca.db> invite --count 5 --days 7      # five one-time codes for any new name
dc-lobby-server account --db <ca.db> reset-password alice <pw>
dc-lobby-server account --db <ca.db> disable alice                  # refused at login (wrong-password message)
dc-lobby-server account --db <ca.db> enable alice
dc-lobby-server account --db <ca.db> list                           # accounts, password kind, state, gm, open codes
dc-lobby-server account --db <ca.db> gm alice --on                  # game master (--off: player again)
dc-lobby-server account --db <ca.db> add alice --password <pw> --email alice@example.org   # with a recovery address
dc-lobby-server account --db <ca.db> set-email alice alice@example.org                     # add or change it later
dc-lobby-server account --db <ca.db> grant alice --nx 10000                              # give NX (also --nx-credit, --gp, --exp; negative takes away)
```

`list` shows the address masked (`a***@example.org`); the lobby log never holds a full address.

**Game masters** (LG-170..175). `gm NAME --on` makes the account a GM: from its next login the game gives it the
operator commands (`/` lines in chat, `/k NAME` and "Kick User" in a room), it cannot be kicked from a room, its
room kicks show "Kicked by GM." and it gets an operator slot on a full channel. A session that is already logged in
keeps its old privilege until it logs in again. The orange GM icon and yellow chat follow the character NAME: the
game draws them for any name that begins with `GM` (exact case), so only GM accounts may take such a name; a GM who
wants the icon creates a character named `GM...` (characters are not renamed automatically). The registration page
refuses login names beginning with `GM`.

Names are 3 to 16 characters (letters A-Z, digits, `_ - .`, unique in any case); passwords 6 to 16 characters
(the game sends at most 16). A code is valid for `--days` days (default 7, `0` = no expiry) and works once.
A password given on the command line can end up in the shell history; `add NAME --invite` lets the player choose
it on the registration page instead. A disabled player who is already in the lobby stays until they log out.

**Registration page.** Start the lobby with `--register` (invite code required) or `--register-open` (no code;
anybody who reaches the port can make an account). The page is `http://<host>:<auth port + 8080>/register`
(`http://<host>:18080/register` with the default ports; `--clanweb` moves it, `--clanweb 0` turns the web port and
the page off), so forward and open TCP 18080 too. The form asks for name, password twice and the invite code, shows
errors on the page and, on success, the server address and port to type into the game. At most 5 attempts per
address in 10 minutes; the form body is capped at 4 KB. In the game, the login screen's register button opens an
in-game dialog that talks to the same endpoint (auth host, port + 8080; CLI-1450); with registration
off the dialog and the page say so. The dialog asks for an email address when the lobby asks for one, and the
login screen's Find ID / Find password buttons open dialogs that use the lobby's `/recover` mail (CLI-1500 .. CLI-1503;
without mail set up they say "not set up on this server").

**Password and login-ID recovery** (LG-240 .. LG-245). Registration (page and in-game dialog) asks for an e-mail
address; it is stored with the account and only used to mail a code. With a mail server set up (the `[mail]` keys
in docs/SERVER.md section 4b), a player who forgot the password asks for a code with login name and address (the
login screen's Find password button, or the page `https://<host>:18080/recover`), gets an 8-character code by mail,
and sets a new password with it within 15 minutes (5 wrong entries burn the code, at most 3 codes per account an
hour). Find ID mails the login names registered with an address. The answers never say whether a name or an address
exists. Without a mail server the buttons and the page say recovery is not set up: reset the password yourself with
`account reset-password`. `mail.dry_run = true` writes the mails into the lobby log instead (local tests).

**Lockout.** Five failed logins for one name (from any address) or from one address (for any name) within 10
minutes refuse that name or address for 60 s, even with the right password; the lobby log has one `login lockout`
line per lock. A good login clears its name's count. Loopback logins on a `--public` server are exempt, as from the
account gate.

**What the login sends, and how it is protected.** The 2009 auth connection is plain TCP (PROTOCOL_2009.md §1.1):
the login request 0x200 carries the login token (`LOCAL:<name>` from our client), the password box text as UTF-16
(up to 16 characters, at +0x2006), an adapter address text and a partner number (PROTOCOL_2009.md §2.2). DeCombat
wraps the auth port and the web port (registration page) in TLS 1.3 (our own deviation, LG-220 .. LG-226): a public
lobby refuses plain logins from outside, so no password crosses the network in clear text. `dc-lobby-server tls --db <ca.db>`
prints the lobby's certificate fingerprint; give it to your players (`--fingerprint <fp>` of the package scripts puts it
into their launcher), or let their client remember the certificate at the first login. The
2009 client itself cannot speak TLS: it can only reach a public lobby started with `--plain-auth-public` (unsafe,
trusted networks only). A reverse proxy, tunnel or port forwarder on the same host as the lobby makes remote players look like loopback (the lobby judges the socket peer), so their passwords would cross the public leg in clear text. If one runs there, start the lobby with `--tls loopback`; the lobby logs this at start-up when `--public` is on and `--tls` is not `loopback`. Details, certificate replacement and pinning: docs/SERVER.md section 4b. Passwords are still
best not shared with other services.

## Watching the connections

During a match `tools\server_net.ps1 -Watch` (or `curl http://127.0.0.1:10090/net`) lists every player's round trip
(mean / min / max of the last 10 s), input jitter, resends and flow drops as the game server measures them, and the
in-match scoreboard shows each player's ping. After the match, collect the remote players' `logs\dc-client.net.jsonl`
and run `python -I tools/net_report.py --dserver <the match's dserver log> <their logs...>` for a per-player summary,
or `--compare` two sessions (for example before and after moving the server to a VPS). Details: docs/SERVER.md
section 4a.

