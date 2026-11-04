# WEAPON_FIRE_2009 addendum: the seeded table, the fire routine and the shot cone, replayed from the captures (clean-room)

- **Title:** addendum to `WEAPON_FIRE_2009.md` sections 2, 3 and 5: where the cone's random table comes from, what the fire routine
  cshell+0x23bca0 does per pellet, the exact cone arithmetic, and the proof by capture replay.
- **Scope:** the table load (cshell+0x2974d0), the fire routine (cshell+0x23bca0, not in the clean output: read by our own capstone
  disassembly of the ROM CShell.dll), its fire-vector helper (cshell+0x234d50) and quaternion axis helpers (cshell+0x1b800, +0x1b850, +0x1b8a0),
  the cone (cshell+0x176cb0, disassembled to recover the x87 operands the clean C hides), the fire gate of the weapon update (cshell+0x237180),
  the writers of the seed counter. Not covered: the spread update law (CLI-61..64), the aim mark, the press function cshell+0x23b050 (O-10).
- **Evidence sources:** the clean decompiler output (`CShell.dll`, image base 0x37100000, cited `cshell+0xRVA`), our own
  read-only capstone disassembly of the ROM CShell.dll (tag DISASM), the observed captures `captures/*` (the 0x82 fields; the
  0x7d of the server), the specs `WEAPON_FIRE_2009.md`, `GAME_MESSAGES_2009_C2S.md` 4.1, `GAME_MESSAGES_2009_S2C.md` (0x7d, 0xda),
  `MOVEMENT_2009.md` 5.1. no text from the reference stack, no forbidden source. No ROM value appears here: the table is described by its size, its hash and its
  generator; weapon rows are named by key, never by value.
- **Status:** DRAFT 1, capture-replayed. Written 2026-10-25. Test: `crates/dc-dserver/tests/fire_cone_cap.rs` (capture-gated, ROM-gated part).

Grades as in `WEAPON_FIRE_2009.md`; in addition **PROVEN-REPLAY** = the stated algorithm reproduces the directions the real 2009 client sent, pellet
for pellet, in the captures (section AC-5).

## AC-0. Summary

1. **The table is fixed** (PROVEN, DISASM + PROVEN-REPLAY). It is not generated at run time and not seeded per session: the shell loads it once at
   start-up from a ROM attribute entry (`Attributes\randomtable.txt`, tag `RandomTable`, keys `Random0` ... `Random9999`), 10000 single-precision
   values in [0, 1). Every client with this ROM holds the same table. The values themselves were produced (offline, by whoever wrote the file) by
   the C runtime's classic `rand()` from its default state, scaled by 1/32766 (AC-1.3): a test can rebuild the table without the ROM.
2. **The seed is the 0x82 `stat` field, and the server sets it** (PROVEN, DISASM + PROVEN-REPLAY): the fire routine hands the cone the address
   `local_player + 0x1fc`; the cone reads `table[seed mod 10000]` twice per pellet (right axis, then up axis), adding 1 after each read; the 0x82
   carries the counter after the pellet's two reads. The counter is written by the server's `GS_MATCH_SETTINGS` 0x7d (`own_value`, := ) and
   `GS_PLAYER_POINTS` 0xda kind 1 (+= `delta`); both captures' servers sent 0, so every link starts at `stat` 2.
3. **Replay: 3807 of 3807 pellets** of 49 real-client links (34 weapons, 1, 8-pellet and zoomed shots) reproduce to 1.7e-7 rad from `aim`, `stat`,
   `shot_scale`, `frac` and the weapon row (AC-5). The cone is a square in the aim's tangent plane, the half-width is one integer in thousandths for
   both axes, and there is **no duck and no mod Min/Max term in the shot** (those live only in the aim-mark / debug bounds, cshell+0x2334f0).
