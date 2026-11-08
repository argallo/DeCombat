# Camo generators

Each folder here holds the script that drew the camo of the example pack with the same name in `examples/mods/`: our own
procedural art (numpy and Pillow), with no game pixels in it. A script writes `paint.png` (the pattern) and, for a camo
with a light effect, `fx.png` (what moves over the gun as light), 512 x 512 and tileable. Run it from its own folder:

    cd tools/camos/<pack>/ && python3 gen.py [arguments]

Most need no arguments. These six were made with:

| Script | Arguments |
|---|---|
| `variant_frostbite_deagle` | `.` (the output folder) |
| `variant_blood_moon_p226` | `.` (the output folder) |
| `variant_autumn_fall_m24` | `3` (the seed) |
| `variant_royal_marble_g23` | `3.5` (the vein width) |
| `variant_disco_fever_uzi` | `32 12` |
| `variant_honey_heist_ump` | `0.62` (the brightness scale) |

Every script reproduces its pack's pattern pixel for pixel with these arguments. The pack's `assets/pattern.dtx` (and
`glow.dtx`) is that PNG through dc-gunkit: `set_pattern` (and `set_pattern` with `layer: "effect"`), the motion the
pack's `main.lua` names (`set_effect` for `fx=`, `set_flow` for `flow=`), then `save_pack` with the pack's row. The
generated PNGs are not committed.
