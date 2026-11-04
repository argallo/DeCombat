# MAPKIT_PLAN.md: dc-mapkit, the community map builder

Status: plan written 2026-11-01 (Mac) from a nine-agent read of the tree; user requirements below. The
sections "Decisions" and "Phases" are binding for the implementers; "Open points" lists what the first end-to-end run
must settle. Rules land in `DC_DEFAULTS.md` under the new prefix **MAP-nn**.

## 1. User requirements (2026-11-01, verbatim intent)

1. A **standalone map building tool** for the community mods, released on GitHub for **mac and windows**.
2. It helps you **set up a map, generate terrain, place assets** that come with the ROM **or add new assets**, and
   **identify spawn points, bomb sites** and the like.
3. A **starter template** map to kick things off.
4. **Really simple** for anyone, with **enough features for those who go all out**.
5. Maps **export** and work with the community-mods infrastructure (**Lua** mods) that is already set up.
6. Testing happens on a modded server, **"vanilla++"**: the standard vanilla server/client plus additions such as
   community maps and game modes.
7. Everything in **Rust and Lua**, like the rest of the tree.
8. (added) Make it **very easy for AI agents to help people build maps**: the tool and the tree are "AI native".

## 2. What the read of the tree established (facts the plan rests on)

- A map is a version-85 world file (`specs/FMT_DAT85.md`); `dc_world::synth::SynthWorld` already writes one. The
  server needs sections 3-7 and 9 (BSP, objects); the client additionally needs section 10 (render data). Blind data,
  the quadtree shape and the light-group grids have no consumer in DeCombat: write them empty / one node.
- **Collision walks the BSP node tree** (`dc-sim/src/collide.rs`), never the polygon list. A trivial tree is wrong
  (chain) or exponential (shared children); `dc_sim::build_bsp` is correct but explodes on real geometry (242 M nodes
  for one ROM level model) and truncates past depth 4096. dc-mapkit needs its own builder (section 5.4). Polygons:
  convex, CCW from the front, plane `dot(n, p) = d`, surface flag bit 0 = solid, `texflags` = footstep surface id
  (0..255, a ROM surface-table row), 0x10 = sky polygon, 0x400000 = no-step. Missing child: negative `-2`, positive
  `-1` (the ROM convention; dc-sim accepts either).
- **The renderer walks render blocks flat**; one block per render world is enough, no block tree, no lightmaps
  required: shader kind 1 = textured, vertex-lit (`texture * colour * 2`, so vertex colour 0x80 = unit brightness).
  Vertex = 68 bytes (pos, uv0, uv1, ARGB, normal, tangent, binormal). Textures resolve by archive-relative path
  through the ROM only; a mod texture needs a new hook in the client's world-texture closure.
- **The game server implements every mode in Rust**; the mode controller, triggers, start-up commands and blind data
  are optional. A map needs: player spawns (team + mode filter), bomb sites / flag zones (box, HALF extents, floors
  96 / 48), capture flags (team + ground model), kill volumes and ladders (closed, convex brush models linked by
  name), exactly one unlinked level model, optionally a world-settings object (fog, far plane) and a sky brush.
- **Every ROM class and property is identified by hash** (`dc_world::roles`, ~70 `PropHash` constants in
  dc-dserver, dc-render, dc-sim, dc-client). Our own names may be committed; ROM names may not. A community map
  therefore gets its **own vocabulary** (section 4) and the readers learn it as aliases.
- **The lobby's map table is the ROM's `Map<n>` rows**; the map index is a u32 on the wire; 0x311 carries at most 30
  (map, mode mask) pairs; the 2009 client cannot play a community map and a modded lobby refuses it at login anyway
  (MOD-1 is not implemented; `allow_legacy_clients` does not exist). Vanilla++ is DeCombat-client-only.
- **Mods**: a `shared` mod (needs description, author, one screenshot) is hash-pinned, downloaded by the client
  (32 MiB hard cap per package, also the lobby default of `mods.max_zip_bytes`), unpacked under
  `<profile>/mods/unpacked/<sha>/` (client) and `<data>/mod_cache/<sha>/` (lobby). `mod.content.<domain>` is driven
  by one list (`CONTENT_DOMAINS`); only `weapons` has a Rust accessor today. `@mod:<id>/assets/<rel>` resolves
  only under `assets/`. The game server runs no Lua and gets mod data only through launch flags (`-content`).
- **dc-dserver** already has `-worldfile <path> -announce <client path>`; the client resolves the announced name by
  basename against `WORLDS.rez` only (`net.rs loader_for`), so a `@mod:` branch is needed there.
- **Release**: ci.yml / release.yml build per package and upload workflow artifacts; no GitHub Release step exists yet
  (the repo is private); mac packing = lipo + ad-hoc codesign + Info.plist + icns; windows = crt-static exe + zip.
  wgpu 25 / winit 0.30 pin **egui 0.32.3** (MIT OR Apache-2.0); its default font pack is OFL + Ubuntu Font Licence
  and stays out (section 6.3). No inline numeric table of 64+ values and no ROM string goes in.
