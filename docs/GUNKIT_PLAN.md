# GUNKIT_PLAN.md: weapon variants for Vanilla++ and the dc-gunkit tool

Status: plan written 2026-11-02 (Mac), owner answers recorded the same day. P0 done; P1 built and tested locally, on no live entry yet. Rules are `VAR-nn` in `DC_DEFAULTS.md`.

## 1. What we are building

New looks for the GP guns, earned through weapon challenges, on Vanilla++. A variant never changes how a gun performs.

| Tier | What changes | Example |
|---|---|---|
| 1 Static | the gun's texture | a woodland camo |
| 2 Animated | the texture moves or glows; same model | pulsing green, a drifting galaxy |
| 3 Model | physical pieces added, and possibly tiers 1 and 2 | ghillie wrap and a default suppressor |

`dc-gunkit` is the community tool for making them, built the way `dc-mapkit` is: a text project, a CLI with `--json`, an MCP
server and pictures an agent can look at.

## 2. What the code already gives us

- **A ROM variant is its own weapon row.** The ROM's ghillie L96 rows name their own first-person and third-person models,
  skins, render styles and attachments. There is no separate camo system to find.
- **Packs can add rows.** `mod.content.weapons:register` adds a row in 900..999 (LG-314), and server and client apply the
  same content document.
- **Other players' guns already follow the row.** The third-person model and skins come from the weapon's row (CLI-618).
- **Attachments are drawn at named sockets** (`crates/dc-client/src/attach.rs`), including a default per row (DSV-80).
- **Pieces can be added to a model at run time** (`ModelScene::place_body_pieces`, used for bodies today).
- **Render styles have stages and a static texture matrix**, but `model.wgsl` has no clock, so nothing animates yet.
- **Items are per-account rows** (`items` table, `db::add_item`), so an unlock can be an ordinary item grant.

What is missing:

- **No weapon challenges.** A search found no such system. `@player` reports only match totals (kills, deaths, headshots),
  with nothing per weapon.
- **Curated packs cannot declare any of this.** Capabilities must be added on lobby and client.
- **`dc_tex` writes only 32-bit textures.** A 512 x 512 one is about 1 MB, which matters for download size.

## 3. Rules that shape the design

**3.1 No ROM pixels in a pack (owner decision: follow gen1recomp).** gen1recomp, which our mod system is modelled on, sets the
rule we adopt: a distributed mod never contains anything derived from the ROM, and that includes preview images. Art has two
legal paths:

- **Original art ships in the pack.** The author's own patterns, masks, and fully original textures painted to the gun's UV
  layout.
- **Derived art ships as a recipe.** A sandboxed `transforms.lua` runs on the player's machine against the player's own ROM. It can
  read a texture, recolour, blend and mask, and nothing else: no files, no network, no `require`. It is cheap enough to run in memory at each load (section 6), so nothing goes to disk, and a failing recipe
  disables only that pack's looks, with the error shown.

```lua
-- transforms.lua
return function(ctx)
    local base = ctx.read_texture(ctx.skin_of(25, "first_person", 1))
    local camo = ctx.read_own("assets/woodland.dtx")
    local mask = ctx.read_own("assets/mask_g36e_pv.dtx")
    ctx.write_texture(ctx.blend(base, camo, { mode = "overlay", mask = mask }), "g36e_woodland_pv")
end
```

So the gun keeps its wear and shading, and the pack holds no ROM data. A generated pattern is fine when no ROM pixels went into
making it.

**This is enforced, not just reviewed.** As gen1recomp's `MK301..MK304` do, `dc-modkit lint` gets hard rules, and `pack` refuses
on any of them: files pointing into ROM paths, ROM archives or patches, and textures that are byte-identical or perceptually
near-duplicates of the author's own ROM. Provenance is a review item too: each asset is declared original, recipe-made or
third-party with a compatible licence.

We take the design from gen1recomp, not its code: it is GPLv3 with extra terms, and its launcher is proprietary.

