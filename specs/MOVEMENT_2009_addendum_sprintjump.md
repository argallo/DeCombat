# MOVEMENT_2009 addendum (sim-sprint-jump): the speed rows per held weapon, the jump-key rule

Status: staged by the sim-sprint-jump worker (2026-10-26) for the next spec-fold, as corrections to §3.2-§3.4,
§5.3, §7 and §16.2 (V-M1, V-M4) of MOVEMENT_2009.md. Our words; evidence by RVA in the clean decompiler tree
(`CShell.dll/<rva>.c`) and capstone disassembly of the user's CShell.dll (DISASM).
Implemented in dc-sim (`loadout.rs`, `player.rs`) on branch `sim-sprint-jump`. Key and section names are the 2009
readers' own names; every value is the user's ROM data, read at run time.

## S1. §3.3 / §3.4 / §3.2: what the speed records are (PROVEN; replaces "weapon records", "UNPROVEN which keys")

The records §3.3 calls weapon records (stride 0x49c, base table db+0x15b0) are the BODY rows of the model table
(cshell+0x1e4760): `Model<n>`, then `AIModel<n>`, then `Overidable_Model<n>` in one array whose count ends as the
sum of the three (db+0x15ac); the override slot of §3.3 is `Overidable_Model<slot>` (index `models + AI models +
slot`, db+0x15b4). Their keys: `MovePenalty` -> +0x408 (§3.3 `weapon_speed_mod`), `DashSpeed` -> +0x438
(`weapon_sprint_mod`), `FigureType` -> +0x48. The index the mover passes (loadout +0x4c) is the body row.

- §3.1 figure records are the `FigureType<n>` rows: `VariableSpeed` +0x24, `VariableJump` +0x28,
  `VariableWeight` +0x30, `DashSpeed` +0xe4 (default 1.4).
- §3.2 "gear" records are the `Hair<n>` rows (head gear) of the same table: `MovePenalty` +0x74, `DashSpeed` +0x7c.
- §3.4 the second database is the attachment table (cshell+0x1dfc0a): `Backpack<n>` rows `MovePenalty` +0x120,
  `DashSpeed` +0x128 (index inventory +0x170), `Goggle<n>` rows `DashSpeed` +0x128 (index inventory +0x174).
- `held_item_mod` (§7 step 5) is the held weapon row's `MovePenalty` (weapon-table reader, DISASM cshell+0x17dbd5:
  key pushed before the read into row +0x2d8, the previous value as default; `ZoomedMovePenalty` +0x2e4 follows
  and is not read by the speed path).

So the §7 formulas read, in ROM terms:

- `p = clamp(weapon.MovePenalty + body.MovePenalty + figure.VariableWeight + backpack.MovePenalty +
  head.MovePenalty - figure.VariableSpeed, -1, 1)` (cshell+0x5c270, that order, x87 sum rounded once);
- sprint `s = (body.DashSpeed + head.DashSpeed + backpack.DashSpeed + goggle.DashSpeed + 1) x run_speed x
  figure.DashSpeed` (cshell+0x1fd390), and every stance `x (1 - p)`.

The held weapon therefore changes run AND sprint by its own `MovePenalty`; the sprint factor belongs to the
character (in the user's ROM every body row carries the same dash term and the figure rows a factor of 1, so the
sprint is 1.4 x run for every weapon).

## S2. §16.2 V-M1: the plateaus are explained (PROVEN-CAPTURE)

Median reported speed of the on-ground, forward, steady updates of the real 2009 client, per held weapon, in tdm1,
tdm4, tdm6 (reference server) and rust4, rust6 (our server), body row 0 (both servers' default): weapon 0 run 255.4 /
sprint 356.3; 145 and 125: 227.0 / 316.6; 7, 143 and 164: 246.9 / 344.4; 27: 187.0 / 261.1. Against the ROM rule of
S1 (body row 0, no gear) the difference between two weapons is `-(p_a - p_b) x stance speed` to within 0.3
units/s in every pair of every capture (dc-sim test `rom_speed.rs::capture_replay_matches_the_rom_speed_rule`,
ratios to weapon 0 within 0.5 % in `rom_rows_give_the_2009_speed_per_weapon`); the absolute plateaus sit 0.4 %
under `(1 - p) x 285` and `(1 - p) x 399` (the client's report at 1-2 ms frames). The realclient capture
(20261025_204341, the real client on our stack with a different character) shows the same per-weapon differences
on a body term of about -0.08 and a sprint factor of about 1.33 (its loadout's rows; not identified).

## S3. §5.3: the "equipment state check" is a jump-key-held flag; mover+0x84 is found (PROVEN)

- The object at VA 0x3759e110 keeps a byte flag at +0x188 (slot +0x11c writes it, slot +0x120 reads it: DISASM
  cshell+0x64180 / cshell+0x64190; zeroed by its constructor cshell+0x63e60; no other user). mover+0x468 is written
  only by the mover's constructor (0, cshell+0x1fdba0).
- In the sampling branch that runs only on the ground, off a ladder, not airborne after a jump and not crouched
  (DISASM cshell+0x1ffad3..0x1ffb75): jump command off -> flag := 0; on -> if the flag equals mover+0x468 (0) the
  flag := 1 and, with the standing box, `JUMP` is set. So one press gives one request; a key held across the
  landing, or pressed and released in the air (the branch does not run there), requests nothing until it is seen
  up on the ground and pressed again. With the latch's 0 -> 1 edge this makes `JUMP` a one-frame pulse (consistent
  with C2S: the bit is never caught by the 15 Hz sampler).
- mover+0x84 (the latch enable) := 1 when the player state turns alive (shell 0x67 state 1, cshell+0x1f97d0) and on
  the respawn path (cshell+0x1f4110). No timer gates a jump after a landing: neither the ground test
  (cshell+0x1ffec0) nor the jump notice (cshell+0x1ff0b0) keeps one, and the only console values the law reads for
  jumping are the hop set of §6.7 (`JumpMin`, `JumpDec`, `JumpInc`, `JumpDelayMulti`: the hop penalty recovers
  within about 0.58 s, inside a flat jump's 0.6 s flight).
- No server part: the client decides and reports (`[0x76][1][1]`); nothing in the client waits for a server answer
  to jump.

## S4. §16.2 V-M4 and the landing-to-jump gap (OBSERVED)

Chained jumps of the real client on flat ground (take-off notice to take-off notice, the landing estimated from
the ballistic flight to the height of the first on-ground update): the shortest ground times were 34 ms and 68 ms
(realclient), 88 ms (rust6), 165 ms (tdm1): a human re-press at the client's 1-2 ms frames, no fixed delay. The
Rust client of the user's session 20261026_064539 (before this change) re-jumped 16 ms (one 60 Hz step) after
landings with the key held (e.g. landing notice at 254.531 s, take-off 254.547 s): the bunny hop of the user's
finding 5. dc-sim now refuses a held or buffered key (tests `speed_jump.rs`).
