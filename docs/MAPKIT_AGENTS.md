# MAPKIT_AGENTS.md: the playbook for agents building DeCombat maps (v3, stage 3)

Status: v3, 2026-11-01. Every verb is implemented: the stage-1 data verbs (`crates/dc-mapkit/src/cli.rs`;
MAP-10 .. MAP-14), the stage-2 slices `build`, `export`, `render`, `rom` (MAP-40 .. MAP-55) and the AI-native
surface `run`, `mcp`, the Lua `map` API, the typed MCP tools (docs/MAPKIT_PLAN.md section 7, D8; MAP-56 ..
MAP-60), and the stage-3 `play` (Vanilla++ on this machine, section 16; MAP-67 .. MAP-69, MAP-89) and `gui` (the editor,
section 17; MAP-61 .. MAP-66). No verb answers `not_implemented`; the result shapes in section 3 are taken from
the binary.

This file is also served by the MCP server as the resource `mapkit://playbook`.

## 1. What a map is

A map is a folder (MAPKIT_PLAN.md 5.1; DC_DEFAULTS.md MAP-10):

| file | role |
|---|---|
| `map.json` | the project: `format: 1`, name, key, seed, terrain settings, `materials`, `solids`, `prefabs`, `imports`, `objects`, `settings`, `mod_meta`, `build`. Unknown keys are refused. |
| `terrain.r16` | heights: little-endian u16, row-major (row = z), `(cells_x + 1) x (cells_z + 1)` entries; world y = `origin[1] + raw / 65535 * height_scale` |
| `assets/` | own or credited textures (PNG) and OBJ meshes (section 18); ROM paths are a run-time choice and never go into a committed project |
| `AGENTS.md` | the project's own short guide (scaffolded by `new`) |
| `build/`, `.mapkit/` | outputs and history; ignored by git |

Units: world units; a standing player is 100 tall (a box 30 wide, 100 high: leave at least that above a floor); the templates are 4096 x 4096. A solid's `pos` is its CENTRE and
`size` its full extents; an object's `pos` is its point; rotations are degrees (a spawn's yaw: 0 = +z, about y;
yaw 90 looks along +x). Ids are `<prefix>_<n>` (`player_spawn_3`, `solid_12`) and are never reused. Paths are
forward-slash and relative to the project (no backslash, drive letter, leading slash or `..`).

The full field reference with one line per field: `dc-mapkit schema` (JSON). Ops only: `dc-mapkit schema --ops`.
The vocabulary of classes and properties as the game reads them: docs/MAPKIT_VOCAB.md (MCP resource
`mapkit://vocab`).

## 2. The loop

```
dc-mapkit info <dir> --json                      what is here: counts, spawns per team, modes, extent, size, warnings
dc-mapkit get <dir> --class dc.player_spawn --json   query objects (--id, --near x,z,r)
dc-mapkit apply <dir> ops.json --json            change it with JSON ops (or `-` for stdin); --dry-run to check first
dc-mapkit run <dir> gen.lua --json               or generate it with a Lua script (section 6); --dry-run as well
dc-mapkit terrain <dir> raise --x 0 --z 0 --radius 300 --amount 40   one terrain op as flags
dc-mapkit validate <dir> --json                  project checks; exit 1 on errors, each with a code and a fix hint
dc-mapkit build <dir> --json                     build/<key>.dat plus the world-level sweeps
dc-mapkit render <dir> --view top --labels --json     look at it without a GPU (top | iso | spawn:<n> | free:...); the PNG is result.image (<dir>/build/render/top.png)
dc-mapkit export <dir> --zip --json              the mod folder and the zip the lobby serves
dc-mapkit history <dir> / revert <dir> --to <n>  undo (apply, terrain and run snapshot before every change)
```

Read `info`, decide, `apply` or `run`, `validate`, `build`, `render`, look, repeat. Every verb takes `--json` and
prints exactly one JSON object on stdout:

```json
{"ok": true, "verb": "apply", "result": {...}, "diagnostics": [{"code": "...", "severity": "...", "path": "...", "message": "...", "fix": "..."}]}
```

