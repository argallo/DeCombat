# WEAPON_FIRE_2009: the shot cone, random source, cone bounds, range, trigger and ammunition of the 2009 client (clean-room)

- **Title:** WEAPON_FIRE_2009, how the 2009 client turns one trigger pull into pellet directions and ammunition changes: the shot cone, the seeded
  random table, the weapon-side cone bounds, the trace range and pellet count, the trigger press / release state machine, the ammunition counters
  and the aim-mark colour source.
- **Scope:** the callees of the weapon's fire routine. Not covered (not read, section 9): the spread update law (already in `dc-client` `spread.rs`,
  CLI-61..64), the fire routine itself (cshell+0x23bca0), the fire rate (cshell+0x237180), the recoil functions (**now read: CORRECTION 2026-10-26, `WEAPON_FIRE_2009_addendum_recoil.md`**), the aim-mark draw, the table fill of
  the random source.
- **Evidence sources:** (a) the auditor report `reports/cpp-tu-weapon-audit.md` section 5 and its `.tsv`, which describe, in the
  auditor's own words with `cshell+0x<rva>` citations, formulas of byte-exact reconstructions of 2009 functions; (b) the clean decompiler output
  `CShell.dll/<rva>.c` (image base 0x37100000, cited `cshell+0xRVA`) to re-check each formula where a function exists there;
  (c) the clean specs `MOVEMENT_2009.md`, `GAME_MESSAGES_2009_C2S.md` (4.1 the 0x82 fire message), `UDP_SESSION_2009_B.md`, the capture records
  `CAPTURE_tdm1_GAME_VALIDATION.md` / `CAPTURE_tdm4_WEAPON_FORMS.md`, the `DC_DEFAULTS.md` rows CLI-31 and CLI-61..65, and the crates
  `dc-client` `spread.rs` / `arms.rs` (read only to state what differs). no text from the reference stack was opened; the auditor's formulas are the source and the rvas are
  the evidence. No ROM value appears here: row keys are named, their numbers are read from the user's ROM at run time. The few numbers below are
  **binary constants** (initial values stored by a constructor, cited with the rva).
- **Status:** DRAFT 1. Not implemented. Written 2026-10-25.

Evidence grades: **PROVEN** = literally visible in the clean decompiler output of the cited function (or in a capture, then marked PROVEN-CAPTURE);
**DISASM** = read by the auditor in read-only disassembly of a function that is not in the clean output, or a detail the clean C hides (x87 arguments);
second-hand here, so it is the weakest of the evidence grades and needs a second read before an implementation relies on it alone;
**PLAUSIBLE** = the reading consistent with the surrounding code or with the captures, one step not literally visible; **UNPROVEN** = not visible.
Where the auditor's text and the clean C disagree, the clean C wins and the difference is stated.

Conventions. "Row" = the weapon's table record read at load time (`[Weapon<n>]`), "mod" = an attachment record (up to three mod slots on a weapon),
"ammo row" = the ammunition record. Offsets are record offsets of the 2009 client (binary layout, not ROM data). "Player manager" = the client's
local-player object at cshell+0x49bf60 (the object C2S 4.1 calls `local_player`). Perturb values are integers in the row and are scaled by 0.001 to
tangent units (section 2).

## 0. Contents

1. Function map
2. The shot cone (cshell+0x176cb0)
3. The random source and the seed (cshell+0x2973a0)
4. The cone bounds (cshell+0x2334f0)
5. Range and pellet count (cshell+0x23a910, fire routine)
6. The trigger state machine (cshell+0x23b050, +0x23b340)
7. Ammunition bookkeeping
8. The aim-mark colour source (cshell+0x17f7b0, +0x176f10)
9. What is not in the read functions, and follow-ups
10. What `dc-client` does today versus 2009
11. Open items

## 1. Function map