4. **The 0x82 `moving` bit is the zoom flag** (PROVEN-REPLAY, 13 zoomed shots of a scoped weapon: they replay only with the row's zoomed bounds).
   **`shot_scale` is the scale the cone used** (weapon+0x20c, written by the fire routine right before the loop).
5. Consequence: a server that tracks each player's counter can recompute every pellet direction except for one integer (the half-width), and can
   bound that integer from the weapon row and the reported spread (AC-6). `dc-client`'s disk cone (CLI-31) is replaced by AC-2 (AC-7).

## AC-1. The random table

AC-1.1 **Load (DISASM cshell+0x2974d0, method slot 0 of the table manager, class table VA 0x374f7750).** Called once by the shell's
database start-up (clean C cshell+0x29e740: the manager is created by cshell+0x29e640, size 0x15c0, and initialised with the entry name
`Attributes\randomtable.txt`; on a failure the start-up reports an error and returns 0). The method returns at once if the global pointer
cshell+0x4b2af0 is already set; otherwise it opens the entry, stores itself in the global, reserves 10000 entries (immediate 0x2710 at
cshell+0x297538) in a vector whose element pointer is at +0x15b0, sets the count +0x15bc := 0, then loops: key = `"Random" + count` (format
`%s%d`, VA 0x374d0f58, with the strings VA 0x374f7760 / 0x374f7768); while the tag `RandomTable` has that key, read it as a float (cshell+0x2993f0),
store it at `data[count]`, count += 1. The table is therefore exactly the contiguous run `Random0 ...` of the entry, and its size is the count.
Nothing reseeds, shuffles or refills it later: the global has four references in CShell (this store and test, and the two reads in
cshell+0x176cb0), and the manager's own methods are this load, an empty method (cshell+0x297390), the destructor (cshell+0x2974b0 ->
+0x297460) and a base method of the attribute-file manager (DISASM; the constructor cshell+0x2973e0 zeroes the fields).

AC-1.2 **The ROM's table** (ROM-gated test `rebuilt_table_equals_the_rom_entry`): 10000 entries, all in [0, 1); SHA-256 of the 40000 bytes of
the entries as little-endian binary32 = `c25eed27d064c4331cb91d8edae4c28276ca3103218c42c280c5c52b160ba386`.

AC-1.3 **Generator (identified, OURS).** The entries equal, 10000 of 10000 as binary32, the values `v_i = f32( r_i x f32(1/32766) )` written with
six significant digits and parsed back, where `r_i` is the i-th output of the C runtime's classic linear congruential `rand()` from its default
state 1 (`state := state x 214013 + 2531011 mod 2^32`, output `(state >> 16) & 0x7fff`). This is a statement about the data file, not about the
client's code (the client only reads the file). Use: a test can rebuild the table without committing it; a DeCombat component that needs the
2009 table still reads the user's ROM entry at run time (the ROM model; a mod may replace the entry).

AC-1.4 **Read (PROVEN, cshell+0x2973a0; operand recovered by DISASM).** `read(&seed)`: if `seed >= count` then `seed := seed mod count` **and the
reduced value is stored back**; the function loads `data[seed]` onto the x87 stack (`fld dword [data + 4 x seed]`; the clean C shows only the
address). The caller adds 1 to the stored seed after each read. Wrap rule (follows): with count N, a counter at N - 1 before a pellet reads N - 1,
then N reduced to 0, and the message carries 1; a counter at N reads 0 and 1 and the message carries 2. The +2 per message of the captures holds
only while the counter stays below N (max `stat` seen: 560).