Everything else (log lines such as `snapshot 3 taken`, a script's `print` output as `script: ...`) goes to
stderr. Exit codes: 0 ok (warnings keep 0), 1 error diagnostics (`ok: false`, `result: null`), 2 usage.

## 3. Every verb, with its example and result shape

| verb | example | `result` |
|---|---|---|
| `new <dir> [--template starter\|flat\|arena] [--name N] [--author A] [--seed S]` | `dc-mapkit new my_map --template arena --name "My Map" --author me --json` | `{dir, template, key, files}` |
| `info <dir> [--rom DIR]` | `dc-mapkit info my_map --json` | `{name, key, format, tool_version, seed, terrain: {cells_x, cells_z, cell_size, min_y, max_y}, counts: {objects, solids, brushes, prefabs, imports, materials, polys}, objects_by_class, spawns: {team1, team2, any}, modes, extent: {min, max, span}, size_estimate_bytes, own_textures, rom_textures, errors, warnings, rom}` |
| `validate <dir>` | `dc-mapkit validate my_map --json` | `{errors, warnings}`; the findings are in `diagnostics` |
| `schema [--ops]` | `dc-mapkit schema --ops` | `{format, tool_version, ops: {doc, list: [{op, doc, fields: [{name, type, doc}]}]}, project: {structs}, prop_value, classes, modes, files, codes, exit_codes}` |
| `get <dir> [--class C] [--id ID] [--near x,z,r]` | `dc-mapkit get my_map --near 0,-1024,200 --json` | `{objects: [MapObject], count}`; with `--id`: `{kind: object\|solid\|prefab\|import, item}` |
| `apply <dir> <ops.json\|-> [--dry-run]` | `dc-mapkit apply my_map ops.json --json` | `{applied: [{op, ids}], changed: ["map.json", "terrain.r16"], dry_run}` |
| `terrain <dir> gen\|raise\|lower\|smooth\|flatten [flags]` | `dc-mapkit terrain my_map gen --preset valley --amplitude 128 --json` | as `apply` |
| `run <dir> <script.lua\|-> [--budget N] [--dry-run]` | `dc-mapkit run my_map examples/mapkit/koth_tower.lua --json` | as `apply` plus `ops` (the count queued) and `prints` (the script's `print` lines) |
| `snapshot <dir> [--label L]` | `dc-mapkit snapshot my_map --label "before cover" --json` | `{n, label, time, terrain}` |
| `history <dir>` | `dc-mapkit history my_map --json` | `[{n, label, time, terrain}]` oldest first |
| `revert <dir> --to <n>` | `dc-mapkit revert my_map --to 3 --json` | the entry restored (the current state is snapshotted first) |
| `build <dir>` | `dc-mapkit build my_map --json` | `{file, bytes, stats: {polys, brushes, sky_faces, nodes, depth, textures, vertices, triangles, sections, bytes}}`; the world-level findings are in `diagnostics` |
| `export <dir> [--zip] [--no-rom-paths]` | `dc-mapkit export my_map --zip --json` | `{dir, zip, sha256, files, bytes, stats}` (the mod folder under `build/`; `zip` and `sha256` are null without `--zip`; `stats` as `build`) |
| `render <dir> --view top\|iso\|spawn:<n>\|free:x,y,z,yaw,pitch [--size WxH] [--labels] [--grid] [--grid-step N] [-o out.png]` | `dc-mapkit render my_map --view iso --labels --json` | `{view, image, width, height, triangles, legend: [{key, color}], markers: [{id, class, key, x, y, visible}], grid_step, top_bounds}` (`image` is the PNG written, `<dir>/build/render/<view>.png` unless `-o`; section 12) |
| `rom textures\|sounds\|sprites\|surfaces\|worlds\|modes [--filter F] [--thumbs DIR] [--rom DIR]` | `dc-mapkit rom textures --filter rock --json` | `{kind, rom: {path, source} \| null, count, thumbs, rows: [{path, size, width?, height?, thumb?, id?}]}`; every kind but `modes` needs a ROM (`--rom` > `CA_ROM` > `<profile>/mapkit.toml` > a sibling `Game/` folder; `rom.missing` otherwise); `modes` rows are `{id, path}` (section 14) |
| `play <dir> [--rom DIR] [--binaries DIR] [--headless] [--seconds N] [--keep-profile] [--modded]` | `dc-mapkit play my_map --headless --seconds 20 --json` | `{shape, entry, entry_name, community_map_base, pilot_entry_flag, lobby_port, admin_port, game_port, map_index, map_key, mod_id, zip, sha256, profile, config, binaries, rom, headless, client_exit, stopped_by, launch_line, match_lines, client_world, textures_missing, frames, spawn, match_ended, logs}`: Vanilla++ on this machine (a vanilla lobby with the map on its featured entry + game server + client; section 16) |
| `gui [<dir>]` | `dc-mapkit gui my_map` | `{binary, pid}`: starts the editor window (`dc-mapkit-gui`, section 17) as its own process and returns |
| `mcp` | `dc-mapkit mcp` | no JSON envelope, with or without `--json`: stdout carries the MCP session's response lines only (section 7) and the verb exits 0 when stdin closes |

## 4. Ops (DC_DEFAULTS.md MAP-11)

An ops document is a JSON array of `{"op": "<name>", ...}` (or `{"ops": [...]}`). The batch is all-or-nothing: the
first invalid op refuses everything (its diagnostic path is `ops[i].<field>`) and nothing is written. The answer
is `{applied: [{op, ids}], changed: [...], dry_run}`: `ids` echoes the new ids, so add then move in the next batch.

| op | fields |
|---|---|
| `add` | `class` (a `dc.*` point class), `pos`, `name?`, `rot?`, `props?`, `ground?` |
| `remove` | `id` |
| `clear` | no field: every object, solid, prefab and import (materials, terrain, settings and metadata stay, ids start over); `group?`: only those solids; `class?`: only those objects |
| `move` | `id`, `pos` or `delta`, `ground?` |
| `set` | `id`, `prop` (`name`, `pos`, `rot` or a class property), `value` |
| `rename` | `id`, `name` |
| `solid.add` | `kind` (`"box"`, `"wedge"`, `{"cylinder": {"sides": 8}}`, `{"prism": {"points": [[x, z], ...]}}`), `pos`, `size`, `material`, `rot_y_deg?`, `faces?`, `group?`, `brush?`, `ground?`, `rises?` (wedge) |
| `solid.set` | `id` + any of `pos`, `rot_y_deg`, `size`, `material`, `faces`, `group`, `brush`, `rises` (wedge) |
| `terrain.gen` | `preset` (`hills`, `valley`, `ridge`, `flat`), `seed?`, `amplitude?` (256; 0 is a level floor at `base`, as `flat` is), `base?` (0), `scale?` (1024), `octaves?` (5), `cells_x?`, `cells_z?`, `cell_size?` |
| `terrain.raise` / `terrain.lower` | `x`, `z`, `radius`, `amount`, `falloff?` (`smooth`, `linear`, `flat`) |
| `terrain.smooth` | `x?`, `z?`, `radius?` (all three, or none for the whole terrain), `passes?` |
| `terrain.flatten` | `x`, `z`, `radius`, `height`, `falloff?` |
| `material.set` | `id` + any of `texture` (`{"own": "assets/x.png"}` or `{"rom": "<path>"}`), `tile`, `surface_id`, `flags` (`{solid, no_step, sky, hidden}`), `fit?` (`[u0, v0, u1, v1]`) |
| `import.add` | `file` (OBJ), `pos`, `material`, `rot_y_deg?`, `scale?`, `collision?`, `ground?` |
| `prefab.add` | `prefab`, `pos`, `rot_y_deg?`, `scale?`, `ground?` (the prefab library comes with the build slice) |
| `settings.set` | any of `fog_enable`, `fog_near`, `fog_far`, `fog_color` (rgb), `far_z` (> 0), `sun_dir` (vec3, not zero), `sun_color`, `sun_bright`, `ambient`, `sky_size`, `sky_material` (must exist): the world settings the build turns into `dc.world_settings`, `dc.global_light` and the sky model; only the given fields change |
| `meta.set` | any of `name` (the map's label), `version`, `author`, `description`, `tags`, `homepage` (`""` clears), `modes` (DeCombat mode names, upper-cased), `goals` (`{mode: int}`), `round_seconds` (0 clears), `players` (`{min, max}`): the mod metadata `export` writes |

Property values: text, number, bool, `[x, y, z]` or `{"rgb": [r, g, b]}`. `ground: true` snaps y to the floor
(terrain or the top of a box / cylinder / prism solid whose material is not a sky face) plus the standing offset
(40 for a player spawn, the half height of an object with an `extent`, half the size for a solid, 0 otherwise).

Example: add a team-1 spawn on the ground, then a crate next to it.

```json
[
  {"op": "add", "class": "dc.player_spawn", "pos": [-1200, 0, 300], "rot": [0, 90, 0],
   "props": {"team": "1", "modes": ""}, "ground": true},
  {"op": "solid.add", "kind": "box", "pos": [-1100, 0, 300], "size": [64, 64, 64], "material": "block",
   "group": "cover", "ground": true}
]
```

**The end of the round.** `dc.end_camera` is the overview players cut to at the round result: `pos` is the place, `rot` x looks
down, `rot` y turns, in degrees. Place it above the cover with a clear view of the middle. Without one, `build` adds an overview
worked out from the spawns, so look at it with `render --textured --view free:x,y,z,yaw,pitch` (pitch up is positive there, so
`rot` x 35 is pitch -35). (DC_DEFAULTS.md MAP-101)

**Which way a wedge climbs.** No check can tell a sideways ramp from a meant one, so say it: `rises` is `north` (+z),
`east` (+x), `south` (-z) or `west` (-x). `size` stays `[width across, height, run along the slope]` whichever way it
climbs; `pos` is the centre, so the low edge is at `pos.y - size.y / 2`. Without `rises`, a wedge climbs toward its
local +z, and `rot_y_deg` 90 turns that to +x. Giving both is refused.

**Fitting a texture to a face.** A material tiles by default: one repeat every `tile` units, so a picture of a whole
object gets cut wherever the face ends. With `fit`, the region `[u0, v0, u1, v1]` of the texture (0..1, v runs down)
goes once across each face of a solid: right way up and unmirrored from outside, u along the solid's local +x on a
top face. Use one material per region and name it in `faces`, for example `faces = {top = "bin_lid", north = "bin_front"}`.
That is how an atlas texture goes on, and how a container side lands with its rails at the edges. A reversed region
mirrors the picture. Terrain and imports ignore `fit`. (DC_DEFAULTS.md MAP-97)

## 5. The vocabulary (MAPKIT_PLAN.md section 4, docs/MAPKIT_VOCAB.md)

Point classes `add` accepts: `dc.player_spawn` (`team` `"1"` / `"2"` / `""`, `modes` comma list, empty = every
mode), `dc.item_spawn` (`modes`), `dc.bomb_site` (`extent` HALF extents, `number` 0 = A / 1 = B, `modes`,
`defuse_team?`), `dc.flag_zone` (`extent`, `team`, `modes`), `dc.capture_flag` (`team`, `model` empty = default),
`dc.world_settings`, `dc.global_light`, `dc.point_light` (`radius` > 0, `color`, `bright`, `lights_objects`),
`dc.sound`, `dc.sprite`. Brush classes come from solids with a `brush` role: `world_model`, `kill_volume`,
`ladder`, `breakable`, `sky`. The build fills `dc.world_settings`, `dc.global_light` and the `dc.sky_model` from
`settings`. Mode names: `TDM`, `FFA`, `ELIM`, `BOMB`, `FLAG` (and the lobby's other playable names; every name
also serves its `_CLAN` twin). Material ids of the templates: `ground`, `wall`, `block`, `sky`.

What the client's renderer takes from these (DC_DEFAULTS.md MAP-26): the fog of `dc.world_settings` (`fog_enable`,
`fog_near`, `fog_far`, `fog_color`; the sky pass uses `sky_fog_near` / `sky_fog_far` under the same enable), the sun
from `dc.global_light` (`color` x `bright`, direction = `rot`, `to_ambient`), a `dc.point_light` only when
`lights_objects` is true and `radius` > 0 (`color` x `bright`, `cone` for a spot; it lights characters and weapons,
not the world, whose light is baked into the vertex colours), and a `dc.world_model`'s `visible`, `blend`
(`opaque` / `translucent` / `additive`) and `alpha`. The map's light grid is one cell of the baked ambient colour
(MAP-28).

How the game server reads these (DC_DEFAULTS.md MAP-20 .. MAP-25): `team` is `"1"`, `"2"` or `""`; a `modes` list
serves a room when one of its names serves the room's mode (`BOMB` also serves `ELIM`; an empty `modes` serves
every mode; an ELIM room without a `BOMB`-tagged point falls back to the `TDM` points); a `dc.bomb_site` without
`defuse_team` is defended by any team; a `dc.capture_flag` without `model` is created with no model file and the
client draws its own flag marker; a shot that first meets a sky face (surface flag 0x10) stops there with no
decal; the `dc.world_settings` values are sent as the client's own variable names, which the server resolves from
the ROM at launch (so a `-worldfile` run without a ROM sends no fog or far-plane values).

## 6. Lua generators: `dc-mapkit run` (DC_DEFAULTS.md MAP-56, MAP-57)

A script runs in the mod sandbox (Lua 5.4; `string`, `table`, `math`, `utf8`; no `os`, `io`, `debug`, `load`,
`require`) with one extra global, `map`. Every mutating call builds one op, applies it to a working copy (so a
bad call fails at its line and later queries see what the script already did) and queues it; when the script
ends the queue is applied as ONE batch, all-or-nothing, with a snapshot before the write. A script that errors
changes nothing. `--dry-run` runs the script and checks the batch without writing. The instruction budget is
1e9 (`--budget N`); the bulk terrain ops reshape the whole grid in one call, so a script rarely needs a per-cell
loop. `print(...)` lines come back in `result.prints` and on stderr.

Vec3 arguments take `{x, y, z}` or `{x = .., y = .., z = ..}`; a missing optional field takes the op's default;
an unknown field name is an error (typos do not pass; a solid table with both `rot` and `rot_y_deg` is refused
too). Calls work with `.` or `:`.

`math.random` is deterministic: it is the tool's own generator, seeded from the project's `seed` (`map.seed`;
`dc-mapkit new --seed N`, or `"seed"` in map.json), with Lua's call forms (`math.random()` a float in [0, 1),
`math.random(m)` an integer in 1..m, `math.random(m, n)` in m..n; an empty interval or a non-integer bound is an
error as in Lua; there is no `math.randomseed`). The same project and script queue the same ops on every run, so
`--dry-run` previews exactly what `run` writes; want another randomised layout, change the project's `seed`.

