# DeCombat modding guide

For people who write mods. Design background lives in `HOSTING_AND_MODS.md` (sections 3, 4 and 8); this guide is
the practical version. Using an AI tool? `AGENTS.md` and `.claude/skills/modding/SKILL.md` hold the same rules as guard
rails.

A mod is a folder of **Lua text and data files**. It never contains native code and never contains Nexon assets.
It runs inside a sandbox that the server (or the client) hosts, and it can only do what its `manifest.json`
declares.

Contents: 1. Three kinds of mod, 2. Quick start, 3. Manifest reference, 4. Capabilities, 5. The sandbox,
6. The `mod.*` API, 7. How servers deliver mods, 8. Assets, 9. Testing with `dc-modkit`, 10. Vanilla means no mods,
11. Error messages, 12. Making content for Vanilla++, 13. Maps with dc-mapkit.

## 1. Three kinds of mod

Every manifest declares a `kind`. There is no default; the loader refuses a manifest without one.

| kind | runs on | typical content | who needs it |
|---|---|---|---|
| `server_rules` | the server only | reward formulas, weapon stat patches, mode rules, custom room presets, GM tools | nobody else; players join as usual |
| `shared` | server and client | stat patches the client predicts, text overrides, new HUD elements, a new mode or map with its own assets | the client must hold exactly the server's mod set; it is downloaded on join |
| `client_visual` | the client only | HUD skins, crosshairs, sound packs, procedural recolours of ROM textures | the player, on servers that allow it |

`requires_client` follows from the kind (`false` for `server_rules`, `true` for the others). You may write it out,
but a value that contradicts the kind is a manifest error.

### 1.1 Example: a server-rules mod

A reward tweak and a weapon patch. The server loads it; players notice only the results.

`manifest.json`

```json
{
  "id": "double_exp_ftm",
  "name": "Double EXP on FTM",
  "version": "1.0.0",
  "kind": "server_rules",
  "capabilities": ["content.weapons", "hooks.lobby"],
  "github": "yourname/double_exp_ftm"
}
```

`main.lua`

```lua
---@param mod ModCapability
return function(mod)
    -- weapon 145 is the M4A1_D: a bigger clip on its weapon row, more damage on its ammo row (ROM keys)
    mod.content.weapons:patch(145, { ShotsPerClip = 40 })
    mod.content.weapons:patch("Ammo145", { InstDamage = 41 })

    mod.hooks:wrap("lobby.calc_exp_gp", function(base, ctx)
        local res = base(ctx)
        if ctx.mode_key == "FTM" then
            res.exp = math.floor(res.exp * 2)
        end
        return res
    end)
end
```

### 1.2 Example: a shared mod

A custom HUD element plus a rule that depends on it. Both sides load the same package.

```json
{
  "id": "streak_hud",
  "name": "Kill Streak HUD",
  "version": "0.3.0",
  "kind": "shared",
  "capabilities": ["content.hud", "content.text", "hooks.combat", "hooks.hud", "events", "assets"]
}
```

```lua
return function(mod)
    mod.content.text:override("IDS_STREAK", "Streak!")
    mod.content.hud:override("streak_icon", mod.assets:path("assets/streak.dtx"))
    mod.events:on("combat.player_killed", function(ctx) --[[ count kills ]] end)
    mod.hooks:wrap("hud.draw", function(base, ctx) return base(ctx) end)
end
```

Render and HUD hooks (`hooks.render`, `hooks.hud`) are what an "x-ray" or outline server mod would use; every player on
that server gets the same view, and the server advertises the mod set so nobody is surprised (section 7).

### 1.3 Example: a client-visual mod

```json
{
  "id": "ring_crosshair",
  "name": "Ring Crosshair",
  "version": "1.0.1",
  "kind": "client_visual",
  "capabilities": ["content.hud", "assets"]
}
```

```lua
return function(mod)
    mod.content.hud:override("crosshair", mod.assets:path("assets/crosshair.dtx"))
end
```

A `client_visual` mod cannot declare `hooks.lobby`, `hooks.combat` or any content domain that the server decides
(`weapons`, `items`, `shop`, `gameroom`, `ai_brains`, `missions`); only `hud`, `music` and `text` content plus the plain
capabilities are allowed.

## 2. Quick start

```sh
dc-modkit scaffold my_mod --profile content        # or overhaul, total_conversion, client_visual, map
cd my_mod
dc-modkit validate .       # runs main.lua in the real sandbox
dc-modkit lint .           # static checks, including undeclared capability use
dc-modkit pack . -o my_mod.zip
```

`scaffold` writes `manifest.json` (kind and capabilities filled in for the profile), `main.lua`, `README.md`,
`assets/`, `overrides/`, `.luarc.json` and `dc_mod_api.d.lua`. Use `--kind server_rules|shared|client_visual` to
override the profile's kind; scaffold refuses a combination whose capabilities the kind cannot hold.

| profile | default kind | declares |
|---|---|---|
| `content` | `server_rules` | `content.weapons` |
| `overhaul` | `server_rules` | `content.weapons`, `hooks.lobby`, `events`, `save` |
| `total_conversion` | `shared` | `content.weapons`, `content.gameroom`, `content.hud`, `hooks.combat`, `assets` |
| `client_visual` | `client_visual` | `content.hud`, `assets` |
| `map` | `shared` | `content.maps`, `assets` (one map record, a placeholder world under `assets/maps/`; see "Map mods" in section 6) |

## 3. Manifest reference

`manifest.json` is plain JSON (a leading BOM is tolerated). Unknown fields are ignored.

| field | type | required | default | meaning |
|---|---|---|---|---|
| `id` | string | yes | | letters, digits, `_` and `-` only; unique among the mod set |
| `name` | string | yes | | human name, not empty |
| `version` | string | yes | | `major.minor.patch` |
| `kind` | string | **yes** | | `server_rules`, `shared` or `client_visual` |
| `requires_client` | bool | no | derived from `kind` | must agree with the kind |
| `capabilities` | array of string | no | `[]` | everything the mod may use (section 4); a mod with none can do nothing with the host |
| `asset_policy` | string | no | `original_only` | `original_only` or `credited`; see section 8 |
| `credits` | array | for `credited` | `[]` | one `{title, author, license, source, changes}` per work you did not make; see section 8 |
| `entry` | string | no | `main.lua` | script returning `function(mod) ... end`; relative, no `..` |
| `api` | integer | no | `1` | mod API level |
| `priority` | integer | no | `0` | lower loads earlier; ties break by `id` |
| `dependencies` | array | no | `[]` | `"other_id"` or `"other_id@^1.2.0"` (`^ ~ >= <= > < =`) |
| `optional_dependencies` | array | no | `[]` | ordered first when present |
| `conflicts` | array | no | `[]` | refuses to load next to a matching mod (`incompatible` is accepted as an alias) |
| `permissions` | array | no | `[]` | engine-level extras (`network`, `background`, `engine_internals`); not the same thing as capabilities |
| `options_schema` | string | no | none | file with the declarative options |
| `github` | string | no | none | `owner/repo`; recorded as `source_repo` in the mod-set entry |
| `games` | array | no | `["all"]` | which games the mod targets |
| `description` | string | **yes** | | 20 to 500 characters saying what the mod does; the server browser shows it |
| `author` | string | **yes** | | who made the mod, 1 to 100 characters |
| `homepage` | string | no | none | an `http://` or `https://` page of the mod |
| `screenshots` | array of string | **yes** for `shared` and `client_visual`, no for `server_rules` | `[]` | 1 to 6 images `screenshots/<name>.png` or `.jpg` (letters, digits, `_`, `-`), PNG or JPEG by content, at most 1 MiB each (checked by `dc-modkit lint`); packed into the zip |
| `tags` | array of string | no | `[]` | up to 8 words for the browser's filter, each 1 to 20 characters of `a-z`, `0-9`, `-` |
| `changelog` | string | no | none | what changed in this version |
| `commands` | array of `{command, description}` | no | `[]` | up to 16 chat commands for the Mods screen's "Commands" section: `command` is `/` and `a-z`, `0-9`, `_`, `-` (at most 24 characters), `description` 1 to 120 characters. Text only: nothing runs from it |
| `settings` | array | no | `[]` | what the player can set on the Mods screen (`client_visual` only); see below |

