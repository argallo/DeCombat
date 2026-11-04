# MOVEMENT_2009: player movement, collision and camera of the 2009 client (clean-room)

- **Title:** MOVEMENT_2009, the local player's per-frame movement law, collision shape and world query, time step,
  prediction / server reconciliation, and camera rules of the 2009 EU-test client.
- **Scope:** what the 2009 client does every frame to turn input into motion of its own (client-side) player body:
  command sampling, the acceleration / speed law, the engine integrator (gravity, friction, ground handling), the
  swept-box world query and slide loop, step-up, containers (ladder, liquids, gravity volumes, currents), jump,
  crouch and sprint, what the client reports to the server and what the server can impose, and the first-person
  camera. Not covered: weapon spread / perturb rules (only named where they touch speed), animation playback,
  the server's own validation rules (GAME_RULES, DeCombat defaults), rendering.
- **Evidence sources:** decompiler output of the 2009 `CShell.dll` (`CShell.dll/<rva>.c`, image base
  0x37100000, cited `cshell+0xRVA`) and `Engine.exe` (`Engine.exe/<rva>.c`, image base
  0x00460000, cited `engine+0xRVA`); read-only capstone disassembly of the same ROM binaries where the C lost a
  `this` pointer, an x87 operand or a stack slot (marked DISASM); `*.funcs.orig.tsv` for sizes; sibling clean
  specs `UDP_TRANSPORT_2009.md`, `FMT_DAT85.md`, `FMT_LTB.md` and the in-progress drafts `UDP_SESSION_2009_A.md` /
  `UDP_SESSION_2009_B.md` (cross-references only). Public: IEEE-754 binary32, x87 / SSE semantics, `timeGetTime`
  (WINMM, milliseconds).
- **Status:** DRAFT 1, complete for review. Not implemented. What the client reports on the wire (sections 5.1, 12.2, 12.3) and several speeds, the gravity and the stop rule are checked against captures of the real client
  (tdm1, tdm2, tdm4: `OBSERVED: captures/<id>/capture.jsonl`, grade PROVEN-CAPTURE = the real client's own bytes, never downgraded; Appendix C); the integrator, collision and camera rules are not.
- **Date:** 2026-10-24

Evidence grades (same as `UDP_TRANSPORT_2009.md`): **PROVEN** = literally visible in the decompiler output or the
disassembly of the cited function; **PLAUSIBLE** = reconstructed where one step is not literally visible but the
reading is the only one consistent with the surrounding code; **UNPROVEN** = not visible, needs a capture, a live
test or a further read before an implementation relies on it.

Conventions. Units are world units (the BSP's units) and seconds. The coordinate frame is the engine's: +Y up,
gravity along -Y, and an object's local +Z axis is "forward", local +X is "right" (PROVEN from the basis
construction in cshell+0x2001c0, section 6.3, and the analog-stick mapping in cshell+0x1fc7f0, section 5.2).
`|v|` is the Euclidean length, `v.xz` the vector with y set to 0, `normalize(v)` divides by `|v|`.
All arithmetic in the original is binary32 stored, with x87 80-bit intermediates in CShell and SSE binary32 in the
engine integrator; see section 15 for what a deterministic port must reproduce.

Names. Every name in this spec is ours. String keys that the client looks up at run time are given as
`sha256(key)[:16]/length` of the ASCII key; the key itself is in the user's ROM (`Game/CShell.dll`) and is read
from there by the implementation, never copied into the tree.

## 0. Contents

1. Function map
2. Tunables: console variables and their hard-coded defaults
3. Tunables from ROM tables (character, gear, weapon records)
4. Time step
5. Input sampling and the control word
6. Velocity law (per-frame acceleration, speed cap, jump, knockback)
7. Stances and their speeds
8. The engine integrator (gravity, friction, ground)
9. Collision shape and the engine move / sweep
10. Ground detection, falling and landing
11. Containers: ladder, liquids, gravity volumes, currents, pushers
12. Prediction and server reconciliation
13. Camera
14. Stance / animation state as it drives movement
15. Determinism notes for the shared simulation crate
16. UNPROVEN items and validation plan

## 1. Function map

Our names: **mover** = the client-side local-player movement controller object of CShell (class table VA
0x374ef960, derived table VA 0x374d3204, singleton pointer at VA 0x3759c220); **controller** = the CShell player
object (class table VA 0x374eefc4, derived VA 0x374d327c, singleton pointer at VA 0x3759bf60; `UDP_SESSION_2009_B`
section 7.1 lists the same object as a shell-message handler stage); **body** = the client-side engine object that
the mover creates and moves (handle at mover+0x08); **phys** = the engine's client physics interface (CShell
pointer at VA 0x375964ec, engine method table VA 0x681858, instance VA 0x6d28c0); **shell** = the CShell
game-client object (pointer at VA 0x37597e50).

| role (our name) | rva | grade |
|---|---|---|
| mover constructor / reset of fields | cshell+0x1fdba0, cshell+0x1fcd80 | PROVEN |
| mover init: registers the movement console variables with defaults | cshell+0x1fdcb0 | PROVEN (+DISASM for the handle addresses) |
| mover per-frame update (`mover_frame`), calls the steps below in order | cshell+0x201990 (the decompiler filed the tail of this body under cshell+0x1fd740) | PROVEN |
| step 1: mark the server's copy of the own player object (object flag 0x8000) | cshell+0x1fc8e0 | PROVEN |
| step 2: re-sync the body after the server player object's model changed | cshell+0x1ff250 | PLAUSIBLE (purpose) |
| step 3: sample commands into the control word (`sample_input`) | cshell+0x2013f0 (input body shared with cshell+0x1ff980) | PROVEN |
| step 4: movement gate, then the velocity law | cshell+0x201470 -> cshell+0x2001c0 (`move_law`) | PROVEN + DISASM |
| step 5: freeze-in-place enforcement | cshell+0x1ff3f0 (56 bytes; the decompiler appended unrelated code) | DISASM |
| step 6: pushers (radial impulses) | the sixth callee of cshell+0x201990 (its rva is 0x201990 - 0x370) | PROVEN |
| step 7: stuck test / un-stick | cshell+0x1fe510, cshell+0x1fe560 | PROVEN |
| step 8: physics step (gravity, integrate, move) | cshell+0x1fe6c0 (`phys_step`) | PROVEN + DISASM |
| containers and friction refresh | cshell+0x1fe300, cshell+0x1fce10 | PROVEN |
| container drag (viscosity) | cshell+0x1fe060 | PROVEN |
| ground / landing / fall-start detection | cshell+0x1ffec0, fall probe cshell+0x1fe1f0 | PROVEN |
| slide-to-stop (no keys on the ground) | cshell+0x1fe940 | PROVEN (caller not identified, see 16) |
| base speed selector | cshell+0x1fd390 (`target_speed`), run-speed helper cshell+0x1fd2e0 | PROVEN |
| speed penalty fraction (derived mover, method +4) | cshell+0x5c270 (base class returns 0, cshell+0x1fc980) | PROVEN |
| stand-up test | cshell+0x1fcfd0 | PROVEN + DISASM |
| jump / fall / land notification to the server | cshell+0x1ff0b0, inline copies in cshell+0x1ffec0 | PROVEN |
| movement trailer of the player update (to server) | cshell+0x1fed20 | PROVEN + DISASM |
| server -> client mover update (shell id 0xa0) | dispatcher cshell+0x1f0470 -> cshell+0x1f27f0 (DISASM, not in the function table) -> cshell+0x1ff430 | PROVEN |
| teleport + new collision box | cshell+0x1feb90 | PROVEN + DISASM |
| server view-angle set (shell id 0x6a) | cshell+0x1f4ed0 | DISASM |
| controller state byte (shell id 0x67) | cshell+0x1f97d0 | DISASM |
| damage direction for knockback (shell id 0x74) | cshell+0x1f7d20 | PROVEN |
| shell frame-time clamp | cshell+0x1aa810 | PROVEN + DISASM |
| engine frame clock | engine+0x17460 (time source engine+0xaa7d0 = `timeGetTime` minus a base) | DISASM |
| engine: client physics method table (see 9.1) | engine+0xd6980 .. engine+0xd78b0 | DISASM |
| engine: integrator (`integrate`) | engine+0x6fe50 | PROVEN + DISASM |
| engine: world gravity record init | engine+0x18770, setter engine+0x16cd0 | PROVEN |
| engine: move object to a target (`move_to`) | engine+0x73060 | PROVEN (arguments partly lost, see 9.3) |
| engine: mover context setup (box from dims, step height) | engine+0x75110 | PROVEN |
| engine: move with collision (`sweep_move`) | engine+0x796e0 | PROVEN |
| engine: slide loop (`slide_move`) | engine+0x78650 | PROVEN |
| engine: stair variant of the slide (`stair_move`) | engine+0x78c50 | PROVEN (one retry PLAUSIBLE) |
| engine: single sweep against world + objects (`sweep`) | engine+0x78280 | PROVEN |
| engine: sweep against one object (box or world model) | engine+0x781c0 -> engine+0x77db0 (BSP walk) -> engine+0x77530 (polygon); engine+0x75860 (box) | PROVEN (BSP walk, polygon test shape), box test not read |
| engine: sweep against blocker polygons (capsule) | engine+0xc5660 (method +0x14 of the table at VA 0x67fd5c) | PROVEN |
| engine: ground probe after a move / touch pass | engine+0x79210 / engine+0x79450 | PROVEN / shape only |
| camera variables, view angles, recoil, eye position | cshell+0x1f6fb0, cshell+0x1f5420, cshell+0x1f3070, cshell+0x1f9c40, cshell+0x2763e0, cshell+0x1f2650 | see 13 |

## 2. Tunables: console variables and their hard-coded defaults

The mover registers 29 engine console variables at init (cshell+0x1fdcb0). Registration helper cshell+0x25620: a
3-word handle `{engine_var, client_interface, key}`; if the variable does not exist it is created by running the
console string `"<key>" "<default>"` (client interface slot +0x208), so **the default is the code constant below
unless a config file or the console set the variable earlier** (whether the ROM's own config files set any of
these is UNPROVEN; the implementation reads the ROM's configs the same way it reads any other console variable).
Reads go through the handle: client interface method +0x5c returns the float; cshell+0x17f90 is the "read or
return the argument" helper (all call sites pass 0). Our names, the key id, the handle address (where the mover
reads it) and the default (PROVEN, cshell+0x1fdcb0, DISASM for handle addresses):

| our name | key id | handle VA | default | used for (section) |
|---|---|---|---|---|
| `SPECTATE_MUL` | c6bd1496526498d5/17 | mover+0x2f8 | 1.0 | free-fly acceleration factor (6.4) |
| `FALL_DMG_MIN_H` | 135d840d1785b94d/19 | 0x3759c284 | 300.0 | fall damage (10.4) |
| `FALL_DMG_MAX_H` | 06ba6788017bdbf0/19 | 0x3759c290 | 700.0 | fall damage (10.4) |
| `FALL_DMG_MIN` | 2b8dff1a9c51171a/13 | 0x3759c29c | 0.0 | fall damage (10.4) |
| `FALL_DMG_MAX` | 01a679166f82d119/13 | 0x3759c2a8 | 200.0 | fall damage (10.4) |
| `LAND_MIN_H` | fabd8ed89051d2bd/16 | 0x3759c278 | 50.0 | landing camera dip (13.3) |
| `LAND_DIP` | 8801faa492c59c95/15 | 0x3759c2b4 | -15.0 | landing camera dip (13.3) |
| `LAND_DOWN_T` | e670b9fd1b5f2bef/15 | 0x3759c2c0 | 0.15 | landing camera dip (13.3) |
| `LAND_UP_T` | 8a472f8b32b68383/13 | 0x3759c2cc | 0.4 | landing camera dip (13.3) |
| `LAND_ROLL` | 384fbe4cfe3519c3/14 | 0x3759c2d8 | 5.0 | landing camera roll (13.3) |
| `LAND_ROLL_T1` | b57b2c58d0864a9d/16 | 0x3759c2e4 | 0.05 | landing camera roll (13.3) |
| `LAND_ROLL_T2` | 464e8e09049ce81d/16 | 0x3759c2f0 | 0.15 | landing camera roll (13.3) |
| `LAND_PITCH` | 612912e7940fc0ba/15 | 0x3759c2fc | 5.0 | landing camera pitch (13.3) |
| `LAND_PITCH_T1` | 25ba3d9cdbb7bf95/17 | 0x3759c308 | 0.05 | landing camera pitch (13.3) |
| `LAND_PITCH_T2` | 3d314bee3cfaee8d/17 | 0x3759c314 | 0.15 | landing camera pitch (13.3) |
| `GRAVITY` | 9fc54aebedf325f8/13 | 0x3759c224 | -1100.0 | gravity along Y during the player's physics step (8.1) |
| `AIR_CONTROL` | aa3dbdb8c30e496b/20 | 0x3759c230 | 0.1 | acceleration factor while airborne after a jump (6.4) |
| `STOP_TIME` | e1540b6551ef2934/15 | 0x3759c320 | 0.1 | slide-to-stop time (6.8) |
| `PUSH_MAX_VY` | ee35af542dfe1a69/18 | 0x3759c32c | 100.0 | cap of the upward pusher velocity (11.5) |
| `BASE_ACCEL` | 26681e47d6467679/13 | 0x3759c338 | 3000.0 | acceleration magnitude per input axis (6.4) |
| `START_ACCEL` | c1cf4960d3cbcaad/10 | 0x3759c344 | 500.0 | acceleration at the start of a move (6.5) |
| `MAX_ACCEL` | 3b5a63d58408748c/8 | 0x3759c350 | 3000.0 | ramp ceiling and total-acceleration cap (6.5) |
| `ACCEL_RATE` | a623e52f1d3b424e/8 | 0x3759c35c | 6000.0 | ramp rate, units/s^2 per second (6.5) |
| `WALK_SPEED` | 3e95ab5f9e1a9021/7 | 0x3759c368 | 70.0 | walking speed cap (7) |
| `RUN_FWD` | 4a43f777a29df2fd/7 | 0x3759c374 | 285.0 | running speed when moving forward (7) |
| `RUN_BACK` | 43949891ad381b62/7 | 0x3759c380 | 285.0 | running speed when moving backward (7) |
| `RUN_SIDE` | c533a0ac4f352a74/7 | 0x3759c38c | 285.0 | running speed when only strafing (7) |
| `JUMP_SPEED` | 8e7bb709f24b78a8/7 | 0x3759c398 | 330.0 | jump take-off vertical speed (6.7) |
| `CROUCH_SPEED` | 9d1fc487a2914389/7 | 0x3759c3a4 | 50.0 | crouched speed cap (7) |