| call | returns |
|---|---|
| `map.add{class, pos, name?, rot?, props?, ground?}` | the new id |
| `map.remove(id)` | the id |
| `map.clear{group?, class?}` (the `clear` op) | how many things it removed |
| `map.settings{fog_enable?, fog_near?, fog_far?, fog_color?, far_z?, sun_dir?, sun_color?, sun_bright?, ambient?, sky_size?, sky_material?}` (the `settings.set` op) | nothing |
| `map.move(id, {x, y, z})` / `map.move(id, {delta = {x, y, z}})` / `map.move(id, {ground = true})` | the id |
| `map.set(id, prop, value)` (value: text, number, bool, `{x, y, z}`, `{rgb = {r, g, b}}`) | the id |
| `map.rename(id, name)` | the id |
| `map.solid.box{pos, size, material, rot?, faces?, group?, brush?, ground?}` | the new id |
| `map.solid.wedge{rises?, ...}` / `map.solid.cylinder{sides?, ...}` / `map.solid.prism{points = {{x, z}, ...}, ...}` | the new id |
| `map.solid.set(id, {pos?, rot?, size?, material?, faces?, group?, brush?, rises?})` | the id |
| `map.prefab(name, {pos, rot?, scale?, ground?})` | the new id |
| `map.asset{asset, pos, rot?, scale?, ground?, solid?, surface?}` (section 18) | the first new id |
| `map.meta{name?, description?, author?, ...}` (the `meta.set` op) | nothing |
| `map.import{file, pos, material, rot?, scale?, collision?, ground?}` | the new id |
| `map.material.set{id, texture?, tile?, surface_id?, flags?, fit?}` (`texture` text = an own PNG path; `{rom = ...}` for a ROM path) | the id |
| `map.terrain.gen{preset?, seed?, amplitude?, base?, scale?, octaves?, cells_x?, cells_z?, cell_size?}` (`map.terrain.noise` is the same call) | nothing |
| `map.terrain.raise{x, z, radius, amount, falloff?}` / `map.terrain.lower{...}` / `map.terrain.flatten{x, z, radius, height, falloff?}` | nothing |
| `map.terrain.smooth{x?, z?, radius?, passes?}` (no table = the whole terrain) | nothing |
| `map.terrain.height(x, z)` | the terrain height or nil |
| `map.ground(x, z)` | the floor (terrain or a solid top) or nil |
| `map.query{class?, id?, near = {x, z, r}?}` (`near` also as `{x = .., z = .., r = ..}`) | a list of `{id, class, name, pos, rot, props}` (plain tables) |
| `map.solids{group?, brush?}` | a list of `{id, kind, pos, rot_y_deg, size, material, faces, group, brush}` |
| `map.info()` | `{name, key, seed, counts, spawns, modes, terrain, extent, pending_ops}` |
| `map.ops()` | the number of queued ops |

