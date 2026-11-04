# Freight Hall (dc-mapkit project)

This folder is a DeCombat community map: `map.json` (the project, format 1), `terrain.r16` (heights), `assets/` (own textures), `build/` (outputs, ignored by git). Made from the `flat` template; key `freight_hall`; 64 x 64 cells of 64 units.

## The loop

1. `dc-mapkit info . --json` to see what is here (counts, spawns per team, modes, size).
2. Change it: `dc-mapkit apply . ops.json` (or `-` for stdin) with a JSON list of ops, or `dc-mapkit terrain . <op>`; `dc-mapkit schema --ops` lists every op and field. `dc-mapkit get . --class dc.player_spawn --json` queries objects. Or generate a whole layout with a Lua script against the `map` API: `dc-mapkit run . gen.lua` (playbook section 6; `examples/mapkit/` in the repository; a script that may run twice starts with `map.clear()`). An MCP client gets the same verbs from `dc-mapkit mcp`.
3. `dc-mapkit validate . --json` (project checks; exit 1 on errors, each with a code and a fix hint).
4. `dc-mapkit build .` then `dc-mapkit render . --view top -o build/top.png` and look.
5. Undo: `dc-mapkit history .` / `dc-mapkit revert . --to <n>` (apply snapshots before every change).

Facts: units are world units (a player stands 100 tall); solids are centre + full size; `ground: true` snaps to the floor; ids are `<prefix>_<n>` and only start over after a `clear` empties the project; paths are forward-slash relative to this folder; only own textures under `assets/` may be committed.

The playbook is `docs/MAPKIT_AGENTS.md` in the DeCombat repository.