Variables that the mover reads **by key at use time without registering them** (cshell+0x19fec0: look the
variable up by key through client interface slot +0x104, return its float, or the code default when it does not
exist; cshell+0x19fde0 is the same returning an integer). Defaults PROVEN at the call sites in cshell+0x2001c0:

| our name | key id | default | used for |
|---|---|---|---|
| `HOP_ACCEL_MUL` | 057b6d4ab8cfb153/14 | 1.0 | acceleration factor while the hop penalty is active (6.7) |
| `HOP_RECOVER` | fa038dec546fc9ad/7 | 1.2 | hop-penalty recovery per second (6.7) |
| `HOP_MIN` | a14ff406e351b762/7 | 0.3 | hop-penalty value after a jump (6.7) |
| `HOP_DECAY` | af1077a184078a3d/7 | 0.05 | further decrease per second for chained jumps (6.7) |
| `KNOCKBACK` | a810fdc1e1685274/9 | 0 (integer) | knockback strength (6.9) |

A further value is read through client interface slot +0x134 by key (default 30.0 passed by the caller):
`STEP_HEIGHT`, key id eda10d3a6476dd56/15 (cshell+0x1fe6c0, PROVEN). Slot +0x134 is PLAUSIBLY the read of a
server-pushed engine variable (`S2C_EVAR`, `UDP_SESSION_2009_B` section 8.6), so the server can set the step
height. When the value differs from 30.0 the mover writes it into phys (method +0x0c, engine+0xd6a20: stores the
float at instance+0x08); at phys creation the shell writes 30.0 (cshell+0x1a3910) over the engine's static initial
value -1.0 (instance VA 0x6d28c0 + 8, set by engine+0xd7130).

**Correction (spec-fold-8, from aa-triage-world-physics item 10 and aa-fix-world-physics):** slot +0x134 is the engine's
server-variable float read (engine+0xd760, EVIDENCE; resolved through the client method table at VA 0x674b08). It **stores 0
into the output before the lookup** and returns not-found when the variable is missing, so the caller's preset 30.0 does not
survive: without an `S2C_EVAR` for `STEP_HEIGHT` the value is 0, which differs from 30.0, and 0 is pushed into phys, so the
`step` of 9.7 becomes 0.5. The server must send the variable (the reference stack sent `30.000000` before `S2C_LOAD_WORLD` in tdm1 / tdm4
/ tdm6; `UDP_SESSION_2009_B.md` 8.6). PROVEN-CAPTURE of the effect: on the same SNOWVALLEY staircase (x -560..-370, z
1680..1800, 16-unit risers) the real client ran up on the ground against the reference stack (tdm4 seq 3741-3777, t 81.172-81.453, 24
units per update, no jump) and was pinned four times with control FWD + RUN and `on_ground` 1, needing jumps, against our
stack without the variable (rust4 seq 39278-39936, t 419.0-436.9; 31 blocked-while-running episodes in rust4, 7 in tdm4); the
staircase of rust3 / tdm6 (V-STAIR-1) fits the same cause. The other mover inputs were equal (0xa0 body block, friction 5.0,
box (15, 50, 15)). *Superseded:* `CAPTURE_rust3_RUSTSTACK_VALIDATION.md` 3.3 / 7.4 "absent and 30 behave the same"
(corrected there). DeCombat: DSV-53 (sent right before `S2C_LOAD_WORLD`, value text `30.000000`, the name found in the user's
CShell.dll by hash; PENDING). V-STAIR-2 (the real client walks and runs up both staircases without a jump) has not been run.

DeCombat note: the shared simulation crate takes all of the above as a parameter block (`MoveTunables`) with these
defaults and lets the server override them per match; nothing in this section is a ROM table, all are code
constants of the 2009 client.

## 3. Tunables from ROM tables

These values come from the user's ROM attribute files, read at run time; only the keys, record layouts and the
formulas are given here (values are ROM data). The character attribute database object is at VA 0x3759a20c (its
loader is cshell+0x1e4760); a second database is at VA 0x3759a118.

**3.1 Character figure records** (section prefix key b12ac8e7c12cfeec/10 followed by a decimal index, record
stride 0xec, count at db+0x15dc, array at db+0x15e0; loader cshell+0x1e4760, PROVEN). Fields used by movement:

| record offset | key id | getter | our name |
|---|---|---|---|
| +0x24 | 0780a543e0955b12/13 | cshell+0x1e1b00 | `fig_speed_bonus` |
| +0x28 | d07f022ab11c0ba1/12 | cshell+0x1e1b30 | `fig_jump_bonus` |
| +0x30 | c6ccb274eea10745/14 | cshell+0x1e1c80 | `fig_weight` |
| +0xe4 | a2dbeb5bdb3423a8/9 | cshell+0x1e1c20 | `fig_sprint_mul` (code default 1.4 when the key is missing, double constant at the call) |
| +0xd8, +0xdc, +0xe0, +0xe8 | (four integer keys) | | sprint gauge costs and stamina (not used by the movement law itself) |

**3.2 Gear / model records** (stride 0x94, count at db+0x15cc, array at db+0x15d0; loader cshell+0x1e4760 first
loop). Fields: +0x74 key 3a41a522aad9994b/11 (getter cshell+0x1e16c0, `gear_penalty`), +0x78 key
92630a4ab8739661/10 (stamina), +0x7c key a2dbeb5bdb3423a8/9 (getter cshell+0x1e1720, `gear_sprint_add`).

**3.3 Weapon records** (stride 0x49c, base table at db+0x15b0 with count db+0x15ac, per-instance overrides at
`(db+0x15b4 + slot) * 0x49c`, count db+0x15b8). Two fields are used: `weapon_speed_mod` at +0x408 (getter
cshell+0x1e2bc0: the override is used when `|override| > 0.01`, else the base record) and `weapon_sprint_mod` at
+0x438 (getter cshell+0x1e2cc0: override used when `> 0.1`). Their key ids are in the weapon loader, which this
spec did not read (UNPROVEN which keys).

**3.4 Second database** (VA 0x3759a118, record stride 300): `item_speed_mod_a` +0x120 (cshell+0x1de870),
`item_speed_mod_b` +0x128 (cshell+0x1de8d0) and `item_sprint_c` +0x128 of a second table at db+0x15d4
(cshell+0x1deb70). Their keys and the table identity are UNPROVEN (not read).

Which record indices apply (PROVEN by use in cshell+0x1fd390 and cshell+0x5c270): the mover's loadout pointer
mover+0x304 holds the figure index at +0x58 and the weapon base index at +0x4c; the controller's inventory object
(`controller->+0x1b8`, reached through cshell+0x1b1a10) holds the gear index at +0x150, the item indices for
`item_speed_mod_a` / `item_speed_mod_b` at +0x170 and for `item_sprint_c` at +0x174, and the weapon override slot
at +0x2ac; `held_item_mod` uses the equipped weapon id (slot +0xc0 of the object at VA 0x3759e110).

## 4. Time step

- **Engine clock** (engine+0x17460, DISASM): every client frame the engine reads `t_ms = timeGetTime() - base`
  (engine+0xaa7d0), stores `frame_dt = (t_ms - t_prev_ms) * 0.001` (the u32 difference converted as unsigned) and
  `time = t_ms * 0.001`. There is no clamp in the engine. Client interface method +0x70 returns `frame_dt`
  (engine+0xe530 reads client+0xa24), method +0x6c returns `time` (engine+0xe520 reads client+0xa28). So
  `frame_dt` is a whole number of milliseconds. The scale constant 0.001 is the binary32 value at VA 0x6731a0.
- **Shell clamp** (cshell+0x1aa810, DISASM): the shell copies `frame_dt` to shell+0x1770 and replaces it by 0.1
  (0x3dcccccd) when it is greater than 0.1. Call this `dt` (clamped).
- **Who uses which** (PROVEN per call site):
  - `dt` (clamped): the physics step (8), container currents and drag (11), slide-to-stop (6.8), pushers (11.5).
  - `frame_dt` (unclamped): the acceleration ramp (6.5) and the hop-penalty decay / recovery (6.7).
- One mover step per rendered frame, variable `dt`. There is no fixed-step sub-stepping on the client. A shared
  deterministic crate may run at a fixed step (DeCombat decision), but it then differs from the 2009 client by design;
  section 15 lists what changes.

## 5. Input sampling and the control word

**5.1 Control word.** `mover+0x24` holds this frame's control bits, `mover+0x28` last frame's (copied first, then
cleared, cshell+0x2013f0; only `CROUCH` is carried over when the body is missing). Bits (PROVEN, set in
cshell+0x1ff980 / cshell+0x2013f0 and tested in cshell+0x2001c0):

| bit | our name | set when |
|---|---|---|
| 0x00000001 | `FWD` | command 0 is on and mover+0xa8 (movement allowed) is set |
| 0x00000002 | `BACK` | command 1 is on and movement allowed |
| 0x00000004, 0x00000008 | `TURN_A`, `TURN_B` | no writer found in the mover (UNPROVEN; with `STRAFE_MOD` they would become `RIGHT` / `LEFT`) |
| 0x00000010 | `JUMP` | command 6 is on, see 5.3. **PROVEN-CAPTURE: it never appears in a control word sent on the wire** (0 of 2,338 + 1,844 + 1,103 updates although 27 + 13 + 26 jumps were made): the bit lives on the take-off frame only and the 15 Hz sampler never catches it; jumps are reported by shell 0x76 `[1][1]` (12.3) |
| 0x00000020 | `CROUCH` | command 5 is on, or command 0x5c is on, or the stand-up rule of 5.4 keeps it |
| 0x00000040 | `STRAFE_MOD` | command 9 is on |
| 0x00000080 | `LEFT` | command 10 is on and movement allowed |
| 0x00000100 | `RIGHT` | command 11 is on and movement allowed |
| 0x00000200 | `RUN` | (command 7 on) XOR (run-lock toggle mover+0x6c), only when not crouched |
| 0x00000400 | `FIRE` (fire held; formerly "(aim)") | command 8 on, command 2 off, not sprinting, and a weapon test (cshell+0x1f4080) false. **PROVEN-CAPTURE:** it is set exactly while the primary-fire button is held: tdm1's 39 runs coincide with the 0x82 bursts, tdm4's five updates bracket four rifle shots and the 2.2 s grenade hold; it never coincides with `SPRINT` (the bit is off while sprinting even though the hold continues), and `pu_flags` 0x2000 is set in the same frames. Command 8 is the primary-fire command |
| 0x00000800 | `ANY_MOVE` | any bit of mask 0x19f is set (`FWD`, `BACK`, `TURN_A`, `TURN_B`, `JUMP`, `LEFT`, `RIGHT`) |
| 0x00080000 | (command 0x3d) | command 0x3d on and not sprinting |
| 0x00400000 | `SPRINT` | see 5.5 |