Constants: `map.MODES`, `map.CLASSES` (the point classes), `map.BRUSHES`, `map.PRESETS`, `map.FALLOFFS`,
`map.seed` (the project's seed, what `math.random` starts from).

```lua
-- a ring of crates on the ground, then every spawn re-grounded
for i = 0, 7 do
  local a = i * math.pi / 4
  map.solid.box{ pos = { 600 * math.cos(a), 0, 600 * math.sin(a) }, size = { 64, 64, 64 }, material = "block", ground = true }
end
for _, o in ipairs(map.query{ class = "dc.player_spawn" }) do
  map.move(o.id, { ground = true })
end
print("queued", map.ops())
```

Errors: `{"code": "script.error", "path": "script:<line>", ...}` for a syntax or runtime error; a refused op keeps
its own code (`id.unknown`, `object.class`, `op.invalid`, ...) with the path `script:<line>.<field>`;
`script.budget` when the budget runs out. A call inside `pcall` that is refused queues nothing and the script
goes on.

Three complete generators live in `examples/mapkit/` (a test runs each against the flat template and validates):
`arena_symmetric.lua` (mirrored cover and bases), `canyon_river.lua` (terrain, a river bed, bridges, re-grounding),
`koth_tower.lua` (a stacked tower with a spiral ramp, moved objects, prisms, lights).

**Running a generator again.** A script adds to what is there. A script that may run twice starts with
`map.clear()` (everything it does not need to keep) or `map.clear{group = "name"}` with every solid it makes in that
group. Exact repeats are reported as `solid.duplicate` / `object.duplicate` warnings; `dc-mapkit revert` undoes a
second run.

## 7. MCP: `dc-mapkit mcp` (DC_DEFAULTS.md MAP-58, MAP-59)

The same verbs as tools over stdio (JSON-RPC 2.0, one object per line, logging on stderr; no ROM or GPU
needed). Every tool takes `project` (the folder, absolute or relative to the server's working directory);
`schema` and `rom_list` accept it but do not need it. Tool results carry the CLI's JSON envelope as text
content (the result shapes of section 3); `isError: true` means the verb failed (read `diagnostics`). `render`
takes `view`, `width`, `height`, `labels`, `grid`, `grid_step`, `textured` and `out`, answers the CLI's `render` result
(`image`, `markers`, `legend`, `top_bounds`, ...; `out` echoed when given) and adds `{type: "image", data:
<base64 PNG>, mimeType: "image/png"}` content. Protocol versions: `initialize` answers 2025-06-18 or 2024-11-05
as the client's own and 2025-06-18 to any other (2025-03-26 included: that revision wants JSON-RPC batches,
which this one-object-per-line server refuses with `-32600`). A line that is not JSON, or not UTF-8, is answered
`-32700` and the session goes on; stdout never carries anything but response lines, not even after stdin
closes.

Tools: `new`, `info`, `validate`, `build`, `export`, `render`, `apply` (`ops` array), `get`, `terrain` (flat flags),
`snapshot`, `history`, `revert`, `run` (`script` = inline Lua text, `budget`, `dry_run`), `rom_list`, `schema`, and
the flat typed twins `add_object`, `move_object`, `set_property`, `remove_object`, `terrain_noise` (each one op
with the op's fields as arguments plus `dry_run`). Resources: `mapkit://playbook` (this file),
`mapkit://schema` (the schema JSON), `mapkit://vocab` (docs/MAPKIT_VOCAB.md).

Claude Code (`.mcp.json` in the folder you work from; the binary must be on `PATH` or give its full path):

```json
{
  "mcpServers": {
    "mapkit": {"command": "dc-mapkit", "args": ["mcp"]}
  }
}
```

Claude Desktop (`claude_desktop_config.json`), Cursor (`.cursor/mcp.json`) and other stdio clients take the same
`{"command": "dc-mapkit", "args": ["mcp"]}` entry under their server list; a client that wants a working
directory sets it to the folder that holds your projects and passes `project` names relative to it. From the
checkout without an installed binary: `{"command": "cargo", "args": ["run", "-q", "-p", "dc-mapkit", "--", "mcp"],
"cwd": "<the DeCombat checkout>"}`.

A typical session: `initialize` -> `tools/call info {project}` -> `tools/call run {project, script}` or
`add_object {project, class, pos, props, ground}` -> `tools/call validate {project}` -> `tools/call build
{project}` -> `tools/call render {project, view: "iso"}` (look at the image) -> repeat; `history` / `revert` to
undo.

## 8. Reading diagnostics (DC_DEFAULTS.md MAP-13)

Each diagnostic has a `code` (`dc-mapkit schema` lists them all with their meaning), a `severity`, a `path` into
`map.json` (`objects[3].props.team`), an op (`ops[1].id`) or a script line (`script:12.pos`), a `message` and
often a `fix`. Fix errors first; a warning does not stop a build. The ones you will meet most: `id.unknown` (list
ids with `get`), `material.missing` (add it with `material.set` first), `object.class` (brushes are solids),
`spawn.count` (8 per team, 16 team-less), `bomb.number` (0 and 1), `extent.*` (keep the map within 32,768 units
per axis), `size.estimate` (own textures count w x h x 4 bytes; 512 x 512 is 1 MiB; the estimate also counts
the world at 300 bytes per polygon, the radar picture and a screenshot), `script.error` /
`script.budget` (section 6), `play.binaries` / `play.lobby` / `play.client` (section 16), `not_implemented` (`gui` is not in this
build yet). From `build`: `solid.shape` for a cylinder or prism past 255 corners, `bsp.budget` when long rotated
walls all cross one area (split them or make them brushes; section 10), `spawn.floor` / `spawn.brush_floor` /
`spawn.clear` for a spawn the server would not place (section 10), `sky.surface` for a sky face with the wrong
surface id (a tool bug: report it). The `code` text is the same dotted spelling everywhere: `--json`, the MCP
tool results, the plain output and `schema`.

## 9. Templates (DC_DEFAULTS.md MAP-12)

`dc-mapkit new <dir> --template starter|flat|arena [--name N] [--author A] [--seed S]`. All three satisfy the
template contract (walls, sky, kill volume, 8 + 8 + 16 spawns, 2 bomb sites, 2 flag zones + flags, 4 item spawns,
metadata with every mode) and validate clean. `flat` is the blank canvas (terrain at y 0, the kill slab at -96,
walls to 640, the sky lid above them), `arena` adds bases and cover, `starter` puts `arena` on valley terrain.
Start from `arena` for a playable map in one step; from `flat` when a script generates everything.

## 10. Limits

- Own or credited textures only in anything committed; a `{"rom": ...}` texture is checked at build time against the user's
  ROM and warned `rom.unverified` by `validate`.
- Keep the world within 32,768 units per axis (pos48 precision) and the package under 32 MiB (`mods.max_zip_bytes`; every player downloads it once per version, so smaller is still kinder).
- A cylinder has at most 255 `sides` and a prism at most 255 `points` (the world file counts a polygon's
  corners in one byte): `solid.shape` refuses more.
- Long rotated walls that all cross one area blow up the collision tree (every such wall is duplicated into
  every other's subtrees; the builder does not clip): `build` stops at its node budget and answers `bsp.budget`
  naming the solids. Split those walls into shorter pieces, align them to the axes, or make them `world_model`
  brushes. Three 5,000-unit diagonals over a map are fine; six through one point are not.
- A spawn is placed the way the game server places it: the server's 106 x 48 standing body (half extents
  24 / 53 / 24; the real ROM body is smaller) is lifted 18 and dropped onto the LEVEL geometry only. A spawn
  standing on a `world_model` / `breakable` brush warns `spawn.brush_floor` (the server drops through brushes;
  `ground: true` counts them as floors, the server does not), one with nothing under it `spawn.floor`, one whose
  body touches a wall or ceiling where it comes to rest `spawn.clear`.
- `export` refuses a build whose world-level checks reported errors (`ok: false`, `build.fail` "nothing
  exported"); `--force` packs it anyway.
- A script gets 1e9 Lua instructions, 64 MiB of Lua heap, no file or network access, no `require`; one script =
  one batch = one history entry.
- The MCP server answers one request at a time on stdio; run one server per client; the ROM listings and the
  GPU views need the user's ROM (`rom_list` answers `isError` with `rom.missing` without one, except `kind:
  "modes"`, DeCombat's own list; section 14).
- Prefabs are not in this stage; `prefab.add` stores the instance for the library that comes with the build
  slice. The editor is section 17.

- A built world can be walked without a server: `dc-client --world-file build/<key>.dat --mod-dir build/mod/<id>
  [--rom DIR] [--spawn N] [--headless-frames N --out DIR]` (the client's standalone preview, MAP-33: ROM textures by
  name when a ROM is at hand, `@mod:` textures from the mod folder, a solid colour for anything else; headless
  writes one PNG per frame). `render --textured` runs it for you and keeps the last frame (section 12).
- `build` is implemented (DC_DEFAULTS.md MAP-43 .. MAP-48): the world's light is baked into vertex colours
  (`build.bake` weights over `settings.ambient` / `sun_*`), own textures are named `@mod:<mod id>/assets/<name>.dtx`
  in the world (export writes the DTX), every brush solid becomes a model plus its `dc.*` object, the build adds
  `dc.world_settings` and `dc.global_light` from `settings`, and the sky needs all three pieces the templates have:
  a level solid with a sky material (its faces are solid 0x10 faces and the sky portals), a solid with brush `sky`
  and a sun; every sky face is written with the client's sky surface id (0x6e) so shots into it play no impact.
  The world-level checks run after every build; `spawn.floor` / `spawn.brush_floor` / `spawn.clear` drop the
  server's 106 x 48 standing body through the game's own collision code the way the server does.
- The collision tree of a built map comes from the crate's `bsp` module (DC_DEFAULTS.md MAP-40 .. MAP-42): the
  level gets axis-aligned helper splits down to cells of `build.cell_polys` polygons (16) and a naive BSP inside,
  brushes the naive BSP alone; nodes come out at about 1.1 x the polygon count on terrain and the tree is verified
  against dc-sim's own builder by random sweeps. You never edit it; `build.cell_polys` is the one knob. The
  builder has a node budget (`max(64, 16 x the cell's polygons)` per cell, `16 x polygons` overall): past it the
  build fails with `bsp.budget` instead of running for minutes (see the limits above).

## 11. Troubleshooting

| symptom | cause / fix |
|---|---|
| the same generator gives the same map every run | by design: `math.random` is seeded from the project's `seed` (section 6); change the seed for another layout |
| `map.query{near = ...}` refused | `near` is `{x, z, r}` (three numbers) or `{x = .., z = .., r = ..}`; a fourth key is an error |
| `play.binaries` | build or install `dc-lobby-server`, `dc-dserver` and `dc-client` next to dc-mapkit (or give their folder with `--binaries`); `play.lobby` / `play.client` name the log whose tail explains the failure (section 16) |
| `script.error` with `attempt to call a nil value (field 'open')` or `os` | the sandbox has no `io` / `os`; generate everything through `map.*` |
| `script.error ... unknown field` | a typo in a call's table; the message lists the op's fields |
| `script.budget` | raise `--budget`, or replace a per-cell loop by `map.terrain.gen` / `smooth` / `raise` |
| objects end up far above the floor after terrain work | re-ground them: `map.move(id, {ground = true})` for each `map.query()` result |
| the MCP client drops the server at the handshake | something printed to stdout: run the server with your shell and check that only JSON lines appear; logging belongs on stderr |
| `tools/call` answers `-32602` | unknown tool name or `arguments` is not an object; `tools/list` has the names and schemas |
| `project.io: already holds a map.json` on `new` | pick another folder; `new` never overwrites |
| `history.missing` on `revert` | `history` lists the numbers that exist (50 are kept) |

## 12. Looking at the map: `render` (DC_DEFAULTS.md MAP-53)

`dc-mapkit render <dir> --view V [--size WxH] [-o out.png] [--labels] [--grid] [--grid-step N] --json` draws the
project's geometry with a CPU rasteriser (no GPU, no ROM) and writes a PNG (default `<dir>/build/render/<view>.png`,
1024 x 768; `-o` is resolved against the working directory, not `<dir>`; the path written comes back as `image`).
The MCP `render` tool is the same call with the same answer plus the picture as image content. Views: `top` (straight down, +x right, +z up: the radar orientation), `iso` (from the +x +z corner),
`spawn:<n>` (first person from the n-th `dc.player_spawn` in file order, 0-based: what `get --class
dc.player_spawn` lists), `free:<x,y,z,yaw,pitch>` (degrees; yaw 0 = +z, pitch up positive). `top` and `iso` skip
the sky faces so the lid does not hide the map. `--labels` draws a disc per object, coloured by class (team
variants for spawns, zones and flags); the JSON answer carries `markers` (id, class, legend key, pixel x / y,
visible) and `legend` (key -> colour), so you can read the picture without a font. `--grid` draws lines every
`--grid-step` units (512) on the floor of the extent in the two orthographic views; `top_bounds` in the JSON is the
world rectangle the top picture covers. Expect a few seconds for the starter map at 1024 x 768.

**The map's own pictures:** `dc-mapkit pictures <dir> [--preview-view V] [--loading-view V] [--rom DIR] [--binaries DIR] --json`
draws the preview shown in the waiting room (256 x 256, `assets/pictures/<key>.png`) and the loading screen (1024 x 768,
`assets/pictures/<key>_load.png`) with the game's renderer, and the title banner shown at load-in (512 x 128,
`assets/pictures/<key>_title.png`) from the map's name; `export` packs them, and warns with `picture.missing` when one is
absent. Your own PNGs at those paths work the same. Pick a view that shows the map off (`free:x,y,z,yaw,pitch`). A map using ROM textures gets plain name cards instead of renders,
because a render would put ROM pixels in the pack, so the views do nothing there; `render --textured` still shows such a map to
you locally. (DC_DEFAULTS.md MAP-100a, MAP-103)

**The real textures:** `dc-mapkit render <dir> --textured --view V [--size WxH] [-o out.png] [--rom DIR] [--binaries DIR]
--json` exports the project and draws it with the game's own renderer through a headless `dc-client`: the project's
textures (ROM ones too), its light, fog and sky, no HUD. It writes `<dir>/build/render/<view>_textured.png` by default and
needs `dc-client` and a GPU; without them use the flat view. `spawn:<n>` and `free:` show what a player sees. `top` and `iso`
stand outside the world, so fog hazes them and the sky appears as a box behind the map. `--labels` and `--grid` belong to
the flat view. A picture with ROM textures is the user's own and stays under `build/`. (DC_DEFAULTS.md MAP-95)

## 13. Shipping it: `export` (DC_DEFAULTS.md MAP-50 .. MAP-52)

`dc-mapkit export <dir> [--zip] [--no-rom-paths] [--force] --json` builds the world and, unless the world-level
checks reported errors (then `ok: false` with the build's diagnostics and nothing written; `--force` packs it
anyway), writes `build/mod/<mod id>/`:
`manifest.json` (kind `shared`, capabilities `assets` + `content.maps`, description / author / tags / homepage /
version from `mod_meta`), `main.lua` (`mod.content.maps:register{...}` with `id` = the project key, the name,
description, modes, goals, round seconds, players, the radar picture with its world rectangle, and `picture` /
`loading` when the project has `assets/pictures/<key>.png` / `<key>_load.png`), `assets/maps/<key>.dat`, every own
PNG as `assets/<same path>.dtx`, `assets/pictures/<key>_radar.dtx` (the `top` render, 256 x 256),
`screenshots/main.png` (the `iso` render, 640 x 480) and a `README.md`. The folder is validated, linted and packed
by dc-modkit (the lobby hashes the same bytes); `--zip` writes `build/<mod id>.zip`; the JSON answer carries
`dir`, `zip`, `sha256`, `files`, `bytes`. Mode names outside the DeCombat list are dropped with `mode.unknown`;
no valid mode is `meta.invalid`.

## 14. The user's ROM: `rom` (DC_DEFAULTS.md MAP-54, MAP-55)

`dc-mapkit rom textures|sounds|sprites|surfaces|worlds|modes [--filter TEXT] [--thumbs DIR] [--rom DIR] --json`
lists what the user's own ROM offers, at run time only (never write a ROM path into a committed project). The ROM
is found by `--rom`, else `CA_ROM`, else `rom = "<dir>"` in `<profile>/mapkit.toml`, else a `Game/` + `Engine.exe`
folder next to the executable; `info --json` reports `rom: {path, source}` or `null`, and a listing without a ROM
fails with `rom.missing` naming the four attempts (`modes` needs none: it is DeCombat's own list). Rows carry the
entry `path` exactly as the archive spells it (use that text in `material.set {"rom": ...}`), `size`, and with
`--thumbs` a PNG `thumb` (64 px, cached under `DIR/<rom digest>/`) plus `width` / `height`; `surfaces` rows carry
the table's own `name` (the listing filters on it, and this is how to pick a `surface_id`) and
the surface `id` for `material.set {"surface_id": ...}`; `modes` rows carry the mode class.

## 15. What the lobby does with an exported map (DC_DEFAULTS.md MAP-35 .. MAP-39)

The exported mod's `main.lua` registers the map record (`mod_meta` of `map.json`: `modes`, `goals`,
`round_seconds`, `players`). A modded `dc-lobby-server` whose `server.toml` declares the mod offers the map after
the ROM's rows: the room's mode choices are the record's `modes` (so list every mode the spawns and objectives
serve), `goals[mode]` is the room's starting goal, `round_seconds` the round clock, and the game server is started
on the mod's world file (`-worldfile ... -announce @mod:<id>/assets/maps/<key>.dat`). `GET /status` on the lobby's
admin port lists `community_maps`, a quick check that the map arrived. Clan rooms do not play community maps, and on a
modded lobby neither do remote game hosts; a map on a Vanilla++ entry does run on a remote host (MAP-83). On such an
entry the map list is in `GET /entry_mods/<id>.json`, not in `/status`.

Developing a map never needs any of this: `dc-mapkit play` runs it on a temporary server on your own machine
(127.0.0.1 only, its state wiped each run unless `--keep-profile`), and it never joins or uploads to a live server
(MAP-67). Publishing is the separate review step in docs/MODDING_GUIDE.md section 12.

## 16. Playing it on this machine: `play` (DC_DEFAULTS.md MAP-67 .. MAP-69, MAP-89)

`dc-mapkit play <dir> [--rom DIR] [--binaries DIR] [--headless] [--seconds N] [--keep-profile] [--modded] --json` is
Vanilla++ on this machine, the shape the real server runs: the project is built and exported as a zip, a VANILLA
`dc-lobby-server` with one featured entry "Vanilla++" (entry 1) whose curated set is that zip starts on 127.0.0.1
together with its `dc-dserver`, and `dc-client` joins it through the 2009 front end. It needs the three binaries (next
to dc-mapkit, on `PATH`, or in `target/debug` / `target/release` of the checkout; `--binaries DIR` names a folder) and
the user's ROM (found as `rom` finds it). The instance lives under `<profile>/mapkit/play/` (`server.toml`,
`mods/<sha256>.zip`, `data/`, `client/`; wiped before each run unless `--keep-profile`), the logs under
`<dir>/build/play/`: `lobby.log`, `dserver_c<ch>_r<room>_g<n>.log`, `client.log`, `client.net.jsonl`.