Load order is deterministic: dependencies first, then `priority`, then `id`.

**Settings** (MOD-24, kind `client_visual` only, at most 16). Each entry has `key` (1 to 32 of `a-z`, `0-9`, `_`), `label`
(1 to 40 characters), `type` and `default`:

| `type` | `default` | on the settings page | `mod.settings.<key>` |
|---|---|---|---|
| `colour` | `"#rrggbb"` or `"#rrggbbaa"` | a palette | that text |
| `choice` | one of `options` | a cycler | the option's value |
| `bool` | `true` / `false` | a toggle | a boolean |
| `number` | a number on its grid | a slider (click or drag) with `-` and `+` buttons of one step, its value shown | a number |

A `number` also needs `min`, `max` and `step`: `min` < `max`, `step` > 0, `max` is `min` plus a whole number of steps (at
most 60000), and `default` lies on that grid. A `colour` row has 16 presets and red, green, blue and opacity sliders.
Optional `group` (1 to 40 characters) starts a heading above the row; later rows without one stay under it. The page
scrolls when the rows do not fit, and every page has Reset to defaults and a share code (Copy code, Paste code, also
Ctrl+C and Ctrl+V): all values as text such as `DCS-053GC-0801M-...`, valid only for the same mod id and the same
declared settings; a bad code shows a message and changes nothing.