Commands are the engine's numbered input commands (client interface member slot +0x204 = engine+0xf3f0, which
tests byte `cmd` of the engine's 255-entry command state, engine+0x151d0). Command 12 (0x0c) toggles the run lock
(`mover+0x6c ^= 1` on every frame the command reads as on; PROVEN as written, so a held key toggles every frame
unless the engine reports the command for one frame only, UNPROVEN). Which keys are bound to which command numbers
is configuration (read from the ROM's config files at run time).

**5.2 Analog input** (cshell+0x1fc7f0): when controller+0x140 is set, the client interface slot +0x1c fills an
axis pair `(ax, ay)`; `ax < 0` sets `LEFT`, `ax > 0` sets `RIGHT`, `ay < 0` sets `FWD`, `ay > 0` sets `BACK`. This
fixes the handedness: `LEFT` is -X, `RIGHT` is +X.

**5.3 Jump request** (DISASM cshell+0x1ffad3..0x1ffb75). `RUN` and `JUMP` are sampled only when `CROUCH` is
clear. `JUMP` is set only when mover+0x60 (on ground) is set, mover+0x30 (on ladder) is clear and mover+0x80
(airborne after jump) is clear, command 6 is on, an equipment state check passes (slot +0x120 of the object at VA
0x3759e110 equals mover+0x468; meaning UNPROVEN; when equal, slot +0x11c is called with 1, and with 0 when command
6 is off) and the body's current box height (phys method +0x20, `dims.y`) satisfies `stand_dims.y - 0.5 <=
dims.y` (the body is not in a crouched box; `stand_dims` = mover+0x32c..+0x334, section 9.2). The controller-state
rule (not 2, 3, 5 or 6) applies to crouch sampling here and to the jump itself in 6.7. A jump is **latched** (global VA 0x3759c21c := 1)
on the frame `JUMP` goes from 0 to 1 while mover+0x80 is clear and mover+0x84 is set (mover+0x84 has no writer in
the mover code read, UNPROVEN). While the crouch camera offset controller+0x108 (13.4) is non-zero, `JUMP` and
`SPRINT` are removed again (mask 0xffbfffef), so the player cannot jump until the eye is fully back up after
crouching.

**5.4 Crouch.** Pressing crouch records `GetTickCount()` at mover+0x34c (press edge only). When crouch is released,
the mover keeps `CROUCH` set unless at least 100 ms have passed since the press **and** the stand-up test
(cshell+0x1fcfd0) succeeds:
- if the body's current box height is greater than mover+0x324, the test succeeds at once (already tall);
- otherwise it remembers the position, tries phys "set box" (method +0x24) with `stand_dims` and the "must fit"
  flag 1, restores the old box with flag 0 and the old position (client interface slot +0xc4), and succeeds when
  the trial returned 0 (the standing box fits). mover+0x324 has no writer in the mover code read (UNPROVEN source).
- Crouch is not sampled while the controller state is 2, 3, 5 or 6.

**5.5 Sprint.** With `FWD` on, `RUN` on, command 0x77 on, a stamina value above 0 (from the object at VA
0x3759e110, slots +0xec / +0xe0), the derived mover's method +8 false and not on a ladder: `SPRINT` is set and the
sprint state is started with `GetTickCount()` (VA 0x3759e110 slot +0xcc). A frame without `FWD`, or with `BACK`,
clears `SPRINT`; a non-zero value from cshell+0x6f860 clears `SPRINT` and `RUN`.

## 6. Velocity law (`move_law`, cshell+0x2001c0)

Run once per frame from cshell+0x201470 when mover+0xa8 (movement allowed) is set and a shell flag
(`[VA 0x375acfb0]+0x15fc`) is non-zero; before it, mover+0x98 / +0x9c (gravity override, section 11) are
cleared. Otherwise cshell+0x201470 takes the **frozen branch** (DISASM): the current acceleration `a` becomes
`(0, a.y', 0)` with `a.y' = 0` when `a.y > 0`; when `a.y < 0`: `a.y' = 0` if the controller state is 2, 3, 5 or 6
and the body is not falling (mover+0x68 clear), else `a.y' = max(a.y, -1100)` (hard-coded -1100, VA 0x374efb54);
the velocity becomes `(v.x * 0.5, min(v.y, 0), v.z * 0.5)`; both are written every frame. All steps below are
PROVEN with DISASM unless marked.

**6.1 Modes.** Let `fly = mode in {1, 2, 4, 11}` (mode = mover+0xb0, copied every frame from controller+0xe0) **or**
on ladder (mover+0x30) **or** spectating (controller+0xac). `spectate = controller+0xac != 0`. `ladder =
mover+0x30 != 0`.

**6.2 Start of frame.**
1. Acceleration := (0,0,0) (phys method +0x3c).
2. Containers refresh (cshell+0x1fe300, section 11) and ground detection (cshell+0x1ffec0, section 10).
3. Body rotation := the controller's view rotation (cshell+0x1f5d10, then client interface member slot +0x228).
4. `v` := current velocity, `speed` := `target_speed()` (section 7).
5. Jump request `jr`: if `fly`, `jr = JUMP bit` and the latch is cleared; else `jr = latch && !airborne`
   (airborne = mover+0x80).

**6.3 Speed cap (before acceleration).**
- On a ladder: if `v.y > 200`, `v.y := 200` and the velocity is written back.
- Else if on ground (mover+0x60), not spectating and not `jr`: if `|v.xz| > speed`, the **whole** vector `v`
  (including y) is scaled by `speed / |v.xz|`.
- Else (airborne, spectating or jumping this frame): if `|v.xz| > speed`, only x and z are scaled so that
  `|v.xz| = speed`; y is kept.

Basis. If not spectating, not on a ladder and not `fly` by mode, the rotation `q = (x, y, z, w)` is the body's
rotation (client interface slot +0x9c), rotated about +Y by the angle at `loadout+0x7c` when mover+0x304 is set
(cshell+0x181a0); otherwise `q` is the controller's camera rotation (cshell+0x1f5cd0). From `q`:
`F = (2(xz + wy), 2(yz - wx), 1 - 2(x^2 + y^2))` (rotated +Z) and
`R = (1 - 2(y^2 + z^2), 0, 2(xz - wy))` (the X and Z components of the rotated +X; not renormalised).

**6.4 Per-axis acceleration `a0`.**
- Normal: `a0 = BASE_ACCEL` (0 when the variable is missing). The forward direction is `Fdir = normalize(F.x, 0,
  F.z)` when not `fly`; when `fly` and not on a ladder: `F` with `F.y := 0` if `F.y > 0`, then normalised (you can
  dive along the view but not climb with it); on a ladder `Fdir = F` unnormalised.
- Spectating: velocity := 0 first; `a0 = (RUN ? 600000 : 300000) * SPECTATE_MUL`.
- `jump_v = JUMP_SPEED * (1 + fig_jump_bonus) * hop` (hop = mover+0x35c, section 6.7).
- If `CROUCH` and not on a ladder: `a0 *= 0.5`.
- If airborne (mover+0x80) and not `fly`: `a0 *= AIR_CONTROL`.
- If the controller state is 3, steps 6.5 and 6.6 are skipped (the acceleration stays zero; 6.7 and 6.9 still run).
- `AF = Fdir * a0` (forward axis), `AR = R * a0` (strafe axis).

**6.5 Acceleration ramp** (only when not `jr` and not `CROUCH`). Two accumulators: `fa` (mover+0x350,
forward/back) and `sa` (mover+0x354, strafe). `S = START_ACCEL`, `M = MAX_ACCEL`, `K = ACCEL_RATE`, `ft =
frame_dt` (unclamped). `boost = 0` at the start of the step.
1. If `ANY_MOVE` is on now and was off last frame: `boost = S`, `fa = sa = S`, `reset`.
2. If `FWD` now and `BACK` last frame, or `BACK` now and `FWD` last frame: `AF := normalize(AF) * S`, `fa = S`,
   `reset`.
3. If `LEFT` now and `RIGHT` last frame: `AR := normalize(AR) * S`, `sa = S`, `reset`.
4. If `RIGHT` now and `LEFT` last frame: `AR := normalize(AR) * S`, `sa = S`, and step 5 is skipped this frame.
5. If not `reset`: if `FWD` or `BACK`: when `fa + ft*K < M`: `AF := normalize(AF) * (fa + ft*K)`, `fa += ft*K`;
   else `AF := normalize(AF) * M`, `fa = M`. The same for `sa` / `AR` when `LEFT` or `RIGHT`.
6. If neither `FWD` nor `BACK`: `fa = S`. If neither `LEFT` nor `RIGHT`: `sa = S`.

With the defaults the ramp goes from 500 to 3000 in about 0.42 s. When `jr` or `CROUCH` is set the ramp is not
run and both axes use `a0` directly.

Consequence of step 5 as written (OBSERVED in dc-sim, Appendix Z M2): `AF := normalize(AF) * (fa + ft*K)` discards
`a0`, so on a ramped frame the per-axis magnitude is the ramp value whatever `a0` is. `a0 *= AIR_CONTROL` (6.4)
therefore only changes the boost frame and the frames where the ramp is skipped (jump request, crouch); crouch
skips the ramp, so its `a0 * 0.5` does act. Implemented as written (airborne strafe acceleration is `(sa + ft K) *
hop`). UNPROVEN whether the original ramp scales by `a0 / BASE_ACCEL`: needs a DISASM re-check at cshell+0x2001c0
or the airborne capture V-M11.

**6.6 Sum and caps.** `acc = 0`; `FWD`: `acc += AF`; `BACK`: `acc -= AF`; if `fly`: `jr` adds `a0` to `acc.y`,
`CROUCH` sets `acc.y = -a0`. `LEFT`: `acc -= AR`, then if `|acc| > MAX_ACCEL`, `acc := normalize(acc) *
MAX_ACCEL`. `RIGHT`: `acc += AR`, same cap. If `boost > 0` and `|acc|^2 > 0`: `acc := normalize(acc) * boost`
(the first frame of any movement accelerates with exactly `START_ACCEL`).

Spectating replaces the above: `acc = sum of +-F * a0 for FWD / BACK and +-R3 * a0 for RIGHT / LEFT` (R3 = the
full rotated +X axis from cshell+0x1b800), then `acc := normalize(acc) * (mover+0x358 + 300000)`. With no key
held this divides by zero (the original then passes a NaN acceleration; the engine's dead-zone tests are false for
NaN). A port should treat a zero sum as zero (DeCombat decision, differs from the original only in that case).

**6.7 Hop penalty** (`hop` = mover+0x35c, 1.0 at init; PROVEN):
- Acceleration: if `hop < 1`, `acc *= HOP_ACCEL_MUL * hop`.
- After the acceleration is written and the velocity re-read: if `jr` and the controller state is not 2, 3, 5 or
  6, on ground and not on a ladder, **the jump happens**: mover+0x80 := 1 (airborne), latch := 0, `v.y := jump_v`
  (set, not added; x and z keep their values), and `hop := HOP_MIN` if `hop >= HOP_MIN`, else `hop -= frame_dt *
  HOP_DECAY`. `jump_v` was computed with the `hop` value before this update, so the first jump uses `hop = 1`.
- Otherwise (no jump this frame, or the state forbids it): if `hop >= 1`, `hop := 1`; else `hop += frame_dt *
  HOP_RECOVER` (no clamp in this step; it is clamped to 1 on a later frame).

**6.8 Slide to stop** (cshell+0x1fe940; PROVEN body, the caller is not identified, UNPROVEN that it runs every
frame): when no `ANY_MOVE`, not airborne, on ground, the ground probe of cshell+0x5e3f0 is true and the container
drag sum mover+0x94 is 0: let `h = |v.xz|`; if `h <= 5`, velocity := (0, 0, 0) (all three components); else
`d = normalize(v.xz) * (run_speed() / T) * dt` with `T = STOP_TIME` (0.1 when `T <= 0`) and `run_speed()` =
cshell+0x1fd2e0 (section 7); if `|d| >= h`, velocity := `(0, v.y, 0)`, else `v.xz -= d`.

**6.9 Knockback** (PROVEN): while mover+0x314 is set and `now - mover+0x308 < mover+0x30c` (`now` = mover+0x310
when positive, else the engine time): `k = KNOCKBACK` (integer, stored at mover+0x318 when non-zero), `mag = (1 -
(now - start)) * k * k`; when `CROUCH`: with `ANY_MOVE` `mag *= 0.25`, without it velocity := 0 and `mag = 0`.
`acc` is **replaced** by `normalize(d.x, 0, d.z) * mag` with `d` = controller+0x1ec..+0x1f4 (the last damage
direction, from shell id 0x74, cshell+0x1f7d20) when `d.x^2 + d.z^2 != 0`. Who sets mover+0x308..+0x314 was not
found in the code read (UNPROVEN).

**6.10 End of frame.** Acceleration is written (phys method +0x3c). Velocity re-read; jump (6.7). When spectating:
`v *= 0.9`, and `v := 0` if `|v| < 0.1`. Then `v += mover+0x88..+0x90` (the container current accumulated this
frame, section 11) and the velocity is written (phys method +0x2c). If the controller state is 3 or
controller+0x80 is 0: velocity and acceleration := 0. Finally the jump notification (cshell+0x1ff0b0, section
12.3) with "jumped this frame" as argument.

## 7. Stances and their speeds (`target_speed`, cshell+0x1fd390)

`run_speed()` (cshell+0x1fd2e0): `RUN_FWD` when `FWD`; else `RUN_BACK` when `BACK`; else `RUN_SIDE` when `LEFT`
or `RIGHT`; else the last value (mover+0x364, 0 at init). The result is stored back into mover+0x364.

`target_speed()`:
1. On a ladder: `s = 200` (hard-coded).
2. Else if the sprint test is true (sprint test = `SPRINT` and the object returned by cshell+0x2aca0 has byte
   +0x1f0 set; **that byte is written by the server's shell message 0xca `[allowed]`**, `GAME_MESSAGES_2009_S2C.md` 3.2, PROVEN-CAPTURE by the client's reaction: in the 22 updates of tdm1 that carried `SPRINT` before the
   server's echo `[1]` arrived the speed stayed at the run plateau, the first update after it was 60-100 units faster, and the sprint plateau of about 1.39 x run followed; so a server must answer each sprint start with 0xca `[1]`):
   - `CROUCH`: `s = CROUCH_SPEED`;
   - else, when the counters controller+0xf0 `< 1` and controller+0xf4 `== 0`:
     `s = (weapon_sprint_mod + gear_sprint_add + item_speed_mod_b + item_sprint_c + 1) * run_speed() *
     fig_sprint_mul` (the item terms count 0 when their index is -1 or the second database is missing; the weapon
     term uses the override rule of 3.3);
   - else `s = WALK_SPEED` (a blocked sprint walks, it does not run).
3. Else if `RUN`: `CROUCH` -> `s = CROUCH_SPEED`; else `s = run_speed()` when cshell+0x5c450 returns 0,
   otherwise `s = WALK_SPEED`.
4. Else (walking): `s = WALK_SPEED`, or `CROUCH_SPEED` when `CROUCH`.
5. Result: `(1 - p) * s` with `p` = the derived mover's method +4 (cshell+0x5c270):
   `p = clamp(held_item_mod + weapon_speed_mod + fig_weight + item_speed_mod_a + gear_penalty - fig_speed_bonus,
   -1, 1)`, where `held_item_mod` is the float at +0x2d8 of the record returned by cshell+0x172300 for the equipped
   weapon id (0 when none). The base mover class returns `p = 0`.

With default variables and no penalties: walk 70, run 285, crouch 50, ladder 200, sprint `285 * fig_sprint_mul`
times the additive modifiers. Prone does not exist in this client (no prone state or speed found).

## 8. The engine integrator

**8.1 The physics step of the mover** (`phys_step`, cshell+0x1fe6c0, PROVEN + DISASM), once per frame after the
velocity law:

1. `g := GRAVITY`, kept in mover+0xa0. The field is rewritten only when the variable's value is **not** -1100
   (mover+0xa0 is reset to -1100 by cshell+0x1fcd80, so once the variable was set to another value, setting it
   back to exactly -1100 has no effect until the next reset), and it becomes 0 when the variable does not exist
   (PROVEN as written, DISASM cshell+0x1fe6c0..0x1fe718).
2. `STEP_HEIGHT` is read (section 2) and pushed to phys when it is not 30.0; a variable the server did not send reads 0 and is
   pushed (section 2, correction of spec-fold-8).
3. The world gravity record (8.2) is saved, then set to `(0, g', 0)` with `g' = mover+0x9c` when the gravity
   override flag mover+0x98 is set (gravity volumes, section 11) and `g' = g` otherwise.
4. `integrate(body, dt)` (phys method +0x50, engine+0xd6980 -> engine+0x6fe50, section 8.3) returns the
   displacement `out` for this frame. It runs only when the body's "awake" bit (engine object +0x16c bit 0x40) is
   set; setting a velocity or an acceleration sets it (engine+0xd6a90, engine+0xd6b00), so the mover's body is
   always awake while `move_law` runs.
5. If `|out|^2 > 0.01`: `target = pos + out`, then `move_to(body, target, 0)` (phys method +0x40, section 9.3).
   Otherwise **the body does not move at all this frame** (displacements up to 0.1 units are dropped; the velocity
   is kept). PROVEN.
6. After the move: if `|out.y| > 0.01` and the new position's y differs from the old y by less than 0.01, the
   vertical velocity is set to 0 (blocked by floor or ceiling).
7. The world gravity record is restored.

**8.2 World gravity record** (engine client world + 0xd88; set by engine+0x16cd0, initialised by engine+0x18770,
PROVEN): `{force: vec3, dir: vec3 = force / |force| (0 when |force| <= 1e-5), mag: f32 = |force|, standable:
f32}`. Engine defaults: force `(0, -1600, 0)`, `standable = -0.7071` (bits 0xbf350481). The player's own step
always runs with `(0, g', 0)` (8.1), so the engine default gravity only affects other client-side objects.
Per-object override: engine object +0xf0..+0xf8; when all three are 0 the record's force is used (the mover never
sets it).

**8.3 `integrate`** (engine+0x6fe50; SSE binary32 except where noted; PROVEN + DISASM). Inputs: `v` (object
+0xe4), `a` (object +0x134, the acceleration the mover set), `F` (the force from 8.2), `grav` (object flag
0x20000), the standing contact (object +0x164 = standing object, +0x168 = standing polygon; its plane normal is
`n`), `k` = object +0x130 (friction coefficient, phys methods +0x18 / +0x1c), `dt`.

```
if |v|^2 < 0.1: v = 0                 # dead zones, both tested on the input values
if |a|^2 < 0.1: a = 0
out = 0
if not grav:
    if v == 0 and a == 0: REST
    else: PLAIN(a)
elif no standing object:
    a = a + F ; PLAIN(a)
else:
    if v == 0 and a == 0: REST
    n = plane normal of the standing polygon, or -dir(F_world) when there is none
    A = a + F
    Fd = F scaled to length 1 (engine+0x1b7f0; stays 0 when |F| = 0)
    if dot(Fd, n) > standable:                       # steep: normal more than 45 deg away from "up"
        m = |A|                                      # x87 sqrt
        a = A - n * dot(A, n)
        e = dot(a, Fd); if e < 0: a = a - Fd * e     # no acceleration component against gravity
        a = a scaled to length m (when |a| != 0)
        PLAIN(a)
    elif dot(v + F*dt, n) > 0.01:                    # leaving the surface
        a = A ; PLAIN(a)
    else:                                            # supported by the surface, gravity cancelled
        if dot(a, n) > 0.01:
            m = |a|
            a = a - n * (dot(a, n) + 1.0)            # remove the into-surface part plus 1 unit
            a = a scaled to length m
        if k > 0: FRICTION(a, k) else PLAIN(a)
REST:     v = 0, a = 0, clear the awake bit, out = 0
PLAIN(a): out = v*dt + a*(dt*dt/4) ;  v = v + a*dt
FRICTION(a, k): E = exp(-k*dt)                       # x87 f2xm1 / fscale with log2(e)
          out = (a/k)*dt + (1 - E) * (1/k)^2 * (k*v - a)
          v   = a/k + (v - a/k) * E
```

Notes (all PROVEN): the displacement uses **dt^2/4**, not dt^2/2 (`0.5 * dt * dt` is computed at entry and
multiplied by 0.5 again before use, constant at VA 0x672918). `a` is written back into the object (so after the
step the stored acceleration includes `F` in the airborne and steep cases). In FRICTION `v` is the velocity before
the step. The friction law is the exact solution of `dv/dt = a - k v`, i.e. the terminal speed is `|a| / k`.
Dot products in the steep branch are summed in the order x, z, y (as the code does); a bit-exact port must keep
the order (section 15). In the standing case `v == 0 and a == 0` gives REST before the steep test, so a body that
comes to rest on a steep surface stays there until something sets a velocity; a landing body is moving, so it takes
the steep branch and slides (the mover writes `a` every frame but it is zero without input; OBSERVED in dc-sim,
Appendix Z M3).

**8.4 Friction coefficient of the player** (cshell+0x1fce10, PROVEN): `k = 5.0 * min(1.0, min over the current
containers of their friction factor)`, written every frame with phys method +0x1c, **unless** the client
interface boolean slot +0x178 reports true (PLAUSIBLE meaning: connected to a remote server), in which case the
container list and `k` come only from the server's mover update (shell id 0xa0 flags 0x04 / 0x08, section 12.4).
With the defaults: on flat ground with no key held the speed decays as `exp(-5 t)` (time constant 0.2 s), plus the
slide-to-stop rule of 6.8; with `BASE_ACCEL` 3000 the terminal speed would be 600, so the speed cap of 6.3
(target speed) is what limits running speed.

## 9. Collision shape and the engine move / sweep

**9.1 Client physics method table** (engine table VA 0x681858, CShell calls it through VA 0x375964ec; DISASM). The
methods the movement code uses (our names):

| slot | engine rva | our name | effect |
|---|---|---|---|
| +0x08 / +0x0c | 0xd6a10 / 0xd6a20 | `get_step` / `set_step` | step height float at the instance +0x08 |
| +0x10 / +0x14 | 0xd7630 / 0xd7650 | | object +0xc4 (a per-object float; PLAUSIBLY mass, not used by the mover) |
| +0x18 / +0x1c | 0xd75f0 / 0xd7610 | `get_friction` / `set_friction` | object +0x130 (`k` of 8.3) |
| +0x20 | 0xd77d0 | `get_box` | object +0xb8..+0xc0 (half extents) |
| +0x24 | 0xd6cd0 | `set_box(obj, dims, must_fit)` | engine+0x73be0; returns 0 on success; refused for object types 2 and 9 |
| +0x28 / +0x2c | 0xd7760 / 0xd6a90 | `get_vel` / `set_vel` | object +0xe4..+0xec; `set_vel` sets the awake bit |
| +0x38 / +0x3c | 0xd7840 / 0xd6b00 | `get_acc` / `set_acc` | object +0x134..+0x13c; `set_acc` sets the awake bit |
| +0x40 | 0xd6b70 | `move_to(obj, pos, teleport)` | calls engine+0x73060 with move flags 1, or 9 when `teleport & 1`, only when `pos` differs from the current position |
| +0x44 | 0xd78b0 | `standing_info` | standing object +0x164, polygon +0x168, plane, flags |
| +0x48 / +0x4c | 0xd6e40 / 0xd6e70 | `get_gravity` / `set_gravity` | world record 8.2 |
| +0x50 | 0xd6980 | `integrate` | 8.3 |
| +0x54 | 0xd6e90 | push-move variant (move flags 0x15) | not used by the mover |

**9.2 Shape.** The body is an engine object with half extents `dims = (dx, dy, dz)` (object +0xb8..+0xc0).
For collision the engine uses a **vertical cylinder**: radius `r = max(dx, dz)` and half height `h = dy`
(engine+0x75110 builds `(r, h, r)`, bounding radius `sqrt(r^2 + h^2)`, `r * 0.5` and `step = get_step() + 0.5`;
PROVEN). Its support distance along a unit plane normal `n` is (engine+0x74c90, PROVEN):
`support(n) = s * r + sqrt(1 - s^2) * h` with `s = min(1, |n.xz|)`. Against blocker polygons (9.6) the same
body is treated as a capsule (segment of half length `max(h - r, 0)` along Y, radius `r`).

Dims per stance: the client takes its box from the server. The server sets the box with the mover update (shell id
0xa0 flag 0x20: new position and new half extents, section 12.4; the client first shrinks the box to
`(0.5, 0.5, 0.5)`, teleports, then sets the new box with `must_fit = 1`, falling back to `must_fit = 0`). The
standing box `stand_dims` (mover+0x32c..+0x334) is read from the player's model: the user dims of the model
animation whose name has key id 7f007bf2ff2bcc78/3 (client interface slot +0xa4 looks up the animation, model
interface slot +0x18 returns its dims; cshell+0x1ff430 tail, DISASM). The numbers are therefore ROM model data
(`FMT_LTB.md` animation records). The crouched box is not set by any client code found (UNPROVEN: PLAUSIBLY the
server sends it with 0xa0 / 0x20 when the reported stance changes; capture item V-M7). There is no prone stance. OBSERVED: the stand clip is carried by 3 player models of the ROM (all in one character
archive) and their dims agree; dc-sim's ROM test reads them at run time and treats them as the body's half
extents (Appendix Z M7).

Body flags at creation (cshell+0x1ff430 flag 0x01, DISASM): object flags `0x62000` = solid (0x2000) + gravity
(0x20000) + stair stepping (0x40000), second flag word `0x8` (collides with the world, required by
engine+0x70860); one bit of the message clears 0x2000 (non-solid body). The mover switches the gravity flag every
frame (off on ladders and while spectating, cshell+0x1fe300).

**9.3 `move_to`** (engine+0x73060; the decompiler lost the arguments, DISASM: `(ctx, x, y, z, flags)`). Move
flags: bit 0 = after the move clear the standing contact when nothing is below; bit 2 = carry riders; bit 3 =
teleport (no collision test); bit 4 = ignore world models; bit 5 = single sweep without sliding (engine+0x75110).
Steps (PROVEN unless marked):
1. Refused while the object is already being moved (+0x16c bit 0x02) or has +0x16c bit 0x20 set.
2. Objects that are not solid (flags 0x2000 / 0x8000 rules of engine+0x70810) or when collision is globally off
   (VA 0x6cb2a0) are placed directly.
3. Otherwise `sweep_move(start, target)` (9.4) gives the final position, which is then stored.
4. Riders (objects standing on this one, list at +0x140; up to 33) are moved by the same offset with flags
   `previous | 7` and the y target lowered by 0.1, then re-attached when their box still touches (within 0.01).
5. If the object stood on a moving object (not a world polygon) before the move, it is re-attached when the
   contact test engine+0x71020 with tolerance -0.01 still passes.

**9.4 `sweep_move(start, target)`** (engine+0x796e0, PROVEN):
- teleport flag, or `target == start`: result = target.
- "single sweep" flag: one `sweep`, result = `start + (target - start) * t`.
- object flag 0x40000 (stair stepping, the player has it) and a horizontal component: `stair_move` (9.7).
- else: `slide_move` (9.5).
- NaN result -> stay at start.
- Standing refresh: when the move did not go downward, or ended above the target, run the ground probe of 10.1;
  otherwise, with move flag bit 0, clear the standing contact.
- Touch pass: the box is grown by 0.2 in every half extent, objects in the swept region get touch notifications
  (engine+0x79450), then the box is restored.

**9.5 `slide_move(P0, E0)`** (engine+0x78650, PROVEN). Up to three real contact planes:
```
d0 = E0 - P0 ; L0 = |d0| ; remaining = L0 ; P = P0 ; E = E0 ; planes = 0 ; tsum = 0 ; last_n = 0
loop:
    hit = sweep(P, E)                      # t in [0, 1], normal N
    if not hit: return E
    flag "no step" when the hit polygon's surface flags have bit 0x400000
    d = E - P ; C = P + d * hit.t          # contact point
    if planes == 0 or |N - last_n|^2 > 1e-6 or hit.t != 0 or tsum != 0:
        last_n = N
        if planes == 0:
            n0 = N
            dn = dot(n0, d)
            if dn < 0: d = d + n0 * |dn * 1.001|        # clip against the first plane with 0.1 % push-out
        else:
            if planes != 1 or |dot(N, n0)| >= 0.999: return C
            c = normalize(cross(n0, N)) ; d = c * dot(c, d)   # slide along the crease
        d = d * (1 - hit.t) ; tsum += hit.t
        if |d|^2 < 0.0001 or planes == 4 or dot(E - P0, d0) < 0: return C
        P = C ; E = C + d
    else:                                  # same plane again with no progress
        nudge = L0 * 0.25
        if remaining < nudge: return C
        remaining -= nudge ; P += N * nudge ; E += N * nudge ; planes -= 1
    planes += 1
    if planes > 4: return E
```

**9.6 `sweep(P, E)`** (engine+0x78280, PROVEN). Result: `t` (fraction of `E - P`, 1 when nothing is hit), hit
normal, plane distance, hit object, hit polygon. Two stages:
1. **Blocker polygons** (the world's player-clip soup of `FMT_DAT85.md` section 9.1; object at VA 0x6e28b4, method
   +0x14 = engine+0xc5660): skipped when `|E - P| < 0.0001`; a bounding-sphere pre-filter around the segment
   midpoint (radius `|E - P| / 2 + |dims|`), then each candidate polygon (48-byte records) is tested against the
   moving capsule of 9.2 (engine+0xc4bf0, not read); the nearest hit distance minus 0.001 becomes `t * |E - P|`.
   The end point is pulled back to the hit.
2. **Objects**: candidates are gathered from the world's spatial tree with the swept box (engine+0x74d90 ->
   engine+0xbf530) and filtered (engine+0x745e0: not the mover itself; solid by flag rules; world models skipped
   when move flag bit 4 is set; two objects that both have second-flag 0x200 do not collide). Each candidate:
   world models (engine object types 2 and 9 without flag 0x4000) use the BSP sweep (9.8); other objects use the
   box test engine+0x75860 (not read). On a hit with fraction `t'` along the current segment: `t *= t'`, then a
   back-off: with `u = (E - P) / |E - P|` and `q = -dot(N, u)`, if `q > 0.001`: `t -= clamp((0.1 / q) / |E - P|,
   0, t)` (the contact stays 0.1 units away from the plane along the normal); stop when `t <= 0`.

The static world geometry is a set of world-model objects, each holding one BSP of `FMT_DAT85.md` section 5
(PLAUSIBLE that the main level geometry is one of them; the sweep has no separate "world" path).

**9.7 `stair_move(P0, E0)`** (engine+0x78c50, PROVEN; `step` = `STEP_HEIGHT + 0.5`, `h` = half height):
1. If standing on something: probe from `P0` down to `(P0.x, min(P0.y, E0.y) - 0.1 h, P0.z)`. If it hits a
   surface with `0 < n.y < 0.7071` that the move goes into and the move climbs it, the move is replaced by its
   projection on the slope's level line `t = normalize(n.z, 0, -n.x)`: `d = t * dot(t, d)` (no walking up steep
   slopes).
