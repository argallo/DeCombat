# Lua map generators for dc-mapkit

Each script runs against a `flat` template project (`dc-mapkit new <dir> --template flat`) and adds a whole
layout in one batch:

- `arena_symmetric.lua`: two raised bases, a central plateau with four ramps, cover mirrored on x, side lanes.
- `canyon_river.lua`: hills, a winding river bed (flatten brushes along a sine), banks, bridges, stepping stones,
  the kill slab kept under the lowest point, every object re-grounded.
- `koth_tower.lua`: a stepped cylinder tower with a spiral of ramps, battlements, bomb site A and the item spawns
  moved to the top, sandbag prisms and point lights.
- `rom_atlas.lua`: every world of the user's ROM as ROM pieces (docs/MAPKIT_AGENTS.md section 19) packed on one level
  ground, each world closed (its own invisible walls and kill volumes), walled corridors between neighbours, every
  region sealed with invisible walls but where a corridor crosses, both faces drawn. `plan_links.py` plans its
  corridors from each world's playable floor (`walk --reach --nodes`); `check_round.py` walks every region at three
  grids and `rails_from.py` turns each round into rails (and cheeks beside corridor mouths) and left-out spawns,
  until a round finds nothing; the steps are in the script's header. It needs a ROM, names no world itself,
  and the project it makes names ROM paths, so that project is never committed (the test below leaves it out).

```
dc-mapkit new my_map --template flat
dc-mapkit run my_map examples/mapkit/canyon_river.lua --json
dc-mapkit validate my_map
```

The `map` API is documented in docs/MAPKIT_AGENTS.md section 6 (and `crates/dc-mapkit/src/script.rs`). The test
`script::tests::the_example_generators_run_against_the_flat_template_and_validate` runs the first three and keeps them
valid.