- **ROM assets**: world textures live in `TEXTURES.rez` (2,342 DTX in 14 folders), the ROM maps place **no** LTB
  props (decoration is brush geometry); sounds in `SND.rez`, sprites in `FX.rez`. DTX decodes to RGBA for
  thumbnails; `dc_tex::write_bgra8` writes our own textures.

## 3. Decisions

| # | decision | why |
|---|---|---|
| D1 | Community maps use **DeCombat's own object vocabulary** (`dc.*` classes and short property names, section 4) and the readers accept them as aliases next to the ROM hashes. No ROM class or property name is ever written. | CLEANROOM; the 2009 client cannot play these maps anyway; an agent can read a map without a ROM. |
| D2 | A map ships as a **`shared` mod** carrying `assets/maps/<key>.dat`, pictures and own textures under `assets/`, and a `main.lua` that calls `mod.content.maps:register{...}` (new content domain `maps`). | Rides the existing delivery, hash pinning, consent prompt and `dc-modkit` tooling. |
| D3 | The map's **source of truth is a text project** (`map.json` + `terrain.r16` heightmap + own assets) in a folder the tool, the CLI, Lua scripts and agents all edit; `.dat` is a build output. | AI-native, diffable, reproducible builds. |
| D4 | **Geometry model**: a heightmap terrain, convex solids (boxes, wedges, cylinders, prisms with per-face textures), prefabs made of solids, and imported OBJ meshes (triangles); every surface carries a ROM texture path or a mod texture, a surface id and flags. | Covers simple (terrain + a few blocks) and all-out (Blender import) without a CSG engine. |
| D5 | **Own BSP builder**: an axis-aligned kd split over non-solid helper polygons down to small cells, then a naive BSP per cell; verified by sweep equality against `dc_sim::build_bsp` on small sets and by hit statistics on terrain. | The only workable shape (section 2); the ROM does the same. |
| D6 | **Lighting is baked into vertex colours** by the tool (ambient + sun by normal + optional hemisphere/AO); no lightmaps in v1. Fog, far plane and a sky box come from `dc.world_settings` / `dc.sky_model`. | Kind-1 sections are the minimal drawable path. |
| D7 | The **GUI is eframe/egui 0.32.3 on wgpu 25**, sharing one wgpu with dc-render for the 3D viewport in this direction: the GUI builds `GpuContext::headless(AdapterChoice::Auto)` first (its fields are `pub`, wgpu handles are `Clone`) and starts eframe with `WgpuSetup::Existing { instance, adapter, device, queue }` and `eframe = { default-features = false, features = ["wgpu"] }`; the reverse is impossible (eframe's `RenderState` has no `wgpu::Instance`, `GpuContext::with_adapter` is private and requests `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` plus resolution-scaled limits that eframe's default device lacks, so dc-render's MSAA 2x/8x would be refused). The viewport is an `egui::PaintCallback` over a new render-to-texture entry in dc-render (next to the offscreen readback path); a dual-GPU fallback rebuilds on the other backend when the window surface reports no formats. UI font = Roboto (Apache-2.0) shipped as a binary file with its licence text, bound to BOTH `FontFamily::Proportional` and `Monospace` (egui panics with `default_fonts` off otherwise); file/folder dialogs through `rfd` 0.17 (MIT, default features, no `gtk3`) with a Cargo.toml justification. | Only egui line on the workspace's wgpu/winit; licences inside CLEANROOM 4a. |
| D8 | **AI-native surface**: every GUI action is a CLI verb with `--json`; `dc-mapkit mcp` serves the same verbs as MCP tools over stdio; `dc-mapkit run <script.lua>` runs a generator in the mod sandbox against a `map` API; `render` makes PNGs without a GPU (software top-down / isometric) and with one (perspective through dc-render); every project scaffolds `AGENTS.md`; the repo gains a `mapkit` skill and `docs/MAPKIT_AGENTS.md`. | Requirement 8. |
| D9 | **Development is local, publishing is Vanilla++** (amended 2026-11-01, user decisions). A mapper tests with `dc-mapkit play`: a temporary server on their own machine (127.0.0.1 only, wiped each run), never a live server. A finished map is published as a curated pack on the Vanilla++ featured entry of the vanilla lobby (`[[lobby.servers]] mods = [...]`, LG-313), after review; there is no second lobby instance. | The user built Vanilla++ as an entry that shares progress with vanilla, and mappers should not need a server to iterate. |
| D10 | **Map indices**: community maps are appended after the ROM's `Map<n>` rows in mod-set load order, then record order; 0x311 keeps the 30-pair cap (warn past it); map label, picture, description and loading pictures come from the record and `@mod:` paths. | Minimal wire change; the 2009 wire stays untouched. |
| D11 | The lobby launches the game server with `-worldfile <mod_cache path> -announce @mod:<id>/assets/maps/<file>.dat`; the client's `loader_for` takes a `@mod:` announce first and resolves it through `ModAssets`. | No basename collisions, no WORLDS.rez involvement. |
| D12 | **Scope cuts for v1**: no doors, switches, water, breakables with explosions, co-op objects, LTB prop placement (the ROM maps have none and the client draws none), lightmaps, bots on community maps (remote `dc-host` delivery was cut here and later built for the Vanilla++ entry, MAP-83). Each is listed in section 9 with the extension point. | Ship a playable, simple tool first. |

## 4. The DeCombat map vocabulary (written into the `.dat`, recognised by the readers)

Object property 0/1/2 stay `name` (text), `pos` (vec3, flags 0x00100000), `rot` (euler) by index. All other names
are ours; the readers match them literally (ASCII case-insensitive) next to the ROM hashes.

| class | role / consumer | properties beyond name/pos/rot |
|---|---|---|
| `dc.player_spawn` | `ObjectRole::PlayerSpawn` | `team` (`"1"`, `"2"`, `""` any), `modes` (text, comma list of DeCombat mode names, e.g. `TDM,FFA,ELIM`; empty = every mode) |
| `dc.item_spawn` | `ObjectRole::ItemSpawn` | `modes` |
| `dc.bomb_site` | `ObjectRole::BombSite` | `extent` (vec3 HALF extents, flags 0x4), `number` (int, 0 = A, 1 = B), `modes`, `defuse_team` (optional, `"1"`/`"2"`) |
| `dc.flag_zone` | `ObjectRole::FlagZone` | `extent`, `team` (`"1"`/`"2"`), `modes` |
| `dc.capture_flag` | `ObjectRole::CaptureFlag` | `team`, `model` (text, ROM LTB path; empty = the server's default flag look, MAP rule) |
| `dc.kill_volume` | `ObjectRole::KillVolume` (brush) | `pending_sec` (real); `damage`, `damage_kind` reserved (the server kills outright) |
| `dc.hazard` | `ObjectRole::HazardVolume` (brush) | as kill volume (no server behaviour today) |
| `dc.ladder` | `ObjectRole::Ladder` (brush, convex) | none (the container kind alone makes it climbable) |
| `dc.world_model` | `ObjectRole::PlainWorldModel` (brush) | `visible`, `solid`, `rayhit` (bool, default true), `blend` (`opaque`/`translucent`/`additive`), `alpha` (real) |
| `dc.breakable` | `ObjectRole::Breakable` (brush) | `hit_points` (real) |
| `dc.sky_model` | `ObjectRole::SkyWorldModel` (brush) | `sky_index` (int), `sky_dims` (vec3), `sky_inner` (vec3) |
| `dc.world_settings` | `ObjectRole::WorldSettings` | `far_z`, `fog_enable`, `fog_near`, `fog_far`, `fog_color` (rgb 0..255), `sky_fog_enable`, `sky_fog_near`, `sky_fog_far`, `wind` (vec3), `all_sky_portals` (bool) |
| `dc.global_light` | dc-render `global_light`, dserver `worldlight` | `color` (rgb), `bright` (real), `to_ambient` (real); direction = `rot` |
| `dc.point_light` | dc-render `world_lights` (characters and held weapons only; the world's light is the baked vertex colour) | `color`, `radius` (required, > 0), `bright`, `cone` (real, 0 = point), `lights_objects` (bool, default true: the reader returns `None` unless this flag is true and the radius is > 0, `lights.rs static_light`) |
| `dc.sound` | dserver world sound | `file` (ROM WAV path or `@mod:` path), `volume`, `inner_radius`, `outer_radius`, `loop`, `ambient` |
| `dc.sprite` | dserver scale sprite | `file` (ROM SPR/DTX path or `@mod:`), `scale_x`, `scale_y`, `color`, `alpha` |

Team texts `"1"`/`"2"` are accepted by `TeamNames::team_of` next to the ROM team names (MAP rule). Mode names are
the lobby's playable-mode names (`dc-config` `modes.playable`). Today's rule cannot serve them: dc-dserver compares
the CAPITAL LETTERS of the filter with the room line's `key=` (a ROM mode code, `game/arrival.rs mode_matches`), a
point without a filter never matches, and the BOMB/ELIM special sets key on the ROM S&D filter hash
(`game/bomb.rs spawn_set`). MAP rule: a filter that contains a comma or equals a `modes.playable` name
case-insensitively is a DeCombat list; it is split on commas and matched against the room's DeCombat mode name,
derived from the `type=` class the server already holds (`rules.mode_class`; `mode_class_of_name` moves from
dc-lobby-server to dc-config and is inverted, so no new room-line key); an empty filter matches every mode on a
DeCombat-vocabulary map only; a `BOMB` set also serves `BOMB_CLAN`, `ELIM` and `ELIM_CLAN` rooms (DSV-470
semantics) and every name serves its `_CLAN` twin; the ROM capitals rule stays for ROM maps.
Key mapping notes for the alias layer: `dc.player_spawn.team` and `dc.capture_flag.team` alias `PropKey::TeamFilter`,
`dc.flag_zone.team` aliases `PropKey::Team`; `dc.bomb_site` may carry an optional `defuse_team` (else any team);
`dc.sky_model.sky_inner` is one vec3 that the alias layer serves as the three inner-percent reals;
`dc.kill_volume.damage` / `damage_kind` are reserved (the server kills outright); brush classes ignore `rot` (the
create carries the model origin and no rotation; the tool writes 0); world-settings rows matched through an alias
send no file name as evar name (`evars.rs name: None`) and `runner.rs load_world` resolves evar / client-var names
for `-worldfile` too when a ROM is at hand. Brush classes link to the BSP model of the same name; the model's `origin`
equals the object's `pos`. Level geometry is the one model no object names (bsp flag bit 1 clear); every brush model
carries bit 1 and, when visible, a same-named nested render model. Surface flags: solid 0x1, sky 0x10 (sky polygons
are solid and live in the level model), no-step 0x400000.

## 5. dc-mapkit (new crate `crates/dc-mapkit`, lib + bin)

### 5.1 Project folder (`<name>/`)

```
map.json            the project: metadata, terrain settings, solids, prefab instances, imports, objects, materials,
                    mod metadata (author, description, modes, goals), build settings
terrain.r16         little-endian u16 heights, row-major, size from map.json (absent = flat)
assets/             own textures (PNG -> DTX at build), own sounds, screenshots, imported OBJ files
AGENTS.md           the agent guide for this project (scaffolded, section 7)
build/              outputs: <key>.dat, the mod folder, the zip, renders (gitignored by the scaffolded .gitignore)
```

`map.json` is serde-typed (`Project`), versioned (`"format": 1`), with stable ids for every placed thing, and a
documented field list (`dc-mapkit schema`, kept in step with the types by a test). All units are world units
(a standing player is 100 units tall; the template is 4096 x 4096).

Template contract (every template satisfies it; a test pins it): level model = terrain + four boundary walls
higher than a jump + sky faces; one `dc.sky_model` + sky table; one `dc.world_settings` (fog on, `far_z` >= the
map diagonal); one `dc.global_light`; a `dc.kill_volume` slab under the lowest terrain point; 8 `dc.player_spawn`
per team (`modes` empty) facing the centre and standing on the floor (the server drops a spawn to the floor,
DSV-48; the validator's `spawn.floor` rule still checks that a ray from the point down hits a solid within 8 units
and `spawn.clear` that an 80 x 32 box is free) plus 16 team-less spawns for FFA; 2 `dc.bomb_site` (`number` 0 and
1, half extents 128 x 48 x 128); 2 `dc.flag_zone` (team 1/2) + 2 `dc.capture_flag` (team 1/2 inside them); 4
`dc.item_spawn`; mod metadata with every mode and a generated screenshot (a `shared` mod needs one). Room size is
capped at 16 (`max_players_cap`), hence 8 per team. `flat` = the contract on a flat plane (the blank canvas);
`arena` = `flat` + mirrored cover solids and two raised bases (the one-click playable map); `starter` = `arena` on
seeded noise terrain with a central valley (the showcase). pos48 precision is span / 2^19 on x and z and
span / 2^18 on y over the world extents (0.125 / 0.25 units at a 65,536 span): the validator warns above 32,768
and when the extents do not enclose every model box (positions outside are clamped).

### 5.2 CLI (every verb has `--json`; exit codes 0 ok, 1 diagnostics, 2 usage)

```
dc-mapkit new <dir> [--template starter|flat|arena] [--name N] [--author A]
dc-mapkit info <dir>                 counts, modes, spawn balance, size estimate, warnings
dc-mapkit validate <dir>             structured diagnostics (code, severity, path, message, fix hint)
dc-mapkit build <dir>                writes build/<key>.dat; parses it back; runs the sweep checks
dc-mapkit export <dir> [--zip]       writes build/mod/<id>/ (manifest.json, main.lua, assets/, screenshots/) and
                                     packs it with dc-modkit (same bytes = same sha256 as the lobby)
dc-mapkit render <dir> --view top|iso|spawn:<n>|free:<x,y,z,yaw,pitch> [--size WxH] -o out.png
                                     top/iso: software rasteriser, no GPU; spawn/free: dc-render offscreen
dc-mapkit apply <dir> <ops.json|->   JSON operations: a serde enum `Op` in the lib (`add{class,pos,rot?,props,ground?}`,
                                     `remove{id}`, `move{id,pos|delta,ground?}`, `set{id,prop,value}`, `rename{id,name}`,
                                     `solid.add{...}`, `terrain.*{...}`, `material.set{...}`; `schema --ops` prints it, a
                                     test round-trips every variant); returns `{applied:[{op, ids}], diagnostics, changed}`,
                                     assigns ids (`<class>_<n>` from map.json `next_id`), writes map.json + terrain.r16 by
                                     temp file + rename, `--dry-run` validates and writes nothing, the whole batch is
                                     refused on the first invalid op; `ground: true` snaps y to the terrain/solid top plus
                                     the standing offset (the server's own floor drop, DSV-48, is a fallback, not a substitute)
dc-mapkit terrain <dir> gen|raise|lower|smooth|flatten|noise ... (parameters as flags)
dc-mapkit rom textures|sounds|sprites|surfaces|worlds|modes [--filter F] [--thumbs DIR]   ROM asset listings (run time only)
dc-mapkit run <dir> <script.lua>     runs a generator script in the dc-mod-lua sandbox with the `map` API
dc-mapkit play <dir>                 vanilla++ on this machine: lobby + game server + client on the map
dc-mapkit mcp                        MCP server over stdio exposing the verbs above as tools
dc-mapkit schema                     the project format reference (JSON)
dc-mapkit get <dir> [--class C] [--id ID] [--near x,z,r]   query objects (the CLI twin of the Lua map.query)
dc-mapkit snapshot <dir> [--label L] | history <dir> | revert <dir> --to <n>
                                     shared undo: .mapkit/history/<n>/{map.json,terrain.r16} (keep 50; apply and
                                     the GUI take one before every change, so GUI, CLI and MCP share one history)
dc-mapkit gui [<dir>]                the editor (default when started without arguments / double-clicked)
```

`validate` checks the project (no `.dat` written, fast, offline); `build` re-runs the same `Diagnostic` type plus
the world-level sweeps; the diagnostic codes are one list printed by `schema`. `validate --fix` applies the
mechanical fixes (renumber bomb sites, ground spawns).

ROM discovery (MAP rule): `--rom` > `CA_ROM` > `rom` in `<profile>/mapkit.toml` (the client's `own_profile_dir`
rule, `%APPDATA%\DeCombat` on Windows) > a sibling-folder scan next to the exe for `Game\` + `Engine.exe` (the
friend package's rule) > none. `dc_rom::resolve_rom` reads `CA_ROM` only, which a double-clicking Windows user
never has. `info --json` reports `rom: {path, source}` or `null`; the GUI shows a banner with 'Choose ROM folder'
and remembers it; `validate` and `export` succeed without a ROM (WARN `rom.unverified` per ROM texture path);
only `rom *`, `render --view spawn|free` and `play` hard-require it (exit 1 naming the four attempts).

`play` is the cross-platform path (there is no Mac-only script in the user-facing plan): it locates
`dc-lobby-server`, `dc-dserver` and `dc-client` next to its own exe, then on `PATH`, then in
`target/{debug,release}`, names the missing one, writes `<profile>/mapkit/play/server.toml` (`policy = "modded"`,
the ROM, ephemeral ports, a data dir under the profile, the freshly exported zip as the set), starts the lobby
then the client (`--menu lobby --lobby 127.0.0.1:<port> --rom <rom>`), streams the three logs to `build/play/`,
and stops everything on close; the lobby passes `-worldobjects world` so brushes are created on the client.

On Windows `gui` is a second thin binary over the same lib, `dc-mapkit-gui` (`windows_subsystem = "windows"`,
the one the .app and the zip put in front), so the console `dc-mapkit` keeps stdout for `--json` and `mcp`; the
CLI's `gui` verb spawns it.

Paths: every path in `map.json` and in ops is forward-slash and relative to the project (the validator rejects
`\`, drive letters and `..`, and warns on two `assets/` entries differing only in case); the scaffolded
`.gitignore` covers `build/` and `.mapkit/`. Own textures count `w*h*4` bytes in the size estimate (the DTX writer
is uncompressed 32-bit): the GUI import defaults to <= 512 x 512 with a warning above; a DXT1 writer in dc-tex is a
P1 stretch goal. ROM texture thumbnails are decoded lazily on a worker thread and cached as PNG under
`<profile>/mapkit/thumbs/<rom digest>/`, never in the project.

### 5.3 Build pipeline (library, headless, tested in CI without ROM or GPU)

1. **Geometry**: terrain grid -> triangles (two per cell, normals up, texture per cell by paint layer, uv by world
   position / tile size); solids -> convex polygons with per-face texture, uv projection by face axis; OBJ -> triangles
   (optional "collision" flag per import); prefabs expand to solids. Each polygon gets surface flags and a surface id.
2. **Vertex colours**: ambient + sun (`dc.global_light` direction) by normal, optional hemisphere term and a cheap
   terrain AO from height differences; written as ARGB with 0xFF alpha, scaled so 0x80 = neutral.
3. **BSP** (D5): helper splitters on grid lines, cells of <= 16 polygons, naive BSP inside; nodes ~2x polygons;
   `-2` / `-1` sentinels (a DeCombat convention: dc-sim maps both to "no child"); one level model. Helper polygons
   live in the LEVEL model only: the walk uses a node polygon for its plane and sweeps `SURF_SOLID` faces only
   (`collide.rs`), but ladders take every polygon plane of their brush (`from_world.rs brush_volume`) and kill
   volumes every polygon with >= 3 corners (`killvol.rs`), so a helper inside a brush breaks them. Helpers are
   written with 0 corners, flags 0, texflags 0 and an exact unit axis plane (the parser accepts corner count 0;
   `PolySet::push` zeroes the flags of a < 3-corner face; a huge quad would only inflate the model bounds). Brush
   models (kill volumes, ladders, world models) get the naive BSP (<= 16 polygons each) as their own models with
   `origin` = object `pos`, box from corners, flag bit 1, rot 0 (the create carries the model origin and no
   rotation). The writer uses the walk's node convention (coplanar polygons on the positive side, the file's first
   child negative, `FILE_FIRST_CHILD_IS_NEGATIVE`) and the sweep-equality suite includes a one-sided slope.
   Validator: no polygon with flags 0 in a model an object names.
4. **Render data**: one top-level block for the level (sections grouped by texture, kind 1) covering every level
   polygon EXCEPT the 0x10 sky faces (the renderer batches every section of a block and render data carries no
   surface flags, so a sky face in the block is drawn as a textured wall over the sky); the level block's
   `sky_portals` = the vertex loops of those 0x10 faces (world space, plane-polys of FMT_DAT85 §10.4): dc-render
   collects portals from the level's top-level blocks only and projects them with the main camera
   (`scene.rs`, `gpu.rs portal_screen_rect`), and without one the sky pass is skipped; one nested render model per
   visible brush; the `dc.sky_model`'s nested render world carries the sky box's inside faces (kind 1) and no
   portals (it is drawn through the sky camera at the sky eye); a 1x1x1 light grid of the ambient colour (character
   lighting only: the world's light is the baked vertex colour).
5. **Objects** in the vocabulary of section 4; one `dc.world_settings`; one `dc.global_light`.
6. **Validation**: `World::parse` + `check_layout`; every solid polygon referenced by a node; ladders convex; kill
   volumes closed; spawn counts per team and mode; bomb sites numbered 0/1; extents vs pos48 precision (span
   <= 65,536 units per axis, warn above 32,768); package size estimate vs the 16 / 32 MiB caps; texture paths exist
   in the ROM (when `CA_ROM` is set) or under `assets/`.
7. **Mod export**: manifest (kind `shared`, capabilities `content.maps` + `assets`, metadata from the project),
   `main.lua` registering the map record, own PNGs converted to DTX (`dc_tex::write_bgra8`), the screenshot from
   `render --view iso`, radar picture from `render --view top` plus the mapping numbers, then `dc_modkit::pack`.

### 5.4 Map record (`mod.content.maps:register`)

```lua
mod.content.maps:register({
  id = "crossfire_valley",            -- the map key: a-z 0-9 _ (1..32)
  name = "Crossfire Valley",          -- label shown in the room list
  description = "...",                -- room settings panel
  world = mod.assets:path("assets/maps/crossfire_valley.dat"),
  modes = { "TDM", "FFA", "ELIM", "BOMB", "FLAG" },
  goals = { TDM = 100, FFA = 50 },    -- optional default goals per mode
  round_seconds = 600,                -- optional, else the lobby's round clock policy
  picture = mod.assets:path("assets/pictures/crossfire_valley.dtx"),       -- optional
  loading = mod.assets:path("assets/pictures/crossfire_valley_load.dtx"),  -- optional (one picture, tiled by the client)
  radar = { picture = mod.assets:path("assets/pictures/crossfire_valley_radar.dtx"),
            min = { -2048, -2048 }, max = { 2048, 2048 } },               -- optional
  players = { min = 2, max = 16 },    -- optional
})
```

`dc-mod-lua` checks the record at `:register` (required keys, types, key spelling, `@mod:` world path) and exposes
`ModHost::map_records() -> Vec<MapRecord>` as plain data.

## 6. Changes in existing crates

### 6.1 dc-world (vocabulary)
- `names.rs`: `Key { hash: [u8; 8], len: usize, ours: &'static str }` with `matches(&[u8]) -> bool`; `ROLE_TABLE`
  rows gain the `dc.*` class name; `PropKey` gains the short names; `ObjectRole::of_class` and `prop_by_key` accept
  both. Synthetic tests use the `dc.*` names (no ROM needed).

### 6.2 dc-dserver, dc-sim, dc-render, dc-client (alias adoption)
- Every `PropHash`/`NameKey` constant of a property or class in section 4 becomes a `dc_world::Key` with its `ours`
  name; the six local `prop()` helpers and the `key_of(class)` comparison sites use `Key::matches`. Team text
  `"1"`/`"2"`; `modes` list vs the room line's `dcmode=`; `dc.capture_flag` without a model uses a default look.
  Synthetic tests per feature (spawns, bomb sites, flag zones, kill volume, ladder, fog, sun, sky) with `dc.*` worlds.

### 6.3 dc-mod-lua, dc-modkit, dc-client (mods)
- `CONTENT_DOMAINS += "maps"`, `mapkeys.rs` record check, `ModHost::map_records()`, `capability_text` line
  "adds new maps", scaffold profile `map`, `dc-modkit validate` checks the world path exists and parses (new
  dependency on dc-world), guide and DC_DEFAULTS rows, `examples/mods/starter_map` built by dc-mapkit from the
  starter template. CLEANROOM §4 forbids ROM texture names in the tree and no automatic check would catch them (its
  ROM haystack is the exe/dll/fxd/lto only): the shipped templates (starter/flat/arena) and every committed example
  use own textures only (PNG under `assets/`, DTX at build, `@mod:<id>/assets/...` names in the `.dat` texture
  table); `dc-mapkit export --no-rom-paths` refuses a project with ROM texture paths and the example's test turns
  it on; ROM textures stay a run-time choice of the user's own project (`dc-mapkit rom textures`). Before the
  first commit `.gitattributes` gains `examples/mods/**/*.dat binary`, `examples/mods/**/*.dtx binary` and
  `examples/mods/**/*.r16 binary`: today `examples/mods/** text eol=lf` rewrites CRLF inside any binary (an
  explicit `text` attribute converts regardless of content; a 14-byte test blob lost two bytes). The pinning test
  is shaped like xray_walls (validate + lint + pack + `dc_world::World::parse` and `check_layout` of the committed
  `.dat`; `validate_mod` runs without a ROM in CI, so the world check is parse-only); determinism is 'build twice
  in one process, compare bytes', never 'compare to a committed sha256' (cross-platform byte identity needs no
  libm transcendentals and no HashMap iteration in the build path).

### 6.4 dc-lobby-server, dc-config
- `RomData` is frozen in an `Arc` and handed to `Core::new` (main.rs:205-212) before the mod set is built and
  loaded (main.rs:289, 332, 361), so the community table is NOT appended to `RomData`. It lives on `Core`:
  `st.maps: MapTable { rom: Arc<RomData>, community: Vec<MapRecord> }` with the seven wrappers (`map_count`,
  `map_mode_masks`, `map_plays_mode`, `mode_defaults`, `world_for`, `room_line`, `map_factors`) that branch on
  `index >= rom.map_count()`, filled right after `st.mods = Some(m)` from the records the mods thread sends over
  its ready channel (a fourth plain-data element next to the weapon content, computed before `ready_tx.send`; a
  malformed record is an `Err` that stops the lobby, as for the declared set); every `st.rom.<query>` call site
  (game.rs, room.rs, lobby.rs, clan.rs, main.rs start-up line) is retargeted. Clan rooms (`clan.rs` reads
  `st.rom.game_room` directly) do not offer community maps (D12). `world_for` returns the ABSOLUTE mod-cache path
  for a community index (`cfg_dir` is relative to `--db`; canonicalise it once; the game server runs in its own
  workdir) and the launch adds `-worldfile <abs path> -announce @mod:... -worldobjects world` only then, passing the
  record's `world` string verbatim (forward slashes, case kept: `ModAssets::resolve` is case-sensitive; nothing
  upper-cases or backslashes it, MAP rule); the community branch of `world_for` and the launch change land in the
  same commit (`dc_bute::world_for_room` silently falls back to a ROM key for an out-of-range index); 0x311 version
  derived from the set hash; no `dcmode=` key (the server derives the DeCombat mode name from the room line's
  `type=`, section 4); the room line carries the record's `round_seconds` and goal when set; `/status` lists community maps; a new `[mods] maps_enabled` key is not
  needed (presence in the set is the switch). Remote hosts: refuse to start a community map room with a clear log
  line (D12).

### 6.5 dc-client
- `loader_for`: `@mod:` announce -> `ModWorld` source (bytes from `ModAssets::resolve`, ROM textures through
  `TextureIndex`, mod textures through `ModAssets::read`); the windowed match installs the mod layer before the world loads already (`window.rs start`:
  `MatchMods::open` precedes `source.load`), but the headless launch does not (`main.rs run_headless` and
  `headless.rs` never open `MatchMods`, `NetOptions` has no mods field, so `modassets::active()` is `None` there
  and a `@mod:` read fails with 'no mod set is loaded'): the headless launch opens `MatchMods::open(recipe)` before
  `run_headless` and keeps it alive across the run; the P2 e2e test runs that path. `MatchMods::drop` installs
  `None`, so the menu re-installs its read-only view when it takes the window back, else community pictures
  vanish after the first match. Menu: map label / description / picture / loading picture for indices past the ROM rows from the join
  report's map records (`@mod:` pictures through the existing `RomTextureSource`); the menu installs a read-only
  `ModAssets` view for those pictures. Standalone preview: `dc-client --world-file <dat> [--rom DIR]` in map mode
  (ROM textures + mod textures from `--mod-dir`).

### 6.6 Release
- ci.yml fmt/clippy lists; release.yml jobs `mac-mapkit` and `windows-mapkit` build and bundle all five binaries (dc-mapkit, dc-mapkit-gui, dc-lobby-server, dc-dserver, dc-client: today the servers are built on Linux only, release.yml `linux-servers`, and `play` needs them next to the exe; both servers compile on Windows) (+ packers `tools/mapkit_pack_mac.sh`,
  `tools/mapkit_pack_win.py`), a tag-triggered `gh release upload` step with `permissions: contents: write`;
  `assets/icon/mapkit_logo.png`; docs/RELEASES.md rows.

## 7. AI-native surface (D8)

- `docs/MAPKIT_AGENTS.md`: the agent playbook (loop: `info` -> `apply`/`run` -> `build` -> `validate` -> `render`
  -> look -> repeat; how to read diagnostics; the vocabulary; the ROM listing verbs; the limits).
- Scaffolded `AGENTS.md` in every project (short, points at the playbook, lists the project's own facts).
- `.claude/skills/mapkit/SKILL.md` in the repo (invocable `/mapkit`): drives the CLI for a user's project.
- `dc-mapkit mcp`: JSON-RPC 2.0 over stdio, implemented with serde_json only, with this minimum (a scripted-client
  conformance test pins it): `initialize` -> `{protocolVersion: <the client's if supported, else "2025-06-18">,
  capabilities: {tools: {listChanged: false}, resources: {subscribe: false, listChanged: false}}, serverInfo}`;
  `notifications/initialized` (no id) accepted silently, never answered; `ping` -> `{}`; `tools/list` -> tools with
  `name`, `description`, `inputSchema` (JSON Schema, `type: "object"`, generated from the `Op` and verb types);
  `tools/call` -> `{content: [{type: "text", text}], isError}` with verb failures inside `isError: true`, never a
  JSON-RPC error; `resources/list` and `resources/read` -> `{contents: [{uri, mimeType, text}]}` (the playbook and
  the schema); `notifications/cancelled` ignored; unknown method `-32601`, malformed `-32700` / `-32600`; no
  `prompts` capability. Transport: one JSON object per line, UTF-8, no embedded newlines, ALL logging to stderr
  (the verbs' libraries print to stdout today, so `mcp` routes them), and `mcp` runs with the ROM and the GPU
  absent. `render` returns `{type: "image", data: <base64 PNG>, mimeType: "image/png"}` content. Tools = the CLI
  verbs with JSON arguments, plus flat typed twins of the common ops (`add_object`, `move_object`,
  `set_property`, `terrain_noise`) generated from the same `Op` enum; the `run` tool accepts inline script text.
- `render` is camera-generic on the software path (`top | iso | spawn:<n> | free:{..}`, z-buffered, flat per-vertex
  colour, `--labels`, `--grid`), so an agent without a GPU or ROM can judge sight lines; `--gpu` takes the dc-render
  path with a software fallback noted in `--json`; thumbnails, the screenshot and the radar stay on the software
  path so `export` never needs a GPU.
- `run` scripts get a `--budget` instruction limit (default 1e9 through `SandboxLimits::with_instruction_limit`;
  the mod default of 10 M would stop a per-cell loop over a 512 x 512 grid) and host-side bulk ops
  (`map.terrain.noise{}`) so scripts rarely loop per cell.
- `dc-mapkit run`: Lua generators (`map.terrain.noise{...}`, `map.add{class=..., pos=...}`, `map.solid.box{...}`,
  `map.prefab("crate", {...})`, `map.query{class=...}`) in the dc-mod-lua sandbox (same limits), so an agent can
  write a whole map procedurally in one script; `examples/mapkit/*.lua` shows three.
- Deterministic everything: same project + same tool version = same `.dat` bytes and same zip hash (seeded PRNG,
  sorted iteration), so an agent's diff is meaningful.

## 8. Phases (each is one workflow of worktree-isolated implementers plus an adversarial review)

| phase | deliverable | depends on |
|---|---|---|
| P0 vocabulary + mods | 6.1, 6.2, 6.3; MAP-1..MAP-6 rules; synthetic tests | - |
| P1 mapkit core | crate with 5.1, 5.3, CLI verbs new/info/validate/build/export/render(top,iso)/apply/terrain/rom/schema; templates starter/flat/arena; tests (BSP equality, round trips, determinism) | section 4 names (fixed here) |
| P2 lobby + client | 6.4, 6.5; e2e test: lobby + game server + headless client on the starter map; `dc-mapkit play` | P0, P1 build output |
| P3 GUI | eframe editor: viewport (fly camera, picking, gizmos), terrain brushes, solid tools, prefab/asset browser with ROM thumbnails, object inspector, validate/build/export/play buttons, undo | P1 |
| P4 AI-native | `run` (Lua API), `mcp`, playbook, skill, scaffolded AGENTS.md, examples | P1 |
| P5 release + vanilla++ | 6.6, packers, `examples/server-vanilla-plus.toml`, `tools/vanilla_plus_mac.sh`, VPS instance notes | P1-P4 |

P0 and P1 run in parallel; P2 follows P0; P3 and P4 follow P1; P5 last.

## 9. Scope cuts and their extension points (D12)

| cut | extension point |
|---|---|
| doors, switches, rotating models | `dc.door` class alias in `dc-sim` MOVER_CLASSES + `doors.rs`; the project already keeps solids as named brushes |
| water volumes | `CLASS_WATER` alias + client container handling |
| LTB prop placement | a `dc.prop` object -> dserver create record with model + skins -> client `ModelLook` draw; the ROM worlds have none, so the client path is new |
| lightmaps | kind-2/4 sections through `pack_lightmap`; the baker already produces per-vertex light |
| bots on community maps | `dc-botclient --mods-dir` resolving `@mod:` announces |
| more than 11 community maps per lobby | a DeCombat-only map-list push next to 0x311 |

## 10. Open points the first end-to-end run settles

1. Half vs full box extents on the real client (server treats half; the tool writes half).
2. Sky: closed from code, all three pieces are needed on DeCombat: the solid 0x10 face in the level (collision)
   model for the client's sun test (`dc-client/src/world.rs sun_visibility`: the ray must HIT a polygon whose flags
   carry 0x10, and the sweep tests solid polygons only), the portals in the level's top-level render block for the
   sky scissor rectangle, and the sky brush + table for the sky geometry. The P2 e2e run asserts
   `SceneStats.sky_portals > 0` and a sky rectangle from the first spawn. Shots stop at the sky face (`trace_world`
   is the level model) and the impact surface is the face's `texflags`, so P2 also decides the sky face's `texflags`
   (or answers a 0x10 hit with the no-hit record) before the first playtest, else bullet decals land on the sky.
3. pos48 budget: the template is 4096 units; the validator warns above 32,768.
4. Room-line fields a community map must supply per mode (time, goal, bomb timer) beyond `round_seconds` and
   `goals`; the P2 implementer enumerates `pipe.rs` consumers and writes the rule.
5. Whether the lobby reads the map records through Lua on the mods thread (30 s budget) or a cached JSON next to the
   zip; P2 starts with Lua (one source of truth).