2. `r1 = slide_move(P0, P0 + d)`. If it hit something and the "no step" surface flag was not seen:
   - probe up from `P0` to `P0 + (0, step, 0)`; if free: `r2 = slide_move(U, U + d)` with `U = P0 + (0, step, 0)`,
     then probe down from `r2` by `step`: landing point `D`;
   - when the down probe hits nothing or lands on `n.y >= 0.7071`: take `D` if its progress along `d`
     (`dot(D - P0, d)`) exceeds `1.01 *` the progress of `r1` (clamped at 0), else `r1`;
   - when it lands on a steep surface and the raised slide was free: one retry along the horizontal downhill
     direction of the slope, distance `max(0.5 r, 2 (1 - n.y^2) step + 0.1)`, then the same choice (PLAUSIBLE: the
     retry's own result is not used, only its hit test).
3. Otherwise `r1`.

**9.8 BSP sweep of one world model** (engine+0x77db0, PROVEN). Node = `{polygon (whose plane is the split plane),
negative child, positive child}`; the two sentinels of `FMT_DAT85.md` 5.2 end a path. With `S`, `E` the segment,
`L = |E - S|`, `Rb` = the cylinder's bounding radius and `off = support(n)`:
- distance of the segment midpoint `c = dot(n, mid) - dist`: `c > L + Rb` -> positive child only; `c < -(L + Rb)`
  -> negative child only;
- otherwise classify `S` and `E` separately: distance `> Rb` -> positive, `< -Rb` -> negative, else positive when
  `> off`, touching when `>= -off`, negative otherwise;
- both on the same side (not touching): go to that child;
- otherwise (the two classes differ, or both touch): the node's polygon is tested (9.9) when `S` is positive or
  touching, or both touch, and only when the polygon's surface flags have bit 0 set and the segment moves into the
  front face (`dot(n, E - S) < -0.0001 L`); a segment that starts on the negative side is not tested against this
  polygon. Then the child on `S`'s side is visited first (`dot(n, S) - dist > -0.001` means positive) and the other
  child is pushed on an explicit stack (sentinels are not pushed);
- at a sentinel pop the stack; when empty, finish.

The node layout in memory (which field is the negative child) maps to the file's `node_front` / `node_back` in a
way not shown by this function. OBSERVED over the 21 ROM worlds: the file's first child field (`node_front` of
`FMT_DAT85.md` 5.2) is the child on the **negative** side of the node polygon's plane (1,218,026 child-subtree
root polygons lying strictly on one side are placed this way, 8,630 the other way). Polygons that cross a split
plane are referenced from both subtrees (about three nodes per polygon, `FMT_DAT85.md` 11.1), which is why the
one-child descent above still finds them. The confirmation against the loader engine+0x33060 stays UNPROVEN
(Appendix Z M4).

