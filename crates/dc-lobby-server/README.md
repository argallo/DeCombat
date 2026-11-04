# dc-lobby-server

The DeCombat lobby, auth and clan server for the 2009 Combat Arms client (ROM model: the game folder is read at run
time from `CA_ROM`, nothing from it is stored here). One process serves auth, the lobby, the clan lobby and a small
clan web page, keeps accounts in SQLite, and can start one `dc-dserver` game server per room.

## Run

    dc-lobby-server [flags]              # serve
    dc-lobby-server doctor [flags]       # check the set-up, print a table, exit 0 (no FAIL) / 1 / 2 (usage)
    dc-lobby-server --help               # every flag and the precedence order

Common flags (all have defaults; `--help` lists them all):

| flag | meaning |
|---|---|
| `--port`, `--lobbyport`, `--clanport`, `--clanweb` | auth, lobby, clan-lobby and clan-web ports (0 = derived, `--clanweb -1` = port + 8080, 0 = off) |
| `--bind`, `--lobbyip`, `--gameip` | listen address, address advertised in the server list, address advertised in the game-start packet |
| `--db`, `--log` | SQLite file, append-mode log |
| `--dserver`, `--dserver-exe`, `--dserver-world`, `--dserver-idle`, `--dserver-logdir` | game server per room; `--dserver-idle` is seconds without clients (0 = never, negative = 0) |
| `--mods-dir` | server-hosted mod packages (also served by the clan web page, see below) |
| `--config` | `server.toml` (or `DC_CONFIG`) |

When `--gameip` is a non-loopback IPv4 address the game server is started with `-bindip <that address>` (unless
`--dserver-args` already has `-bindip`). A game server that is still running after `@end` plus
`dserver.quit_grace_secs` (default 5) is stopped. A per-room working directory made under the temp directory (no
`--dserver-logdir`) is removed when the game ends.

## Settings precedence

    CLI > env > server.toml > lobby.json > built-in

A command-line flag beats a `DC_*` environment variable (`DC_SERVER__PUBLIC`, `DC_DSERVER__TICK_HZ`, ...), which beats
`server.toml`, which beats `<db dir>/lobby.json` (a flat JSON object of rule values such as
`{"economy.exp.kill": 5}`), which beats the built-in default. `doctor` shows the resolved result.

## server.toml

The schema, the `DC_*` environment names and the validation live in the `dc-config` crate (`dc-config check
server.toml`). The lobby reads the same file as `dc-dserver` and the client.

## doctor

`doctor` takes the flags and environment the server would start with and checks: config, `lobby.json` keys and values,
listener ports, the ROM folder, the database (opened read-only), the mods folder and the advertised mod-set hash.

## Clan web page

`GET /` and the clan pages on `--clanweb`; with a mods directory also `GET /mods/<sha256>.zip` (the verified package
whose content hash is in the name) and `GET /server_list.json`.

## Tests and harness

    python cargo_env.py cargo test -p dc-lobby-server          # unit tests plus tests/*.rs process tests
    python tools/harness_run.py --exe <release exe> main persist_restart clan account_grant

The process tests start the real lobby binary on ephemeral loopback ports with a temporary database and a small
stand-in game server (`tests/support/fake_dserver.rs`); ROM-gated cases skip when `CA_ROM` is unset. See
`tools/README.md` for the harness suites.
