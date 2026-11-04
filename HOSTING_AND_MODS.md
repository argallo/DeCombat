# HOSTING_AND_MODS.md: lightweight servers, community mods, and how players find them

Status: requirements from the user (2026-10-24) plus the coordinator's proposed design. Open questions at the end
are for the user; everything else is binding for the server, client and modkit crates.

## 1. User requirements (verbatim intent)

1. The Rust server must be **lightweight and efficient to run**, and **hostable** by anyone.
2. **Community mods** must be able to run their own modded servers; the mod system must make that possible.
3. Players need a way to **choose vanilla servers vs modded servers** (a landing page / browser).
4. The Rust **client supports Windows and macOS** first, other platforms later.

## 2. Server: lightweight and hostable

Targets (measured in CI once the crates exist; numbers are goals, not claims):

| item | target |
|---|---|
| deliverables | two static binaries, `dc-lobby-server` and `dc-dserver`, or one binary with both roles (`dc-server --all-in-one`) for small hosts |
| platforms | Windows x64, Linux x64/arm64 (cheap VPS), macOS. Pure Rust + bundled SQLite; no external services, no runtime deps |
| configuration | one `server.toml` (ports, ROM path, mods dir, DeCombat default overrides, directory registration) plus env overrides; sane defaults so `dc-server --rom <folder>` just works |
| storage | SQLite file per server (WAL mode), periodic online backup command; no DB server |
| game rooms | **many rooms per process** in `dc-dserver`: one fixed-tick simulation loop per room on a small thread pool, world data shared read-only (`Arc<World>`), so one host runs dozens of rooms without spawning a process per room (the 2009 design spawned one DServer per room) |
| memory | lobby < 100 MB at 1,000 sessions; dserver < 20 MB + ~10 MB per loaded world (worlds are mmap'd or shared) |
| CPU | dserver tick 30 Hz by default (configurable), idle rooms cost ~0; lobby is event-driven (tokio) once the core is stable, thread-per-connection is acceptable for the first cut |
| network | loopback-safe defaults (bind 127.0.0.1); explicit `--public` to bind 0.0.0.0; UDP port range per room (CD-10); IPv4 first, IPv6 later |
| operations | structured logs (JSON optional), `--metrics` Prometheus text endpoint, graceful shutdown that ends matches cleanly, `dc-server doctor` that checks ROM hashes, ports, DB, mods |
| ROM | the host needs a 2009 client folder too (the server reads GAMEROOM/ITEMS/WEAPONS/.gms and world .dat files from it); hashes checked by `identify_rom` |

A modded server is its own lobby instance (one mod set and one policy per lobby, its own ports and DB); one host
runs several side by side, for example the vanilla reference server and a modded one (project decision,
docs/VPS.md "Several instances on one box").

## 3. Mods: what a "modded server" means

Three kinds of mod, all declared in `manifest.json` (docs/MODDING_GUIDE.md section 1):

| kind | runs on | examples | distribution |
|---|---|---|---|
| **server rules** | server only | reward formulas (CD-03/04 hooks), weapon stat patches, mode rules, custom rooms, GM tools | lives on the server; clients need nothing |
| **shared content** | server + client | weapon stat patches that affect prediction, new room presets, text overrides, custom HUD layouts | the client must have the same mod set as the server; delivered on join (see §4) |
| **client visual** | client only | HUD skins, crosshairs, sound packs, procedural recolours of ROM textures | installed locally by the player, but loaded only on a modded server whose policy sets `client_visual_allowed`; never on a vanilla server (§8.1); a locally installed mod may not use render/HUD hooks, only server-declared mods may (§3.1) |

### 3.1 What mods can do (capability matrix)

| wish | original 2009 client | DeCombat Rust client | how |
|---|---|---|---|
| tweak weapon stats, prices, rewards, rank curve | yes (server rules; client sees results) | yes | `mod.content.*:patch`, `lobby.*` / `combat.*` hooks |
| variants of existing modes (time, goals, spawns, friendly fire, bots) | yes | yes | server-rule Lua on `dc-server-logic` |
| **entirely new game modes** with new HUD elements / texts | no (client knows only the ROM mode table) | yes | server rules + a shared mod carrying HUD/text additions |
| **new maps** | no (the client only reads its ROM archives) | yes | shared mod ships an ORIGINAL `.dat` world + textures written with our DAT85/DTX writers under `assets/`, in DeCombat's own object vocabulary (`dc.*` classes, docs/MAPKIT_VOCAB.md, MAP-1..3; the client renderer reads it, MAP-26), and registers it with `mod.content.maps:register` (capability `content.maps`, scaffold profile `map`; MAP-4 / MAP-5); delivered by the server, size-capped; the authoring tool is dc-mapkit (docs/MAPKIT_PLAN.md; no SDK editor); game-server rules MAP-20..25 |
| cosmetic client changes (HUD skins, crosshairs, sounds) | no | yes, only on modded servers (vanilla = no mods) | client-visual mod declared by the server |
| **"wallhack" / x-ray servers**, outlines through walls, radar | no | yes, as a declared modded server | shared mod using the render/HUD hooks (`render.entity_visibility`, `hud.draw`); everyone on that server has the same view |
| anti-cheat posture on vanilla servers | n/a | server-side visibility culling: positions of enemies a player could not plausibly see are not sent (PVS from the world BSP + recent-exposure window), so a modified client learns little; a wallhack server simply disables the culling | `dc-server-logic` default, DeCombat row to be added |

Render and HUD hooks are available only to mods a server declares in its mod set; a local mod can never load on a
vanilla server (policy vanilla = empty set, enforced by the client and checked by the server via the mod-set hash).

Rules that keep this legal and fair:

- Mods never contain Nexon assets. They may reference ROM paths and apply **procedural transforms** at run time
  (recolour, composite, re-encode) and may ship their own original assets.
- A server publishes its **mod set**: list of `(mod_id, version, sha256 of the packed zip)` plus a `policy`
  (`vanilla`, `modded`, `client_visual_allowed: bool`). "Vanilla" means an empty shared/server-rule mod set.
- Every hook a mod can use is a DeCombat default (DC_DEFAULTS.md) so vanilla behaviour is always the documented default
  and mods are diffs against it.
- Server-rule mods are authoritative; the client never trusts its own copy for anything the server decides.

## 4. Joining a modded server (proposed flow)

1. Server list entry carries the server's mod set and policy (new field in our server-list JSON, see §5; the 2009
   wire entry stays unchanged for the original client, which can only join vanilla servers).