**9.9 Cylinder against one polygon** (engine+0x77530, PROVEN shape, one sub-test not read). Polygon record:
vertex count (low byte of word 0), bounding sphere centre and radius (words 1..4), plane pointer (word 6), vertex
pointers (word 7 on).
1. Pre-filter: closest approach of the sweep line to the polygon's sphere (the code computes the line parameter as
   `dot(c, d) / (|d|^2 |c|)` with `c` = centre - start; PROVEN as written) farther than `sphere radius + Rb` -> no
   hit.
2. Only when the start is on the front side (`ds = dot(n, S) - dist >= 0`).
3. `vn = dot(n, d)`; entry `t_in = (ds - off) / -vn`, exit `t_out = (ds + off) / -vn` (with the infinities of the
   parallel case); no hit when `t_in >= best` or `t_out < 0`; `t_in = max(t_in, 0)`.
4. Contact point `P = S + d t_in`. For each edge: the edge plane normal is `normalize(cross(edge, n))`, or, for a
   sloped polygon (`0.0001 <= |n.y| < 0.9999`) and a non-vertical edge, `normalize(cross(edge, k))` with `k` =
   the cylinder's support offset `(n.xz / |n.xz| * r, sign(n.y) h)`. If `P` is outside an edge plane by more than
   that plane's `support()`, no hit; if outside by less, the edge / vertex sweep engine+0x76120 (5127 bytes, not
   read: UNPROVEN) refines the time and normal.
5. A hit is recorded when its time is below `best` and the motion goes into the final normal (`dot(d, N) < 0`).

**9.9a Deviations of the dc-sim implementation** (OBSERVED, not 2009 behaviour; Appendix Z M5). A port should know:
- Step 1: the line parameter as written can reject polygons a long move reaches; dc-sim uses the exact closest
  point of the segment instead.
- Step 4 edge / vertex refinement (engine+0x76120, not read): dc-sim clips the sweep against the convex
  intersection of necessary half-spaces (face slab, in-plane edge planes pushed out by the support distance,
  vertical planes through the edges pushed out by the radius, and the `cross(edge, k)` planes for sloped faces).
  Conservative: it never misses a contact; near acute polygon corners it can stop early (sharp instead of
  rounded corners).
- Static "box must fit" (engine+0x73be0, not read): dc-sim tests exactly (polygon clipped to the cylinder's height
  band, projected to XZ, 2D distance to the axis against the radius), both polygon sides, 0.01 tolerance.

**9.10 Consequences of the front-face-only rules** (OBSERVED in the ROM spawn test, Appendix Z M6). Faces are
tested only while the move goes into their front side (9.8 `dot(n, E - S) < -0.0001 L`, 9.9 step 2), and a move
parallel to a face is judged at its start (9.9 steps 3-4: `t_in` = start). In the ROM spawn test (1,560 spawns, 5 s
standing + 5 s running) no body centre ever crossed a solid front face and standing never created an overlap, but
47 bodies ended the run overlapping geometry they did not overlap at placement: back faces passed through,
sub-unit lips between floor models walked into sideways, a stair step-up clipping the corner of a steep face, thin
double-sided panels entered from their edge. A port that wants to be stricter than 2009 must change 9.9 steps 3-4;
dc-sim keeps the 2009 rules. Suggested capture: V-M10 along such lips.

## 10. Ground detection, falling and landing

**10.1 Engine ground probe** (engine+0x79210, PROVEN): after a move, sweep from the final position down by
`0.02 h`. No hit -> clear the standing contact (when move flag bit 0). Hit with normal `n.y < 0.001` -> clear.
Hit with `n.y < 0.7071` while the previous contact was standable (`>= 0.7071`): second probe from `pos + (0, step,
0)` along the horizontal downhill direction for `max(0.5 r, 2 (1 - n.y^2)(0.02 h t + step) + 0.11 r)`; nothing
there -> keep the old contact. Otherwise the hit object and polygon become the standing contact (the integrator's
steep branch then makes the body slide on steep contacts).

**10.2 Mover ground state** (cshell+0x1ffec0, every frame before the velocity law, PROVEN):
- `standing_info` (phys +0x44); `on_ground` (mover+0x60) := a standing object exists;
- landing: if it exists and the body was falling without a jump (mover+0x80 == 0 and mover+0x68 set): send the
  land notice `[0x76][1][3]` (12.3); mover+0x68 := 0;
- mover+0x48 / +0x4c := the two standing ids from `standing_info` (or -1 / -1), mover+0x50 := the surface type of
  the standing polygon or object (cshell+0x27f430 / +0x27f470), mover+0x64 := standing object has flag 0x20;
- mover+0x7c := current y; if the body was airborne after a jump: mover+0x80 := 0 and the jump notification is
  sent (which reports 3 = landed, 12.3);
- `on_ground := 0` when the standing plane normal y is `< 0.707`, when the standing flags have 0x1000, or when
  airborne after a jump;
- while not on ground: mover+0x7c := max(mover+0x7c, y) (spectating: -1e7). mover+0x7c is the fall start height;
  fall damage itself is not computed by the client (the `FALL_DMG_*` variables have no reader in the CShell code;
  PLAUSIBLE: the server computes it from the land notice);
- on a ladder the surface type is forced to 200;
- fall start: in walking mode, not on a ladder, not spectating, not airborne after a jump, when the probe
  cshell+0x1fe1f0 finds no ground within 150 units below (ray with filter flags 7, skipped for surface types 0xca
  and 0x6e) and mover+0x68 is clear: mover+0x68 := 1 and the fall notice `[0x76][1][2]` is sent.

**10.3 Stuck rule** (cshell+0x1fe510 / cshell+0x1fe560): if the body's current box no longer fits at its
position (`set_box(current, must_fit=1)` fails), the box is forced (`must_fit = 0`) and the body re-placed; if a
standing object of a special kind is found (cshell+0x5f480), the body is moved by half of that object's offset
vector. PLAUSIBLE purpose: recover from being embedded.

**10.4 Fall damage tunables** exist only as registered variables (section 2); no client reader. DeCombat default: the
server applies `FALL_DMG_MIN .. FALL_DMG_MAX` linearly between `FALL_DMG_MIN_H` and `FALL_DMG_MAX_H` of fall
height (DeCombat-DEFAULT, to be listed in CAK_DEFAULTS.md; the 2009 rule is not in the client).

## 11. Containers

**11.1 Container list** (mover+0x2f4 count, up to 16 entries of 0x24 bytes at mover+0xb4, PROVEN). Entry fields
(our names): `gravity` (+0x00), `viscosity` (+0x04), `current` (+0x08..+0x10, units/s), `kind` (+0x14),
`hidden` (+0x18), `object` (+0x1c), `extra` (+0x20). Locally filled from the containers the body touches
(client interface slot +0x24, container records via cshell+0x20fbe0: record +0x80 gravity, +0x84 viscosity,
+0x74..+0x7c current, +0x90 kind, +0x8c extra, +0x88 friction factor) or received from the server (12.4).

**11.2 Per frame** (cshell+0x1fe300, PROVEN), after clearing the current accumulator (mover+0x88..+0x90) and the
drag sum (mover+0x94):
- gravity flag := on, unless spectating or on a ladder;
- first pass, kind 11 (ladder) and not hidden: mover+0x2c := 11; the first one makes the body "on ladder"
  (mover+0x30 := 1, mover+0x34 := its object) and turns gravity off; drag (11.3); `current_acc += dt * current`;
- second pass, every other kind, not hidden: mover+0x2c := kind; drag; kind 19 (0x13) sets the gravity override
  (mover+0x98 := 1, mover+0x9c += gravity; mover+0x9c is cleared every frame by cshell+0x201470); `current_acc +=
  dt * current`.
- The accumulated current is added to the velocity at the end of `move_law` (6.10).

**11.3 Drag** (cshell+0x1fe060, PROVEN): for a container that is not hidden, when on a ladder or not airborne
after a jump: `drag_sum += viscosity`; if `viscosity > 0` and `|v| > 1`: `v -= normalize(v) * viscosity * 5000 *
dt`, and `v := 0` if that would reverse it.