**3.2 Performance never changes (owner decision).** Only a new gun may change gameplay, and new guns are separate work not yet
enabled. A row with `variant_of = <base>` inherits every key from its base, and the loader refuses it if it sets anything outside
a cosmetic allowlist (models, skins, styles, icons, name). Tests compare the resolved rows. Every gameplay lookup resolves to the
base: weapon limits, Gun Game, rank gates, ammo, damage. That includes sound and default attachments, so a tier 3 suppressor can
only be a look.

**3.3 Old clients must not be locked out by surprise.** Shipping Container showed that a client refuses a whole entry when a
pack declares a capability it does not know. So the client that understands variants ships **before** the first variant pack
goes on an entry. Part of phase 1 is to make that failure friendlier.

**3.4 Unlocks are earned on the server.** Only the server counts progress. Curated packs still cannot touch rewards, GP, NX
or the shop, so a challenge's only reward is its cosmetic item.

## 4. How it works

### 4.1 A variant in a pack

```lua
mod.content.variants:register({
    id = 500,                     -- a row in the variant range (section 6)
    variant_of = 25,              -- the G36E; every other key comes from this row
    name = "Woodland",
    tier = 1,
    skins = {
        first_person = { [1] = "@derived/g36e_woodland_pv" },   -- written by transforms.lua (3.1)
        third_person = { [0] = "@derived/g36e_woodland_hh" },
    },
    icon = "assets/woodland_icon.dtx",
})
```

The lobby and client both expand this into a normal weapon row, so most of the game does not need to know about variants. Only
the client runs the recipe; a game server never needs a texture.

Masks exist because a camo should cover the gun's body, not its sights or the hands. Skins that the record does not name stay
as they are in the base row.

### 4.2 Tier 2: animated

Add one small effect to the model shader, driven by a clock and described by data:

```lua
fx = { kind = "scroll",  texture = "assets/galaxy.dtx", mask = "assets/mask.dtx", speed = { 0.02, 0.01 }, blend = "add" }
fx = { kind = "pulse",   mask = "assets/glow.dtx", colour = "40ff60", min = 0.3, max = 1.0, seconds = 2 }
fx = { kind = "frames",  sheet = "assets/flame.dtx", columns = 4, rows = 4, fps = 12 }
```

These three cover glow and galaxy looks. There is no scripting in shaders: a fixed set of effects stays safe, cheap and easy
to review. An effect must have a budget and must never hide or reveal a player. Bright glow on a sniper is a real gameplay
concern, so review checks it (D5).

P5 built this as one effect instead of three kinds: `fx=<drift u>,<drift v>,<lowest gain>,<highest gain>,<seconds per pulse>` in a `@camo:` path (VAR-4). A drifting pattern is the galaxy look, a pulsing one the glow. `frames` is not built. The effect uses the painted pattern, or one of its own as `<painted>|<effect>`, so that moving lines are not also painted still underneath. See `examples/mods/variant_galaxy_g36e` and `variant_toxic_l96a1`.

### 4.3 Tier 3: model additions

Three levels, cheapest first. An author uses the lowest that does the job.

1. **Another ROM model by row.** Built as `variant_look_of = <row>` (VAR-5): a variant wears the models of a row the ROM already has, and takes nothing else from it. The pack ships a row number, not a path or a mesh. See `examples/mods/variant_ghillie_l96a1`.
2. **A piece at a socket.** An original mesh placed at a named socket, the way attachments are.
3. **A piece on a gun bone.** An original rigid mesh bound to a named node of the ROM model at load, so it follows the
   animations. A modified copy of a ROM model cannot be shipped, because that would be extracted content, so pieces are added
   in memory. Meshes import from OBJ, as `dc-mapkit` does, or glTF.

Both first-person and third-person views need a piece. The tool warns when one is missing.

### 4.4 Challenges and unlocks

**For now (owner decision 2026-11-05):** camos are unlocked from the Camo Crate, a case that drops in matches on an entry with
camos and opens into one of them or into the plain gun of one of them, for 7 days, 30 days or permanent (DC_DEFAULTS.md
CRATE-1..CRATE-4, CRATE-10). The challenges below are not built.


