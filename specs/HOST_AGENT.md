# HOST_AGENT.md: remote game hosts (a second region on another machine)

Written 2026-10-31 (macOS work) for the user's request: a second server entry "US West" whose matches run on a
Linux machine at the user's house, while every account stays with the lobby in Chicago ("US Central"). Our own
design; the 2009 servers had one game-host pool per lobby too, but nothing of their host protocol is known.

## 1. Topology

- One `dc-lobby-server` (Chicago) keeps doing everything it does today: login, server list, channels, rooms, clan,
  shop, the SQLite database. It is the only owner of account state.
- `dc-host` (new binary, crate `crates/dc-host`) runs on the remote machine. It owns a UDP port range the router
  forwards to it, the `dc-dserver` binary and the server ROM, and starts one `dc-dserver` per match on request.
- The agent dials OUT to the lobby (the lobby's new host port, TLS with the lobby's existing certificate, the agent
  pinning the fingerprint like a client). No inbound TCP at the house: only the UDP game range is forwarded.
- The lobby keeps one control connection per agent and multiplexes the game servers' stdin/stdout pipe lines
  (DSERVER_PIPE.md) over it. Everything the lobby reads from a local child's stdout it reads from the agent instead;
  the pipe contract itself does not change, so XP, GP, kills and HiSec results land in the database as today.
- The server list: each `[[lobby.servers]]` entry names its host (`host = ""` = the lobby's own machine, as today;
  `host = "us-west"` = the agent of that name). A room's channel belongs to one server entry; its match starts on
  that entry's host; the game-start packet carries the host's public address and the port the agent chose.

## 2. Configuration

Lobby (`server.toml`):

```toml
[[lobby.servers]]
name = "US Central"
capacity = 1000
host = ""                 # local game servers (default)
channels = 3              # channels of this entry (default 3; the clan entry has its own rooms as today)

[[lobby.servers]]
name = "US West"
capacity = 1000
host = "us-west"
channels = 3

[hosts]
port = 0                  # the host port the agents dial; 0 = auth_port + 5 (10005)

[[hosts.agents]]
name = "us-west"
token = "..."             # shared secret (at least 24 characters); the agent sends it in its hello
public_address = ""       # the address advertised for its game servers; "" = the address the agent connected from
```

The clan entry (id 2) stays the implicit local one when `lobby.servers` has one entry, as before (CD-21).

Agent (`host.toml`, next to `dc-host`):

```toml
name = "us-west"
lobby = "lobby.example.org:10005"
fingerprint = "AA:BB:..."  # the lobby certificate's SHA-256 (the client's pin); required
token = "..."
dserver_exe = "./dc-dserver"
rom = "/opt/decombat/rom"  # the server ROM folder (CA_ROM of the game servers)
port_range = "27888-27899" # the UDP ports the router forwards here; one match per port
bind_ip = "0.0.0.0"
log_dir = "/opt/decombat/logs"
dserver_args = ""          # extra arguments for every game server (the lobby's own list is added to it)
```

## 3. Control protocol (one TLS stream, UTF-8 lines ending in `\n`)

Hello, agent first:

```
hello name=<name> token=<token> version=<git short sha or "unknown"> ports=<lo>-<hi> running=<n> content=1 packs=1 update=1
```

`content=1`: the agent takes a `start` request's weapon content. `packs=1`: it takes map packs (below). An agent without
one of them is refused the matches that need it, with a log line on the lobby; every other match is unchanged.
`update=1`: it takes the lobby's server build (section "Updates" below); an agent without it is never offered one.

The lobby answers one of:

```
ok name=<name> lobby=<lobby version>
refuse <reason>              # unknown name, bad token, a second connection of the same name; the lobby closes
```

After `ok`, both sides send lines freely. `<gid>` is the lobby's game id (a decimal number, unique per lobby run).

Lobby to agent:

```
start <gid> <json>           # {"port":0,"world":"...","guid":"...","idle":30,"log":"dserver_c1_r3_g9.log","args":["-tickhz","30"]}
                             # port 0 = the agent picks a free port of its range; a non-zero port is a request
                             # optional "content" + "content_hash" (LG-313): a featured entry's weapon content; the
                             # agent writes it to content-<hash>.json in its log folder and passes it as -content.
                             # An agent that takes this says content=1 in its hello; the lobby refuses to start such
                             # a match on an agent without it, and a request over 16 KiB.
                             # optional "world_pack" + "world_file" + "announce" (MAP-83): a featured entry's community
                             # map, see "Map packs" below; "world" is then the record's @mod: name, not a path.
pack <sha> <base64>          # one piece of a map pack, in order (MAP-84); only before the `start` that needs it
line <gid> <text>            # one stdin line for that game server (verbatim; DSERVER_PIPE.md §3)
quit <gid>                   # close its stdin, then the grace kill (DSERVER_PIPE.md §2: 5 s)
ping <n>
```