2. The Rust client compares the mod set with its per-server cache (`<data>/mods/by-hash/<sha256>.zip`).
3. Missing mods are fetched from the **server's own HTTP endpoint** (`/mods/<sha256>.zip`, the lobby already runs a
   small HTTP server for the clan web pages) with a size cap and a progress UI; optionally from a registry URL the
   manifest names (GitHub release asset). Hash must match or the join is refused.
4. Mods are loaded into the sandbox (`dc-mod-lua`) in the deterministic order the server declares; the client sends
   the resulting mod-set hash in its login so the server can refuse mismatches.
5. Leaving the server unloads the mods; the cache is kept (LRU, size-capped).
6. **Mod-set fragment and hash** (decided 2026-10-24). The server-list entry and `dc-modkit modset` carry
   `{"policy":"vanilla|modded","client_visual_allowed":bool,"mods":[{"id","version","sha256","kind",
   "requires_client","capabilities":[...],"source_repo"?}]}` in load order. The **mod-set hash** sent at login is
   SHA-256 over the UTF-8 text made of one line `id@version:sha256` per *server-declared* mod (kinds server_rules and
   shared, in load order, `
` after each line). The **empty-set hash** is SHA-256 of the empty string
   (e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855); a vanilla server accepts only that value.
   Client-visual mods are not part of the hash.

