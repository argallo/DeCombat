---
name: mapkit
description: Build or change a DeCombat community map with the dc-mapkit CLI (a text project folder of map.json + terrain.r16 + own assets). Use when the user wants a new map, wants objects / solids / terrain placed or changed, wants a map validated, built, rendered or exported as a mod, or wants a Lua generator written for one. Drives `dc-mapkit` verbs with --json; the rules are docs/MAPKIT_AGENTS.md.
---

# mapkit: drive dc-mapkit for the user's map project

Use this skill for anything about a DeCombat community map: "make me a map", "add cover between the bases",
"raise the terrain at the centre", "why does validate fail", "export it as a mod", "write a generator for a
canyon". Read `docs/MAPKIT_AGENTS.md` (the playbook) once per session; the vocabulary is `docs/MAPKIT_VOCAB.md`.

## The binary

`dc-mapkit` from the DeCombat checkout: `cargo run -q -p dc-mapkit -- <verb> ...` (or the installed `dc-mapkit`
on PATH). Every verb takes `--json` and prints one JSON object `{ok, verb, result, diagnostics}` on stdout; log
lines go to stderr; exit 0 ok, 1 error diagnostics, 2 usage. Always pass `--json` when you read the result.

## The loop

1. Find the project: the folder the user names, or one holding `map.json`. None yet: `dc-mapkit new <dir>
   --template arena --name "<name>" --author "<user>" --json` (`flat` when you will generate everything, `starter`
   for valley terrain). Never run `new` on a folder that already holds a `map.json`.
2. `dc-mapkit info <dir> --json`: counts, spawns per team, modes, extent, size estimate, error / warning counts.
   `dc-mapkit get <dir> --class dc.player_spawn --json` (or `--id`, `--near x,z,r`) for the objects.
3. Change it:
   - a few edits: write an ops file (JSON array of `{"op": ..., ...}`; `dc-mapkit schema --ops` lists every op and
     field) and `dc-mapkit apply <dir> ops.json --json` (`--dry-run` first when unsure). Ids come back in
     `result.applied[i].ids`; `ground: true` puts things on the floor.
   - a layout or terrain: write a Lua generator against the `map` API (playbook section 6; the examples in
     `examples/mapkit/`) and `dc-mapkit run <dir> gen.lua --json` (`--dry-run` to check). One script is one
     all-or-nothing batch; errors name the script line.
   - one terrain brush: `dc-mapkit terrain <dir> raise|lower|smooth|flatten|gen --x .. --z .. --radius .. --amount ..`.
4. `dc-mapkit validate <dir> --json`: fix every error (code, path, message, fix hint); warnings may stay.
5. `dc-mapkit build <dir> --json` (fix its world-level `diagnostics` too), then `dc-mapkit render <dir> --view top
   --labels --json` (and `--view iso`) and look at the PNG before you tell the user it is done: `result.image` is
   its path (`<dir>/build/render/<view>.png`; `-o` is resolved against your working directory, not `<dir>`, so
   write `-o <dir>/build/top.png` when you pass it), `result.markers` / `result.legend` tell you which disc is
   which object. `play --headless --seconds 10 --json` proves the map plays (playbook section 16).
6. Ship: `dc-mapkit pictures <dir> --preview-view free:... --loading-view free:... --json` (the waiting room preview and the loading screen; look at both), then `dc-mapkit export <dir> --zip --json` writes the mod folder and zip under `build/` (`--no-rom-paths` for
   anything that will be committed).
7. Undo: `dc-mapkit history <dir> --json`, `dc-mapkit revert <dir> --to <n> --json` (every apply / run / terrain
   took a snapshot first).

## Rules

- Keep the template contract when you build on one: 8 spawns per team + 16 team-less (`modes` empty), two bomb
  sites numbered 0 and 1, a flag zone and flag per team, walls, sky, a kill slab under the lowest point.
- After terrain changes or new floors, re-ground the objects (`{"op": "move", "id": ..., "ground": true}` or
  `map.move(id, {ground = true})` for each `map.query()` result).
- Own or credited textures only (`assets/*.png`, the templates' `ground`, `wall`, `block`, `sky` materials, models
  and textures brought in with `dc-mapkit asset`, playbook section 18); a ROM texture
  path is the user's run-time choice and never goes into a committed project.
- Stay within 32,768 units per axis and 32 MiB; a standing player is 100 tall; a solid's `pos` is its centre.
- Set a wedge's direction with `rises` (`north` +z, `east` +x, `south` -z, `west` -x); `size` stays `[width, height, run]`.
- A generator that may run twice starts with `map.clear()` or `map.clear{group = "name"}`; a `solid.duplicate` warning means it did not.
- A flat floor is `map.terrain.gen{preset = "flat", cells_x = .., cells_z = ..}`.
- `render` is flat colours and needs nothing; `render --textured --view spawn:0` shows the real textures through `dc-client`. Check both before you say a map is done.
- A texture that is a picture of one object or an atlas needs `fit = {u0, v0, u1, v1}` on a material per region, named in `faces`; tiling cuts it.
- Place a `dc.end_camera` above the cover for the round's end overview (`rot` x looks down); without one, `build` adds a default.
- Do not edit `map.json` by hand when an op does it; the ops keep ids and history consistent.
- When the user has an MCP client, `dc-mapkit mcp` serves the same verbs as tools (playbook section 7).

## Commands at a glance

```
dc-mapkit new <dir> [--template starter|flat|arena] [--name N] [--author A] [--seed S]
dc-mapkit info <dir> | validate <dir> | get <dir> [--class C] [--id ID] [--near x,z,r]
dc-mapkit apply <dir> <ops.json|-> [--dry-run]
dc-mapkit run <dir> <script.lua|-> [--budget N] [--dry-run]
dc-mapkit terrain <dir> gen|raise|lower|smooth|flatten [flags]
dc-mapkit build <dir> | render <dir> --view V [--textured] -o out.png | export <dir> [--zip] [--no-rom-paths]
dc-mapkit pictures <dir> [--preview-view V] [--loading-view V]
dc-mapkit snapshot <dir> [--label L] | history <dir> | revert <dir> --to <n>
dc-mapkit schema [--ops] | rom <kind> [--filter F] | play <dir> [--headless --seconds N] | mcp
dc-mapkit asset <dir> <model.glb> --name N [--max-tris T] [--tex S] [--orient x90] --license L --author A --source URL
```
