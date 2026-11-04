# Architecture

DeCombat is a Rust workspace. The client, the lobby and the game server are separate programs. All of them read game data
from the player's or operator's own copy of the 2009 client folder (the ROM) at run time.

- **`dc-client`** is what a player runs: window, renderer, input, sound, HUD and menus.
- **`dc-lobby-server`** holds accounts, characters, the shop, clans and rooms in one SQLite database.
- **`dc-dserver`** plays one match. The lobby starts one per match, on its own machine or through a remote **`dc-host`**
  agent ([HOST.md](HOST.md)).
- **Mods** are Lua and data packages that the lobby serves and the client hash-checks ([MODDING_GUIDE.md](MODDING_GUIDE.md)).
  Maps are mods built with **`dc-mapkit`** ([MAPKIT_AGENTS.md](MAPKIT_AGENTS.md)).

The evidence for 2009 behaviour is in [specs/](../specs/README.md). Rules that are DeCombat's own are in
[DC_DEFAULTS.md](../DC_DEFAULTS.md). Design notes for hosting and mods are in [HOSTING_AND_MODS.md](../HOSTING_AND_MODS.md).

## Crates

| crate | purpose |
|---|---|
| `dc-client` | The game client: window, wgpu renderer, input, sound, HUD, menus, lobby and game-server links. |
| `dc-lobby-server` | Auth, lobby, shop, inbox and clan server (SQLite). |
| `dc-dserver` | Dedicated game server: rooms, modes, movement, hits, world objects. |
| `dc-host` | Remote game host agent: dials the lobby over TLS and runs one `dc-dserver` per match. |
| `dc-botclient` | Headless client: lobby flow, UDP game session, a stand-in player with simple AI. |
| `dc-rom` | Locate and verify the ROM folder; read its resources. |
| `dc-rez` | Read `.rez` archives, a virtual file system with mod overrides. |
| `dc-bute` | Parse the `.txt` / `.bms` / `.gms` table format. |
| `dc-wire` | TCP frame and bit-aligned UDP packet reader/writer; FILETIME helpers. |
| `dc-net-engine` | The game-connection UDP transport (link, host, joiner); pure, time is an input. |
| `dc-sim` | Deterministic movement and collision shared by the server and the client. |
| `dc-cone` | The shot cone shared by the client, server and bot. |
| `dc-world` | World file (.dat v85) reader. |
| `dc-model` | Model container (.ltb) reader/writer. |
| `dc-tex` | Texture container (DTX) decoder. |
| `dc-fx-data` | Sprite, texture-script and effect content parsers. |
| `dc-ui-data` | UI strings, layout tables, screen pages and fonts from the ROM. |
| `dc-audio` | Sound mixer, attribute records, music and sound decode. |
| `dc-render` | wgpu renderer and offline world viewer. |
| `dc-mod-lua` | Sandboxed per-mod Lua 5.4 runtime with manifest, hooks and events. |
| `dc-modkit` | Mod developer CLI: scaffold, validate, lint, pack. |
| `dc-config` | `server.toml` schema, environment overrides, the directory and review-list checks. |
| `dc-mapkit` | Community map builder: text project, JSON ops, Lua generators, world build, export, MCP server, GUI. |
| `dc-hangdump` | Hang and crash diagnostics (Windows). |
| `dc-ios-app` | The iPad app's static library. |
| `dc-ios-hid` | iPadOS keyboard, mouse and pointer lock. |
| `dc-xtask` | Dev-only tooling: differential dumps of the ROM through the crates. |