- **Counting:** the game server adds an `@wstat uid=<id> weapon=<base> kills=<n> hs=<n> ...` line per player and weapon at match
  end. Kills with any variant count toward the base gun.
- **Storing:** new tables `weapon_stats` and `variant_unlocks`.
- **Defining:** data in the pack, with a small fixed set of counters to start.

```lua
mod.content.challenges:register({
    id = "g36e_woodland", weapon = 25, unlocks = 500,
    title = "Woodland", text = "Get 100 headshots with the G36E",
    goal = { headshots = 100 },
    after = nil,                  -- or the id of a challenge to finish first, for ladders
})
```

- **What counts (owner decision):** kills on bots count the same as kills on players, because the bots are competitive. Team kills
  do not count. Farming in private bot rooms is accepted for now, and it costs nothing but a cosmetic.
- **Rewarding:** at the threshold the lobby grants a permanent item with the variant's number and shows the existing award box.
  Grants are idempotent, so a replay or a retry cannot duplicate one.
- **Equipping:** a variant is its own inventory item, so the existing inventory screens and equip flow work with little UI.
- **Progress screen:** a new page lists each gun's challenges and progress. This is the largest piece of client UI.
- **Elsewhere:** a variant exists only on an entry with its pack loaded. On other entries LG-314 currently substitutes the
  slot's default item, and for a variant it should substitute its base gun.

### 4.5 `dc-gunkit`

Built first as a camo studio with a window and live MCP tools: `docs/GUNKIT_AGENTS.md` (VAR-6 to VAR-8). A pack folder is its save file, so there is no separate project format. The list below is the original plan; `batch`, `play` and tier 3 pieces are not built.

A project is a folder of text and images: `variant.json`, `challenge.json`, `assets/`.

```
dc-gunkit guns [--filter F]             the GP guns: row, name, skins, sockets, UV size
dc-gunkit new <dir> --gun 25 --tier 1
dc-gunkit template <dir>                from the player's own ROM, locally: UV layout, starter mask, never committed
dc-gunkit pattern <dir> art.png         checks size and tiling, and converts it
dc-gunkit apply <dir> ops.json          add an effect, a piece, a challenge; each change snapshotted
dc-gunkit validate <dir>                stat parity, budgets, missing views, near copies of ROM art
dc-gunkit render <dir> --view side|pv|hh|turntable [--frame N]     real renderer, real ROM; a PNG to look at
dc-gunkit batch <recipe.lua>            one recipe across many guns, reproducibly
dc-gunkit play <dir> [--headless]       local Vanilla++ with bots, gun equipped, as `dc-mapkit play`
dc-gunkit export <dir> --zip            the pack, its hash and its review row
dc-gunkit mcp                           the same verbs as tools
```

### 4.6 Making it easy for agents

- **One command per decision, `--json` on all of them**, with stable diagnostic codes and a fix hint on every error.
- **Pictures as the feedback loop.** `render` returns a PNG in the same call, so an agent judges its work by looking. An
  animated look renders several frames as a contact sheet.
- **Bring your own image generator.** The tool never calls one. An agent or person generates a tileable pattern anywhere, then
  runs `pattern`, which accepts or refuses it with a reason. This keeps the tool free of API keys and vendors.
- **Batch recipes**, because "woodland for every assault rifle" is the common job: pattern plus per-class mask rules, with one
  contact sheet to review.
- **A playbook and a skill**, `docs/GUNKIT_AGENTS.md` and `.claude/skills/gunkit/`, as with maps, including what makes a pattern
  read well at gun scale.
- **Mechanical checks before a human looks:** parity, budgets, both views present, and a headless match that fires the gun.

### 4.7 What P1 built

P1 replaced `transforms.lua` and `content.variants` with something smaller, and the sections above stay as the plan for when a pack
needs more than this. A variant is a `mod.content.weapons:register` of a row in 500..899 with `variant_of`, and each skin is a
painted path that the client resolves when it loads the texture (VAR-3):

```lua
PVSkin1 = "@camo:paint:25:PVSkin1:keep=928,928,92:" .. mod.assets:path("assets/candy.dtx"),
```

