# Fonts shipped with DeCombat tools

- `Roboto-Regular.ttf`: the UI font of `dc-mapkit` (the egui editor), Apache License 2.0 (`LICENSE-Roboto.txt`,
  copyright Google). Loaded with `include_bytes!` and bound to both egui font families; chosen because egui's own
  default font pack carries OFL / Ubuntu Font Licence files that CLEANROOM.md 4a does not list.