- Window (default): the player logs in (any name and password: accounts are created on the fly), picks the entry
  (its set loads at the pick, with no consent prompt), creates a room, picks the map after the ROM's maps in the waiting
  room's list and starts it, alone or with friends on the same machine; the run ends when the window closes (`--seconds
  N` stops it after N seconds).
- `--headless`: no window; the scripted menu player picks the entry, creates a TDM room on the map's community index
  (`community_map_base` + the record's position in `maps`, read from the lobby's `GET /entry_mods/1.json`), starts it
  alone and plays `--seconds` seconds (default 20), then leaves; the result is judged: the JSON answer carries `shape`
  (`entry`), `entry`, `entry_name`, `community_map_base`, `client_world` (`@mod:<id>/assets/maps/<key>.dat`),
  `textures_missing` (0 when every own texture resolved), `frames`, `spawn`, the lobby's `launch_line` (`... world <path
  of the unpacked pack> (community map <key>, announced as @mod:...)`) and `match_ended` (the game server's `@end`); no
  match or no `@mod:` world is a `play.client` error. This is the agent's way to check that a map really plays: `build`,
  `validate`, `render`, then `play --headless --seconds 10 --json` and read `textures_missing`, `frames`, `match_ended`.
- `--modded`: the same run on a modded lobby with the zip as its lobby-wide set (`shape` is `modded`, the index comes
  from `GET /status`, the window's player answers the real consent prompt). Use it to check a map mod that is not meant
  for the curated entry; the entry shape refuses a pack the curated limit refuses, and `play.lobby` then quotes the
  lobby's `curated pack ... left out` line.

Everything stops when the client exits or the time is up: the lobby through its admin endpoint (`POST /stop`,
the match ends through the game server's pipe), then by signal; game servers the lobby log names are stopped too.
The same chain is pinned by `cargo test -p dc-client --test vanilla_plus` on the committed example
`examples/mods/starter_map` (MAP-70, MAP-72), and a server offers maps the same way, as curated packs on its Vanilla++
entry (docs/MODDING_GUIDE.md section 12 "Maps", docs/VPS.md, `examples/server-vanilla-plus.toml`, MAP-71).

## 17. The editor: `dc-mapkit-gui` (DC_DEFAULTS.md MAP-61 .. MAP-66)

The editor is the same tool with a window: `dc-mapkit-gui [<dir>]` (or `dc-mapkit gui [<dir>]`, which starts
that binary and returns). It does what the CLI does, on the same folder, through the same ops: every gesture
(placing, dragging, deleting, a typed property, a brush stroke, a texture assignment, a world-settings or
metadata edit) is one op batch applied through `apply` with one history snapshot, so an agent and a person can
work on one project in turns: `dc-mapkit history` lists the editor's `gui ...` entries next to `apply` and `run`
ones, `revert` undoes either, and the editor's Undo / Redo are the same `revert`. The editor never writes
`map.json` by another path.

What is where: the welcome screen makes a new map under `Documents/DeCombat Maps/<name>` (templates `starter`,
`flat`, `arena`: exactly `dc-mapkit new`) or opens a folder, and remembers the ROM folder in
`<profile>/mapkit.toml` (the CLI then finds it too). The viewport is the real build (`build` runs on a worker
thread after every change, dc-render draws the `.dat`; own textures from `assets/`, ROM textures with a ROM, a
flat colour otherwise); objects are coloured markers (the `render --labels` colours), a selected solid shows its
box. Right-drag looks, WASD / QE fly, scroll changes the speed, F frames the map, Delete removes the selection.
Tools: Select / move, Place object (the vocabulary's point classes), Solid (box / wedge / cylinder, size, brush
role), Terrain brush (raise / lower / smooth / flatten). The inspector edits the selection's properties with
typed widgets from docs/MAPKIT_VOCAB.md, a solid's pos / size / rot / material / role, the terrain generator,
the world settings (`settings.set`) and the mod metadata (`meta.set`). The Assets tab lists the project's PNGs
and, with a ROM, the ROM's textures with thumbnails (cached under `<profile>/mapkit/thumbs/<rom digest>/`) and a
filter; a click assigns one to the selected solid face, the solid or the terrain. Problems = `validate` plus the
last build's world checks; a row click selects what it names. Top bar: Build (`build/<key>.dat`), Validate,
Export (+ zip), Render (`build/render/top.png`, `iso.png`), Play (`play`), Screenshot
(`build/render/viewport.png` and `editor.png`), Output (the log). `dc-mapkit-gui --smoke <out dir> <dir>` runs
those actions by itself (build, place a spawn, export, play, screenshots) and closes: a quick check of the editor
on a machine.

Building it: `cargo build -p dc-mapkit` (the `gui` feature is on by default; `--no-default-features` is the
data tool alone, which the MCP server and CI builds can use).

## 18. Models from outside: `asset` (DC_DEFAULTS.md MAP-104, MOD-28)

`dc-mapkit asset <dir> <model.glb> --name N --max-tris T --tex S [--orient x90,y45] --license CC-BY-4.0 --author A
--source URL [--title T]` (MCP tool `asset`; `--own` instead of the three credit flags for your own work) reads a glTF
binary, meshopt compression and WebP textures included, and writes `assets/<name>_<k>.obj` and `.png`, one material
`<name>_<k>` per part and a credit. Then `{"op": "asset.place", "asset": N, "pos": [x, y, z], "rot_y_deg": 0, "scale":
K, "ground": true}` (`map.asset{...}` in Lua) places it. The model's units are kept, so glTF's metres need a `scale`.

- **Textures only:** `--textures-only --keep 6,7` takes materials for floors and walls, and no model.
- **Heavy models:** a model of a million triangles takes a minute or more to convert; do it once, then keep the project.
- **What the converter does:** applies texture transforms, turns the model per `--orient`, drops pieces under 2 % of its size
  (bolts) and the surfaces no ray from outside can reach (the
  metal under a can's label), simplifies to `--max-tris`, resizes the base colour to at most `--tex` per side and bakes
  its tint in, puts the base on y 0. Size budget: 256 for a big prop, 128 for a small one (what the game's own world
  textures use), and 200 to 2,000 triangles per prop.
- **Reading the result:** `tris` in the answer can be above what you asked. The simplifier stops short of what visibly
  breaks a model, and only one more than 4 times over budget is forced down. Look at it close up from eye level: the
  simplifier judges shape, not the picture, so a label can still get stripes. If it does, raise `--max-tris` or pick
  another model (`food_can_tomatoSoup_a01` needs 3,000).
- **Collision:** `asset.place` stops players and bullets by itself: hidden boxes that follow a rebaked model's solid parts,
  otherwise the model's outline seen from above extruded to its height (DC_DEFAULTS.md MAP-105, MAP-109). `surface` picks the core's surface id, so its impact marks and footsteps
  (`dc-mapkit rom surfaces` lists them), and `solid: false` leaves out the core for what nobody can reach, a roof lamp. For
  your own hull, a solid of a material flagged `hidden` is collision only. Things stand on a core with `ground`.
- **Choosing how to convert (look at every prop close up; DC_DEFAULTS.md MAP-106):**
  - `--proxy rebake` (start here, for almost everything): a sealed shell around the model, simplified, with one new texture
    drawn from the full-detail source, with contact shading and fine shape painted in. It takes CAD models full of open
    edges and hidden parts in its stride, and thin open sheets get a little thickness. It also records collision boxes
    that follow the solid parts, so a shot passes through what can be seen through. Use `--tex 256` or 512.
  - `--proxy box` (crates, cartons, cabinets) or `--proxy round` (drums, bins): a simple solid of 12 to 44 triangles with
    the full-detail model's look projected on it. Labels stay sharp, nothing can smear, and it is the shape the core
    blocks. Fine detail standing proud of the solid is lost; give these `--tex 256`.
  - `--strict` (a forklift, a rack, a ladder): real geometry near `--max-tris`. A part stuck at its texture seams turns
    into its own average colour, because one colour has no seams to break. Thin lattices can still warp: a rail or a truss
    you can draw is often better as a few see-through solids and a hidden block.
  - neither: real geometry and real textures, for a model with few triangles; it keeps more than asked rather than smear.
- **Keeping to the size limit:** an opaque texture exports as compressed DXT1 with mipmaps, about a sixth of its old size;
  a texture that is one plain colour becomes 4 x 4 pixels and a picture the project already has is reused. A model of many parts adds up fast: use `--tex 128` for all but the big things.
- **Real scale:** 60 units per metre makes the 100 unit tall player a person. Then racks, pallets and crates are what they
  look like, cover and all; props at another scale make a map hard to read and to move through.
- **Map size:** a player runs about 215 units a second. For 3v3 keep team spawns about 12 seconds apart (2,500 units) and
  set `players` to `{min: 2, max: 6}` with `meta.set`; size the plan first and keep the props at real scale.
- **Lighting:** `{"op": "settings.set", "bake_lightmap": 10}` turns on real lightmaps at 10 units a texel (DC_DEFAULTS.md
  MAP-108). Every `dc.point_light` then shines with soft shadows and one bounce of coloured light, the sun comes in only
  where nothing blocks it (leave holes in a solid roof for skylights), models take the same lamps at their corners,
  and players take the light of where they stand from a baked grid. A big map costs tens of lightmap pages: `build` reports
  `lightmap_pages` and `lightmap_bytes`. Use a colour per area and leave some lamps dark; even light reads as a test map.
  A bake takes seconds in a release build. Two solids sharing a plane now show as flickering stripes: do not overlap
  them. `bake_shadow` is the older per-corner shadow, for a map without lightmaps.
- **Bullet marks:** a mark lands on the collision surface, so anything drawn in front of that hides it. Over thin see-through
  solids (wall grime, trim, floor slabs) put a thin solid of a material with `flags={solid=true,hidden=true,skin=true}` just in front
  of them: it stops shots and leaves the light alone. Not every surface id leaves a hole: `dc-mapkit rom surfaces --json` gives `marks` for each row of your ROM. Check by shooting
  at it.
- **Paint on walls:** `asset <dir> tag.png --name tag --width 5 --own` turns a PNG with power-of-two sides into a flat sheet whose
  transparent parts stay holes; place it 1.5 units off the wall with `map.asset{ asset="tag", pos=..., rot=..., solid=false }`. It
  faces +z at `rot` 0. Hard edges only: a texel is paint or a hole. The build warns with `paint.off_wall` when a sheet hangs past its wall or is
  covered by something painted on it.
- **Wire mesh and chain link:** this renderer has no cut-outs, so a bake turns them into a solid noisy sheet. Leave them out with
  `asset --leave-out grid,clips` (words of the material names) and look at the result.
- **Walls and floors:** `asset --textures-only --keep 3,7 --tex 512 --relief 0.35` takes a model's materials as surfaces, with
  their normal maps drawn into the colour. Thin see-through solids make good floor slabs, joints, wall grime and trim;
  their edges and undersides are not drawn in a lit map.
- **Try it in the game:** walk it (`play`), a render shows neither a backward ramp nor a too narrow gap.
- **Credits:** `export` sets `asset_policy` to `credited`, writes `CREDITS.txt` and adds the authors to the description.
  Allowed licenses are `CC-BY-4.0` and `CC0-1.0` (docs/MODDING_GUIDE.md section 8). Keep the models out of the DeCombat
  repository: a project is the mapper's own.