| rva | role (our words) | in clean decompiler | used by | grade of this spec's reading |
|---|---|---|---|---|
| cshell+0x176cb0 | shot cone: one pellet direction | yes (x87 arguments hidden by the float-to-int helper) | fire routine at cshell+0x23c01e, once per pellet | structure PROVEN, bound arithmetic DISASM |
| cshell+0x2973a0 | seeded random-table read | yes | two call sites in 176cb0 (cshell+0x176ddc, +0x176e2c) | PROVEN |
| cshell+0x2334f0 | weapon cone bounds and current spread | yes | fire routine at cshell+0x23c1bb | structure PROVEN, mod arithmetic DISASM |
| cshell+0x23a910 | hitscan trace of one pellet | yes | fire routine at cshell+0x23c087 | PROVEN (range factor DISASM) |
| cshell+0x23b050 | trigger press | **no** | weapon method table slot +0x150 (cshell+0x3f1a64) | DISASM (auditor) |
| cshell+0x23b340 | trigger release | yes | slot +0x154 (cshell+0x3f1a68) | PROVEN |
| cshell+0x2307f0 | state change | yes | everything below | PROVEN (role by use) |
| cshell+0x233d90 | per-shot ammunition decrement | yes | after a shot | PROVEN |
| cshell+0x2317a0 (and cshell+0x2399e0, no clean C) | clip reload | 2317a0 yes | reload key, empty clip | PROVEN / DISASM |
| cshell+0x2315a0, +0x2316b0, +0x231620 | ammunition change (with reload, immediate, by type) | yes | ammunition switch | PROVEN |
| cshell+0x17f7b0, +0x17bd20 | weapon row defaults and loader | yes | row load | PROVEN (stores), key association DISASM |
| cshell+0x176f10, +0x175730 | mod row loader and defaults | yes | mod load | PROVEN |

Callers read by the auditor in the (not clean) fire routine cshell+0x23bca0: it calls method slot +0x44 (current spread) at cshell+0x23bd15, the
fire-vector function cshell+0x234d50 at +0x23be35, 176cb0 at +0x23c01e, 23a910 at +0x23c087, 2334f0 at +0x23c1bb (DISASM, auditor). The
seed pointer handed to 176cb0 is the player manager + 0x1fc (DISASM, auditor).

## 2. The shot cone (cshell+0x176cb0)

Inputs, as the clean C shows them: a path record `P` (a 16-bit weapon index at +0x00, the aim direction vector at +0x04, an "up" axis vector at
+0x10, a "right" axis vector at +0x1c, a zoom byte at +0x30) and a pointer to the seed integer (section 3). `this` is the weapon table object: it holds
the table's data pointer at +0x15ac and its count at +0x15b0; the row is `table[P.weapon id]` (PROVEN, cshell+0x176cb0: the bound check, the row
pointer; no row = nothing is done).

2.1 **Bounds (DISASM for the arithmetic, structure PROVEN).** Four integer helper calls (float to integer, arguments are x87 values the clean C does not
show) precede the gate, two more precede each draw:
- two conditional ones, only when the zoom byte is set and the row's ZoomedMin (+0x298) / ZoomedMax (+0x29c) is non-zero: they add the per-shot
  addend of the zoomed bounds (PROVEN as conditions, PLAUSIBLE as "addend");
- two that produce `min` and `max` (integers). The auditor: the bounds are the row's MinPerturb (+0x288) / MaxPerturb (+0x28c); with the zoom byte set
  and the row's zoomed pair present they are the zoomed pair plus the per-shot addend; in triple-shot mode and with the row's TripleShotMin/MaxPerturb
  (+0x330 / +0x334) present they are those; both bounds are multiplied by a per-shot scale (2.3) and truncated to integers (DISASM, auditor; the clean
  C shows only the helper calls).

2.2 **Gate (PROVEN).** The cone is applied only if `min <= max` **and** `max > 0`; otherwise the direction is left exactly as it was and **no draw is
made, the seed does not advance** (the increments sit inside the gate).

2.3 **Per-axis half-width (DISASM, auditor; arithmetic PLAUSIBLE).** Two half-widths, `h_right` first, then `h_up`, each
`h_axis = trunc( min + trunc((max - min) x f_axis) x scale )` in the auditor's words ("min plus the truncated product of the range and that axis's
spread fraction, times the scale, truncated"; the placement of the scale, applied to the bounds and again to the product, is UNPROVEN, open item O-3).
- `f_axis` is the weapon's **current spread** (the value of method slot +0x44, the same float that goes out as 0x64 `weapon_f` and 0x82 `frac / 255`)
  for **both** axes; the fire routine passes it twice (DISASM, auditor, cshell+0x23bd15).