AC-1.5 **The seed counter (PROVEN).** The address handed to the cone is `local_player + 0x1fc` (DISASM cshell+0x23c000..0x23c00e: the player
manager pointer cshell+0x49bf60 plus 0x1fc is pushed as the second argument of cshell+0x176cb0). Writers of that field in CShell: the server
message `GS_MATCH_SETTINGS` 0x7d (clean C cshell+0x2c430, case 0x7d: the third read, 32 bits, is stored to it; `GAME_MESSAGES_2009_S2C.md` calls
it `own_value`) and `GS_PLAYER_POINTS` 0xda kind 1 (`+= delta`, cshell+0x61521..0x61539, S2C row). The C2S 0x82 `stat` is a read of the same
field after the loop body of that pellet (C2S 4.1 layout). Captures: every one of the 49 real-client links sent `stat` 2 first (own_value 0 in
the 0x7d its server sent, checked by the test for five captures), then +2 per message across deaths, respawns and weapon changes (3758 steps, all
+2). Proposed renames (spec fold): 0x7d `own_value` -> `shot_seed`; 0xda kind 1 -> "advance the shot seed". Whether the 2009 server sent non-zero
values is unknown (no capture of a 2009 server exists).

## AC-2. The cone, exact (cshell+0x176cb0; DISASM for the operands, PROVEN-REPLAY)