**11.4 Ladder movement** (from 6 and 7): speed 200, upward speed capped at 200, gravity off, the acceleration
follows the full 3D view direction (look up to climb), `CROUCH` gives `acc.y = -a0`, jump gives `+a0`.

**11.5 Pushers** (the step-6 function of section 1, PROVEN shape, the strength operand is partly lost): a list of radial pushes
(mover+0x0c list; each with centre, radius, delay, duration). Delay and then duration count down by `dt`; while
active and the body is inside the radius with a clear line from the centre (client interface slot +0x64), the
velocity gets `normalize(body - centre) * strength` (strength x 0.1 when the body is not standing on anything);
the resulting upward velocity is capped at `PUSH_MAX_VY`. Expired pushes are removed (cshell+0x201320,
cshell+0x1fef70). Who adds pushes (PLAUSIBLY explosion messages) was not traced.

## 12. Prediction and server reconciliation

**12.1 Authority model (PROVEN by structure).** The 2009 client simulates its own body locally every frame
(sections 5-11) and is the authority for its own position: it does not wait for the server and does not replay
inputs. The server receives the client's absolute position and velocity (12.2) and can only impose state with the
mover update (12.4) and a few related messages (12.5). There is no client-side rewind / replay of unacknowledged
inputs in the code read (UNPROVEN in the negative sense: nothing was found). PROVEN-CAPTURE (tdm1): the client sent absolute position and velocity at 15 Hz for 157 s, the reference server sent no 0xa0 after the spawn set and never corrected it, and the client
never waited for the server (one 438 ms local stall aside); its positions are self-consistent with its velocities (median difference 0.10 units against the mean velocity times dt). The client also did not adopt the streamed position of its
own object (`UDP_TRANSPORT_2009.md` C-T14).

**12.2 What the client sends.** The shell player update, shell id 0x64, unreliable, at most `SEND_RATE` times per
second (console variable registered by cshell+0x1f6fb0, key id see 13.1, default 15.0; read by the two builders,
the regions cshell+0x5f7f0 (live, derived slot +0x100) and cshell+0x1fa060 (base slot +0x100, replaced in the live
object); DISASM, `GAME_MESSAGES_2009_C2S.md` 2.1), layout in `UDP_SESSION_2009_B.md` section 10.3. Its movement trailer is
written by cshell+0x1fed20 (PROVEN + DISASM; bit widths as written by the reader-writer methods +0x24 = write N
bits, +0x2c = write raw bits from memory, +0x28 = write 64 bits):

