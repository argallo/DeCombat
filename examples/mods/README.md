# Example mods

- `hacking_mod`: the Hacking Mod, a server mod where every player sees every other player through walls (MOD-22), rooms may add no-recoil (MOD-23); see its README.
- `starter_map`: a shared map mod, the EXPORTED folder of dc-mapkit's starter template (`dc-mapkit new --template
  starter --name "Starter Map" --author "DeCombat"`, `build`, `export --no-rom-paths`; DC_DEFAULTS.md MAP-70), the
  shape every community map ships in (`mod.content.maps:register`, the world under `assets/maps/`, own textures as
  DTX, the radar picture, a screenshot). Regenerate it with `tools/mapkit_regen_example.sh`; play it with
  `dc-mapkit play` on its project or declare it on a Vanilla++ entry as a curated pack (the lines in
  `examples/server-vanilla-plus.toml`, docs/VPS.md; it is a test example and has no row on the review list). Build your own with dc-mapkit (docs/MAPKIT_AGENTS.md), or scaffold the bare shape with
  `dc-modkit scaffold <id> --profile map`.
- `drawn_crosshair`: optional cosmetic mod, a drawn crosshair with colour, style and recoil settings (MOD-24, CLI-1633); not part of any mod set, offered through a lobby's `[[mods.catalog]]` (MOD-25).
- `hacking_mod`: server mod, see players through walls (MOD-22) with chams, nametags, health bars and team mate colours; rooms may add no-recoil (MOD-23).

The other packs of the project (the weapon camos, the Case Reel, the inspect pack) live in their own repository, https://github.com/argallo/DeCombat-mods, built with
`tools/modrepo.py`; the five camos still here are the ones dc-gunkit shows as its built-in examples. Pack one with
`dc-modkit pack examples/mods/<name> -o <name>-<version>.zip`.
