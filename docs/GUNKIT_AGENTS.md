# GUNKIT_AGENTS.md: designing camos with `dc-gunkit`

`dc-gunkit` is a small camo studio. A person picks a gun and turns it with the mouse, and an agent changes the skin on it live over
MCP. It draws with the game's renderer and paints with `dc_render::camo`, the same code as the game, so the picture is what a player
gets. The rules are `VAR-6` to `VAR-8` in `DC_DEFAULTS.md`; the bigger plan is `docs/GUNKIT_PLAN.md`.

## Connect

```
claude mcp add dc-gunkit -- /path/to/dc-gunkit mcp
```

From a checkout: `cargo build -p dc-gunkit --bins`, then use `target/debug/dc-gunkit`. The server finds the window that is open and
opens one if none is, so the person sees every change. `dc-gunkit mcp --headless --rom DIR` runs with no window, for a machine with
no display.

The game folder is found from `--rom`, `CA_ROM`, the last one used, or a `rom` folder near the program. The window asks when it
cannot find one, and until then the tools say so.

## The loop

1. `select_gun` with a row or a name. `list_guns` lists them; the design stays when the gun changes.
2. Write a PNG and call `set_pattern` with its `path`. **The studio watches that file, so after this just save over it** and the gun
   updates by itself. Calling the tool again is not needed.
3. `screenshot` to look, and `set_camera` to see other sides. `yaw` -90 shows the left side and 90 the right.
4. `set_view` to check `first_person` and `third_person`. First person shows a whole gun: the third-person model's shape with the first-person
   texture baked on, so a gun can be judged from every side at full detail (`status` says `whole_gun`, with how well the two models fit). Both sides
   read the side the player sees, so writing on the far side is backwards, and a part the first-person model lacks shows that model's own texture,
   painted. The game draws its own first-person model: `set_view` with `whole_gun` false shows that one, and `status` says `reflected` when the studio
   has closed it with its own reflection. A gun whose two models are too unlike keeps that view (`whole_gun_refused_overlap`). The bake is the studio's
   alone and never goes into a pack. Their textures differ in size (mostly 1024 and 256), so a pattern with fine
   detail can read well on one and turn to noise on the other.
5. A gun the game gives a sight has its lenses kept clean by itself. `describe_texture` shows what was found. See below.
6. `save_pack` when it is done.

## How a pattern is painted

A pattern repeats twice across the gun's texture, and each texel is the pattern's colour scaled by how light the gun is there. So:

- **It should tile.** Seams show on the gun.
- **Mid colours work best.** The gun's shading multiplies them, up to 1.5 times brighter and down to 0.3 times. Pure white clips.
- **Large shapes read better.** The texture is cut into many parts, so a pattern is broken up across the gun. A 512 x 512 pattern is
  plenty, and `SIZE` in `patterns.rs` is 512 for that reason.

`preset` makes a starter pattern, and `colours` and `seed` change it. `status` lists the presets.

## Animation

`set_effect` moves a pattern over the gun as light, and can pulse it. It uses the painted pattern unless `set_pattern` with
`layer: "effect"` gives it its own. A dark base and bright details on an effect pattern is how Toxic and Galaxy work.

Values are held to the game's limits (VAR-4), and the answer shows what was kept: drift at most 1 repeat a second, a pulse at least
1 second. Keep it subtle. **A camo must never hide a player or give one away**, so a bright glow on a sniper will not pass review.

`set_flow` moves the paint itself, like a lava lamp (VAR-12): the pattern's colours drift and are swirled by slow waves, so
its blobs travel, stretch and fold over the gun. It is paint, lit like the gun, so it does not glow, and it takes the place of
`set_effect`'s light. Swirl 0.1 with 8 seconds per swirl is a gentle lava lamp; SCAR-L Prism uses that with a drift of 0.03, 0.01.
A busy pattern with many colours shows the movement best. It needs a client with VAR-12; an older one draws the plain gun.

A `screenshot` of an animated camo shows one moment. Take two a second apart to see it move.

## Keeping a lens clean

You usually do not need to do anything. When the weapon table gives a gun a sight, the studio finds its lenses from the model and keeps them clean on both
models, including in the whole gun and a saved pack. On a first-person model a lens is a round flat face high on the gun looking along the barrel. On a low third-person
model it is a large square picture on the texture on a small face, or for a reflex sight its two windows. A sight on a skin of its own is not painted, so it lists no
lens and needs none. `status` lists what was found as `lenses_kept_clean`. A gun with no sight gets none, so a whole gun is painted.

`describe_texture` shows what is where: the lenses, the kept circles, and the largest parts with where each lies on the gun, plus a picture of
the painted texture with the parts outlined in yellow and the kept circles in cyan. **Look at that picture.** The search finds a scope's lens, but
it does not know about a small knob or a sight's glass on a gun the table says nothing about, and it may miss one.

To change the list, `set_keep` replaces it for this gun and view only. Circles are in texels of that texture, with `x` to the right and `y` down,
and each view needs its own: the same lens is at a different place on the other texture. `set_keep` with `circles: []` paints the lens too, and
`auto: true` goes back to what was found.

## Your own whole texture

`set_full_texture` replaces this gun and view's texture with art drawn to its layout. No shading is added, so draw it in. It needs a
client with `VAR-7` to show in a match, and a client without it draws the gun's own texture.

## What may ship

**Nothing of the game goes in a pack or a commit, and nothing made from it.** That includes a texture from `get_texture`, a recolour
of one, and a picture of the gun. Use them to look at, then delete them. `save_pack` refuses art that follows the gun's own texture
closely (`likeness` in `dc_render::camo`), but that check is a backstop and not permission: art traced from the game's texture is
still the game's.

The pack's picture is a swatch of the pattern alone, for the same reason.

## What a save makes

A Vanilla++ pack in `Documents/DeCombat Camos/<name>`: `manifest.json`, `main.lua`, `assets/` and `screenshots/`. It passes
`dc-modkit validate` and `lint` before the tool answers, and any warnings are in the answer. `open_pack` reads it back, so the pack is
the save file. A save covers the gun on show, on both of its models.

A row is a variant's number, in 500..899. With none given, the row comes from the name, so check it against the rows of the other
packs on the entry you want (`docs/GUNKIT_PLAN.md` section 6).

A variant never changes how a gun plays (VAR-1), and a pack goes on an entry through review like any other
(`docs/MODDING_GUIDE.md` section 12).

## Without MCP

```
dc-gunkit guns [--filter F]
dc-gunkit render --gun 25 -o out.png --pattern tiger --yaw -60
dc-gunkit render --open <pack folder> -o out.png --view third_person
dc-gunkit check <pack folder>
```

## Tools

`status`, `list_guns`, `select_gun`, `set_view`, `set_pattern`, `clear_pattern`, `set_full_texture`, `set_effect`, `set_flow`, `set_keep`,
`set_camera`, `screenshot`, `window_screenshot`, `describe_texture`, `get_texture`, `save_pack`, `open_pack`. `tools/list` gives each one's arguments.
