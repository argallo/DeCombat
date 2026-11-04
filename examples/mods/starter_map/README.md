# Starter Map

Starter Map: a DeCombat community map made with dc-mapkit from the starter template.

Author: DeCombat. Version 0.1.0. A DeCombat community map (a `shared` mod with the `maps` content domain) exported by dc-mapkit 0.1.0.

Modes: TDM, FFA, ELIM, BOMB, FLAG.

## How to play it

1. Put the packed zip (`dc-mapkit export --zip`, or `dc-modkit pack` on this folder) into the server's mod set (HOSTING_AND_MODS.md section 4).
2. Start a room on a modded (vanilla++) lobby and pick the map `starter_map` from the list.

Clients download the package from the server and unpack it under their profile; nothing here needs a manual install.

## Files

- `assets/block.dtx`
- `assets/ground.dtx`
- `assets/maps/starter_map.dat`
- `assets/pictures/starter_map_radar.dtx`
- `assets/sky.dtx`
- `assets/wall.dtx`
- `main.lua`
- `manifest.json`
- `screenshots/main.png`

The world under `assets/maps/` and the pictures are build outputs: edit the dc-mapkit project and export again.