That reads: paint this pack's pattern over the texture `PVSkin1` of row 25, leaving a circle at texel (928, 928) with radius 92
as the base drew it. So the pack holds its pattern and some numbers, and no ROM path or pixel. `paint` is the only mode so far.

- **Each gun needs its own `keep` circles.** A camo must not cover a scope's lens, and where that sits differs per texture. These
  numbers belong in `dc-gunkit`'s gun list (P2), so an author doesn't have to look for them. A rectangle shape or a mask may be
  needed for guns whose sights are not round.
- `examples/mods/variant_candy_g36e` is the example. Its pattern is generated from noise, with no game pixels in it. The studio's
  "Open > Examples" reads the five example packs from an `examples/` folder next to the program (the GunKit package ships it, filled
  by `tools/gunkit_examples.py`; `DC_GUNKIT_EXAMPLES` overrides), not from bytes built into the binary (VAR-13).
- **Third person:** other players' gun textures reach the client through the game server's file list, so a variant needs the new game
  server as well as the new client. An older server upper-cases the path and the gun draws black. The owner saw it correct on a bot in a
  local match on 2026-11-02, from the side. The kept circle for `HHSkin0` is a first estimate from the texture, not yet seen from the front.
- **ROM-only tables:** the client's first-person models, sounds and muzzle effects are read from the ROM, which has no variant rows, so they look a variant up through its base gun. The weapon order works by name and a variant shares its base gun's name, so a variant takes no command of its own and uses its base gun's switch command and class. Any new ROM-only weapon lookup needs the same care.
- **Not done yet:** D6. Off the entry that carries the pack, a variant follows LG-314 (the default item), not the base gun.

## 5. Build order

Each phase ends with something testable. Client phases ship to players before any pack that needs them.

| Phase | Build | Done when |
|---|---|---|
| P0 Checks | the questions in section 6 | done 2026-11-02, by survey and code reading |
| P1 Static variant, by hand (built as section 4.7) | `content.variants`, `transforms.lua`, row expansion, the `variant_of` lookups of section 6, stat parity, the lint rules of 3.1, friendlier unknown-capability error | a GM-granted camo on the G36E, seen correctly in first person and by another player, stats identical in a test |
| P2 Tool, tier 1 | `guns`, `new`, `template`, `pattern`, `validate`, `render`, `export`, `mcp`, playbook | an agent, starting from a pattern, produces a reviewed pack with no hand edits |
| P3 Challenges | `@wstat`, tables, `content.challenges`, grant, award box, progress page | a played match on the real servers unlocks a camo, counting kills on bots and not team kills |
| P4 First real set | the G36E and L96A1 variants on Vanilla++ | players unlock them on live, including on Windows |
| P5 Animated | shader effects, contact sheets | glow and galaxy on both views, frame cost measured on Mac and Windows |
| P6 Model additions | levels 1, then 2, then 3 | a new ghillie-style L96A1 look in both views, through every animation, stats identical |
| P7 Later | batch polish, shop page integration, GUI if wanted | as needed |

## 6. P0 results (2026-11-02)

Read-only: a survey of the player's own ROM with a program kept outside this repository, plus reading the code. Nothing here was
played in a match, and "the code says" means exactly that.

**Both first guns qualify.** Rows 25 and 27 are on sale for GP and hold one main gun texture each.