- `scale` is the weapon's DynamicPerturbFactor (row +0x274; binary default 1.0, cshell+0x17f7b0 store) plus each mod's DynamicPerturbFactor (mod
  +0x260; PROVEN offset, cshell+0x176f10); by movement state it is replaced by the row's Run / Walk / Jump factors (row +0x278 / +0x27c / +0x280, binary
  default 1.0 each, cshell+0x17f7b0 stores). The auditor flags this as "my reading of 23bd1c..23bd8d, confirm it by DISASM before use" (UNPROVEN
  until then; open item O-2). The wire `shot_scale` of the 0x82 (C2S 4.1.1, 1.0 or 1.2, constant per weapon, PLAUSIBLE equal to the row's
  DynamicPerturbFactor) is the unvaried factor.

2.4 **Offsets (PROVEN structure, cshell+0x176cb0).** For each axis in turn (right, then up): `r = table_read(seed)` then `seed += 1` (section 3);
`off_axis = (r x 2 x h_axis - h_axis) x 0.001`, that is `(2r - 1) x h_axis` thousandths of a unit (binary32 constant 0.001, x87 intermediates). The new
direction is `d + off_right x right_axis + off_up x up_axis` (the clean C pairs the first offset with the vector at +0x1c and the second with the one at
+0x10), with a call to a vector helper whose operand the clean C does not show; the auditor reads it as the normalisation (DISASM, auditor, PLAUSIBLE;
the clean C places the call before the three stores).

2.5 **Consequences (PROVEN structure).**
- The cone is a **square in tangent space with independent axes**, not a disk: each axis is uniform in `[-h, +h]` if the table value `r` is uniform in
  [0, 1) (that range of the table is PLAUSIBLE, section 3).
- **DuckPerturb / MoveDuckPerturb do not appear in 176cb0**: they belong to 2334f0 (section 4) and the aim mark.
- Two draws per pellet: the seed advances by 2 per pellet (the same figure as the capture, section 3.3).
- The 0.001 factor makes a half-width of `h` a tangent offset of `h / 1000`: the milliradian reading CLI-31 used (PLAUSIBLE) is now supported by code.
- Zoom: the zoom byte of `P` selects the zoomed bounds. Triple-shot: the row's triple pair. Both only when the row has the keys (non-zero values).
  The **binary defaults** of the zoomed pair are non-zero (cshell+0x17f7b0 stores: 1 and 500), so the zoomed variant exists on every row that does not
  say 0 (PROVEN).

## 3. The random source and the seed (cshell+0x2973a0)

3.1 **The read (PROVEN, cshell+0x2973a0, 58 bytes).** `this` is a global object (cshell+0x4b2af0, DISASM auditor) holding a float table: data pointer at
+0x15b0, element count at +0x15bc (the auditor wrote +0x15ac for the pointer; the clean C reads +0x15b0, and +0x15ac is the weapon table's data pointer in
176cb0; the clean C wins, open item O-12). The function takes the address of the seed integer; if `seed >= count` it replaces the seed by `seed mod
count` **and writes it back**; it returns the table entry `data[seed]`. The caller (176cb0) then adds one to the stored seed.