## 5. Finding servers: landing page and in-client browser

- **Server directory**: a tiny JSON document per directory source. The client ships with two default sources:
  the project's community list (a static JSON file on GitHub Pages, maintained by pull request) and a LAN/loopback
  scan. Servers can also self-register to any directory that runs `dc-directory` (a small optional binary with
  heartbeat and expiry) if a community member wants a live list. No central service is required to play.
- **Landing page** (`web/landing/`, static): explains the ROM model, links the client download, and renders the
  community list with filters **Vanilla / Modded**, region, players, mod names, and a "copy join address" button.
  It is the same JSON the client reads, so the two never disagree.
- **In-client browser**: the Rust client's server screen shows the same list with the same filters and a
  "vanilla only" toggle defaulting on for new players, plus a details pane listing the mods a modded server runs,
  with links to their manifests. One click installs the mod set (flow in §4) and joins.
- The original 2009 client keeps its fixed server-list packet (0x2000) and can only see vanilla servers.

## 6. Client platforms

- **Windows (x64) and macOS (Apple silicon + Intel) first**: `wgpu` (DX12 / Metal), `winit`, `cpal`; no Win32-only
  code on the play path; one codebase. Tested in CI on both.
- **Later**: Linux (Vulkan via wgpu is nearly free once the above works), Web (WebGPU/WASM is a later option, not a
  release gate), Steam Deck = Linux.
- Server crates stay platform-neutral from day one (Windows/Linux/macOS), because hosting is cheapest on Linux.

## 7. Consequences for the current crate plan

- `dc-lobby-server`: add `server.toml`, the `/mods/` HTTP endpoint, mod-set hash in login, directory heartbeat.
- `dc-dserver`: multi-room, fixed tick, shared world data; metrics; no per-room processes.
- `dc-mod-lua` / `dc-modkit`: manifest gains `kind` (server_rules / shared / client_visual), `requires_client`,
  `asset_policy`; `dc-modkit pack` produces the hashed zip the server serves; `dc-modkit publish` writes the
  directory entry fragment.
- `dc-directory` (optional, small) and `web/landing/` (static) are new, low-priority deliverables.
- `dc-client`: server browser screen, mod cache, join flow.
- `dc-mapkit` (new, docs/MAPKIT_PLAN.md): the community map builder. A map is a text project (`map.json` +
  `terrain.r16` + own assets) edited through JSON ops (`dc-mapkit apply`, every verb with `--json`), validated
  offline, built into a version-85 world (`dc-mapkit build`: BSP models, baked vertex colours, render sections,
  the `dc.*` objects; MAP-43 .. MAP-48) and exported as a `shared` mod with a `maps` content domain (stage 2).
  Stage 1 ships the project format, ops, diagnostics, templates (`new --template starter|flat|arena`, own
  textures only) and the data verbs; stage 2 adds `export` (the mod folder packed and
  hashed by dc-modkit, so the zip is what a server's mod set lists; a world whose checks found errors is not
  packed without `--force`), `render` (PNGs without a GPU) and `rom`
  (listings of the user's own ROM at run time, found by `--rom`, `CA_ROM`, `<profile>/mapkit.toml` or a sibling
  `Game/` folder); the AI-native surface (`dc-mapkit run` Lua generators in the mod sandbox, `dc-mapkit mcp` serving the
  verbs as MCP tools, the `/mapkit` skill; MAP-56 .. MAP-60); the playbook for agents is docs/MAPKIT_AGENTS.md. Stage 3 adds
  the editor `dc-mapkit-gui` (eframe / egui over the same library and ops, a dc-render viewport of the real
  build, own and ROM texture thumbnails; MAP-61 .. MAP-66). Stage 2 (client side, MAP-30 .. MAP-34): the
  client loads a `@mod:` world announce from the server's mod set (textures layered over the ROM), the headless
  client installs the set like the window, the waiting room lists community maps after the ROM's with the record's
  texts and pictures, and `dc-client --world-file <dat> [--rom DIR] [--mod-dir DIR]` previews a world without a
  server. Stage 3 (`play`, MAP-67 .. MAP-69, MAP-89): `dc-mapkit play <dir> [--headless --seconds N] [--modded]` exports
  the project and runs a vanilla lobby + game server + client on 127.0.0.1 with the zip as the curated set of its
  featured entry "Vanilla++" (`--modded`: a modded lobby with the zip as its set; the instance under
  `<profile>/mapkit/play/`, logs under `build/play/`); `examples/mods/starter_map` is the exported starter template
  (MAP-70), a server offers maps as curated packs on its Vanilla++ entry, with no second instance (docs/VPS.md,
  `examples/server-vanilla-plus.toml`, MAP-71)
  and `crates/dc-client/tests/vanilla_plus.rs` plays the example on the real binaries (MAP-72).