Inputs (the fire routine's path record P, AC-3.3): weapon id, `fwd` (the start direction), `up`, `right`, the two spread fractions `f_r` (+0x2c)
and `f_u` (+0x28), the zoom byte (+0x30), the triple-shot byte (+0x31), `scale` (+0x34), two zoomed addends (+0x3c min, +0x40 max; always 0 from
the fire routine), and the seed address. Row offsets as in `WEAPON_FIRE_2009.md` (keys named, values from the ROM at run time).

AC-2.1 **Bounds.** `lo := row MinPerturb (+0x288)`, `hi := row MaxPerturb (+0x28c)` (integers).
- zoom byte set and row ZoomedMinPerturb (+0x298) non-zero: `lo := trunc(ZoomedMinPerturb + P.min_addend)`; else triple byte set and row
  TripleShotMinPerturb (+0x330) non-zero: `lo := TripleShotMinPerturb`;
- the same for `hi` with ZoomedMaxPerturb (+0x29c) / P.max_addend and TripleShotMaxPerturb (+0x334);
- `lo := trunc(lo x scale)`, `hi := trunc(hi x scale)`.

AC-2.2 **Gate.** Only if `lo <= hi` and `hi > 0`; otherwise nothing is drawn and the counter does not move (never observed).

AC-2.3 **Half-widths.** `range := hi - lo` (integer, converted to binary32). For the right axis `h_r := trunc( trunc(range x f_r) x scale + lo )`;
for the up axis `h_u` the same with `f_u`. The fire routine passes the same value (the weapon's current spread) for both fractions, so
`h_r = h_u = h` in practice (PROVEN-REPLAY: the implied half-widths of the two axes are the same integer in 3807 of 3807 pellets). This answers
O-3: the scale multiplies both bounds and then the truncated range product; `lo` is added already scaled.

AC-2.4 **Offsets.** `r1 := read(&seed)`, seed += 1, `o_r := (r1 x 2 h_r - h_r) x 0.001` (binary32 constant at cshell+0x3ce5cc), stored as binary32;
`r2 := read(&seed)`, seed += 1, `o_u := (r2 x 2 h_u - h_u) x 0.001`. New direction `normalize(fwd + o_r x right + o_u x up)` (the helper
cshell+0xb5ab0 is the normalisation: square root of the sum of squares, reciprocal, three multiplies).

AC-2.5 **The square.** Each offset is uniform over the table's values in `[-h, +h) / 1000` tangent units, the two axes independent: a square
cone. No duck, crouch, movement or mod Min/Max term appears in AC-2.1 .. AC-2.4 (the only reads of the row are the six bound fields).

AC-2.6 **Precision (PROVEN-REPLAY).** The x87 runs at 24-bit precision during the game (the usual Direct3D state): every product above rounds to
binary32 before the truncation. Evidence: a weapon whose two bounds are equal and whose scale is the binary32 nearest to 1.3 fires with a
half-width one above what extended precision gives (`trunc(n x 1.29999995)` in extended precision is one less than when the product first rounds
to binary32); 280 of 280 of its pellets replay only with binary32 rounding. A reimplementation does every step in `f32`.

## AC-3. The fire routine (cshell+0x23bca0, method slot +0x160; DISASM)

AC-3.1 **Entry.** Clears the bytes the 0x82 sends as `has_extra` (+0x1cd) and `flag_c` (+0x1cc). If the ammunition row's first field (`[weapon+0xa4]`) is 0x1a the routine returns
at once (a kind that does not fire).

AC-3.2 **Scale.** Over the three mod slots (method slot +0x11c with 0, 1, 2): `dpf_mods += mod DynamicPerturbFactor (+0x260)` and
`min_mods += mod MinPerturb (+0x270, integer)`. `scale := row DynamicPerturbFactor (+0x274) + dpf_mods`, stored at **weapon+0x20c** (the 0x82
`shot_scale`). Then one movement factor, chosen from the mover (`[player_manager + 0x10]`, `MOVEMENT_2009.md`):
- mover+0x80 set (airborne after a jump): row `DynnamicPerturbFactorJump` (+0x280; the key strings carry the ROM's spelling);
- else control word (mover+0x24) bit 0x800 `ANY_MOVE`: row `...Walk` (+0x27c);
- else bit 0x200 `RUN`: row `...Run` (+0x278);
- if the chosen factor is non-zero (compared with the binary32 0.0 at cshell+0x3cda90) and the scale is below it, `scale := factor` (stored at
  +0x20c). So `scale = max(DPF + mods, factor)` when a factor applies; the factors' binary default is 1.0 (cshell+0x17f7b0) and no ROM row sets the
  three keys.
- Capture check: `shot_scale` equals the row's DynamicPerturbFactor for every replayed weapon whose factor is at least 1.0; two weapons whose
  row factor is 0.0 sent 1.0 (one shot each), as the max rule predicts when a movement factor of 1.0 applied (PLAUSIBLE: their control words were
  not checked). The O-2 reading of the original spec ("replaced by") is corrected to "raised to".

AC-3.3 **Shot frame.** Current spread := method slot +0x44. The fire-vector helper cshell+0x234d50 returns the shot origin, the rotation
(quaternion, the 0x82 `aim`) and its three axes: with the player-manager camera path (camera object at player_manager+0x12c) the axes come from
the helpers cshell+0x1b800 (`right` = rotated +X), cshell+0x1b850 (`up` = rotated +Y), cshell+0x1b8a0 (`fwd` = rotated +Z) of the camera
rotation; the other path uses the weapon's own rotation (+0x14c .. +0x158) with the same formulas inline. Path record: weapon id, `up` -> P+0x10,
`right` -> P+0x1c, both fractions := spread, zoom byte := `player_manager+0xf0 > 0 or player_manager+0xf4 != 0` (**the same test as the 0x82
`moving` bit**), triple byte := the current weapon's method slot +0xf8 (the 0x82 `wflag`), scale := weapon+0x20c, P+0x38 := `min_mods` (not read
by the cone), the zoomed addends := 0. The weapon's word +0x144 := 0x900.

AC-3.4 **Pellets.** `n := row VectorsPerRound (+0x2b0)`; if mod slot 2 exists: `n += trunc(n x mod VectorsPerRoundPer (+0x244))` (answers O-8:
the "count" is the row's own VectorsPerRound). For each pellet `i` in `0 .. n`:
1. `P.dir := fwd` (every pellet starts from the unperturbed forward axis);
2. cone (AC-2) with the seed address `player_manager+0x1fc` (two reads);
3. by the ammunition row's kind (+0x0c): 3 -> the projectile path (cshell+0x23ab90); 0 -> the hitscan trace cshell+0x23a910 (range
   `trunc(row Range x (1 + mod slot 0 RangePer (+0x234)))`, DISASM cshell+0x23aa06..0x23aa32, confirming `WEAPON_FIRE_2009.md` 5.2); 1, 2, 5 -> no trace;
   4 and above 5 -> an error line ("Invalid Ammo Type");
4. send: method slot +0x174 (the 0x82 sender) with `a2` = the spread, the origin, the rotation, the pellet direction, the hit, and
   `last := (i == n - 1)`. So the order per pellet is cone, trace, send, and **`stat` is the counter after that pellet's two reads** (PROVEN).

AC-3.5 **After the loop.** The cone-bounds function cshell+0x2334f0 (with duck, mod and zoomed folds) is called **only for a debug console line**
(format `Perturb: C[%.2f] - Min[%d] Max[%d] Lerp[%.2f]`, VA 0x374f1b14, printed when a console value compared with 0.5 is above it); its bounds do
not reach the shot. Then the shot time (engine slot +0x6c) is stored at weapon+0x21c and the shot effect is started (cshell+0x2a2b50 with kind 6, or
8 / 9 by the ammunition and mode tests at cshell+0x23c11f..0x23c16d).

## AC-4. The fire gate of the weapon update (cshell+0x237180, clean C; offsets of the keys DISASM in the row loader cshell+0x17bd20)

The update asks the state handler (method slot +0x14c) with a "fire wanted" byte; a return of 4 means "fire now". The wanted byte comes from the
fire key handlers (slots +0x164 .. +0x170) and is then cleared or forced:
- **RapidFireCount** (row +0x3e0, key `RapidFireCount`): if it is above 0 and the weapon's shots-this-hold counter (+0x1c4) has reached it, no fire
  (1 = semi-automatic). The counter is incremented on every fire.
- **Triple-shot** (weapon byte +0x1f8 on): while the burst counter +0x1fc is 1 or 2 the fire is forced on; after the third shot the counter resets,
  a lock byte +0x201 is set and the time is noted (+0x204, `GetTickCount`). While the lock is set, firing waits until
  `now - (+0x204) >= TripleShotDelay (+0x3d4)` **and** `now - (+0x208) >= TripleShotSelectDelay (+0x3d8)` (milliseconds; +0x208 is a second
  time stamp, PLAUSIBLY the select time by the key name); then the lock clears.
- **ContinuousFire** (row byte +0x3e4): with the fire key down and method slot +0x100 true, the fire is forced on.
- On "fire now": three per-shot offsets are drawn from a separate random helper (cshell+0x29eb80, uniform in `[-a, a]` for three row floats at
  +0x2a4 / +0x2a8 / +0x2ac, handed to cshell+0x232c10 / +0x2348d0; PLAUSIBLY the view kick; not the cone table), the shot time is stored (+0xc0)
  and the fire routine (slot +0x160) runs when cshell+0x2f0840 returns 0; otherwise only a shot effect is played (meaning of that test
  UNPROVEN).
- The automatic cadence itself is the fire animation (C2S 4.1: the animation key handler sets the pending byte through cshell+0x237130 in state 3);
  the update has no per-weapon delay for plain automatic fire. CLI-29 / CLI-30 stand; DeCombat can add RapidFireCount (done: CLI-30) and the
  triple-shot delays from the row.

## AC-5. Replay (PROVEN-REPLAY)

Method (scratch tool, then the Rust test): for each 0x82, the axes of its `aim`; the observed tangent offsets `a = dir.right / dir.fwd`,
`b = dir.up / dir.fwd`; the draws `r1 = table[(stat - 2) mod N]`, `r2 = table[(stat - 1) mod N]`; the integer `h` implied by the better
conditioned axis; the predicted direction from AC-2.4; a match when the angle is below 2e-5 rad. With the ROM, `h` is also required to lie in the
interval AC-2.1 / AC-2.3 gives for `frac / 255 <= spread < (frac + 1) / 255` and the message's `shot_scale` (binary32 arithmetic). A link is the
real client's when its non-zero pellets replay; the other links are DeCombat bots (no cone at all) or `dc-client` (its CLI-31 disk) and replay none.

| captures (all with the real 2009 client) | links | pellets | replayed | in the row interval | notes |
|---|---|---|---|---|---|
| against the reference stack: tdm1, tdm2, tdm4, tdm6, menu-frida6, menu-frida8, xhair2-frozen | 7 | 293 | 293 | 293 | weapons 0, 7, 125 (8 pellets per shot), 145, 164 |
| against our Rust stack: rust3, rust4, rust5-auto, rust6, rust7, the weapon sweeps sweep-0 .. sweep-29 and sweep-145 / sweep-164, xhair2-rust, sbs1-real | 41 | 3513 | 3513 | 3513 | 34 weapon ids in all; the 13 zoomed shots (rust7) |
| menu-frida7 (no README) | 1 | 1 | 1 | 1 | |
| **total** | **49** | **3807** | **3807** | **3807** | worst angle 1.7e-7 rad; 65 bot / dc-client links (3035 messages) replay none |

Per weapon (pellets = replayed): 0: 52, 1: 21, 2: 280, 3: 120, 4: 120, 5: 185, 6: 28, 7: 8, 8: 1, 9: 1, 10: 30, 11: 120, 12: 120, 13: 120, 14: 1,
15: 5, 17: 175, 18: 1, 19: 1, 20: 120, 21: 120, 22: 43, 23: 280, 24: 45, 25: 120, 26: 190, 27: 37, 28: 3, 29: 3, 125: 80, 143: 39, 144: 76,
145: 1033, 164: 229.

Controls: the zoomed shots replay with the zoomed bounds (error <= 1.1e-7) and miss by 0.03-0.11 rad with the plain bounds; the equal-bounds
weapon of AC-2.6 replays only with binary32 rounding. The Rust test (`crates/dc-dserver/tests/fire_cone_cap.rs`) replays five captures
(tdm1, tdm2, tdm4, tdm6, rust7: 384 pellets, 384 replayed, 384 in the row interval, first `stat` = 0x7d `own_value` + 2 on each link) with the
table rebuilt by AC-1.3, and checks the rebuilt table against the ROM entry when `CA_ROM` is set.

## AC-6. Consequence for `dc-dserver`: server-side shot-direction validation

What the server needs, per player:
1. **The table**, read from the user's ROM at run time (`dc-bute`, entry `ATTRIBUTES\RANDOMTABLE.TXT`, AC-1.1 rule: contiguous `Random<i>`).
2. **The counter**: the value it sent in 0x7d `own_value` (today `gs.rs` sends 0) plus every 0xda kind 1 `delta` it sent, then per received 0x82
   the reads of AC-1.4 (two per pellet, with the store-back reduction). Expected `stat` = the counter after the pellet; a mismatch means a lost,
   forged or reordered pellet (0x82 travel reliable, C2S 4.1). The server may choose a fresh `own_value` per match (a DC-DEFAULT proposal, not yet a DC_DEFAULTS.md row);
   a client knows its own seed in any case, so this does not hide the cone from a cheat, it only makes the server's expectation exact.
3. **The frame**: `aim` (the rotation) gives `fwd`, `up`, `right` (AC-3.3). The aim itself is client-chosen; validation cannot catch aim
   assistance, only a direction that does not follow the cone ("no spread", a forged `dir`).
4. **The check**: compute `a`, `b` as in AC-5, the draws from the expected counter, the implied integer `h`; accept when the angle error is below
   a tolerance (observed 1.7e-7 rad, so 1e-5 is generous) and `h` lies in the row interval for the claimed `frac` and `shot_scale`, and
   `shot_scale` is one of the values AC-3.2 allows for that weapon and its mods (DPF + mods, or a movement factor above it). `frac` itself is the
   client's spread state; bounding it needs the spread law (CLI-61..64, not 2009-exact yet), so a first server checks only the interval for the
   claimed `frac`.
5. **Hit claims** then use the validated `dir` (`origin + hit_dist x dir`, C2S 4.1); `extra_a` must equal that point.
6. Cost: two table lookups, three dot products and a normalisation per pellet.
Status: not implemented; this is the specification for a DSV row (policy is DeCombat's: reject the pellet, or log first).

## AC-7. Consequence for `dc-client`

- Replace the uniform disk of CLI-31 (`arms.rs`) by AC-2: per pellet, two reads of the ROM table through the client's own counter, the square
  offsets, `h` from the row's six bound fields (no duck term), the scale of AC-3.2, every step in `f32`; reset each pellet to the forward axis.
- The counter: set by the server's 0x7d `own_value`, advanced by 0xda kind 1, two reads per pellet with the AC-1.4 reduction; send it as `stat`
  after the pellet (today `STAT_STEP` counts messages, which matches only while the server sends 0 and the counter stays below N).
- `shot_scale` := the scale actually used (AC-3.2), not the row value.
- Zoom: the `moving` bit is the zoom state; set it while the scope is up and use the zoomed bounds.
- Unaffected: the aim mark (it uses the cshell+0x2334f0 bounds with the duck and mod terms, `spread.rs` `mark`), the spread fraction law
  (CLI-61..64), the hit test.
- DC_DEFAULTS: CLI-31's cone shape becomes `EVIDENCE: cshell+0x176cb0` (this addendum); its range default is already corrected by `WEAPON_FIRE_2009.md` 5.2.

## AC-8. Corrections to `WEAPON_FIRE_2009.md` (for the next spec fold)

| place | was | now |
|---|---|---|
| 2.1 / 2.3 | bounds DISASM (auditor), scale placement UNPROVEN | AC-2.1 / AC-2.3, DISASM + PROVEN-REPLAY |
| 2.3 | scale "replaced by" the Run / Walk / Jump factors | raised to the factor when it is larger (AC-3.2); Jump = airborne, Walk = `ANY_MOVE`, Run = `RUN` without `ANY_MOVE` |
| 2.5 | uniform "if the table value is uniform in [0, 1)" | the table is a fixed 10000-entry ROM list in [0, 1) (AC-1) |
| 3.2 | table fill not read | AC-1.1 (load), AC-1.3 (generator of the data) |
| 3.3 | `stat` = seed PLAUSIBLE; reset UNPROVEN | PROVEN (AC-1.5); set by 0x7d, advanced by 0xda kind 1 |
| 4 (intro) | 2334f0 gives the cone bounds | 2334f0 feeds only the debug line in the fire routine (AC-3.5) and the aim mark; the shot uses AC-2.1 |
| 4.1 | `moving` PLAUSIBLY zoom | PROVEN-REPLAY (AC-0 item 4) |
| 5.4 | pellets `+ VectorsPerRoundPer x count` | `n + trunc(n x VectorsPerRoundPer)` of mod slot 2 (AC-3.4) |
| C2S 4.1 | `moving` | rename `zoom`; `stat` = the shot seed counter; S2C 0x7d `own_value` = `shot_seed` |

## AC-9. Open items

| id | item | next step |
|---|---|---|
| O-1 / O-7 | the x87 operands of cshell+0x2334f0 (the aim-mark / debug bounds and their mod folds) | disassemble when the aim mark is reworked; not needed for the shot |
| O-2b | the movement-factor branch was never exercised with a factor above the scale (no row sets the keys; two weapons with a 0.0 factor support it) | a modded row in a live test |
| O-5b | non-zero `own_value` / any 0xda kind 1 never captured | a dc-dserver run with a non-zero 0x7d value and the real client; expect first `stat` = value + 2 |
| O-6b | writers of player_manager+0xf0 / +0xf4 (which input sets the zoom state) | not needed for the cone |
| O-10 | press function cshell+0x23b050 (no clean C) | second disassembly read |
| O-13 | the wrap at the table size (AC-1.4) is inferred from the read function, never captured (`stat` max 560) | a long session or a large `own_value` |
| O-14 | triple-shot fire (`wflag` 1) never captured; its bounds AC-2.1 untested by replay | a capture with a triple-shot weapon |
| O-15 | the camera-path / weapon-path choice of cshell+0x234d50 (which test selects the camera) | read cshell+0x2f0820 |