| Question | Result | Consequence |
|---|---|---|
| Row budget | The ROM's rows stop at 296, the wire carries weapons as 16 bits, and only numbers from 1000 up mean something else to the client. So 603 numbers below 900 are free. | Reserve **500..899 for variants** (400 rows) and leave 900..999 for new guns. Variants need no separate index system. |
| Do screens read a registered row? | **No, not yet.** The code builds the item names and icons, the kill-feed icon and name image, and the lobby's weapon types from the ROM tables alone, so a pack's row has none of them. The lobby would not know its type at all. | The biggest P1 task: see below. |
| Skin slots | Of the 133 guns surveyed, every one has its main texture in first-person slot 1. 23 have more gun textures in slots 2 to 5. Third person uses slot 0. | The tool can default to slot 1 and list the extras. 25 entries on the GP list were not surveyed: they had no standard first-person model or it did not read. Check those before using them. |
| Texture sizes | First person is mostly 1024 x 1024 (118 of 133), third person 256 x 256 (128 of 133). Both are compressed, with one mip level. | Patterns and masks can be far smaller than the textures they cover. |
| Download size | A 256 x 256 pattern and first-person mask plus a 128 x 128 third-person mask come to about 0.6 MB as our 32-bit files, before zip. | About 25 variants fit in the 16 MiB default, more when guns share a pattern. No compressed writer is needed yet. |
| Compositing cost | Decode, blend and write took about 6 ms for a first-person texture and under 1 ms for a third-person one, using a stand-in pattern on this Mac. | Cheap enough to do in memory at load, with **no disk cache**, which also matches the guide's "exists only in memory". Not measured on a low-end machine. A composited texture uses 8 times the GPU memory of the compressed original. |
| Bones | Not usable by name. Gun-side bones are auto-named, the names differ between guns, and first-person models have 44 to 66 nodes. Third-person models are simple: the two checked have 2 nodes and 1 piece. | Tier 3 level 3 needs a bone chosen per gun, which the tool must show visually. It cannot be a batch job. Third person is easy. |
| Sockets | First person: the muzzle socket is on 117 of 133 guns and the sight socket on 110, spelled with mixed capitals. Third person is about the same. | Level 2 works on most guns. Socket lookups must ignore case, as the body socket lookup that was checked does; check the weapon's own. |

**The ROM's ghillie L96A1 is not on sale** (hidden, row 54), and it already has its own models and textures. That makes it a good
first test of tier 3 level 1: a variant of row 27 that names those models by path.

**What P1 must add.** When an item is a variant, each of these resolves through `variant_of` to the base gun, unless the variant
overrides the look:

- Lobby: weapon type and equip slot (`rom.weapon_kinds`), which also feeds the weapon limit, Gun Game and delivery; rank and shop
  gates; the LG-314 substitution off the entry.
- Client: item name and icon (`menu/storage.rs`), kill-feed icon and weapon name image (`hud::weapon_hud_images`), attachment socket
  states, and the shop's model preview.
- Game server: any table read by weapon number, checked against the content document it already applies.

The stat parity test should cover all of these, so a variant can't drift from its base without a failure.

## 7. Decisions

Decided by the owner on 2026-11-02:

| # | Decision | Answer |
|---|---|---|
| D1 | May a pack ship an image derived from a ROM texture? | No. Follow gen1recomp: original art, or a recipe run on the player's machine (3.1). |
| D2 | First guns? | The G36E (row 25) and the L96A1 (row 27). The owner wrote "L9", read as the L96A1 family's base gun. |
| D3 | What counts toward a challenge? | Bots count, because they are competitive. |
| D4 | May a variant change gameplay? | Never. Only a new gun may, and new guns are not enabled yet. |

Still open, with the default the plan uses until told otherwise:

| # | Decision | Default |
|---|---|---|
| D5 | Limits on looks? | No glow or bright colours that make a player easier or harder to see; a checklist for admins. |
| D6 | Are unlocks visible on other entries? | No for now: the base gun shows there. |

The first set is a chance to test all three tiers on guns that suit them:

| Gun | Tier 1 | Tier 2 | Tier 3 |
|---|---|---|---|
| G36E | a camo | a glow or galaxy | |
| L96A1 | a camo | | a new wrap in the ghillie family's spirit, using the ROM's own ghillie model by name first |

Both are GP guns with their main gun texture in first-person skin slot 1 (section 6).

## 8. A related fix

Shipping Container's preview and loading pictures were renders that showed ROM textures. Under the D1 rule they had to go, and maps
on ROM textures now ship plain name cards (MAP-103). gen1recomp's answer is a screenshot described by the script that regenerates it, rather than shipped pixels. The same
would fit here later, if real pictures are wanted: the pack names a view, and the player's client draws the picture from its own ROM on first load. Until then, a map's pictures can use only the pack's own art. The title banner is fine: it is drawn from the map's name.