3.2 **The table fill is not read** (open item O-4): its values (the range is assumed [0, 1) by the cone's `2r - 1` mapping, PLAUSIBLE) and whether the
fill is identical on every client are UNPROVEN.

3.3 **The seed is the 0x82 `stat` field (new, PLAUSIBLE, strong).** The seed integer is the player manager's field at +0x1fc (DISASM, auditor). The 0x82
layout (C2S 4.1) ends with `[stat:u32]` = `local_player + 0x1fc`, and `local_player` is the same object (cshell+0x49bf60: the clean C of 2334f0 reads
the same pair +0xf0 / +0xf4 that C2S 4.1 and 2.1 name for `local_player`). The captures show `stat` = 2, 4, 6 ... +2 **per 0x82 message**, across weapon
changes (tdm1 2..166; tdm4 2, 4, 6, 8, 10; the tdm6 eight-pellet shot +2 per message), grenades included (PROVEN-CAPTURE). That is exactly two draws per
pellet. So the `stat` of a message is the seed value **after** that pellet's two draws; the draws used the seed values `stat - 2` (right) and `stat - 1`
(up), each reduced mod the table count. This **contradicts the auditor's lead** that the fire message's one-byte `seed` (2..255, from a separate random
call, C2S 4.1) feeds the table: nothing in the read functions ties that byte to the seed. The first message of a capture carries 2, so the counter
started at 0 in those sessions; whether respawn, match start or map load resets it is UNPROVEN (open item O-5).
- Server consequence (D): a server that knows the table could recompute the unperturbed cone offsets of every pellet from `stat` alone; without the
  table it cannot. A server must treat `stat` as client-supplied and may cross-check the +2 spacing.

## 4. The cone bounds (cshell+0x2334f0)

Outputs: `max` (first pointer), `min` (second pointer), and the current spread (third pointer, the value of method slot +0x44) (PROVEN). It first loops
over the three mod slots (a method call per slot, result unused in the C; it prepares the mod list the sums below fold over; PLAUSIBLE).

4.1 **Zoom test (PROVEN):** `zoom = player_manager.f0 >= 1 or player_manager.f4 != 0` (fields at +0xf0 / +0xf4). The 0x82 byte `moving` and `pu_flags`
0x0800 are the same test and were **never set in any capture**, although the players walked; this points at a scope / zoom state rather than movement
(PLAUSIBLE; the C2S name `moving` is probably wrong, open item O-6).

4.2 **Max (PROVEN selection, DISASM arithmetic):**
- `zoom` and row ZoomedMax (+0x29c) non-zero: `max = row ZoomedMax + (fold over the mods' ZoomedMaxPerturb, mod +0x2c0)`;
- else triple-shot mode (method slot +0xf8) and row TripleShotMax (+0x334) non-zero: `max = row +0x334`;
- else `max = row MaxPerturb (+0x28c)`.

4.3 **Min (PROVEN selection, DISASM arithmetic):**
- `zoom` and row ZoomedMin (+0x298) non-zero: `min = row ZoomedMin + (fold over the mods)`; the auditor names the mods' MinPerturbPer (mod +0x230) for this
  fold, while the mod loader also has a ZoomedMinPerturb at mod +0x2bc (cshell+0x176f10); which one feeds it is not visible in the clean C (open item
  O-7);
- else triple-shot and row TripleShotMin (+0x330) non-zero: `min = row +0x330`;
- else `min = row MinPerturb (+0x288)`.
- The **fold** is a doubling accumulator, `sum := 2 x sum + value` over the mod slots (DISASM, auditor, PLAUSIBLE; the clean C hides it behind the
  float-to-integer helper).

4.4 **Duck (PROVEN structure):** if row DuckPerturb (+0x290) and row MoveDuckPerturb (+0x294) are both non-zero (binary default of both: 100.0,
cshell+0x17f7b0) and the stance word (`[player_manager+0x10] + 0x24`) has the crouch bit 0x20: if `(stance & 0xa00) == 0` then `min :=` DuckPerturb plus
the mods' DuckPerturb (mod +0x274); otherwise `min :=` MoveDuckPerturb (no mod term visible). This **replaces** the min chosen in 4.3, zoomed or not.
Bits 0xa00 are RUN 0x200 and ANY_MOVE 0x800 of the control word (MOVEMENT_2009 5; CLI-61).

4.5 **Mod term (PROVEN):** the final `min` returned is `min + (the mods' MinPerturb sum, mod +0x270)`: added always, after the duck replacement.

4.6 `max` has no duck term and no mod MinPerturb term. None of the zoom, triple-shot or mod terms is in `spread.rs`.

## 5. Range and pellet count (cshell+0x23a910 and the fire routine)

5.1 **Gate (PROVEN):** if the row byte at +0x4a0 is non-zero the function does **not trace**: it sets the weapon byte +0x1cc to 1 and returns. +0x1cc is
the `flag_c` byte of the 0x82 (C2S 4.1: weapon+0x1cc), always 0 in the captures, so the captured weapons all have +0x4a0 = 0. Meaning of the row flag
UNPROVEN.

5.2 **Range (PROVEN source, DISASM factor):** the trace length starts at the row Range (+0x2a0). The binary default of that field is **3000**
(cshell+0x17f7b0, PROVEN), not the 10000 that `arms.rs` uses. With a mod in slot 0 the length is multiplied by a float the clean C leaves in a register;
the auditor reads it as `1 + RangePer` of the first mod (mod +0x234, key present in the mod loader, cshell+0x176f10; DISASM, auditor, PLAUSIBLE).
`range = row.Range x (1 + mod.RangePer)`.

5.3 **Segment (PROVEN structure):** start = the shot origin, end = start + `range` x the direction, the direction being the pellet's perturbed direction
(held by the caller, not the weapon's aim). The trace uses an object query with a filter (layout, flags and the world-model rule: 5.3.1); a hit is handed to the impact routine. **A miss still
produces an impact record at the end point** with an up normal (0, 1, 0), the surface kind **0x6e** and no hit object (PROVEN, cshell+0x23a910).

5.3.1 **The query (correction and detail of 5.3, 2026-10-26, report `client-brushhits`; disassembly PROVEN structure).** The pellet trace is one engine segment query made through the client
interface member +0x64 (filled at engine+0x14270, pointing at engine+0x10410, which forwards to the world interface's segment query engine+0x40970). The query record that cshell+0x23a910 builds
on its stack holds: from-point at +0x00, to-point at +0x0c, **flags at +0x24 = 7**, the object filter at +0x28 (cshell+0x235220) with its user data at +0x34, and the box test at +0x3c
(cshell+0x235330). The result holds the point (+0x00), the plane (+0x0c), the object (+0x1c), the polygon handle as two words (+0x20 / +0x24, 0xFFFFFFFF = none) and a value (+0x28) read from the
polygon's surface record. The three flag bits: bit 0 tests objects, bit 1 skips every object whose flag word has neither 0x1000 nor 0x2000 (per-object callback engine+0x40920), bit 2 returns the hit
polygon (callback engine+0x407a0). Per object the engine (engine+0x40650) checks the bounds, then the shell's filter, then traces **world-model objects (kinds 2 and 9) through their own BSP with a polygon
result** (engine+0x40190); any other object is traced by its box without a polygon. The filter cshell+0x235220 never accepts the local player's two objects; an object with user flag 0x01000000
that is a character goes to the per-node sphere test (cshell+0x234fb0, rule in `GAME_MESSAGES_2009_C2S.md` 4.1.1, corrections of 2026-10-26), else it passes; objects without that flag are refused only
for two object lists (cshell+0x30ff10 / cshell+0x30ff60). The impact step takes the surface of a hit from the polygon (cshell+0x37f430), and for a model hit without a polygon from the user flag
0x1000 / the node, else a default. Consequence for the wire: a hit on a world model is a polygon hit, so it sends no hit object (C2S 4.1.1); the 0x82 hit object exists for characters only.
Surface 0xc9: after the impact of a hit on that surface the shell traces the same segment again with another filter (cshell+0x2e3410) and plays a second impact (a pass-through surface, PLAUSIBLY
glass or thin material); `dc-client` does not do this yet.

5.4 **Pellets (DISASM, auditor):** `pellets = VectorsPerRound (row +0x2b0; binary default 1, cshell+0x17f7b0) + VectorsPerRoundPer (the third mod slot's
term, mod +0x244) x count` (the "count" is not specified by the auditor; open item O-8). One 0x82 per pellet (C2S 4.1.1, PROVEN-CAPTURE).

## 6. The trigger state machine (cshell+0x23b050 press, +0x23b340 release)

Weapon states (numbers, PROVEN from the switch in the release function): **0** inactive, **1** idle, **3** firing, **9** reloading, **10** out of
ammunition, **0xb** select (draw), **0xc** deselect. State is read by method slot +0x98; the state change is cshell+0x2307f0 (which also sets the next
idle time, DISASM auditor). Row kinds: the kind field at +0x418 (the value 0xb is the case that needs player-manager checks), the sub-kind at +0x21c,
the "world cool time" row flag at +0x4e0.

6.1 **Release (PROVEN, clean cshell+0x23b340; byte-exact except one register choice).** First: byte +0xd8 := 1 (trigger released) and player-manager
method slot +0x58 is called with 0. Then by state:
- **idle (1):** if kind is 0xb and sub-kind 2: if the player-manager check (slot +0x80) is false, a one-shot animation is started (cshell+0x230a90), 6
  when a player-manager counter (slot +0x84) is odd and 7 when even; if the check is true the animation 0 (idle) is started. Then the idle handler
  (cshell+0x237eb0, arg 0).
- **firing (3):** the weapon's "can stop" method (slot +0x15c) is asked; false = go to idle. True: byte +0xd8 := 0; if the weapon kind (a helper keyed
  by the weapon id) is **6 (throwable)**: with **no ammunition left** (the ammunition manager's total for the ammo id is 0) the pending byte +0x1ad is
  set and the weapon does **not** go to idle; otherwise idle handler (arg 1) and state 1; in both cases two flags of a shell object are set. If kind
  0xb: with the cool-time flag (+0x4e0) the player manager is asked (slot +0x9c) and, if true, three of its clear methods run (slots +0x98, +0xa4,
  +0xa8), then the same two shell flags are set and the weapon goes idle; without the flag and with sub-kind 2 and the +0x80 check true the animation 0
  runs, then idle.
- **reloading (9):** hands the release to the reload handler (cshell+0x23a210, arg 0); a false return = state 1.
- **out of ammunition (10):** the matching handler (cshell+0x237e20).
- **select (0xb):** clears a byte (+0xe0), repeats the kind 0xb / sub-kind 2 animation reset, hands the release to the select handler (cshell+0x237d20);
  a false return = state 1.
- **deselect (0xc):** the deselect handler (cshell+0x237da0); a false return sets byte +0x146 and goes to state 1.

6.2 **Press (DISASM, auditor; not in the clean output, PLAUSIBLE until a second read):**
- from **idle**: go to **firing (3)** if ammunition > 0 (or a player-manager override), else to **out of ammunition (10)**;
- during **reload, select or deselect**: the press is handed to the matching handler (the same three handlers as 6.1);
- in the **firing** state, kind 0xb with sub-kind 2 or 3 (+0x21c), and the cool-time flag (+0x4e0), require player-manager checks; a failed check plays
  the idle animation and goes back to idle;
- kinds **6** and **4** toggle HUD / player-manager flags.

6.3 Not here: the fire **rate** and the fire-pending latch (cshell+0x237180, +0x237130; C2S 4.1). CLI-29's 105 ms stands.

## 7. Ammunition bookkeeping (cshell+0x233d90, +0x2317a0, +0x2399e0, +0x2315a0, +0x2316b0, +0x231620)

7.1 **Model (PROVEN):** per ammunition id the ammunition manager keeps **one total** (read by its method slot +0x80 with the ammo id, set by slot +0x38),
and the weapon keeps one **clip counter** (weapon +0xe4) with a target (+0xe8). It is a total plus a clip counter, **not** clip plus stock. The clip
size is method slot +0x128.

7.2 **One shot (cshell+0x233d90, PROVEN).** The ammunition is *infinite* if the global cheat byte (cshell+0x4b30d0) is set or the weapon row's +0x23c is
non-zero; then the total reads as **1000** and is never written. Otherwise the total is read. Only if the clip counter is above 0:
- if the clip size is above 0, the clip counter is decremented and the byte +0x1ad is set;
- if not infinite, the total is written back as `total - 1` (the ammunition manager's set method with the weapon and ammo ids).
(The first call, cshell+0x233160 with two zero arguments, is a sync step whose effect is not traced.)

7.3 **Reload (cshell+0x2317a0, PROVEN with two caveats).** Refused if a cheat-style check (cshell+0x1f0840) is non-zero, or the state is **0xc**
(deselect) and the force argument is 0. Otherwise: the `total` is read (a negative count argument means "the whole total"); then it returns early if
the state is 9 (a log line says the reload is already running; the guard also needs a flag the clean C shows as non-zero), or the clip counter already
equals the clip size, or equals a second compared value (the clean C's register reuse names it as a weapon field; the auditor reads it as the total;
PLAUSIBLE, open item O-9). If `total > 0` and `clip size > 0`:
- target := `min(total, clip size)`; with **ReloadPerBullet** (row +0x250 non-zero) target := `clip + 1` for the animation start;
- if the reload animation exists (a condition on a value the decompiler lost) and its index is set, the state goes to **9** and the function returns
  (the animation path); for ReloadPerBullet rows the target is then reset to `min(total, clip size)` on the path without an animation;
- if the pending-clip byte (+0x1f0) is set, the clip counter := target at once and the byte is cleared.
The auditor's one-line summary "a reload targets the smaller of the total and ShotsPerClip; ReloadPerBullet targets clip + 1 per cycle; skipped when the
clip already equals the clip size or the total" matches this reading.

7.4 **Ammunition change (PROVEN, cshell+0x2315a0 / +0x2316b0 / +0x231620).** Refused while reloading (state 9) unless the force argument is set, and
refused if the "may change to this ammunition" method (slot +0x8c, or +0x148 for the two forms that take a weapon type) says no; 2315a0 also refuses the
current id. On success: the weapon's ammo id (+0x9c) := the new id, the ammo row (+0xa4) := the row of that id; **if the ammo row has animation
overrides (row +0x98 non-zero) the weapon is re-selected** (slot +0x58 / +0x54 forms), **otherwise it reloads** (slot +0x2c / +0x28 forms). 2316b0
additionally stores the id byte in +0x214 and, when the change is refused, calls the ammunition manager's set and sync methods with the current values
(effect not traced).

## 8. The aim-mark colour source (cshell+0x17f7b0, +0x176f10, +0x17bd20)

8.1 **Sources (PROVEN offsets, DISASM order):** a **sight mod** (mod Type = 2 with a sight-colour index in the range the auditor gives, 0..2) supplies
its CrossHairA / CrossHairR / CrossHairG / CrossHairB (mod +0x184 / +0x188 / +0x18c / +0x190; the four keys are read by the mod loader, cshell+0x176f10).
Otherwise the **weapon row's** CrossHairA / R / G / B (row +0x204 / +0x208 / +0x20c / +0x210) apply. The order of the two sources is the auditor's
reading of cshell+0x845f0 (DISASM, auditor, PLAUSIBLE; that function's own C is the aim-mark draw, section 9).

8.2 **Binary default (PROVEN, cshell+0x17f7b0):** the weapon row constructor stores 255 at +0x204 (A), 0 at +0x208 (R), 255 at +0x20c (G), 0 at +0x210
(B): **opaque green, A 255 R 0 G 255 B 0**. It matches the observed (1, 252, 1) core pixels of the aim-mark screenshots (CLI-65, OBSERVED). CLI-65 can
cite `EVIDENCE: cshell+0x17f7b0` for the default instead of a DC-DEFAULT; the per-row and per-sight colours are new.

## 9. What is not in the read functions, and follow-ups

Not in the byte-exact reconstructions and not read here: the spread update (cshell+0x232120, +0x2319b0, the step loop of +0x237180; `spread.rs` has
them from clean decompiler), the fire routine cshell+0x23bca0 (not in the clean output), the recoil (cshell+0x1f5150, +0x1f3070; `spread.rs`; **read 2026-10-26: `WEAPON_FIRE_2009_addendum_recoil.md`**), the aim-mark
draw (cshell+0x82c00, +0x845f0; `spread.rs`), the fire delay (cshell+0x237180), the table fill of the random source. Follow-ups, all disassembly of the
ROM (auditor list, with the findings of this spec):
1. **Read cshell+0x23bca0**: the path record fields (the two axis fractions, the scale and where the movement-state variant is chosen), the pellet
   loop, the order of cone / trace / send per pellet (this fixes whether `stat` is the post-draw value for every pellet), the fire rate.
2. **Find the writers of player_manager+0x1fc** (reset points; item O-5). The `stat` link of 3.3 replaces the auditor's byte-seed lead.
3. **Find where the random table is filled** (item O-4): whether it is a fixed table (a server could then reproduce a shot).
4. **Read cshell+0x237180** for the fire delay (CLI-29).
5. Read cshell+0x23b050 a second time in disassembly (it has no clean C).

## 10. What `dc-client` does today versus 2009

| topic | `dc-client` today | 2009 (this spec) | evidence | change |
|---|---|---|---|---|
| shot shape (`arms.rs` perturb, CLI-31) | uniform in a disk of half-angle `half` around the aim, `half = cone_units x 0.001` rad | **square**, independent right and up offsets, each uniform in `[-h, +h]` x 0.001, added to the aim direction, then normalised | 2.4, PROVEN structure | replace the shape |
| random source | xorshift generator | seeded table read at `seed mod count`, seed advancing by 1 per draw, 2 per pellet | 3.1, PROVEN | table content open (O-4): keep a DeCombat stand-in table |
| seed on the wire | `stat` = messages x 2 (`STAT_STEP`), `seed` byte from the generator | `stat` **is** the seed counter (after the pellet's draws); the `seed` byte is a separate random call | 3.3, PLAUSIBLE | `stat` should be the seed value the cone used |
| half-width | `cone_units(min, max, spread) x 0.001` with the duck-aware min | `trunc(min + trunc((max - min) x spread) x scale)` with zoom, triple-shot and mod terms; **no duck term** | 2.3, 4, DISASM / PLAUSIBLE | drop duck from the shot; keep it for the aim mark |
| per-shot scale | none (wire `shot_scale` only) | DynamicPerturbFactor (+ mods) or the Run / Walk / Jump factors | 2.3, DISASM / UNPROVEN | add after O-2 |
| zoom / triple-shot / mod bounds (`spread.rs` `cone_units`) | min / max / duck only | zoomed pair + mod ZoomedMax, triple-shot pair, mod MinPerturb sum, doubling fold | 4, PROVEN selection, DISASM arithmetic | add |
| range (`DEFAULT_RANGE`) | 10000 when the row has none | **3000** binary default; `range x (1 + mod RangePer)` | 5.2, PROVEN default | change the default (DC_DEFAULTS CLI-31 row) |
| a miss | no impact record | impact at the end point, up normal, kind 0x6e | 5.3, PROVEN | add when impact effects exist |
| pellets | `VectorsPerRound`, default 1 | `VectorsPerRound + mod VectorsPerRoundPer x count` | 5.4, DISASM | add the mod term after O-8 |
| ammunition model (`Carried`: clip + stock) | clip and stock, reload takes `min(size - clip, stock)` | one total + clip counter; reload target `min(total, clip size)` | 7.1, 7.3, PROVEN | change the model |
| infinite ammunition | none | cheat byte or row flag: total reads 1000, never lowered | 7.2, PROVEN | add (mod key) |
| ReloadPerBullet | none | target `clip + 1` per cycle | 7.3, PROVEN / PLAUSIBLE | add |
| ammunition change while reloading | not modelled | refused unless forced; animation overrides re-select, else reload | 7.4, PROVEN | add |
| trigger | clip-driven (`Phase`), no kind checks | state machine with throwable, kind 0xb and cool-time branches | 6, PROVEN release / DISASM press | add the kind branches that matter for TDM (throwable release) |
| aim-mark colour (`spread.rs` `mark::COLOUR`) | constant green, DC-DEFAULT CLI-65 | sight mod colour, else the row's CrossHairA/R/G/B, binary default opaque green | 8, PROVEN default | read the row / mod colours; cite cshell+0x17f7b0 |
| fire rate | fire clip length / 105 ms stand-in (CLI-29, CLI-30) | not in these functions | 9 | unchanged |

## 11. Open items

| id | item | next step |
|---|---|---|
| O-1 | the x87 arguments of the float-to-integer helper calls in 176cb0 and 2334f0 (the clean C hides them) | read by disassembly to turn the DISASM arithmetic of 2.3 and 4.2-4.3 into PROVEN |
| O-2 | the per-shot scale choice (DynamicPerturbFactor vs Run / Walk / Jump factors) and the two axis fractions (auditor: "my reading of 23bd1c..23bd8d") | read cshell+0x23bca0 |
| O-3 | whether the scale multiplies the bounds, the product, or both | read O-1 |
| O-4 | where and with what the random table is filled; its value range; identical on all clients? | read; decides whether a server can reproduce a shot from `stat` |
| O-5 | resets of player_manager+0x1fc (spawn, round start, map load) | writers of +0x1fc |
| O-6 | the meaning of player_manager+0xf0 / +0xf4 (zoom / scope?) and the probably wrong name `moving` in C2S 4.1 | writers of the pair; a capture with a scoped weapon |
| O-7 | which mod key feeds the zoomed min fold (MinPerturbPer +0x230 per the auditor vs ZoomedMinPerturb +0x2bc) | O-1 |
| O-8 | the "count" in the pellet term VectorsPerRoundPer x count | read cshell+0x23bca0 |
| O-9 | the second compared value in the reload early-out (7.3) | disassembly of cshell+0x2317a0 |
| O-10 | the press function cshell+0x23b050 has no clean C; every branch of 6.2 is second-hand | second disassembly read |
| O-11 | the second reload entry cshell+0x2399e0 and the next-usable-ammunition functions cshell+0x232950 / +0x232890 (no clean C) | not described here |
| O-12 | auditor offset +0x15ac for the random object's table pointer vs the clean C +0x15b0 | clean C used (3.1); recheck |