Agent to lobby:

```
started <gid> port=<p> pid=<pid>
failed <gid> <reason>        # could not start (no free port, exe missing, spawn error)
event <gid> <text>           # one stdout line of that game server, verbatim (the `@ready` ... `@end` events and
                             # the engine's console lines alike; the lobby parses them as it parses a local pipe)
exited <gid> code=<c>        # the process ended (code -1: killed / no code)
running <gid> port=<p> pid=<pid>   # after a reconnect: a game server still running from before (the lobby
                             # reattaches when it still knows the gid, else sends quit)
have <sha>                   # after every `ok`: a complete map pack this agent keeps (MAP-84)
pong <n>
```

Rules: a `start` for a gid the agent already runs is answered with `started` again. Lines longer than 16 KiB are
dropped with a `log` line. The agent never interprets stdin lines; the lobby never interprets console lines beyond
what DSERVER_PIPE.md says. Either side may send `ping`; a side that hears nothing for 60 s closes the stream.

### Updates (the lobby's server build for its hosts; DC_DEFAULTS.md HA-2, docs/DEPLOY_FROM_ADMIN.md)

A lobby with `[deploy] dir` offers the build of `<dir>/host/desired.txt` (written by a dashboard deploy) to each agent
that said `update=1` and runs another build, when that build is the lobby's own or was published after the lobby
started:

```
update <commit> <sha256> <size> <tarball> <seq> <sig>
                                             # lobby: after `ok` and at its pings; again after 120 s for the same build;
                                             # <seq> and <sig> are root's (the VPS deploy signs them), passed on as they are
getupdate <commit>                           # agent: no game server runs here and it does not hold that build
updpart <commit> <base64>                    # lobby: 12000 raw bytes per line, in order (from a thread of its own)
updend <commit>                              # lobby: the last piece went
updateno <commit> <reason>                   # lobby: not now (another build, a match of this host runs, 3 transfers
                                             # on this connection already, the file changed on disk)
```

The agent appends the pieces to `<log_dir>/../incoming/.<tarball>.part` (at most `size` bytes, 256 MiB cap), checks size
and SHA-256 at `updend`, renames it to `<tarball>`, deletes older tarballs there and writes `incoming/desired.txt`
(`commit=`, `tarball=`, `sha256=`, `size=`, `seq=`, `sig=`). A new connection drops an unfinished transfer. The agent
installs nothing: `tools/host/update.sh` with `SOURCE=lobby` (root, its own timer) takes the offer only when `sig`
(base64 of an `ssh-keygen -Y sign` file, namespace `decombat-host-update`, identity `decombat-deploy`) verifies over
the message `decombat-host-update 1` / `commit=` / `tarball=` / `sha256=` / `size=` / `seq=` (one line each) against
the key pinned at its setup and `seq` is above the last it took; neither the lobby nor the agent holds that key
(docs/DEPLOY_FROM_ADMIN.md). While pieces go out, the agent's other lines from the lobby wait behind them; a transfer only
starts while the host runs no match.

### Map packs (a featured entry's community maps; DC_DEFAULTS.md MAP-83, MAP-84)

A room on a community map of a featured entry (`[[lobby.servers]] mods`, LG-313) needs the world on the host. The
request names it:

```
start 9 {"port":0,"world":"@mod:valley/assets/maps/valley.dat","guid":"ca","idle":30,"log":"g9.log","args":[],
         "world_pack":{"sha256":"<64 hex>","size":3871234,"url":"/mods/<64 hex>.zip"},
         "world_file":"assets/maps/valley.dat","announce":"@mod:valley/assets/maps/valley.dat"}
```

(one line on the wire). `world_file` is the world's path inside the pack (`/`-separated plain names), `announce` the
record's `world` verbatim, `url` the pack's path on the lobby's web port. All three come together or not at all.

The bytes come over this stream, not over HTTP: the agent has no HTTP client, and the control stream already carries the
pin. Unless the agent said `have <sha>` or got the pack on this connection, the lobby sends the whole zip as
`pack <sha> <base64>` lines right before the `start` (12000 raw bytes per line, so a line stays under 16 KiB and each
piece decodes alone; all of them queued at once, so nothing of another start falls between them). While they go,
other game servers' lines of that agent wait behind them. The `@ready` clock of the match (`--dserver-timeout`, 30 s by
default) runs from the start of the transfer: a pack that takes longer fails that one start like any late server, and the
next start finds the pack on the host.