- `dc-lobby-server` community maps (stage 2, MAP-35 .. MAP-39): a map mod in the declared set is enough (no new
  config key; the folder-discovery fallback without a `server.toml` logs discovered maps as ignored, since nothing
  is published for clients to fetch). The lobby appends the set's maps after the ROM's rows in 0x311 (30 pairs at
  most, warned about; a mode row with `EnablePlay=FALSE` gives no bit) and publishes the first community index as
  `community_map_base` in `server_list.json` (the client decodes the indices from it, not from its own ROM),
  starts their game servers with `-worldfile <mod cache path> -announce @mod:... -worldobjects world` after
  `--dserver-args`, carries the record's `round_seconds` / goal and its radar picture and rectangle on the room
  line, lists them in `GET /status` (`community_maps`), and refuses to start when a map mod is not `shared` or its
  world is missing. Clan rooms do not play community maps (plan D12): a clan room cannot pick one, and a start on one
  stays waiting with a log line. A remote game host (`[[lobby.servers]] host`, dc-host) plays a featured entry's maps
  only: the pack goes to the agent over its control stream (DC_DEFAULTS.md MAP-83, MAP-84; specs/HOST_AGENT.md section 3),
  and a lobby-wide set's map stays waiting there as before.

## 8. Project decisions

1. **Vanilla means no mods at all.** Joining a vanilla server unloads every client mod, cosmetic ones included.
   `policy = vanilla` is the default for a server with an empty mod set; the client enforces it locally and the
   server checks the mod-set hash sent at login (must equal the empty-set hash).
2. **Mod delivery: the game server serves its own mods, with trust anchored outside the server.**
   - A mod is Lua text + data files only; no native code, ever. Lua runs in the `dc-mod-lua` sandbox (no os/io/
     load/debug/package, bytecode rejected, instruction/memory/call-depth limits, scoped require, circuit breaker);
     data files go through the same fuzz-hardened parsers as the ROM.
   - Server-list entry declares `(mod_id, version, sha256)` per mod. The client downloads from the lobby's HTTP
     endpoint (`/mods/<sha256>.zip`, size caps), verifies the hash before unpacking, caches by hash, refuses the join on
     mismatch.
   - `dc-modkit pack` is deterministic, so a mod's GitHub release hash equals any server's copy. The community JSON
     (below) carries a **known-mods list** `(id, version, sha256, source_repo)`: hash present = *verified* badge;
     otherwise *unverified* = explicit per-mod opt-in on first join showing author, source URL and declared
     capabilities. Manifests declare capabilities (content domains, hooks, storage); the loader refuses a package that
     uses more than it declares.
   - Operators cannot swap bytes silently: the client pins to the hash in the server-list entry.
3. **Server directory at launch: a static community JSON on GitHub Pages**, maintained by pull request; the landing
   page and the in-client browser read the same file (servers + known mods). A live registry (`dc-directory`) may be
   added later; it is not a release gate.