```
[move_seq:u8]         mover+0x04: the sequence byte of the last 0xa0 / 0x20 teleport (12.4), echoed
[pos:3 x f32]         body position (client interface slot +0xa0), raw 96 bits
[vel:3 x f32]         body velocity, raw 96 bits; zero while the shell is paused (shell+0x1a08 / +0x1a09)
[time:f32]            client time (client interface slot +0x6c, seconds)
[on_ground:u8]        mover+0x60
[surface:u8]          mover+0x50 (surface type under the feet, 200 on ladders)
[standing:u64]        mover+0x48 (low 32 bits) and mover+0x4c (high 32 bits), the standing ids or -1 / -1
```
PROVEN-CAPTURE (tdm1, tdm2, tdm4; V-M8 passes): the trailer parses exactly as written. `move_seq` is 1 in the first two updates and 2 afterwards (the reference stack's two teleports carried seq 1 and 2; the echo changed 31-63 ms after the second
teleport on both links of tdm2; the first two updates after a teleport still carry the previous seq, so a server ignoring a stale `move_seq` must expect them); the first update's position is the teleport position raised by about 0.12.
`time` is the client's **own clock** in seconds (138.197 at capture time 139.953 s while the server's stream time was about 0.0; the 0x82 `time_ms` uses the same clock): a server may use differences only. `on_ground` is 1 / 0;
`surface` 71, 10, 20, 23 on the ground, 0 in the air, 200 on the ladder; `standing` is -1 / -1 while airborne (401 of 402 such updates), otherwise a high word that is constant for the world model (0x5d in tdm1, PLAUSIBLY a world-model
index; the server never created an object 0x5d) and a low word that varies with the floor polygon. The rate is 15 per second **whether the player moves or stands** (65-70 ms gaps).
Position and velocity are raw IEEE floats here, not the compact `pos48` / `vec72` of `UDP_TRANSPORT_2009.md`
section 15 (those are used by the engine's server -> client object stream). The view angles travel in the head of
the same message (`UDP_SESSION_2009_B.md` 10.3: its `v` floats are controller+0x68..+0x70, which this spec
identifies as the body's Euler angles, 13.2 step 7). PROVEN-CAPTURE: that `state:u32` is the control word of 5.1; its stance bit is `CROUCH` 0x20 (it accompanies the eye drop to +20 in 52 of 52 crouched updates). The body angles
are `(0, body_yaw, 0)`: pitch and roll are always 0.0 and the body yaw is unwrapped and unbounded (-2.85 .. 10.45 rad in the normal captures; 2.4 million rad after one corrupt mouse delta in tdm2, which an f32 cannot turn finely;
see `GAME_MESSAGES_2009_C2S.md` 2.1.1).

The head of the same update also carries `weapon_f` (`pu_flags` 0x0200, sent when the drawn weapon's float differs from the last one sent): the weapon's current
**spread**, the value that the next 0x82 sends as `frac = trunc(weapon_f x 255)` (C2S 2.1, 4.1; PROVEN-CAPTURE tdm1: rising during a burst, saturating at 0.29 for weapon 145,
back to 0 within 0.2-0.4 s). This spec still does not cover the spread law (scope note of the header): it is not read (C2S 4.1.1, spread note), and `dc-client`'s CLI-31
(proposed) is a stand-in that does not match the capture. Spread has no effect on the movement law; the drawn weapon's speed modifiers are 3.3 / 7 step 5. The fire bit
`FIRE` 0x400 of the control word (5.1) and `pu_flags` 0x2000 travel in the same update; a DeCombat client holds them 67 ms after a shot so that a short click reaches an
update (CLI-33 (proposed); folded by spec-fold-7).

**12.3 Movement event notices** (reliable shell messages built in cshell+0x1ff0b0 and cshell+0x1ffec0, PROVEN):
`[0x76][1][e]` with `e = 1` jump take-off, `e = 2` started falling (no jump), `e = 3` ground contact regained. PROVEN-CAPTURE (tdm1, tdm4): after a plain jump the 3 comes **one frame after the 1** (26 of 27), because the ground
test still finds the floor, so the real landing usually sends nothing; on stairs and edges the 2 / 3 pair repeats many times within one frame time. A server takes landings (fall damage, landing sounds) from the 0x64 trailer
(`on_ground` 0 -> 1 and the vertical speed of the last airborne update), not from event 3. The jump notice is
sent on the frame of the jump; it also plays one of two jump sounds (keys f0b51f45c194dbef/14 and
bb3b8d0915691dfd/14, chosen at random) unless the surface type is 0xca.

**12.4 Server -> client mover update** (shell id 0xa0; cshell+0x1f0470 dispatches to the controller's slot
+0xbc = cshell+0x1f27f0, which reads `flags:u16` and calls cshell+0x1ff430, then sets controller+0x13c := 1).
Blocks in this order, each present when its flag bit is set (PROVEN + DISASM; reader method +0x3c = read N bits,
+0x44 = read raw bits, +0x4c = read string, +0x68 = read object reference (PLAUSIBLE)):

| flag | block | client effect |
|---|---|---|
| 0x0020 | `[seq:u8][pos:3 x f32][dims:3 x f32]` | **teleport and new box** (cshell+0x1feb90): `move_seq := seq`; the client's copy of its own server-side player object gets object flag 0x8000 (excluded from the mover's collision by engine+0x70650; cshell+0x1fc8e0 does the same every frame); box := (0.5, 0.5, 0.5); `move_to(pos, teleport)`; `set_box(dims, must_fit 1)`, else `set_box(dims, 0)`; acceleration := 0; velocity := the saved vector mover+0x3c..+0x44 when the flag mover+0x38 is set (then cleared), else 0 and the fall height reset |
| 0x0004 | `[n:u8]` then n x `[kind:u8][current:3 x f32][gravity:f32][viscosity:f32][hidden:u8][object:objref][extra:u8]`, then `[friction:f32]` | container list (11.1) and `set_friction(friction)`; at most 16 entries are stored |
| 0x0001 | `[model:str][n:u8][n x skin:str][not_solid:1 bit]` | (re)create the body: old body removed, new one created with flags of 9.2 at the position of the server's player object (or placed later when that object does not exist yet: mover+0x74 := 1, used by cshell+0x201470) |
| 0x0008 | `[friction:f32]` | `set_friction` |
| 0x0080 | `[freeze:1 bit]` | freeze flag mover+0x338 := bit; when 1 the current position is stored (mover+0x33c) and re-applied every frame by cshell+0x1ff3f0 (teleport to it); 0 ends the freeze |
| 0x0100 | `[release:1 bit]` | when 1, the freeze is not applied (mover+0x348) |

Then the stand box is refreshed from the model (9.2). The shell also builds a local `[0x80 0x100]` message with
`freeze = 1, release = 0` (cshell+0x5f620 / cshell+0x5f710) when it resets the player (PLAUSIBLY on death or
respawn).

Reconciliation rule implied by the code (PLAUSIBLE): the server corrects a client only by a hard teleport (0x0020),
bumping `seq`; the client echoes `seq` in every later update, so the server can ignore updates that were sent
before the client applied the teleport. A Rust server should (DeCombat default): validate each 0x64 update against the
shared simulation (speed, gravity, step height, collision) with a tolerance, and on violation send 0xa0 / 0x0020
with the last valid position, the current box and a new `seq`; updates whose `move_seq` is not the latest are
ignored.

Observed (tdm1, REFERENCE server side, PROVEN-CAPTURE client side): the spawn set carried 0xa0 `0x000d` (body: a model name, 2 skins, an empty container list, friction 5.0 twice), `0x0020` (teleport seq 1, position, half extents (15, 50, 15)),
`0x0080` freeze 1 and 0 around other messages, and after the client's 0x87 another `0x0020` (seq 2, same point) and `0x0008` friction 5.0; no 0xa0 during the 157 s of play and none on crouching (the crouch box needs no server message;
the client crouched anyway, eye +33 -> +20, speed 45.6: V-M7). 0x6a `[0][0, 5.356415, 0]` was applied: the first 0x64 body yaw is bit-identical.

Spawn teleport height (REFERENCE + SIM; dserver-round2 G5, capval-rust3 9.1, folded by spec-fold-8): in tdm6 (10 spawn teleports) the reference stack's teleport height on a spawn point
equals the resting height of the teleport's box dropped onto the level floor below the point (our dc-sim placement gives the same value to the float); the spawn points
themselves lie 36 units higher. Our rust3 server teleported to the raw spawn-point height (y 86.0) and the real client fell 36 units to 50.1 in 0.27 s (seq 742-772;
PROVEN-CAPTURE). The tdm6 respawn teleports carried `move_seq` 3, 4, 5, 6, 7, each at a different spawn point (capval-tdm6 4.3). DeCombat: DSV-48 (spawn on the floor;
PENDING).

**12.5 Other server inputs that affect movement** (PROVEN):
- shell id 0x67 `[state:u8]` -> controller+0x30 (cshell+0x1f97d0): states 2, 3, 5, 6 block jump and crouch; state 3
  zeroes acceleration and velocity every frame (6.4, 6.10).
- shell id 0x6a `[kind:u8][angles:3 x f32]` (cshell+0x1f4ed0): kind 0 sets pitch, yaw, roll (controller
  +0x48..+0x50), kind 1 sets yaw only; then the body angles (controller+0x68..+0x70) are copied from them. Used to
  face the player at spawn (PLAUSIBLE).
- shell id 0x74 (damage, cshell+0x1f7d20): its first 96 bits are the hit direction stored at controller
  +0x1ec..+0x1f4, used by knockback (6.9).
- shell id 0xa3 `[kick:3 x f32]` (cshell+0x1f4e80 -> cshell+0x1f05f0): added to controller+0xa0..+0xa8, each
  component capped at 10 (PLAUSIBLY a camera shake accumulator; its reader was not traced).
- `S2C_EVAR` for the step height (section 2); it must be sent, because a missing variable reads 0 (section 2, correction).

## 13. Camera

**13.1 Camera variables** registered by the controller (cshell+0x1f6fb0, defaults PROVEN; only the ones with a
reader in the code read are listed with their use):

| our name | key id | handle VA | default | reader / use |
|---|---|---|---|---|
| `SEND_RATE` | 9762f149b0141274/9 | 0x3759bf88 | 15.0 | player update rate (12.2); cshell+0x5f7f0, cshell+0x1fa060 |
| `TURN_RATE` | 2d1f85500c158b43/14 | 0x3759c054 | 4.5 | keyboard turn, rad/s (13.2) |
| `TURN_RATE_FAST` | 8d410f312693d6c9/12 | 0x3759c060 | 6.9 | keyboard turn while running |
| `LOOK_RATE` | af2c3fededbf2f4b/10 | 0x3759c06c | 2.5 | keyboard look up / down, rad/s |
| `ADAPTIVE_MOUSE` | 101e4adcb996a377/13 | 0x3759c1a4 | 0 | small-motion curve on / off (13.2) |
| `ADAPTIVE_MAX` | d5dbea1ab7f97101/22 | 0x3759c1b0 | 0.4 | threshold of that curve |
| `CROUCH_CAM_DIP` | d20ee0760feebbe5/17 | 0x3759c1e0 | -13.0 | crouch eye offset (13.4); cshell+0x1f2650 |
| `CAM_MOVE_UP_TIME` | 707859108df8d13a/16 | 0x375ace6c | 0.1 | step-up smoothing (13.4; registered by cshell+0x278260) |
| `CAM_MOVE_UP_MIN_VEL` | d0ad3bbb3c76f2d9/18 | 0x375ace78 | 140.0 | step-up smoothing |
| `CAM_MOVE_MAX_YDIFF` | 91b947aa94b0e130/18 | 0x375ace84 | 30.0 | step-up smoothing |
| `FOV_X`, `FOV_Y` | 1c4b6d24bdb90bbe/4, c9b354de99d75d38/4 | 0x3759bfc4, 0x3759bfd0 | 90, 75 | renderer (out of scope) |

Engine variables looked up by key at use time (13.2): "mouse look" 6a4f76e39f9fd1f9/9 and "invert Y"
3e472aecd93081eb/12.

Registered but without a reader in the CShell code (immediate-operand scan of the whole code section, report):
the camera sway set (keys 563afab635a3e903/15, 35627f1783121856/15, 56e62a90a8654df9/16, 39be299e6df033be/16,
78e51d2e2121d395/26; defaults 13, 5, 12, 1.5, 0.5), the landing dip / roll / pitch set and the fall-damage set of
section 2, recoil-recover 47c9116472d71a8b/16 (0.3) and rotation interpolation 9a8a7b20d299efc8/16 (0.15). A port
need not implement them. Weapon records carry their own sway and recoil fields (loader cshell+0x176f10, record
+0x254..+0x26c and +0x2a4..+0x2b0); their use by the weapon view code is outside this spec.

**13.2 View angles** (controller+0x48 pitch, +0x4c yaw, +0x50 roll, radians; cshell+0x1f5420, PROVEN):
1. Mouse deltas `(mx, my)` come from the engine's axis input (client interface member slot +0x1c), zero while a
   menu has focus, once after controller+0x94 is set, and in analog mode.
2. Zoom scaling: `s = 1 / (3 z + 1)` with `z` = controller+0xf0 (integer zoom level); `yaw_d = mx * s`, `pitch_d =
   my * s`. The user sensitivity does not enter here: CShell computes an axis scale `scale = mul * sensitivity + base` and hands it to the engine's
   input layer through a console command (DISASM cshell+0x2a4e26..0x2a4e8e), so `mx`, `my` already carry it. The CShell half is
   DISASM-graded (Appendix Z M8); how the engine multiplies device deltas by it is UNPROVEN (PLAUSIBLY `delta * scale`, which makes
   the scale radians per relative input unit). At the default sensitivity 5: `scale = 5 * 0.000225 + 0.00001 = 0.001135` (V-M12).
3. Adaptive curve (off by default): when `|yaw_d| < ADAPTIVE_MAX`, `yaw_d = sign(yaw_d) * f(yaw_d) * ADAPTIVE_MAX`
   with `f(x) = (1 - sin(pi x + pi/2)) / 2` for `x < 0.5`, else `x` (cshell+0x19f050; the argument passed is
   not visible, PLAUSIBLY `|yaw_d| / ADAPTIVE_MAX`).
4. `yaw += yaw_d`. Keyboard turn (control bits of the controller copy, controller+0x2c = mover control word): bit
   0x08 `yaw -= rate * dt`, bit 0x04 `yaw += rate * dt` (`rate = TURN_RATE_FAST` with `RUN`, else `TURN_RATE`),
   not while `STRAFE_MOD`. These bits have no writer in the mover (5.1), so keyboard turning is inactive unless
   another path sets them (UNPROVEN).
5. Pitch: with the engine variable "mouse look" on (looked up by key through the engine, key id in 13.1) or
   controller+0x144 forced: `pitch += pitch_d` (`-=` with the "invert" variable). Without mouse look, pitch
   recentres towards 0 by a rate from cshell+0x47d80 (not read). Look commands 0x10 / 0x11 (control bits 0x20000 /
   0x40000) change pitch by `-+ LOOK_RATE * dt`.
6. Clamps: `|pitch| <= 0.68539816` in the camera mode where camera+0x54 is set; always `|pitch| <= 1.4707963`
   (pi/2 - 0.1).
7. Body angles: controller+0x68 (pitch) := 0, controller+0x70 := roll; the body rotation used by `move_law` is
   built from controller+0x68..+0x70 (cshell+0x1f5d10) and the camera rotation from +0x48..+0x50
   (cshell+0x1f5cd0); both through the Euler builder cshell+0x1b700 (rotation order PLAUSIBLY yaw about +Y, pitch
   about +X, roll about +Z; not fully decoded).

**13.3 Recoil** (cshell+0x1f3070, PROVEN shape): pending vertical recoil controller+0x54 and horizontal recoil
controller+0x58 are moved into the **view angles** (pitch +0x48, yaw +0x4c) over time: per frame `step = c *
rate * dt` (rate from the weapon's recoil record: the "while firing" rate when less than the record's window in
ms has passed since the last shot (controller+0x60, `GetTickCount`), else the "after firing" rate; `c` is a value
the decompiler lost, UNPROVEN), clamped to the pending amount; `pending -= step / 2`; `angle += step`. The
horizontal step is additionally multiplied by `1 + |pending| / (max_wag * pi / 180)` (weapon record +0x304). A
camera kick-back distance controller+0x64 decays by `dt * weapon(+0x32c) * 5` and pushes the camera back along the
view direction by `10 *` that value (13.4). Recoil therefore changes the aim that is sent to the server; it is
applied after the mouse and before the body rotation is derived.

**13.4 Eye position** (cshell+0x1f9c40 and cshell+0x2763e0, PROVEN shape):
- First-person base: the camera helper (controller+0x130, class ctor cshell+0x276290) takes the position of its
  target object plus `(fwd_off * dir.x, eye_h, fwd_off * dir.z)` with `fwd_off` = helper+0xc4 and `eye_h` =
  helper+0xc8 (both initialised to 0; no writer found, UNPROVEN: the eye height source is open, see V-M9).
- Step-up smoothing (walking mode, not airborne): when the new eye y is more than 1 above the last one:
  `diff = new - last`; if `diff > CAM_MOVE_MAX_YDIFF`, `last += diff - CAM_MOVE_MAX_YDIFF` and `diff =
  CAM_MOVE_MAX_YDIFF`; `vel = max(diff / CAM_MOVE_UP_TIME, CAM_MOVE_UP_MIN_VEL)`; `y = last + vel * dt` while that
  is still more than 1 below the new value, else the new value.
- Head bob: `y += bob * scale` (cshell+0x2b0780, `scale` = a console variable at handle VA 0x375b47e4).
- Camera effect sum: `+ (shell+0x1efc, shell+0x1f00, shell+0x1f04)` offsets and `(shell+0x1ef0..+0x1ef8)` angle
  offsets summed over up to 30 active wave effects (cshell+0x2b17e0; e.g. damage kicks).
- Crouch dip `crouch_y` (controller+0x108, cshell+0x1f2650): while crouched, `crouch_y = -75 * (now - t0)` from
  the crouch start (first frame counts one frame), floored at `CROUCH_CAM_DIP`; after standing up, `crouch_y +=
  75 * (now - t1)` every frame until it reaches 0 (an accelerating return). Rates -75 / +75 are hard-coded
  (controller+0x10c / +0x110, cshell+0x1f20b0). Disabled (0) on ladders, in fly modes and controller states 2, 3,
  5, 6.
- Kick-back: `pos -= view_dir * 10 * controller+0x64`.

**13.5 Field of view**: `FOV_X` 90 and `FOV_Y` 75 defaults (registered by cshell+0x1f6fb0; their use by the renderer
is out of scope).


## 14. Stance / animation state as it drives movement

The movement code has no animation state machine of its own. The stance inputs are the control bits (5.1) and
the server-set box (9.2). What depends on them (PROVEN in the sections cited):

| state | how it is entered | movement effect |
|---|---|---|
| standing walk | default | `WALK_SPEED`, ramp 6.5 |
| run | `RUN` (command 7 XOR run lock) | `run_speed()` |
| sprint | `SPRINT` (5.5) | sprint formula of 7, stamina handled by the object at VA 0x3759e110 |
| crouch | `CROUCH` (commands 5 / 0x5c) | `CROUCH_SPEED`, `a0 * 0.5`, no ramp, no jump, no run sampling, camera dip 13.4, stand-up test 5.4 |
| airborne (jump) | jump 6.7 | `a0 * AIR_CONTROL`, horizontal-only speed cap, hop penalty |
| falling (no jump) | 10.2 | full `a0`, horizontal-only speed cap |
| ladder | ladder container | 11.4 |
| fly modes 1, 2, 4, 11 | controller+0xe0 | 3D acceleration along the view (no climbing by pitch), jump / crouch move up / down |
| spectating | controller+0xac | 6.4 / 6.6 free-fly |
| frozen | 0xa0 / 0x0080, or controller state 3 | 12.4, 6.10 |

The server learns the stance from the player update (`UDP_SESSION_2009_B.md` 10.3; which field carries it is
UNPROVEN) and imposes it through the box it sends (12.4). Animation selection from these states is outside this spec.

## 15. Determinism notes for the shared simulation crate

A shared server / client crate (`ca-move`, DeCombat name) should implement sections 5-11 as pure functions of
`(MoveTunables, ROM records, world geometry, previous state, inputs, dt)`. Points that decide bit-level agreement
with the 2009 client:
1. Precision: CShell keeps intermediates on the x87 stack (64-bit mantissa) and stores binary32; the engine
   integrator is SSE binary32 with `exp` through x87. A Rust port in plain f32 with the same operation order will
   be close but not bit-identical to the 2009 client; it can be bit-identical between our own server and our own
   client if both use the same crate and avoid fused multiply-add (`-C target-feature=-fma` or explicit `mul` /
   `add`) and `f32::exp` is replaced by one shared implementation (DeCombat decision).
2. Order of summation: keep the orders noted in 8.3 and 6.3 / 6.6 (x, z, y in the engine dot products of the
   steep branch).
3. Time: the 2009 client uses variable `dt` (whole milliseconds, clamped to 0.1 for physics, unclamped for the
   ramp and hop timers). A fixed-step crate differs; server-side validation of 2009 clients must therefore use a
   tolerance band, not replay equality.
4. The 0.1-unit displacement threshold (8.1 step 5) and the 0.1 squared-speed dead zones (8.3) make slow motion
   frame-rate dependent; keep them when emulating the 2009 feel.
5. NaN cases: spectating with no key (6.6). Treat as zero.
6. The quarter `dt^2` term (8.3) is part of the 2009 behaviour; do not "fix" it in the compatibility mode.

## 16. UNPROVEN items and validation plan

**16.1 Open items**

1. Where the slide-to-stop function cshell+0x1fe940 is called from (no direct caller found).
2. Writers of mover+0x84 (jump latch enable), mover+0x308..+0x314 (knockback window), mover+0x324 (crouch height
   threshold) and the control bits 0x04 / 0x08 (keyboard turn).
3. The crouched box: whether the server sends it (0xa0 / 0x0020) when the stance changes.
4. Engine sub-tests not read: capsule vs blocker polygon (engine+0xc4bf0), box vs box (engine+0x75860),
   cylinder vs polygon edge (engine+0x76120), touch pass (engine+0x79450), riders (engine+0x71020).
5. BSP node child order relative to the file fields (9.8): the direction is OBSERVED over the ROM worlds (negative side first); only the confirmation against the loader engine+0x33060 is open.
6. Eye height (13.4): **settled on the wire (PROVEN-CAPTURE, tdm1)** as the 0x64 `eye_offset`: (0, 33, 0) standing and (0, 20, 0) crouched above the body position, blended over about 0.2 s, with the box half extents (15, 50, 15); the
   0x82 `origin` equals the trailer position plus this offset. That it is also the render camera height is PLAUSIBLE; the writer of the helper's `eye_h` / `fwd_off` was not found (client-slice pass, 2026-10-24) and the head-bob
   amplitude source (handle VA 0x375b47e4) is open. DeCombat's CLI-3 stopgap (0.8 x the half height) is superseded by these values once approved (proposal: eye (0, 33, 0) standing, (0, 20, 0) crouched).
7. Keys of the weapon / item speed fields (3.3, 3.4).
8. Meaning of client interface slot +0x178 (8.4) and slot +0x134 (2: server variable read, PLAUSIBLE).
9. The engine's input-side mouse scaling (13.2): the CShell half is read (scale formula and defaults, Appendix Z M8); open is how the engine
   applies the scale to device deltas and whether the "input rate" variable (default 12.0, read next to the sensitivity at cshell+0x21a90) changes that (V-M12).
10. Euler convention of cshell+0x1b700 (13.2).

**16.2 Validation plan** (black-box, scheduled by the coordinator; no game window in this wave)

| id | rule | method | pass condition |
|---|---|---|---|
| V-M1 | 7 speeds | capture 0x64 updates while walking, running, crouching in a straight line on flat ground | horizontal speed plateaus at 70 / 285 / 50 (times `1 - p` for the loadout) |
| V-M2 | 6.5 ramp | same capture from standstill | speed curve matches the ramp 500 -> 3000 over about 0.42 s, first frame at 500 |
| V-M3 | 8.3 gravity / jump | jump on flat ground, log `pos.y`, `vel.y` | `vel.y` starts at `330 (1 + fig_jump_bonus)` and decreases by 1100 per second; with the dt^2/4 displacement and the dropped-displacement rule of 8.1 step 5 the apex is 50.9 units at 60 frames/s, 50.2 at 100 (50.3 when the 0.1-unit drop is disabled), 57.8 at 10 (exact parabola: 49.5; dc-sim simulation, Appendix Z M1; capture still UNPROVEN) |
| V-M4 | 6.7 hop penalty | chain jumps | second take-off speed `330 * 0.3`-ish unless 0.58 s passed |
| V-M5 | 8.4 friction | release keys at full run | decay `exp(-5t)` plus the stop rule |
| V-M6 | 9.7 step | walk into steps of 20, 30, 31, 40 units (a test map, DeCombat) | climbs steps up to about 30.5 (`STEP_HEIGHT + 0.5`), blocked above |
| V-M7 | 9.2 crouch box | capture 0xa0 traffic from the reference stack while crouching | presence or absence of a 0x0020 block |
| V-M8 | 12.2 trailer | decode the 0x64 trailer | position equals the client position, `move_seq` equals the last 0xa0 seq |
| V-M9 | 13.4 eye height | screenshot comparison standing / crouched against a known wall mark | fits a fixed eye offset per stance |
| V-M10 | 9.8 / 9.9 / 9.10 | Rust collision against a ROM world vs logged client positions along a wall slide, and along sub-unit lips between floor models | no penetration, slide direction matches; whether the client also walks into the lips of 9.10 |
| V-M11 | 6.4 / 6.5 air control | strafe in the air after a jump, log the velocity change per frame | UNPROVEN: 500..3000 x `hop` per second as written (ramp value, `a0` discarded by the normalisation); about 300 if `AIR_CONTROL` acted (Appendix Z M2) |
| V-M12 | 13.2 step 2 mouse scale | in the real client at the default sensitivity, move the mouse a known number of raw counts (scripted input injection) and read the yaw change from the 0x64 stream | yaw change = counts x 0.001135 rad (x `1 / (3 z + 1)` when zoomed); a mismatch means the engine applies more than `delta * scale` |

**Capture results (PROVEN-CAPTURE; the client's 0x64 trailer positions and velocities at 15 Hz; speeds are horizontal; tdm1 unless stated).**

| id | result | verdict |
|---|---|---|
| V-M1 | the plateaus depend on the **drawn weapon**: weapon 0 run 257.1 / sprint 358.5; weapon 145 run 230.7 / sprint 318.0; weapon 164 walk 65.6, crouch 45.6, run 250.4, sprint 347.0; weapon 7 run 248.6 / sprint 345.7. Forward, side and back runs reach the same plateau. Weapon 164's ratios to 70 / 50 / 285 are 0.937, 0.911, 0.879 (not one common factor) | DIFFERENT from the bare defaults, consistent with the per-weapon modifier `p` of 7 step 5 (whether the bases 70 / 285 / 50 or the modifier rule differ needs a dc-sim run with the ROM weapon and character tables) |
| V-M2 | from standstill to the run plateau in 0.27-0.34 s (nine starts; samples at 67 ms steps, for example 34, 84, 138, 183, 226) | UNDECIDED: needs a dc-sim run with friction 5.0 (the reference stack's value) and the weapon factor; the samples are the test input |
| V-M3 | gravity **-1100.0 per second** (median of 103 airborne sample pairs, 10th-90th percentile -1100.1 .. -1100.0); take-off speed from the parabola through the first airborne sample 320-333 on flat ground (330.0, 330.5, 326.5, 324.6); apex 47.9-50.5 | CONFIRMED for the gravity; take-off consistent with 330 within sampling error |
| V-M4 | chained jumps only on stairs: one take-off 0.41 s after the previous reached about 263 (0.80 x 330), which matches `HOP_MIN + 0.41 s x HOP_RECOVER` = 0.3 + 0.49 = 0.79; another 0.49 s after showed no reduction (stairs make the estimate unreliable) | UNDECIDED (no flat-ground chain); the one usable case agrees with the recovery rule, not with a flat 0.3 |
| V-M5 | after releasing the move keys at full run the speed is 0.0 by the **second** sample in 7 of 7 cases (<= 0.134 s; `exp(-5t)` alone would leave about 116) | CONFIRMED that the slide-to-stop rule (6.8, `STOP_TIME` 0.1) governs the stop in the live client (answers 16.1 item 1 in part: the rule runs; its caller is still unknown) |
| V-M6 | not tested | UNDECIDED |
| V-M7 | no 0xa0 during four crouch periods; the client crouched anyway | CONFIRMED (the crouch box needs no server message) |
| V-M8 | the trailer decodes as written (12.2) | CONFIRMED |
| V-M9 | `eye_offset` (0, 33, 0) standing, (0, 20, 0) crouched, box half height 50 | settled on the wire; a screenshot is no longer needed for the offset |
| V-M10 | the trailer positions of tdm1 (2,338 samples, ladder and stairs included) and of tdm4 (1,103) are the input for this test | UNDECIDED (needs the dc-sim run) |
| V-M11 | not isolated | UNDECIDED |
| V-M12 | no scripted input (tdm2's yaw was broken by a corrupt mouse delta; tdm4 turned 43.96 rad without a reference count) | UNDECIDED |

Also: 69 ladder updates climb at vertical speed 201-208 (7: ladder `s` = 200; 6.3: `v.y` capped at 200), CONFIRMED within one frame of integration. The two falls of tdm1 (vertical speed -766 and -825 at the landing) were
answered by the reference stack with fall damage; the rule is server policy.

## Appendix Z. Evidence folded from the dc-sim addendum (2026-10-24)

Source: the dc-sim implementation (branch sim-movement), counts only, no ROM values. Grades as in the sections
cited; nothing here upgrades an UNPROVEN item.

- **M1 (V-M3, 8.1 step 5).** The earlier apex numbers (50.88 / 50.33 / 57.75 at 60 / 100 / 10 frames/s) came from a
  scratch script that used only `out = v dt + a dt^2/4`. With the dead zones of 8.3 and the dropped displacement of
  8.1 step 5 (`|out|^2 <= 0.01`: the body does not move, the velocity is kept) the last upward step at 100
  frames/s (`11 * 0.01 - 0.0275 = 0.0825` units) is dropped, so the apex is 50.2425. At 60 and 10 frames/s every
  upward step is above 0.1 units and the numbers stay 50.875 and 57.75. With the drop disabled the implementation
  reproduces 50.325 exactly (dc-sim test `jump_apex_matches_the_spec_at_60_100_and_10_hz`). Grade: OBSERVED in a
  simulation of the PROVEN rules; the client capture V-M3 is still open.
- **M2 (6.4 / 6.5).** Test `airborne_acceleration_is_the_ramp_value_times_hop`. Expected values for V-M11: 500..3000
  x `hop` per second as written, 300 if air control acted. Grade: UNPROVEN (needs the DISASM re-check or capture).
- **M3 (8.3).** Standing REST before the steep test: OBSERVED in dc-sim behaviour; follows from the PROVEN pseudo-code.
- **M4 (9.8).** Child-order counts as quoted in 9.8, measured with dc-world's parser over the 21 worlds. Same
  measurement as `FMT_DAT85.md` Appendix Z D3.
- **M5 (9.9a).** Implementation notes of dc-sim, not claims about 2009 behaviour.
- **M6 (9.10).** ROM spawn test counts as quoted; V-M10 is the suggested capture.
- **M7 (9.2).** The stand clip key is 7f007bf2ff2bcc78/3 (already in 9.2); the count of carrying models is OBSERVED.
- **M8 (13.2 step 2, 16.1 item 9; client-slice pass).** DISASM cshell+0x2a4ca0 (the function), cshell+0x2a4e26..0x2a4e8e (`fld mul; fmul
  sensitivity; fadd base`), defaults at cshell+0x2a50b5..0x2a50ee: `base` bits 0x3727c5ac = 0.00001 (key id `2493c1bd78dd8e67/14`),
  `mul` bits 0x396bedfa = 0.000225 (key id `994058314cae1a83/19`); the sensitivity variable has key id `3c39cb62ad3cb2c5/16`, default 5.0
  stored as an integer (cshell+0x21a90, cshell+0x112800, cshell+0x1142c0), profile default 5 (cshell+0x1ec900), written back as a
  float of the integer (cshell+0x1e94a0). The command (client interface slot +0x208) sets the scale on the two mouse-device input
  objects whose kind field is 1 and 2 (PLAUSIBLY X and Y). "Mouse look" default 1, "invert" default 0 (cshell+0x21a90). DeCombat's
  client uses `radians = counts * scale` (CLI-5, proposed) until V-M12 is run. Eye height B2: see 16.1 item 6.

## Appendix C: capture notes (spec-fold-4, 2026-10-25)

Source: `CAPTURE_tdm1_GAME_VALIDATION.md` section 4 (and 2.2 for the 0x64 stream), `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md` section 5, `CAPTURE_tdm4_KILL_VALIDATION.md` section 2.2 and 5, `CAPTURE_tdm4_WEAPON_FORMS.md` section 4
(evidence records, kept in full). All rows are the real client's own bytes (PROVEN-CAPTURE) unless marked.

- **M9 (5.1).** `FIRE` 0x400 is the fire-held command, not an aim bit; `JUMP` never appears in a sent word.
- **M10 (5.5, 7).** The server's 0xca `[allowed]` is the byte +0x1f0 of the sprint test. Without it the client reports `SPRINT` but runs at run speed (22 updates sampled before the echo).
- **M11 (12.1, 12.2).** Client authority confirmed on the wire (tdm1: no correction in 157 s). Trailer, `move_seq` echo, own-clock `time`, `standing` ids and the 15 Hz rate (moving or standing) confirmed (V-M8).
- **M12 (12.3).** Event 3 follows event 1 one frame later; landings come from the trailer.
- **M13 (13.4, 16.1 item 6).** Eye offset (0, 33, 0) / (0, 20, 0).
- **M14 (16.2).** V-M3 gravity confirmed (-1100.0 per second); V-M5 stop rule confirmed; V-M1 plateaus depend on the weapon; V-M7 confirmed; V-M2, V-M4, V-M6, V-M10, V-M11, V-M12 undecided.
- **M15 (13.2 step 2).** A single mouse delta of about 2^31 units drove the unwrapped yaw to 2,426,654 rad in one session (tdm2, full-monitor windowed mode at 2560x1440 with the high-DPI layer); the mouse scale of V-M12 could not be measured. A server may fold the yaw and send 0x6a kind 0 (proposed CV2-5).

Count: PROVEN-CAPTURE items 7 (M9, M10, M11, M12, M13, M14 as a group of confirmed rows, M15 as the observation); undecided rows 6 (V-M2, V-M4, V-M6, V-M10, V-M11, V-M12).

## Appendix F: fold record (spec-fold-5, 2026-10-25)

Source: `reports/client-reference stack.md` section 4. A client written from this spec (`dc-client` with `dc-sim`) sent 11,061 updates whose widths, field order and flag words
(0x1021 / 0x1121 / 0x1421 / 0x1521) equal the real client's, with the eye offset (0, 33, 0) and `aim_q = Ry(yaw + aux1) * Rx(aux0)` in every update that carries them (RUN; the real forms
are PROVEN-CAPTURE, 12.2, 16.1 item 6). Its remaining differences are properties of the real client already stated in 12.2: `surface` 71 / 10 / 20 / 23 on the ground and 200 on ladders,
`standing` ids on the ground and -1 / -1 only airborne, the trailer `time` on the client's own clock (62-138 s at the join), the body yaw unwrapped, and the `JUMP` bit never on the wire
(our simulation keeps it for one 60 Hz tick: 33 of 8,251 updates; harmless, RUN). Not exercised by our client: crouch, sprint, ladders (control bits and the crouched eye offset are unit-tested
only). No text of sections 1-16 was changed by this fold.

## Appendix H: fold record (spec-fold-7, 2026-10-25)

Source: `reports/client-weapons.md`. 12.2 gains a paragraph on `weapon_f` (the spread float of the 0x64 head; law not read, CLI-31 a stand-in) and the
fire bits (CLI-33). No other text of sections 1-16 was changed; no grade changed. Before this fold the spec named `weapon_f` nowhere (C2S 2.1 had the field); the paragraph
is a cross reference, not a movement rule.

## Appendix I: fold record (spec-fold-8, 2026-10-25)

Sources: `reports/{aa-triage-world-physics, aa-fix-world-physics}.md`, `UDP_SESSION_2009_B_addendum_round2.md` G5 (now a stub),
`CAPTURE_rust3_RUSTSTACK_VALIDATION.md` 7.4 / 9.1, `CAPTURE_tdm6_VALIDATION.md` 4.3; the rows DSV-48 and DSV-53 (PENDING).

| place | change | grade |
|---|---|---|
| 2 (after the slot +0x134 paragraph), 8.1 step 2, 12.5 | a missing step-height variable reads 0 and is pushed; the staircase evidence | EVIDENCE (engine+0xd760) + PROVEN-CAPTURE (rust4 vs tdm4) |
| 12.4 (after the tdm1 paragraph) | spawn teleport height = the box dropped onto the floor; rust3's 36-unit fall | REFERENCE + SIM; PROVEN-CAPTURE |

The earlier reading of section 2 ("default 30.0 passed by the caller") stays true of the caller; what changed is that the read overwrites it. No grade was lowered.