The agent writes the pieces to `<log_dir>/../packs/<sha>.zip.part` and, at the `start`, checks the size and the SHA-256,
renames it to `<sha>.zip` and unpacks it with `dc_modkit::read_stored_zip` into `packs/<sha>/` (built beside it and
renamed, so that folder is always complete). The folders stay across starts, reconnects and restarts; nothing prunes
them yet. A cut-off stream starts over on the next connection. At most 64 MiB per stream, 32 MiB unpacked.

A pack that fails answers `failed <gid> pack <sha>: <reason>` (wrong bytes are deleted). The lobby then forgets that the
agent holds it, the room goes back to waiting, and the next start sends it again.

The game server then gets `-world <abs>` and, after every other argument, `-worldfile <abs> -announce <announce>
-worldobjects world`, with `<abs>` the world's file in `packs/<sha>/`.

## 4. Connection life

- The agent connects at start and reconnects with backoff (1, 2, 4 ... 30 s). While disconnected it keeps its game
  servers running and buffers nothing: `event` lines of that time are lost (a match that ends meanwhile reports its
  results to nobody; accepted for now, noted as open).
- The lobby marks an agent offline when its stream ends. A room start on an offline host fails like a local spawn
  failure (the room goes back to waiting, the players see the start refused). Games of that host whose process the
  lobby no longer hears from are closed after the existing `@end` time-out rules.
- On a reconnect the agent lists its running servers; the lobby reattaches the ones it still tracks and quits the
  rest.

## 5. Security

- The token is compared in constant time; a wrong token or name is refused after a short delay and logged with the
  peer address. The host port is TLS only (no plain mode): the certificate is the lobby's (LG-222 fingerprint).
- The agent trusts only the pinned fingerprint. It never runs anything but `dserver_exe` with the arguments the
  protocol allows (`-port`, `-world`, `-guid`, `-idle`, `-log`, `-bindip`, the configured extra arguments, the
  lobby's `args` list filtered to tokens that start with `-`, and for a map pack `-worldfile`, `-announce`,
  `-worldobjects world`), each argument passed as one `argv` entry: no shell. The lobby's `args` cannot name any of the
  agent's own flags, `-content`, `-worldfile`, `-announce` and `-worldobjects` included: so a lobby's `--dserver-args`
  no longer sets `-worldobjects` on a remote host; `dserver_args` in `host.toml` does.
- A pack is only ever written under `packs/`: its name must be 64 lower-case hex digits, `world_file` must stay inside
  the pack, the zip reader refuses absolute paths, `..`, backslashes, drive letters and symlinks. A pack runs no code on
  the host: the agent unpacks it and the game server reads the world file.
- The agent runs as an unprivileged user; its working directory is `log_dir`. It needs write access to the parent of
  `log_dir` for `packs/` (`tools/host/push.sh` gives the user all of `/opt/decombat`).

## 6. Ports and addresses

- The agent probes a UDP bind on `bind_ip` for each port of its range and takes the first free one not already
  used by one of its own game servers; it passes `-port <p>` and `-bindip <bind_ip>` to the server.
- The lobby advertises `public_address` of the agent (or the address the control connection came from) with that
  port in the game-start packet. The router must forward the whole range to the machine.

## 7. Deployment

- `tools/host/push.sh --host IP --user NAME --key FILE --lobby HOST:PORT --fingerprint FP --token T --name N
  [--rom DIR] [--build]`: rsyncs the checkout, builds `dc-dserver` and `dc-host` on the machine (rustup there),
  installs them under `/opt/decombat` (sudo for `mkdir`, `install`, `chown`, the systemd unit and `systemctl`),
  writes `host.toml`, copies the server ROM once, enables and starts `decombat-host.service`.
- Updating: rerun with `--build`; the lobby keeps running. A dropped control connection (network) is survived: the
  agent keeps its game servers and lists them as `running` on reconnect. An agent RESTART is not: the new process
  cannot take over the old one's pipes, the orphaned game servers see stdin EOF and end (the unit's default
  KillMode ends them with the agent), so update the host between matches.

## 8. Tests

- `dc-host`: unit tests of the line parser / writer, the port picker and the pack store; integration tests that start
  the agent against a fake lobby (a TLS listener in the test with a throwaway certificate) and a stub `dc-dserver`
  (a script that prints `@ready` and echoes), checking hello, start, event, line, quit, exited, a bad pack.
- Lobby: the host registry (hello / refuse, one connection per name, offline marking), a room start on a remote
  host through a fake agent on a loopback stream (started, `@ready` event through to the game-start packet with
  the host's address and port, `@result` lines reaching the result path), and the server-list / channel partition
  (CD-21 amended). The desktop local path keeps its tests unchanged.
- Both: `crates/dc-lobby-server/tests/entry_map.rs` runs the real lobby and the real `dc-host` on loopback over the
  pinned TLS stream with the committed starter map as a featured entry's pack (only the game server is a stand-in).