`options` holds 2 to 32 entries, each a word (`a-z`, `0-9`, `_`) or `{"value": "cross_dot", "label": "Cross + dot"}`. A
mod with settings gets a Settings button on its row. Values live in the player's profile
(`mods.setting.<id>.<key>`), not on any server; a stored value the declaration does not allow (out of range, off a
number's grid) reads as the default. A client older than 2026-11-01 refuses a manifest with `number` as an unknown type.
`mod.settings` is read-only, and a change applies at the next match load. In a match the same page opens from the
Mods tab of the options (Escape, Options): a change there loads the match's mod set again with the new values, so the
mod's entry function runs again (a mod keeps no Lua state across that). The page previews a mod that sets
`crosshair_style`.

The mod-set entry (`dc-modkit modset`, the server-list data) carries `description`, `author`, `tags` and the
screenshot file names; a client reads the images out of the zip it downloads. `dc-modkit scaffold` writes
placeholders for `description`, `author` and `screenshots/main.png`, and `lint` warns until you replace them.

## 4. Capabilities

A capability is a string. The host refuses any use beyond the list: the call raises `capability denied`, the load
fails, and nothing the mod registered stays live. A mod cannot get around this by catching the error with `pcall`; the
denial is recorded and the load still fails.

| capability | allows |
|---|---|
| `content.<domain>` | `mod.content.<domain>:patch / register / override / get`; domains `weapons`, `items`, `shop`, `gameroom`, `ai_brains`, `missions`, `text`, `music`, `hud`, `maps` (`maps`: `register` and `patch` only, checked records; section 6 "Map mods") |
| `hooks.lobby` | `mod.hooks:wrap / on("lobby.<name>", ...)` |
| `hooks.combat` | hook names starting `combat.` |
| `hooks.render` | hook names starting `render.` (for example `render.entity_visibility`) |
| `hooks.hud` | hook names starting `hud.` (for example `hud.draw`) |
| `events` | `mod.events:on` |
| `save` | `mod.save:get / set` |
| `assets` | `mod.assets:path` |
| `world.full_visibility` | the game server sends every player's position to every client (culling off); required by any render hook that draws or outlines remote players, e.g. an x-ray mod |
| `combat.no_recoil` | rooms of this server may turn recoil and spread off (the room's Hacks setting, LG-274 / MOD-23): the shooter's shots go to the centre of the crosshair and the game server skips the cone check for that match |

A hook name is `<namespace>.<name>`; a name outside lobby, combat, render and hud is refused whatever you declare.

Which capabilities each kind may declare:

| | `server_rules` | `shared` | `client_visual` |
|---|---|---|---|
| `hooks.lobby`, `hooks.combat` | yes | yes | no |
| `hooks.render`, `hooks.hud` | no | yes | yes |
| `content.weapons/items/shop/gameroom/ai_brains/missions` | yes | yes | no |
| `content.hud/music/text` | yes | yes | yes |
| `content.maps` | yes (but use `shared`: the client needs the world) | yes | no |
| `events`, `save`, `assets` | yes | yes | yes |
| `world.full_visibility` | yes | yes | no |
| `combat.no_recoil` | yes | yes | no |

`hooks.render` and `hooks.hud` are further limited by where the mod comes from. The host loads a mod with an origin:

- **Server**: the mod is named in the server's mod set. Render and HUD hooks are allowed.
- **Local**: the player installed it on their own machine. It must be `client_visual`, the server's policy must allow
  client-visual mods, and it may **not** declare `hooks.render` or `hooks.hud`. A local mod can never change what the
  game shows through the render or HUD hooks.

Declare what you use and no more. `dc-modkit lint` reports a use that is not declared as an error and a declaration
that nothing uses as a warning.

## 5. The sandbox

Mods run in real Lua 5.4. Each mod gets its own `_G` and private copies of the kept libraries, so one mod cannot patch
another.

Removed (the names do not exist inside the sandbox): `os`, `io`, `debug`, `package`, `coroutine`, `load`, `loadstring`,
`loadfile`, `dofile`, `collectgarbage`, `getfenv`, `setfenv`, `string.dump`, `math.randomseed`. Precompiled bytecode is
rejected everywhere, including in required modules. `setmetatable` refuses a metatable with `__gc`.

Kept: `assert error getmetatable ipairs next pairs pcall rawequal rawget rawlen rawset select setmetatable tonumber
tostring type xpcall unpack` and the `string`, `table`, `math` and `utf8` libraries. DeCombat adds
`math.clamp(x, lo, hi)`. `print` is captured per mod (at most 1,000 lines) and never reaches stdout.

Limits (host configuration; a mod cannot change them):

| limit | default |
|---|---|
| VM instructions per host-to-Lua call (mod init, each hook stage, each listener) | 10,000,000; `pcall` cannot swallow the kill |
| Lua heap for the host | 64 MiB |
| result of `string.rep` / `string.format` | 4 MiB |
| nested Lua frames | 200 |
| errors before the circuit breaker switches the mod's hooks and listeners off | 5 |

`require("a.b")` loads `<mod dir>/a/b.lua` or `a/b/init.lua` only: dot-separated names, no slashes, no `..`, no absolute
paths, no symlink escapes.

Data that crosses into the host (patch values, hook contexts and results, save values) must be plain: nil, boolean,
number, string and tables of those. Functions, userdata and cyclic or very deep tables are rejected.

## 6. The `mod.*` API

`dc_mod_api.d.lua` (written by `scaffold`, with a `.luarc.json` for LuaLS/VS Code) is the authoritative reference and is
generated from the runtime's own constants, so it cannot drift. In short:

- `mod.id / name / version / kind / api / priority`: manifest values.
- `mod.content.<domain>:patch(id, partial)`, `:register(def)`, `:override(key, value)`, `:get(id)`.
- `mod.hooks:wrap(name, function(base, ctx) ... end)` and `mod.hooks:on(...)`: chain-of-responsibility around a host
  calculation. `base(ctx)` runs the next stage inward. Hooks the host offers include `lobby.calc_exp_gp`,
  `combat.damage`, `render.entity_visibility` and `hud.draw`; vanilla behaviour is always the documented default
  (`DC_DEFAULTS.md`), so a mod is a diff against it.
- `mod.events:on(topic, listener)`: observers for lifecycle events such as `match.end`.
- `mod.save:get/set`, `mod.options:define/get/set`, `mod.assets:path(rel)`.
- `mod.exports` and `mod.find(id)`: share an API with other mods.

Weapon content (`mod.content.weapons`, capability `content.weapons`): `patch(id, partial)` may set any key of a weapon
row (`patch(145, {ShotsPerClip = 40})`, id `145` or `"Weapon145"`) or of an ammunition row (`patch("Ammo3",
{InstDamage = 41})`), known to the game or not; the row must exist in the ROM table. `register(def)` adds a new row
whose `id` must be free and which carries every key it needs. Numbers, strings and booleans are allowed as values,
nested tables are not. The server and the client apply the same patches: the lobby writes them as a content document
(`mod_cache/content-<hash>.json`) that every game server of the match reads, and the client applies that document to
its own copy of the table, so damage, clips and the client's prediction agree. A patch that does not fit the table
stops the server's start, not the match (rules MOD-21, DSV-478, CLI-1611 in `DC_DEFAULTS.md`).

A `server_rules` mod's weapon patch applies on the server only: the client never loads that mod, so its own
prediction (clip size, fire rate, spread) can drift from the server's for that weapon. Use `shared` for patches the
client should predict, such as clip size and fire rate; a damage-only patch is fine as `server_rules`. The lobby
therefore writes two documents: `content-<hash>.json` (all mods, for the game servers) and
`content-client-<hash>.json` (the `shared` / `client_visual` mods only, what a client must reproduce).

Drop-in file replacements go under `overrides/<virtual path>` (for example `overrides/Interface/HUD/x.dtx`; the
parts match the ROM path ignoring case) and shadow the ROM entry; a mod later in the load order shadows an earlier one
(CLI-1612).

### Map mods

A community map is a `shared` mod (the client must download the world) that carries its world file and pictures
under `assets/` and registers one **map record** per map (`mod.content.maps:register`, capability `content.maps`;
rules MAP-4 and MAP-5 in `DC_DEFAULTS.md`). The world is a version-85 `.dat` that **dc-mapkit** builds from a text
project; `dc-mapkit export` writes the whole mod folder, so you rarely write this by hand. The record:

```lua
mod.content.maps:register({
  id = "crossfire_valley",            -- the map key: a-z 0-9 _ (1..32); unique across the server's mod set
  name = "Crossfire Valley",          -- label shown in the room list (1..48 characters)
  description = "...",                -- room settings panel (at most 400 characters), optional
  world = mod.assets:path("assets/maps/crossfire_valley.dat"),   -- this mod's own asset, a .dat under assets/
  modes = { "TDM", "FFA", "ELIM", "BOMB", "FLAG" },              -- DeCombat mode names, no duplicates
  goals = { TDM = 100, FFA = 50 },    -- optional default goal per mode (a mode listed above), positive
  round_seconds = 600,                -- optional, 30..3600; else the lobby's round clock policy
  picture = mod.assets:path("assets/pictures/crossfire_valley.dtx"),       -- optional
  loading = mod.assets:path("assets/pictures/crossfire_valley_load.dtx"),  -- optional (one picture, tiled)
  radar = { picture = mod.assets:path("assets/pictures/crossfire_valley_radar.dtx"),
            min = { -2048, -2048 }, max = { 2048, 2048 } },               -- optional, world x / z of the picture
  players = { min = 2, max = 16 },    -- optional, 1..16, min <= max
})
```

The mode names are the lobby's `modes.playable` vocabulary: `TDM`, `TDM_CLAN`, `COOP`, `FFA`, `ELIM`, `ELIM_CLAN`,
`FLAG`, `FLAG_CLAN`, `BOMB`, `BOMB_CLAN`, `INFECT`, `BOUNTY` (spelled like this; MAP-6). The host checks the record
when `main.lua` runs: a missing required key (`id`, `name`, `world`, `modes`), a misspelled key, a value outside its
limits or a `world` that is not this mod's own `assets/...dat` refuses the mod, with the rule in the message
(section 11). Another mod may `:patch(id, partial)` a registered map: a deep merge, except that **patches replace
lists**: `modes`, `goals`, `players` and `radar` are replaced whole when the patch carries them, so
`{ modes = { "FFA" } }` leaves exactly FFA (carry `goals` for the new list too, or the owner's `goals.TDM` fails the
check) and a `radar` must be complete; the world stays the owner's file. A second `:register` of a key the set
already holds refuses the load; `:override` is not available for maps. A mod whose `main.lua` fails after
registering (an error, a second bad record, a denied capability) leaves nothing behind: its records and patches are
rolled back with its hooks, so its map keys are free again and the lobby never lists its maps.

Scaffold one with `dc-modkit scaffold my_map --profile map`: a `shared` manifest declaring `content.maps` and
`assets`, a `main.lua` with the record above, and `assets/maps/my_map.dat`, a flat placeholder floor of ours that
`dc-modkit validate` accepts until dc-mapkit writes the real world. `validate` checks every registered world: the
file must exist under `assets/` and parse as a version-85 world whose sections tile the file (no ROM needed). The
server appends community maps after the ROM's maps in mod-set load order, then by key; `dc_mod_api.d.lua` documents
the record as `ModMapRecord`.

On the client (MAP-30 .. MAP-34): the game server announces the world as `@mod:<id>/assets/maps/<file>.dat` and the
client reads it from the mod set it downloaded for the join, never from its archives; the world's textures are read
in layers (`@mod:<id>/assets/...` names from the set, `overrides/<ROM path>` files of any mod of the set, then the
player's ROM by name; a name nobody answers draws flat). The waiting room lists the community maps after the ROM's
maps with the record's `name`, shows its `description` and `picture` in the settings panel and its `loading` picture
(one picture, stretched over the page) while the match loads; the headless client installs the set's files like the
windowed one. A `dc.capture_flag` with an empty `model` is drawn as a bar in the team colour. To look at a map
without a server: `dc-client --world-file build/<key>.dat --mod-dir build/mod/<id> [--rom DIR]
[--headless-frames N --out DIR]` (the folder stands for every `@mod:` id; a texture nobody answers draws a solid
colour); `dc-mapkit render --textured` runs it for you and keeps the picture.

**What the lobby does with the record** (MAP-35 .. MAP-39 in `DC_DEFAULTS.md`). A modded lobby whose declared set
holds the map mod offers the map after the ROM's rows: its room-list index is the first free one after them (after
dc_bute's twelve fallback rows on a lobby without a ROM), and that first index is published as `community_map_base`
in the lobby's `server_list.json`, which the client reads at the join, so the two sides agree on the indices whatever
ROM each has (a lobby run without a ROM says 12). Its mode bits are the ROM mode rows whose class is one of `modes`
and whose `EnablePlay` is `TRUE` or absent (filtered by `modes.playable`), at most 30 (map, modes) pairs reach the clients
(the start-up log warns past that), and `GET /status` lists `community_maps` by key. A room picks it like any map; the
record's `goals[mode]` becomes the room's goal at that moment (the host may change it), and the Start launches the
game server with `-worldfile <mod cache path> -announce <world as written> -worldobjects world` after the operator's
`--dserver-args`, with `time=` on the room line from `round_seconds` when set and `radar=` from the record's `radar`
(the exported picture and its world rectangle, turned into the game server's scale / origin form), so the match's
radar shows the exported top view. The lobby refuses the set at start when a map mod is not `shared`
(`requires_client`) or its world file is missing; clan rooms do not play community maps, and on a modded lobby neither do remote game hosts
(`[[lobby.servers]]` `host`): a clan room cannot pick one, and such a start stays waiting with a log line. A map on a
Vanilla++ entry does run on a remote host (section 12, MAP-83). Maps are offered from a declared set only: in the folder-discovery fallback (no `server.toml`) nothing is
published and every login is vanilla, so discovered map mods are logged as ignored. The 0x311 room-settings version
is derived from the mod-set hash, so a client fetches the table of the set it joins.

**HUD override keys** (`mod.content.hud:override(key, value)`, capability `content.hud`; CLI-1613). Any other key
refuses the mod at load: `Unknown HUD override key 'x'; the keys are: crosshair, crosshair_colour, hitmarker, hud_scale, crosshair_style, crosshair_recoil, crosshair_length, crosshair_thickness, crosshair_gap, crosshair_dot, crosshair_outline, crosshair_t_style, crosshair_recoil_scale, case_reel, case_reel_ms`.

| key | value | effect |
|---|---|---|
| `crosshair` | texture path: a ROM path or `mod.assets:path("assets/<file>.dtx")` | drawn centred at its own size instead of the aim cross; a texture that does not load keeps the normal cross |
| `crosshair_colour` | RGBA hex `"rrggbb"` or `"rrggbbaa"`, `#` optional | colour of the aim cross, tint of a `crosshair` texture |
| `hitmarker` | texture path | drawn centred for 250 ms after each own hit, fading out |
| `hud_scale` | number 0.5 to 2.0 | every HUD element grows or shrinks about its nearest screen edge (or the centre) |
| `crosshair_style` | `cross`, `circle`, `dot`, `cross_dot`, `circle_dot` | the client draws this shape, no picture, sharp at any resolution, in place of the aim cross / aim mark and of a `crosshair` texture; `crosshair_colour` colours it |
| `crosshair_recoil` | `"dynamic"` or `true` (default), `"static"` or `false`, `"fire"` | a drawn crosshair: dynamic grows with the spread and after a shot, in step with the stock aim mark; static keeps the weapon's rest size; fire keeps the rest size and adds only the growth after a shot |
| `crosshair_recoil_scale` | number 0 to 5 (default 1) | multiplies what the recoil adds to the rest size |
| `crosshair_length` | number 0 to 100 (default 7) | a drawn crosshair's arm length, in pixels of a 1024 wide screen |
| `crosshair_thickness` | number 0.1 to 30 (default 1.2) | its line width (never below one device pixel); a dot's radius follows it |
| `crosshair_gap` | number -50 to 100 (default 0) | added to the distance of the arms from the centre and to a ring's radius; below zero the arms cross |
| `crosshair_dot` | `true` / `false` (default `false`) | a centre dot on any style |
| `crosshair_t_style` | `true` / `false` (default `false`) | no top arm |
| `case_reel` | `true` / `false` (default `false`) | opening a supply case first spins a strip of what the case can hold until it stops on the award, then the award window opens (CRATE-5; `examples/mods/case_reel`); the award is the server's either way. A camo's rarity tier dresses the spin and the reveal with the game's own sounds (CRATE-7), and a won gun leaps out as its turning 3D model (CRATE-8) |
| `case_reel_ms` | number 1500 to 12000 (default 7000) | how long the spin takes |
| `crosshair_outline` | number 0 to 10 (default 0: none) | a black outline of this width around the shape |

The client reads these once when the mod set has loaded; nothing runs per frame.

**The `hud.draw` hook** (capability `hooks.hud`, server-declared mods only; CLI-1614) runs once per HUD frame.
`ctx` holds cosmetics and your own player only, never other players or world positions (a mod that could see them
could draw a wallhack):

| field | meaning |
|---|---|
| `ctx.screen.w`, `ctx.screen.h` | render target in pixels |
| `ctx.health`, `ctx.armor` | own health and armour |
| `ctx.ammo`, `ctx.clip` | rounds outside the clip, rounds in the clip |
| `ctx.weapon` | held weapon's row id |
| `ctx.round_time_ms` | milliseconds left while the round clock shows |
| `ctx.score.kills`, `ctx.score.deaths` | own board row |
| `ctx.elements.<name>` | `{x, y, w, h}` of each element drawn this frame (virtual 1024 x 768): `crosshair`, `health`, `ammo`, `kill_feed`, `team_counters`, `round_clock`, `respawn`, `chat`, `scoreboard`, `weapon_panel`, `kill_banners` and the others the frame draws |

Absent values are `nil`. Return a table of changes by element name; `base(ctx)` returns the inner stages' changes:

```lua
mod.hooks:wrap("hud.draw", function(base, ctx)
    local r = base(ctx)
    if ctx.health and ctx.health < 25 then
        r.health = { tint = "ff4040ff" }          -- or { 255, 64, 64, 255 }
    end
    r.round_clock = { offset = { 0, 12 } }      -- virtual pixels
    r.chat = { visible = false }
    return r
end)
```

The names over heads, the radar, the damage arcs, the team-mate panel, the case markers and the x-ray boxes are not listed and cannot be
changed. An error in the hook switches it off for the rest of the match (logged once).

**The `render.entity_visibility` hook** (capability `hooks.render`, server-declared mods only; CLI-1620) exists only
on a server whose set also declares `world.full_visibility` (MOD-22: culling is off, so every player already knows
every position). A local mod can never use it. It runs about 30 times a second (CLI-1635; at once when the number of players changes; the frames between draw the last
answer at the players' current places) with the players the client knows:

| field of `ctx.entities[i]` | meaning |
|---|---|
| `id` | the player's world object id |
| `kind` | `"player"` |
| `team` | 1 or 2 (0: no team) |
| `is_enemy` | an enemy of the viewer: everybody in a free-for-all, the other team in a team match |
| `distance` | camera to body centre, world units |
| `is_local` | the own player |
| `is_visible_by_line_of_sight` | no world geometry between the camera and the body centre |

Return `{ entities = { { id, visible, outline, mode }, ... } }` (`base(ctx)` returns the inner stages'
list). Each player is drawn in the viewer's x-ray style (a setting with two colours; `/wallhack` in the match chat
cycles it, CLI-1624 / CLI-1625):

| style | `visible = true` on a hidden player | `outline = true` |
|---|---|---|
| `"box"` | a box at its body, filled with the primary colour (alpha / 3), over the world and under the HUD | the box border (secondary colour), hidden or not |
| `"chams"` | its model drawn flat through the walls: surfaces facing the camera primary, the rim secondary | nothing extra (a player in sight keeps its normal look) |
| `"skeleton"` | its bones (primary) and joints (secondary) drawn through the walls | the same, hidden or not |

`mode` (one of the three names) pins the style for that player over the viewer's setting. The colours are the viewer's
and no mod sets them (CLI-1635): enemies take the viewer's two picks, team mates two other palette colours chosen for
the session, and a viewer who switched team mates off (the shop, `/teammates`) sees no x-ray on them at all.
A hook cannot hide a player, and the own player gets nothing. An error
switches the hook off for the rest of the match (logged once). `examples/mods/hacking_mod` is the demo.
Every player with `visible = true` can also carry the viewer's name tag and health bar (two more x-ray settings,
off by default: the shop toggles, `/nametags` and `/health` in the match chat; the health comes from the game server,
which sends it only in a room with wallhacks; CLI-1630 / CLI-1631 / DSV-480).

## 7. How servers deliver mods

Where the zips come from before a server has them, and how our own packs move to repositories of their own, is
docs/PACKS_FROM_REPOSITORIES.md (pinned releases, hashes checked at install, the client unchanged).

1. A server publishes its **mod set**: for each mod `(id, version, sha256)` of the packed zip, in load order, plus a
   policy (`vanilla` or `modded`, and whether local client-visual mods are allowed). `dc-modkit modset` prints this
   fragment (section 9).
2. The client compares the set with its cache (`<data>/mods/by-hash/<sha256>.zip`) and downloads what is missing from
   the **server's own HTTP endpoint** (`/mods/<sha256>.zip`, size-capped, with progress).
   The first join of a curated entry (Vanilla++ has dozens of packs) downloads them, up to four at a time, and the
   wait box shows "Downloading server packs: 12 of 65" with the megabytes so far; later joins read the cache.
3. The client verifies the SHA-256 **before unpacking**. A mismatch refuses the join. Operators cannot swap bytes
   silently: the client pins to the hash in the server-list entry.
4. Mods load into the sandbox in the server's order. The client sends the resulting mod-set hash at login and the
   server refuses a mismatch; it sends the hash of the weapon content its Lua produced too, and the server refuses
   a client whose content differs from the client-visible content (the patches of `shared` / `client_visual` mods only; `server_rules` patches are not part of it) (LG-271, `;m=<hash>;c=<hash or 0>` after the login name). Leaving the server unloads the mods; the cache stays (LRU, size-capped).

Because `dc-modkit pack` is deterministic, the zip you publish as a GitHub release has the same hash as any server's
copy.

The lobby itself loads exactly the mods its `server.toml` declares (`[[mods.set]]`), in that order (LG-270): each
from `<mods_dir>/<sha256>.zip`, or from the folder `<mods_dir>/<id>/` when that folder packs to the declared hash. A
declared mod that is missing or hashes differently stops the server at start; a folder that is not declared is not
loaded. Only a lobby started without a `server.toml` loads every folder it finds.

### 7.0 Optional mods: the catalogue

A lobby can offer cosmetic mods no one has to take (MOD-25, CLI-1634). In `server.toml`, on any policy:

```toml
[[mods.catalog]]
id = "drawn_crosshair"
version = "1.0.0"
sha256 = "<sha256 of the zip>"
```

Put `<sha256>.zip` in the mods dir. The lobby serves the list at `/mods/catalog.json`, leaving out (with a log line) a
zip that is missing, does not match its hash, or is not `client_visual`. The catalogue is not part of the mod set or its
hash, the lobby never loads these mods, and a vanilla lobby stays vanilla. The Mods screen's Get mods section lists
them; Download verifies the size and hash, then the manifest's kind, installs into `mods/local` disabled, and Remove
deletes such a folder. A player's own folder in `mods/local` has no Remove button, and the cache cleanup never touches
the folder.

### 7.0.1 An operator's picture

The package's screenshots must be your own pixels (section 8), so a frame of the real game cannot go in a mod. An
operator can show one from their own server: put `<mod id>.preview.png` (a PNG, at most 1 MiB) in the mods dir next to
the zip and restart the lobby. It works for `[[mods.set]]` and `[[mods.catalog]]` entries (MOD-27). The entry in
`server_list.json` or `catalog.json` then carries `"preview": "<sha256 of the picture>"`, the picture is at
`/mods/<that sha256>.png`, and the client shows it in the detail panel instead of the package's screenshot. It is
never in a package, changes no hash, and a client that cannot fetch it shows the package's screenshot. Do not put such
a picture in a public repository. DeCombat's own servers do not use a game frame, and any operator picture must follow
the asset rule of section 8; the Hacking Mod's picture is generated by code in this repository
(`crates/dc-client/tests/hacking_art.rs`).

### 7.1 Known mods and the verified badge

The community JSON (a static file on decombat.com, maintained by pull request) carries a **known-mods list** of
`(id, version, sha256, source_repo)`. When a server's mod hash appears in that list the client shows the mod as
**verified**. Otherwise it is **unverified**: the player must opt in to that mod explicitly on first join, and the
prompt shows the author, the source URL and the mod's declared capabilities. To get a badge, build your release with
`dc-modkit pack`, publish the zip, and open a pull request adding the entry.

Because the capability list is what the player is shown, keep it honest and minimal.

What the player sees (CLI-1616 .. CLI-1618, LG-272, CLI-1622, CLI-1623):

- **Server list after the login (the main path).** The 2009 server-select screen lists the lobby's own servers
  ("US Central", "US West", ...) and under them the modded servers: the other lobby instances the lobby's operator
  lists (`[[lobby.servers]]` with `lobby = "host:port"`, docs/VPS.md "Several instances on one box") and the modded
  servers of the community JSON, merged by address. Each row shows a MODDED badge, the server's name and region; the
  panel on the right lists every modded server with its mods marked **Verified**, **Accepted** or **Unverified**, and
  holds the **Mods** button. Picking a modded row (Join, Enter, double click) leaves the lobby, reads the server's mod
  set, asks the consent prompt for untrusted mods, downloads and loads the set, and logs in there with the player's
  identity token (no password; with `--identity off` the login screen asks for it); its channel list follows. The
  server list again (or a refused join) returns to the first lobby and unloads the mods. No screen shows a server's
  IP address: the address is data only.
- **Server browser (before the login, fallback).** The login screen's homepage button opens the server list: the lobby the client was started
  with, then every server of the community JSON with a VANILLA or MODDED badge, the player count when the entry
  records one (else its region), and for a modded server the names of its mods, each marked **Verified** (in the known-mods list),
  **Accepted** (the player said yes before) or **Unverified**. Join takes the server's address (and certificate
  fingerprint, when the entry gives one) for the login and pins its mod set; the client refuses a lobby whose own set
  differs from the entry.
- **Consent prompt.** Joining a server whose set holds a mod the player has not trusted shows a prompt *before
  anything is downloaded*: per mod its name, version, `author`, source URL (`source_repo` of the mod-set entry, i.e.
  the manifest's `github`, else `homepage`) and the declared capabilities in plain words (`content.weapons` "changes
  weapon stats", `world.full_visibility` "everyone is visible through walls", `hooks.lobby` "changes rewards and
  lobby rules", ...). Accept remembers that exact package (id, version, sha256) in the player's profile; a new
  version or a different hash asks again. Cancel downloads nothing and logs in nowhere.
- **Mods screen.** The Mods button (server list or browser) opens three sections. (a) **Mods of the selected
  server**: the server highlighted in the list (else the lobby you are in), named with its region and a VANILLA /
  MODDED badge, never its address; per mod the name, version, kind in plain words, author, the Verified / Accepted /
  Unverified mark and the capabilities in plain words, with the first screenshot once the package is downloaded; a
  vanilla server says "Vanilla: this server runs no mods". (b) **Downloaded mods on this PC**: every package in the
  cache (`<profile>/mods/by-hash/`, unpacked under `mods/unpacked/`) with its name, version, author, size and which
  known servers use it; **Remove** deletes the package (the next join that needs it downloads it again; a package the
  loaded mods use cannot be removed). An Accepted mark means you accepted that unverified mod before; **Forget
  consent** clears it and the next join asks again. (c) **Your own cosmetic mods** under `<profile>/mods/local/`
  with an Enable / Disable toggle (**Open folder** opens that folder). A local mod is off until enabled, and the
  toggle takes effect at the next join. When a local mod did not load the screen says why: not `client_visual`, a
  render or HUD hook, a vanilla server (section 10), or a server that does not allow local mods. Mods a server runs
  never go into `mods/local/`: they are downloaded for you and listed in (b).
  Two or more weapon camos of a server (packages tagged `camo` or `variant`, as `dc-gunkit` writes them) show as one
  row, **Camo collection**, with their count; **Show camos** lists them one by one under it (CLI-1943), so a curated
  entry with sixty camos stays readable. Fill in `tags` for that reason too. Every row says where it comes from at the
  start of its second line: **Yours** (a folder under `mods/local`, a catalogue download included), **Optional** (a
  catalogue entry not installed yet) or **Server pack** (a server's set, downloaded by hash).

So: fill in `author`, `description`, `github` and `screenshots` (PNG shows on the Mods screen) in the manifest,
because the player decides on them.

## 8. Assets: no Nexon content, procedural transforms allowed, open work with credit

`asset_policy: "original_only"` is the default. `asset_policy: "credited"` also allows somebody else's openly licensed
work, as described below. Under either, a mod:

- never contains a Nexon asset, string table, key or binary, or anything extracted from the game;
- may **reference** ROM paths and apply **procedural transforms** at run time (recolour, composite, re-encode). The
  result exists only in memory on the player's machine;
- may ship its own **original** assets under `assets/` (textures as DTX, sounds, text) and reach them with
  `mod.assets:path("assets/name.dtx")`, which returns the virtual path `@mod:<id>/assets/name.dtx`. Paths are relative to
  the mod folder; absolute paths, drive letters and `..` raise. The client resolves a `@mod:` path only inside
  `assets/` of a mod of the loaded set; anything else is refused and never falls back to the ROM (CLI-1612). The HUD
  keys of section 6 and the sound files accept these paths.

**Open work, with credit (`credited`, MOD-28).** A mod may ship models and textures other people made when their license
allows it and allows changes: `CC-BY-4.0` or `CC0-1.0`. Licenses with a non-commercial or no-derivatives term are refused,
and so is anything without a license. The manifest names every work, and a `credited` mod without a complete entry does
not load:

```json
"asset_policy": "credited",
"credits": [
  { "title": "food_can_soup", "author": "NVIDIA", "license": "CC-BY-4.0",
    "source": "https://huggingface.co/datasets/nvidia/simready-assets",
    "changes": "converted to the game's format; geometry simplified and textures reduced" }
]
```

`dc-mapkit asset` fills this in for a map, writes `CREDITS.txt` into the mod and names the authors in the description. A
pack that claims a license it does not have is still refused at review: check the source yourself.

A client-visual mod with its own crosshair picture ships `assets/crosshair.dtx` (for example a 32 x 32 32-bit DTX) and its
`main.lua` sets `crosshair` and `crosshair_colour`. Write your own textures with DeCombat's texture writer
(`dc_tex::write_bgra8`), or with any tool that writes the 32-bit DTX layout, from pictures you made yourself.

New maps are the same idea: a shared mod ships an original world file written with DeCombat's own writers, delivered by
the server under a size cap. The objects in such a file use DeCombat's own map vocabulary (`dc.player_spawn`,
`dc.bomb_site`, `dc.world_settings`, ..., docs/MAPKIT_VOCAB.md; rules MAP-1..MAP-3), never a ROM class or property
name; the servers and the client accept those names next to the ROM's hashed ones (the client renderer reads the fog
of `dc.world_settings`, the sun of `dc.global_light`, the `dc.point_light`s that light objects and the `visible` /
`blend` / `alpha` of a `dc.world_model` this way; MAP-26). The own property names count only on a `dc.*` object; a
ROM map's objects keep resolving by hash alone (MAP-2). The map builder that writes them, dc-mapkit, is
docs/MAPKIT_PLAN.md.
The game server's rules for such a map (team texts `"1"` / `"2"`, the `modes` lists, the default flag look, the
sky face: a shot into it answers the sky surface id `dc_world::SURFACE_SKY` so no client draws a decal there, the
world settings) are DC_DEFAULTS.md MAP-20 .. MAP-25, summarised in docs/MAPKIT_AGENTS.md section 4.

## 9. Testing with `dc-modkit`

| command | what it does |
|---|---|
| `dc-modkit validate <dir> [--rom <CA_ROM>] [--json]` | parses the manifest (kind, capabilities and their consistency), runs `main.lua` in a throw-away copy under the real host and the declared capabilities, checks that every registered map's world exists under `assets/` and parses, and with `--rom` checks literal patch ids against the ROM tables |
| `dc-modkit lint <dir> [--json]` | `file:line` diagnostics: bytecode, syntax, forbidden APIs, bad `require` and asset paths, undeclared capability use (error), unused declared capability (warning) |
| `dc-modkit pack <dir> -o out.zip [--json]` | gates on validate and lint, writes the deterministic zip, prints the mod-set entry (`id`, `version`, `sha256`; with `--json` also `kind` and `capabilities` under `modset_entry`) |
| `dc-modkit modset [<dir>...] [--client-visual-allowed]` | packs each mod in memory and prints the server's mod-set JSON fragment, in load order |

Example output of `dc-modkit modset demo_rules`:

```json
{
  "client_visual_allowed": false,
  "mods": [
    {
      "capabilities": ["content.weapons", "events", "hooks.lobby", "save"],
      "id": "demo_rules",
      "kind": "server_rules",
      "requires_client": false,
      "sha256": "47042c7411b2a888932194fffc6aad16195acfda13a38371ef83d372ce8dade0",
      "source_repo": "someone/demo_rules",
      "version": "0.1.0"
    }
  ],
  "policy": "modded"
}
```

(`capabilities` is printed one per line by the tool; shown compact here.) `modset` fails, with a message per problem, on
a mod that does not validate or lint, an unresolved dependency or a duplicate id. With no directories it prints the
vanilla fragment: `"policy": "vanilla"` and an empty `mods` list.

ROM checks need the ROM folder (`--rom` or `CA_ROM`); everything else works without it.

## 10. Vanilla means no mods

A server with an empty mod set is **vanilla**, and vanilla means no mods at all, cosmetic ones included:

- joining a vanilla server unloads every client mod; the client enforces this locally;
- the server checks the mod-set hash sent at login and requires the empty-set hash;
- the host itself refuses to load any mod while its policy is vanilla;
- server-side visibility culling (enemy positions a player could not plausibly see are not sent) is the default on
  vanilla servers; a wallhack-style server is a declared modded server that turns culling off.

The original 2009 client can only see vanilla servers.

## 11. Error messages you will meet

| message | cause |
|---|---|
| `Manifest missing required field 'kind'` | add `"kind"` |
| `Mod 'x': mod.content.hud:override: Unknown HUD override key 'k'; the keys are: ...` | use one of the keys of section 6 |
| `Mod 'x': mod.content.hud:override: HUD override 'hud_scale' must be a number from 0.5 to 2 ...` | fix the value's type or range |
| `Mod 'x': mod.content.maps:register: unknown key 'k' in the map record; the keys are: ...` | use the keys of "Map mods" in section 6 |
| `Mod 'x': mod.content.maps:register: missing required key 'world'; the required keys are: id, name, world, modes` | add it |
| `Mod 'x': mod.content.maps:register: 'world' must be this mod's own asset @mod:x/assets/....dat as mod.assets:path("assets/...") returns it` | ship the world under `assets/` and name it through `mod.assets:path` |
| `Mod 'x': mod.content.maps:register: 'modes' entry 2 "zombie" is not a mode name; the names are: ...` | spell the mode as listed (`TDM`, `BOMB`, ...) |
| `Mod 'x': mod.content.maps:register: map "k" is already registered by mod 'y'` | two mods of the set claim one map key; rename yours |
| `Mod 'x': mod.content.maps:patch: patches map "k", which no loaded mod registered` | a patch needs the registering mod earlier in the set (a dependency) |
| `Mod 'x': mod.content.maps:patch: 'goals' names mode "TDM", which 'modes' does not list` | your patch replaced `modes` whole (lists are values); carry `goals` for the new list too |
| `Mod 'x': map 'k': world '@mod:x/assets/maps/k.dat' does not exist under the mod folder` / `does not parse as a version-85 world` | `dc-modkit validate`: build the world with dc-mapkit |
| `Unknown mod kind 'x'` | use `server_rules`, `shared` or `client_visual` |
| `Manifest 'requires_client' is true but kind 'server_rules' ...` | remove the field or change the kind |
| `Unknown capability 'x'` | check the spelling; the message lists the vocabulary |
| `Capability 'hooks.hud' is not allowed for kind 'server_rules'` | a server-only mod has no client view; use `shared` |
| `Capability 'content.weapons' is not allowed for kind 'client_visual'` | the server decides gameplay content; use `shared` |
| `capability denied: mod 'x' (kind k): mod.save:set needs capability 'save'` | add it to `capabilities` (or stop using it) |
| `... registers hook 'x', which is not '<namespace>.<name>' with a known namespace` | name hooks `lobby.`, `combat.`, `render.` or `hud.` |
| `Mod 'x' refused: vanilla policy means no mods at all` | the host is on a vanilla server |
| `Mod 'x' refused: a local mod must be kind 'client_visual'` | shared and server-rules mods load only from the server's mod set |
| `Mod 'x' refused: capability 'hooks.hud' requires a mod declared by the server` | a local mod cannot use render or HUD hooks |
| `Manifest missing required string field 'description'` / `'author'` | add them; the server browser shows both |
| `Manifest 'description' is N characters; it must be 20 to 500` | say what the mod does, in that range |
| `Manifest 'screenshots' needs at least one image for kind 'shared'` | add an image under `screenshots/` (optional only for `server_rules`) |
| `Invalid screenshot path 'x'` | use `screenshots/<name>.png` or `.jpg`, one level, letters, digits, `_`, `-` |
| `Invalid tag 'x'` / `Manifest 'homepage' 'x' is not an http:// or https:// URL` | fix the tag (`a-z 0-9 -`, up to 20) or the URL |
| `manifest.json:N: screenshot 'x' does not exist` / `is not a PNG image` / `the limit is 1048576 (1 MiB)` | add the file, fix its format, or shrink it |
| `mods: the declared mod set cannot be loaded (LG-270)` | the lobby's `[[mods.set]]` names a mod whose zip or folder is missing or hashes differently |

## 12. Making content for Vanilla++

Vanilla++ is the curated server: new maps, modes and weapons that feel true to the 2009 game. It shares progress with
vanilla, so the bar is higher than on a modded server, and every pack is reviewed before it goes live.

**What a pack may do.** A pack is kind `shared` and declares only `content.weapons`, `content.maps`, `content.text`,
`content.hud` and `assets`. The lobby and the client both refuse anything else at load: `hooks.lobby`, `content.shop` and every other way to
change rewards, EXP, GP, NX, shop prices or lobby rules, `world.full_visibility`, render hooks, `combat.no_recoil`. A new
gun takes a row in the reserved numbers 900..999 (`mod.content.weapons:register({id = "Weapon900", ...})`). No gun is sold
in the shop yet.

**Balance against the original guns.** A new gun has to earn its place beside the 2009 arsenal, not beat it.

- Pick the closest original gun in its class and start from those numbers: damage, fire rate, recoil, spread, magazine,
  reload, movement speed.
- Give it a trade-off. Any gain over its reference gun is paid for somewhere else, so a player can reasonably prefer
  either one.
- No new best in class. If it is the obvious pick for everyone, it is out of balance, however good it feels.
- Name the reference gun and explain each difference in the pack's description, so that review can check it.
- Test it in real matches against the reference gun, at the ranges that class is used at.

**Feel like 2009.** Handling, sounds, look and names should fit the original game. No Nexon assets (section 8).

**Pricing.** A gun may be priced in GP or NX, as the author chooses. NX is not bought with money here: players earn it
from daily login bonuses, clan wars and special events, so an NX price is a rarity choice. Price a gun in line with
comparable original guns.

**Maps.** A map pack is a curated pack: kind `shared` with `content.maps` and `assets`, and it may also carry
`content.text` and `content.hud`. Build it with `dc-mapkit` (section 13) and try it with `dc-mapkit play <project>
--headless`, which runs it on a local Vanilla++ entry. `dc-mapkit export <project> --zip --no-rom-paths` makes the
zip (without `--no-rom-paths` for a map drawn with the player's own ROM textures: the pack then holds texture paths, no ROM data, its source may not be committed here, and `reviewer_note` says so, MAP-100), and the map goes through the same process as a gun: one row in `community/vanilla_plus.json`, with `reviewer_note`
saying what the map is, approved by a community admin. Once the row is merged, the operator adds the entry's `mods` line as below (the same steps with
a map's host notes are in docs/VPS.md "vanilla++"). Players then see the map after the ROM's maps in the waiting room,
on that entry only, and the entry's game servers load the world, also on a remote host.

Before you export, run `dc-mapkit pictures <project>`: it makes the preview players see in the waiting room, the loading
screen and the title banner at load-in. A map without them still works, but `export` warns and those screens stay bare. The
overview at the round result comes from a `dc.end_camera` object; without one, `build` works one out.

**Your own server.** Anyone hosting a modded server may run any maps they want, with no review: `policy = "modded"`, the
zips in `paths.mods_dir` and one `[[mods.set]]` row per map (docs/VPS.md "Several instances on one box"). Players are asked
to accept each unverified mod before it downloads (section 7). A modded lobby plays community maps on its own machine; a
remote game host (`[[lobby.servers]]` `host`) is refused for them.

**How to submit.** Pull requests go to [github.com/argallo/DeCombat](https://github.com/argallo/DeCombat):

1. Build the pack (`dc-modkit lint`, then `dc-modkit pack`) and test it against the original guns.
2. Publish its source in a public repository, with no Nexon assets (section 8).
3. Open a pull request that adds one row to `community/vanilla_plus.json`: `id`, `version`, the `sha256` of the zip,
   `source_repo`, and in `reviewer_note` the reference gun and each difference from it. `dc-config check-vanilla-plus
   community/vanilla_plus.json` must pass.
4. A community admin reviews every pack, guns and maps alike. Today the project maintainer is the only admin; later admins may approve as well. A merged row is the acceptance, and the note is rewritten to say what
   was reviewed.

**Your own mod repository.** A pack lives in a repository of its own, not in the game's. `tools/modrepo.py` (Python 3,
standard library) makes one and keeps it buildable: `modrepo.py init <dir> --github <owner/repo> --decombat-rev <tag>` writes the layout
(`mods/<id>/` per pack, `generators/<id>/` for the scripts that drew a pack's art, `.gitattributes` so Windows checkouts
never change a hash, a GitHub workflow that installs `dc-modkit` and packs every mod on each push, and a copy of the tool);
`modrepo.py add <repo> <mod folder>` copies a pack in from a clean git copy; `modrepo.py build <repo> [--modkit <exe>]
[--rom <client folder>]` validates, lints and packs every mod into `dist/` with `packs.json` and `SHA256SUMS`;
`modrepo.py rows <repo> --format vanilla-plus|known-mods|server-toml|catalog-toml` prints the review-list row, the
known-mods row or the `[[mods.set]]` / `[[mods.catalog]]` blocks; `modrepo.py verify <repo> --against
community/vanilla_plus.json` says whether what you built still hashes to what was reviewed; `modrepo.py lock` writes
`packs.lock.json`, which the workflow checks its own build against (the pinned DeCombat tag is what builds `dc-modkit` there). A `v*` tag publishes a
release with the zips and the rows, so step 2 above is the tag and step 3 is pasting a row. DeCombat's own packs live in
such repositories too; the game repository keeps only the examples its tests read.

**After a row is merged (for the operator).**

1. Put the zip in the lobby's mods folder as `<sha256>.zip`, and run `dc-config vanilla-plus-lines
   community/vanilla_plus.json`.
2. Paste the printed `mods = [...]` lines into the Vanilla++ `[[lobby.servers]]` entry of `server.toml`, and restart the
   lobby. Its log names each pack that loaded or was left out.

**Players' own cosmetic mods.** A plain vanilla server refuses every mod, a player's own crosshair included. The Vanilla++
entry may let them load: `client_visual_allowed = true` in its `[[lobby.servers]]` entry (beside `mods`, LG-426). A
player's enabled `client_visual` mods under `<profile>/mods/local/` (such as `drawn_crosshair`, with its settings) then
load after the entry's packs; the curated limit above still applies to the packs, and a local mod keeps the local rules
(kind `client_visual` only, no render or HUD hooks).

**Players' field of view.** On a modded lobby and on a curated entry the player may pick a field of view in Options (Graphic
tab, "Field of View": 70 to 120 in steps of 5, the 4:3-horizontal angle; the 2009 view is 90, the default). The server
publishes the range it allows as `client_fov: [min, max]` in its mod-set JSON (LG-429, CLI-1916); the client clamps the
player's choice into it. Without a key the range is `[70, 120]`. A server that wants everyone on the 2009 view sets
`client_fov = [90, 90]` in the `[[lobby.servers]]` entry (or `[mods] client_fov` on a modded lobby), which locks the row;
`dc-config check` takes 60 to 130 with the minimum not above the maximum. A vanilla server publishes no range and the
2009 view stays whatever the player saved. Only the world view changes: the first-person weapon and the shop views keep
their fixed look.


### Weapon inspect

A pack can give a gun an inspect: the player presses their inspect key (I by default, set on the Control tab) and the gun
turns in the hands to show it off. It is drawn on their screen only, changes nothing in play, and firing, reloading, switching
weapons, sprinting or zooming cancels it at once. A variant has its base gun's inspect, so one inspect for a gun covers
every camo of it, including camos added later; a variant cannot set its own (the keys are not cosmetic keys). Set these with
`mod.content.weapons:patch`; the inspect pack in the mods repository (https://github.com/argallo/DeCombat-mods/tree/main/mods/inspect_g36e_l96a1) is a full example, and the rule is CLI-1845 in `DC_DEFAULTS.md`.

| Key | Value |
|---|---|
| `InspectLen` | the length in ms, at most 15000 |
| `InspectPivot` | `"x y z"`: the point it turns about, in the view (right, up, forward) |
| `InspectPose<n>` | `"ms pitch yaw roll x y z"`, n from 1 to 24: degrees (nose up, nose right, top to the right) and a move, at that time; it starts and ends at rest |
| `InspectSound<n>` | `"ms k"`, n from 1 to 12: the row's own weapon sound `k` (1 to 3 reload, 4 select, 5 deselect, 6 fire, 10 to 14 misc) |

The motion runs through the poses without stopping, with a slight sway of the hands, so a few well-spaced poses are enough. The first-person model ends at the forearms: keep the nose up and to the left and the gun low, and look at the whole motion in a match. A row without `InspectLen` and a pose has no inspect, and a client older than this feature ignores the keys.

### Weapon variants

A variant is a gun with a new look that plays the same as its base gun, in its own row from 500 to 899. It may set only
cosmetic keys, and a camo ships as your own pattern plus a path that paints it over the player's texture at load, so the pack
holds no game pixels. `examples/mods/variant_candy_g36e` is a full example, and the rules are VAR-1 to VAR-3 in `DC_DEFAULTS.md`.
A client older than this feature refuses an entry that has a variant pack, so a variant goes on an entry only after that client
has shipped. A camo can move: `fx=` drifts a pattern over the gun as light (VAR-4, `examples/mods/variant_galaxy_g36e`), and
`flow=` makes the paint itself roll over the gun like a lava lamp (VAR-12, `examples/mods/variant_prism_scar`); a client
without `flow` draws the plain gun.

Players unlock the camos of an entry from the **Camo Crate** (CRATE-1 to CRATE-4, CRATE-10): on an entry with camos, three of four of the
in-match case drops are Camo Crates (the operator sets the share with `camo_share` in `server.toml` `[hisec]`; 75 by default). A crate holds every camo of the entry and, once each, the plain base gun of each camo (its
`variant_of` row) as a regular gun. Opening one gives a regular gun 70 % of the time, or a camo: Common 18 %, Rare 8 %, Epic
3 %, Legendary 1 %. Either lasts the days of a HiSec case: 7 days 50 %, 30 days 35 %, permanent 15 %. Within a tier each item is as likely,
and a tier the crate holds nothing of gives its share to the others in proportion. Every player has the same odds, and the
crate's detail panel in the inbox shows them. A new camo pack on the entry adds its camo and its plain gun as soon as it loads,
with nothing else to edit; the pack itself sets no price or reward.

A variant may name its **rarity tier** with `variant_rarity = "common"`, `"rare"`, `"epic"` or `"legendary"` (any case; Common
without the key). The tier sets which of the crate's odds it shares (CRATE-6, CRATE-10) and how its opening is shown with the case reel (CRATE-7, CRATE-8: bar colour, the
mark's colour climbing as the strip slows, and a bigger reveal for a rarer tier). `dc-modkit validate` refuses another value.
Pick the tier for how special the look is, not for anything about play: a variant plays the same as its base gun whatever its
tier. A client or lobby older than this key refuses an entry that uses it, so it goes on an entry only after both have shipped.

## 13. Maps: `dc-mapkit`

A community map is a `shared` mod made with `dc-mapkit` (docs/MAPKIT_PLAN.md). The map's source of truth is a text
project folder, not a `.dat`:

| file | what |
|---|---|
| `map.json` | the project (`"format": 1`): terrain settings, materials, solids, prefabs, imports, objects in the `dc.*` vocabulary, world settings, mod metadata, build settings |
| `terrain.r16` | the heightmap (little-endian u16, row-major) |
| `assets/` | your own or credited textures (PNG) and OBJ meshes (section 8); ROM paths are allowed at run time but never committed |
| `AGENTS.md`, `.gitignore` | scaffolded; `build/` and `.mapkit/` stay out of git |

Verbs today (every one takes `--json`; exit 0 ok, 1 diagnostics, 2 usage): `new <dir> [--template starter|flat|arena]`,
`info`, `validate`, `schema [--ops]`, `get`, `apply <dir> <ops.json|->` (a JSON list of ops, all-or-nothing, with
`--dry-run`), `terrain <dir> gen|raise|lower|smooth|flatten`, `snapshot` / `history` / `revert`, `build <dir>`
(writes `build/<key>.dat`: the level model with the kd / BSP tree, one model and object per brush solid, baked
vertex colours, kind-1 render sections per texture, sky portals, `dc.world_settings` and `dc.global_light` from
the project settings; parses it back and runs the world-level checks, `validate --world` runs them without
writing; DC_DEFAULTS.md MAP-43 .. MAP-48). Own textures are named `@mod:<mod id>/assets/<name>.dtx` inside the
world, the path `export` writes the converted DTX to. `render <dir> --view
top|iso|spawn:<n>|free:... [--labels] [--grid]` (a PNG from a CPU rasteriser, no GPU or ROM), `export <dir>
[--zip] [--no-rom-paths] [--force]` (the mod folder `build/mod/<id>/`: `manifest.json`, `main.lua` with
`mod.content.maps:register{...}`, the `.dat`, own PNGs as DTX, the radar picture and the screenshot from the
software renderer, validated / linted / packed by dc-modkit, `--zip` for the hashed zip; a build whose
world-level checks found errors is refused unless `--force`) and `rom
textures|sounds|sprites|surfaces|worlds|modes [--filter] [--thumbs DIR]` (the user's ROM at run time; found by
`--rom`, `CA_ROM`, `rom = "..."` in `<profile>/mapkit.toml` or a `Game/` + `Engine.exe` folder next to the
executable). `run <dir> <script.lua|->` (a Lua generator in the mod sandbox against the `map` API, one all-or-nothing batch;
`math.random` seeded from the project's `seed`, so a script gives the same map every run and `--dry-run` previews it;
three examples under `examples/mapkit/`) and `mcp` (the verbs as MCP tools over stdio for Claude Code and other
agents; rules MAP-56 .. MAP-60, also the repo skill `.claude/skills/mapkit`). The editor `dc-mapkit-gui [<dir>]`
(`dc-mapkit gui` starts it) is the same tool with a window: a welcome screen (new map under `Documents/DeCombat
Maps/<name>`, open, choose the ROM folder), a dc-render viewport of the real build with a fly camera, tools to
place objects and solids and brush the terrain, a typed inspector, an asset browser with own and ROM texture
thumbnails, problems, Build / Validate / Export / Render / Play; every edit is an op batch with a history
snapshot, so the CLI, scripts and agents see the same project (MAP-61 .. MAP-66; two more ops, `settings.set` and
`meta.set`). `play <dir> [--headless] [--seconds N] [--modded]`
is Vanilla++ on this machine: the project is exported, a vanilla lobby whose featured entry "Vanilla++" carries the
zip as its curated set starts on 127.0.0.1 together with its game server, and the client joins it through the 2009
front end (in a window, or hands-free headless with the menu pilot picking the entry and creating a room on the map);
`--modded` runs a modded lobby with the zip as its lobby-wide set instead; logs under `build/play/`, rules MAP-67 ..
MAP-69 and MAP-89. The committed example `examples/mods/starter_map`
is the exported starter template (MAP-70). To get a map onto the server, see section 12 "Maps": a map pack is a curated
pack on the Vanilla++ entry (docs/VPS.md, `examples/server-vanilla-plus.toml`, MAP-71), and `cargo test -p dc-client
--test vanilla_plus` plays the example end to end on the real lobby and game server (MAP-72). The agent playbook is
docs/MAPKIT_AGENTS.md; the rules are DC_DEFAULTS.md MAP-10 .. MAP-14, MAP-40 .. MAP-42 (the collision tree),
MAP-43 .. MAP-48 (build), MAP-50 .. MAP-55 (export, render, rom), MAP-56 .. MAP-60 (run, mcp), MAP-61 .. MAP-66
(the editor), MAP-67 .. MAP-72 (play, the example, vanilla++) and MAP-89, MAP-90 (the entry shape, the review row).
