# Content profiles

A content profile lets the engine run with your own content and no original game folder. It is a folder holding a
small JSON document, `game.json`, that names where the content lives. The client, the game server and the bot client
take it with one flag.

## Status

Implemented and unit-tested: the profile format, its checks, the layered file system it builds (`dc_vfs::install`)
and the flag on the three programs. The flag loads and checks the profile and logs one line; the programs do not read
their content through it yet. Until they do, a run with the flag behaves like a run without it once the profile has
been accepted.

## The flag

| program | flag |
|---|---|
| `dc-client` | `--game <dir>` |
| `dc-dserver` | `-game <dir>` (like its other flags, any number of leading dashes works) |
| `dc-botclient` | `--game <dir>` |

`<dir>` is the profile folder (its `game.json` is read) or the path of the document itself. A profile that fails a
check stops the program at start with a usage error that names the problem. An accepted one is logged as
`game: <id>, <n> roots, <m> packs`. Without the flag nothing is loaded and nothing changes.

## `game.json`

```json
{
  "format": 1,
  "id": "my-game",
  "title": "My Game",
  "roots": ["base"],
  "packs": ["packs/arena"],
  "default_map": "@mod:arena/assets/maps/arena.dat"
}
```

| field | required | meaning |
|---|---|---|
| `format` | yes | `1`. Any other number is refused, so an older build never half reads a newer file. |
| `id` | yes | 1-64 characters of `A-Z a-z 0-9 _ -`. |
| `title` | no | The name players see; the id when absent. |
| `roots` | no | Base content folders. A file is found by its exact relative path in lower case with `/` separators (`models/box.mdl`). A later folder shadows an earlier one. |
| `packs` | no | Mod folders. Each holds a `manifest.json` whose `id` names the pack; two packs may not share an id. |
| `default_map` | no | The `@mod:<pack id>/assets/<file>` path of a map in one of the packs; the file must exist. |

Relative folders are relative to the folder that holds `game.json`, and every folder must exist. Unknown fields are
refused. A document larger than 64 KiB is refused.

Plain text in a base folder or a pack is read as it is. Encrypted text needs the original game's key, so with no
original game folder it is an error.

## Lookup order

A file is looked up in these layers, first match wins:

1. A `@mod:<id>/assets/<file>` path is the file `<pack folder>/assets/<file>` of that pack and nothing else. An
   unknown id, a path outside `assets/` or a `..` is refused, never passed to another layer.
2. The packs' `overrides/` folders, the last pack first.
3. Override folders a program adds for its own reasons (for example the mods of the server you joined).
4. The base content folders from `roots`, the last one first.
5. The original game's archives, only when a game folder is given as well.

Files added to a folder while the program runs are seen; only misses in the archives are remembered.

## A minimal profile

```
my-game/
  game.json            {"format": 1, "id": "my-game", "roots": ["base"], "packs": ["packs/arena"]}
  base/
    models/...
  packs/
    arena/
      manifest.json    {"id": "arena", ...}
      assets/maps/arena.dat
```

`dc-dserver -game my-game -worldfile my-game/packs/arena/assets/maps/arena.dat -announce @mod:arena/assets/maps/arena.dat`
starts a game server on the pack's map with the profile loaded.

## For code

- `dc_config::game::GameProfile::load(path)` reads and checks a profile; `content()` gives the plain data and
  `vfs(archive)` the file system.
- `dc_vfs::install(&content, archive)` builds the layered file system from packs, override folders, base folders and an
  optional archive layer. With empty content it is the file system of a run without a profile.
- The loaded profile: `dc_client::gameprofile::active()` in the client, `cli::Args::game` in the game server,
  `run::BotConfig::game` in the bot client.
