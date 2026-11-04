# dc-host: the remote game host agent

`dc-host` runs on a machine that hosts matches for a lobby elsewhere (specs/HOST_AGENT.md: the "US West" entry
whose game servers run at a house while the accounts stay with the lobby). It dials OUT to the lobby's host port
over TLS, pins the lobby's certificate by its SHA-256 fingerprint, says hello with its name and token, and starts
one `dc-dserver` per match the lobby asks for. The game servers' stdin / stdout pipe lines (specs/DSERVER_PIPE.md)
travel over that one connection. Nothing listens on TCP at the host: only the UDP `port_range` is forwarded.

```
dc-host [--config FILE]     # default: host.toml next to the dc-host executable
dc-host --version           # the git short sha it was built from ("unknown" without git; DC_GIT_COMMIT at build wins)
```

It logs to stderr with UTC timestamps (the systemd journal keeps them) and exits 2 on a bad config.

## host.toml

```toml
name = "us-west"                    # required; the [[hosts.agents]] name in the lobby's server.toml
lobby = "lobby.example.org:10005"       # required; the lobby's host port (hosts.port, default auth_port + 5)
fingerprint = "AA:BB:..."           # required; the SHA-256 of the lobby certificate, as the lobby log prints it
token = "..."                       # required; the agent's shared secret, at least 24 characters
dserver_exe = "./dc-dserver"        # default dc-dserver; relative = next to the dc-host executable
rom = "/opt/decombat/rom"           # the server ROM (CA_ROM of the game servers); empty = the agent's own CA_ROM
port_range = "27888-27899"          # default; the UDP ports the router forwards here, one match per port
bind_ip = "0.0.0.0"                 # default; probed for free ports and passed as -bindip
log_dir = "/opt/decombat/logs"      # default "logs"; the game servers' log files and working directory
dserver_args = ""                   # extra arguments of every game server (whitespace-separated)
```

Relative `rom` and `log_dir` are taken from the agent's working directory. Unknown keys are refused. The
fingerprint may be written with or without colons, in either case. Keep the file readable by the service user only
(it holds the token).

## What the agent does

- Connects, TLS 1.3, accepts exactly the pinned certificate (no CA roots, no trust on first use), sends
  `hello name= token= version= ports=lo-hi running=n` and waits up to 15 s for `ok` or `refuse`. On any failure it
  reconnects after 1, 2, 4 ... 30 s (1 s after a connection that was accepted).
- `start <gid> <json>`: picks the requested port when it is in the range and free, else the first port of the range
  that a UDP bind on `bind_ip` gets and no game server of its own uses, and runs
  `dserver_exe -port P -world W -guid G -idle I -log <log_dir>/<log> -bindip <bind_ip> <dserver_args> <lobby args>`
  with `CA_ROM=<rom>`, working directory `log_dir`, its own process group, no shell. The lobby's `args` keep only
  flags (tokens starting with `-`) with the value after each; the agent's own six flags cannot be repeated. The `log`
  name must be a plain file name. Answers `started <gid> port= pid=` or `failed <gid> <reason>`.
- Every stdout line of a game server goes to the lobby as `event <gid> <text>`; stderr goes to the agent's log.
  `line <gid> <text>` writes one stdin line; `quit <gid>` closes stdin and kills the process group after 5 s;
  `exited <gid> code=<c>` (-1 = killed) when it ends.
- After every `ok` it lists the game servers still running from before as `running <gid> port= pid=`. While no
  connection is up the servers keep running and their lines are dropped.
- Answers `ping` with `pong`, pings after 20 s of its own silence, and closes a connection that brought nothing for
  60 s. Lines over 16 KiB are dropped with a log line, both ways.

## systemd

`examples/decombat-host.service` (repository root):

```ini
[Service]
Type=simple
User=decombat
Group=decombat
WorkingDirectory=/opt/decombat
ExecStart=/opt/decombat/bin/dc-host --config /opt/decombat/config/host.toml
Restart=always
RestartSec=5
```

A stop or restart of the unit ends the game servers too (the default `KillMode=control-group`): a new agent process
cannot take over the pipes of the old one's children. A dropped connection alone keeps them running and the next
connection lists them.

## Tests

`cargo test -p dc-host`: the line parser and writer, the 16 KiB splitter, the start request and argument filter, the
port picker, the pin verifier, the config; `tests/agent.rs` starts the real binary against a fake TLS lobby and the
stub game server `examples/stub_dserver.rs` (hello, start, event, line, ping, quit, exited, the grace kill, the
reconnect with `running`, a refused hello, a wrong certificate, a config without fingerprint or token).
