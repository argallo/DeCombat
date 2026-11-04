# capval-game: capture validation of the game-message, movement and TDM specs (first real-client capture tdm1)

- **Targets:** `specs/GAME_MESSAGES_2009_C2S.md` (cited `C2S §n`), `GAME_MESSAGES_2009_S2C.md` (`S2C §n`),
  `MOVEMENT_2009.md` (`MOV §n`, mainly §12 and §16.2), `GAME_MODES_2009.md` (`GM §n`, TDM chapter 5.1 and 4.1 / 4.2),
  and the rows of `DC_DEFAULTS.md` they lean on. Addendum style: every item quotes the current text, gives the
  evidence, a verdict and the replacement text. Nothing here is folded into a spec yet.
- **Evidence:** `captures/20261025_000014_tdm1/capture.jsonl` (captap format 1). Cited as
  `OBSERVED: tdm1 seq N`, where N is the capture event `seq` of the datagram (or of the last datagram of a reliable
  block group) that carried the message. Conn 5 is the real 2009 client's UDP game link to the reference game server
  through the relay; conn 1 / 4 its auth and lobby TCP (used only to cross-check identity fields). The bot of conn 2 / 3
  stayed in the lobby and never joined the game link: this is a **solo** TDM session (no second player in the world).
- **Grades used here:**
  - **PROVEN-CAPTURE**: the real client's own bytes (client -> server), or the client's reaction to a server message
    (its next messages prove what it accepted).
  - **REFERENCE-CONFIRMED**: a server -> client layout of the spec into which the reference stack's bytes parse exactly
    (every bit consumed, no remainder) while the client kept playing. This shows the reference stack and the spec agree; it is
    not evidence of what the 2009 server sent.
  - **REFERENCE**: behaviour of the reference stack, recorded for comparison only. Server rule values are never taken from it
    (CLEANROOM §2, REFERENCE_TESTING §1).
- **Method:** a scratch decoder (scratchpad, never committed) built on `dc_wire::udp::decode_link_datagram`
  (UDP_TRANSPORT_2009 §6-§9) de-duplicated reliable blocks by `rel_seq`, ordered them and joined `tail_cont` parts;
  13,564 datagrams, 0 `check8` failures, 0 reliable gaps, 0 resend mismatches. Shell fields were read with an LSB-first
  bit reader equivalent to `dc_wire::udp::BitReader`, following the spec layouts (the `dc-botclient` readers
  `read_player_info`, `read_mover_update`, `read_view_angles`, `read_match_settings` give the same fields).
- **No ROM data:** world, model, file and variable names that the session carries are not reproduced; strings are
  given by length only. The user's name and chat text are not reproduced either.

## 0. Session in one paragraph

Link opens at t = 133.31 s (OOB join, seq 266). World load S6 at 139.47 (seq 395). Server shell 0x72 at 139.81
(seq 406); the client answers 0x72 + 0x85 in the same 16 ms tick (seq 408). Spawn set at 139.91 (seq 410 .. 448:
0x6b, 0x6a, 0x67 = 1, 0x66, 0xb2, 0x7f, 0xa0, 0x43, 0x44, 0xb5, 0x45, then the object group that creates the own
character). The client sends its reliable 0x64 state form and 0x87 at 139.95 (seq 457) and plays 157.3 s: 2,338
unreliable player updates, 83 shots with two weapons plus one grenade throw, 4 reloads, 20 weapon switches, 27 jumps,
4 crouch periods, ladder use, 56 sprint changes, two falls that the reference stack answered with damage (health 100 -> 99.06 ->
77.85; no death), two chat lines to all. It leaves through the leave-match box at 297.22 (0xbd, seq 13820), sends
`C2S_BYE` 16 ms later (seq 13826), receives `CTL_BYE_CODE` 5 four times 100 ms apart (seq 13829-13832) and sends its own
transport bye four times 9.0 s later (seq 13836-13839), when its lobby link shows it back on the room screen.
**There was no death, no kill and no round end in this capture**: every respawn, kill and result-screen item stays
UNDECIDED (section 9).

## 1. Client -> server shell ids seen (all 14)

| id | our name | count | R / U | first seq (t) | last seq (t) | bits (each) | verdict of the layout |
|---|---|---|---|---|---|---|---|
| 0x72 | `UP_PLAYER_INFO` | 1 | R | 408 (139.812) | 408 | 643 | CONFIRMED, PROVEN-CAPTURE (2.12) |
| 0x85 | `UP_CLIENT_OPTIONS` | 1 | R | 408 (139.812) | 408 | 25 | CONFIRMED (2.13) |
| 0x76 | `UP_MOVE_EVENT` / `UP_WEAPON_EVENT` | 642 | R | 457 (139.953) | 11762 (271.000) | 32 (638), 40 (4) | CONFIRMED layout, DIFFERENT meaning of event 3 (2.3, 2.10) |
| 0x7f | `UP_WEAPON_CHANGE` | 20 | R | 457 | 11762 | 56 | CONFIRMED, `aux` settled (2.9) |
| 0x64 | `UP_PLAYER_UPDATE` | 1 R + 2338 U | R / U | 457 | 13818 (297.218) | 768..1024 | CONFIRMED, derived layout; `yaw8c` DIFFERENT (2.2) |
| 0x87 | `UP_WORLD_READY` | 1 | R | 457 | 457 | 16 | CONFIRMED (2.14) |
| 0xca | `UP_SPRINT_STATE` | 56 | R | 467 (140.000) | 10949 (261.156) | 18 | CONFIRMED (2.4) |
| 0x80 | `UP_WEAPON_SOUND` | 22 | R | 531 (140.687) | 11784 | 144 | CONFIRMED (2.8) |
| 0x8c | `UP_ROSTER_QUERY` | 1 | R | 552 (140.906) | 552 | 48 | CONFIRMED layout, DIFFERENT trigger detail (2.15) |
| 0x4d | `UP_THROW_RELEASE` | 1 | R | 3293 (168.328) | 3293 | 16 | CONFIRMED, plus the 0x82 that follows (2.11) |
| 0x82 | `UP_FIRE` | 83 | R | 3310 (168.531) | 13811 (297.140) | 623 | CONFIRMED layout; `shot_no` DIFFERENT (2.6) |
| 0x83 | `UP_RELOAD` | 4 | R | 5198 (191.203) | 9880 (247.828) | 32 | CONFIRMED (2.7) |
| 0x68 | `UP_CHAT` | 2 | R | 12410 (279.171) | 12635 (282.187) | 104, 88 | CONFIRMED (2.18) |
| 0xbd | `UP_LEAVE_MATCH` | 1 | R | 13820 (297.218) | 13820 | 16 | CONFIRMED (2.16) |

Never sent in this session (each consistent with the spec's trigger conditions, negative evidence only): 0x33, 0x59,
0x60, 0x61, 0x69, 0x6e, 0x71, 0x75, 0x81, 0x84, 0x89, 0x8b, 0x96, 0x98, 0x99, 0xa7..0xa9, 0xac, 0xad, 0xae, 0xb0,
0xb8, 0xba, 0xbe, 0xc3, 0xc5, 0xcc, 0xcd, 0xce, 0xd1, 0xd2, 0xd5, 0xd8, 0xdb. Every reliable/unreliable choice
matches C2S §1.3: everything R except the unreliable form of 0x64.

Join order on the client's reliable channel (blocks, OBSERVED tdm1 seq 269..467): `C2S_HELLO` (block 1); file acks;
`C2S_LOAD_STATE(0)` + `C2S_RATE` (block 33); `C2S_LOAD_STATE(1)` (34); file acks + 0x72 + 0x85 (35); four pairs
`0x76 [3][2]` + `0x7f [0][0x0091][0x0091]`, the 0x64 state form `[0x0010][1]`, then 0x87 (block 38); 0xca (39).

## 2. GAME_MESSAGES_2009_C2S.md items

### 2.1 C2S §1.3, the list of messages a TDM server must handle

- **Spec:** "Messages a DeCombat game server must handle for a playable TDM round: 0x64, 0x72, 0x76, 0x7f, 0x80, 0x81,
  0x82, 0x83, 0x85, 0x87, 0x89 (or 0xc5 / 0xd2 by game type), 0x68, 0xbd, 0x8c, 0x60 / 0x61 (observer camera), 0xc3
  (only when the server enabled it)."
- **Evidence:** the 14 ids of section 1. Two of them are not in the list: 0x4d (1, the grenade release, followed 203 ms
  later by the grenade's own 0x82) and 0xca (56).
- **Verdict:** CONFIRMED with an addition (PROVEN-CAPTURE).
- **Replacement text:** "... 0x64, 0x72, 0x76, 0x7f, 0x80, 0x81, 0x82, 0x83, 0x85, 0x87, 0x89 (or 0xc5 / 0xd2), 0x68,
  0xbd, 0x8c, 0x60 / 0x61, 0xc3 (only when enabled), **0x4d** (throw release: the next 0x82 carries the thrown weapon's
  ray, 2.11) and **0xca** (sprint state; may be ignored). OBSERVED tdm1: a solo TDM session sent exactly 0x4d, 0x64,
  0x68, 0x72, 0x76, 0x7f, 0x80, 0x82, 0x83, 0x85, 0x87, 0x8c, 0xbd, 0xca."

### 2.2 C2S §2.1 / MOV §12.2: `UP_PLAYER_UPDATE` (0x64)

**(a) Gate and reliable state form.**
- **Spec:** "nothing is sent unless the own player state (`local_player`+0x30) is 1"; reliable state form
  `[pu_flags:u16 = 0x0010][state:u8]` "sent first in the same frame when the controller state byte ... differs".
- **Evidence:** the first 0x64 of any form follows the server's 0x67 `[1]` (seq 410, the only 0x67 of the session).
  One reliable state form, `0b 64 10 00 01` (seq 457, block 38, right before 0x87). No 0x64 after the 0xbd.
- **Verdict:** CONFIRMED (PROVEN-CAPTURE). The state byte observed is 1. The spec text stands.

**(b) Rate.**
- **Spec:** "at least `1 / R` seconds have passed since the last unreliable send ... `R` ... **15.0** when it is
  missing"; C2S §12.2 V-C6 "0x64 rate about 15 per second when moving".
- **Evidence:** 2,338 unreliable updates between client times 138.197 and 295.504 s: 14.86 per second. Gaps measured on
  the trailer clock: 65-70 ms in 2,335 of 2,337 intervals, one 100 ms (the second update) and one 438 ms (a client
  stall at t = 161.5). The mean gap is the same moving (67.1 ms, 1,323 intervals) and standing (67.5 ms, 1,014).
- **Verdict:** CONFIRMED (PROVEN-CAPTURE), with a precision: the client sends at 15 Hz **whether or not it moves**.
- **Replacement text (V-C6):** "0x64 at 15 per second (1/15 s, quantised to frames: 65-70 ms), moving or standing".

**(c) Layout (closes U11).**
- **Spec:** derived layout `[pu_flags]` view block / `aux` / `eye_offset` / `aim_q` / `weapon_f` / `cam_point` /
  `[control:u32]` / trailer; "PLAUSIBLE that the base variant never runs in this build".
- **Evidence:** all 2,338 unreliable updates parse with the derived layout to **exactly** their last bit (0 bits
  remaining in every one); sizes 768, 800, 864, 896, 928, 992, 1024 bits. Head of the first one (seq 457):
  `0b 64 21 15 db 00 00 00 00 00 c0 67 ab 40 00 00 00 00` (`pu_flags` 0x1521, `yaw8c` 0xdb).
- **Verdict:** CONFIRMED (PROVEN-CAPTURE). DC-C2S-2 (derived layout only) is right. U11 closed.

**(d) Flag bits.**
- **Spec:** flag table of C2S §2.1 (0x0001 eye offset, 0x0004 / 0x0008 camera mode change, 0x0020 / 0x0040 / 0x0080
  view block, 0x0100 aim, 0x0200 weapon float, 0x0400 camera point, 0x0800 / 0x1000 / 0x2000 no field).
- **Evidence (counts of 2,338):** 0x0001 2338, 0x0020 2338, 0x1000 2338, 0x0100 1472, 0x0400 1302, 0x0200 491,
  0x2000 95; never 0x0004, 0x0008, 0x0040, 0x0080, 0x0800, 0x4000, 0x8000 (0x0010 only in the reliable form).
  The 0x2000 runs coincide with the automatic-fire bursts of weapon 145 (18 runs, each inside a burst of 0x82 or on
  the grenade frame); weapon 164 bursts set it only once.
- **Verdict:** CONFIRMED for the fields. For U10: 0x0800 is never set in this session (and the 0x82 `moving` bit, which
  C2S §4.1 says reads the same two fields, is 0 in all 83 shots, so the two sources stay zero while walking, running,
  sprinting and firing); 0x1000 is set in every update; 0x2000 accompanies automatic fire (PLAUSIBLE: the drawn weapon
  is in its repeat-fire state). 0x0004 / 0x0008 need a camera-mode change (not done). U10 partly closed.
- **Replacement text (flag table, add a column "tdm1"):** 0x0001 always; 0x0020 always; 0x1000 always; 0x0100 when the
  view rotation changed; 0x0400 when the camera point moved; 0x0200 when the weapon float changed; 0x2000 during
  repeat fire; 0x0004 / 0x0008 / 0x0040 / 0x0080 / 0x0800 never seen.

**(e) `yaw8c`.**
- **Spec:** "`yaw8c = round(yaw(q) * 40.425354)` ... (PROVEN)".
- **Evidence:** against the body yaw sent in the same update, `round(wrap(yaw) * 40.425354)` disagrees in 1,381 of
  2,338 updates; **`trunc(wrap(yaw) * 40.425354)` (toward zero) with `wrap` into (-pi, pi] matches all 2,338**.
  Example seq 457: body yaw 5.356415 rad, wrapped -0.926770, times 40.425354 = -37.46, sent 0xdb = -37.
- **Verdict:** DIFFERENT (PROVEN-CAPTURE): truncation, not rounding.
- **Replacement text:** "`yaw8c = trunc(yaw * 40.425354)` toward zero, with the yaw wrapped into (-pi, pi] first
  (OBSERVED tdm1: 2,338 of 2,338 updates). The 16-bit form presumably uses the same conversion (not observed)."
  The same correction applies to `UDP_SESSION_2009_B.md` §10.3 (`yaw_c8 = round(...)`).

**(f) View block, `aux`, `aim_q`.**
- **Spec:** "angles = `local_player`+0x68, +0x6c, +0x70" (body angles), `[aux: 6 x u32]` "`local_player`+0x40c ..
  +0x420 (two 3-float vectors)", `aim_q` "rotation built from `local_player`+0x48, +0x4c, +0x50"; meanings PLAUSIBLE.
- **Evidence:** body angles are `(0, yaw, 0)` in all 2,338 updates (pitch and roll always 0.0; the yaw is **not
  wrapped**: it ranges -2.85 .. 9.50 rad). Read as floats, `aux[0]` ranges -1.277 .. 1.080 and `aux[1]` -0.7854 ..
  +0.7854 (exactly +-pi/4); `aux[2..5]` are always 0.0 or -0.0. In all 1,472 updates that carry `aim_q`,
  `aim_q = Ry(body_yaw + aux[1]) * Rx(aux[0])` to float precision (largest squared difference 4.2e-10; the other
  composition order fails), with `Ry(a) = (0, sin(a/2), 0, cos(a/2))` and `Rx(a) = (sin(a/2), 0, 0, cos(a/2))` in the
  `quat128` order x, y, z, w. The forward axis is +Z rotated; a positive `aux[0]` looks down (forward y = -sin).
- **Verdict:** CONFIRMED layout; meanings settled (PROVEN-CAPTURE): `aux[0]` = view pitch (rad, positive = down),
  `aux[1]` = view yaw minus body yaw, within +-pi/4; `aux[2..5]` = 0; the body angles carry the yaw only.
- **Replacement text:** "`body_angles` = `(0, body_yaw, 0)`, body yaw unwrapped; `aux` = `[view_pitch:f32]
  [view_yaw_offset:f32]` (offset within +-pi/4 of the body yaw) then four zero words; `aim_q = Ry(body_yaw +
  view_yaw_offset) * Rx(view_pitch)`, forward = rotated +Z (OBSERVED tdm1, 1,472 of 1,472)."

**(g) `eye_offset` (0x0001) and `cam_point` (0x0400).**
- **Spec:** "`eye_offset`: point A ... minus the body position ... Meaning PLAUSIBLE (eye or weapon point relative to
  the body)"; `cam_point` "the camera object position (camera +0x18)".
- **Evidence:** `eye_offset` = (0, 33.0, 0) in 2,236 updates and (0, 20.0, 0) in 52, all of those with `CROUCH` (0x20)
  in the control word; the 50 other values (4.0 .. 31.6) are transitions (crouching takes three updates: 33.0, 28.95,
  23.92, 20.0; landings dip it briefly). `cam_point` minus the trailer position = (0, 33.0, 0) while standing (1,302
  updates). The 0x82 `origin` equals the latest trailer position plus `eye_offset` (median distance 0.0, see 2.6).
- **Verdict:** CONFIRMED (PROVEN-CAPTURE); meaning: the first-person eye point relative to the body position, +33
  standing and +20 crouched, with the box half extents (15, 50, 15) of the reference stack's 0xa0 in force.
- **Replacement text:** "`eye_offset` = eye point minus body position: (0, 33, 0) standing, (0, 20, 0) crouched, about
  0.2 s transition (OBSERVED tdm1). `cam_point` = absolute camera position (body + eye offset in first person)."

**(h) `control` word (MOV §5.1).**
- **Spec:** the bit table FWD 0x1, BACK 0x2, JUMP 0x10, CROUCH 0x20, LEFT 0x80, RIGHT 0x100, RUN 0x200, "(aim)"
  0x400, `ANY_MOVE` 0x800 ("any bit of mask 0x19f"), 0x80000 (command 0x3d), SPRINT 0x400000.
- **Evidence (2,338 updates):** values 0x200 (770), 0xa01 (515), 0x400a01 (261), 0xb01 (132), 0x600 (120), 0x0 (102),
  0xa81 (91), 0x801 (65), 0xa80 (56), 0x400a81 (48), 0x821 (43), 0xb00 (42), 0x400b01 (39), 0x20 (19), 0xa02 (8),
  0xe01 (8), 0xa82 (6), 0x881 (5), 0x80200 (4), 0xb02 (2), 0xf01 (2). `ANY_MOVE` equals (`control & 0x19f` != 0) in
  2,338 of 2,338. `SPRINT` appears only with FWD and RUN, `CROUCH` only without RUN. **JUMP (0x10) never appears**
  although 27 jumps were made (the bit lives on the take-off frame only; 15 Hz sampling caught none). **Bit 0x400 is
  set exactly while the fire button is held**: its 39 runs coincide with the 0x82 bursts (for example the updates
  182.17-182.23 around the shot at 182.156, and 193.83-194.84 around the ten shots 193.80-194.75).
- **Verdict:** CONFIRMED for FWD, BACK, LEFT, RIGHT, RUN, CROUCH, SPRINT, ANY_MOVE (PROVEN-CAPTURE). 0x400 is
  DIFFERENT in meaning: it is the primary-fire command, not an aim. JUMP UNDECIDED (not sampled).
- **Replacement text (MOV §5.1 row 0x400):** "0x00000400 `FIRE` (command 8 = primary fire, on while the button is held;
  OBSERVED tdm1: on in every update inside a burst of 0x82); the conditions 'command 2 off, not sprinting, weapon test
  false' stay as written."

**(i) Movement trailer (MOV §12.2, V-M8).**
- **Spec:** `[move_seq:u8][pos:vec96][vel:vec96][time:f32][on_ground:u8][surface:u8][standing:u64]`; `move_seq` is
  "the sequence byte of the last 0xa0 / 0x20 teleport (12.4), echoed"; `standing` "the standing ids or -1 / -1".
- **Evidence:** `move_seq` = 1 in the first two updates and 2 afterwards (the reference stack's teleports carried seq 1, then 2:
  seq 410 / 468). The first update's position (-979.0, 826.19, -1653.0) is the teleport position raised by 0.12.
  Position differences agree with the mean velocity times dt (median 0.10 units, 90 % below 5.9). `time` is the
  client's own clock: 138.197 at capture t 139.953 while the server's stream time was about 0.0 (a server must use
  differences only); the 0x82 `time_ms` uses the same clock. `on_ground` 1 in 1,897, 0 in 441. `surface` 71, 10, 20,
  23 on the ground, 0 in the air, 200 on the ladder (69). `standing` = -1 / -1 in 402 updates (401 airborne, 1
  grounded); otherwise its high word is 0x5d in every update and its low word varies with the place (for example
  0x00009d27). The server never created an object 0x5d, so it is not a server object id (PLAUSIBLY a world-model index
  and a polygon index).
- **Verdict:** CONFIRMED (PROVEN-CAPTURE). V-M8 passes.
- **Replacement text:** add "`time` is the client clock, not the server's world clock; `standing`: low word varies by
  floor polygon, high word constant for the world model, -1 / -1 while airborne (OBSERVED tdm1)".

### 2.3 C2S §2.2 / MOV §12.3: `UP_MOVE_EVENT` (0x76 sub 1)

- **Spec:** "`event = 1` when the mover's jump-airborne flag (+0x80) is set, else 3"; "3 | landed"; "2 | started
  falling without a jump"; MOV §12.3 "`e = 1` jump take-off, `e = 2` started falling (no jump), `e = 3` landed".
- **Evidence:** 618 notices: event 1 x27, event 2 x282, event 3 x309 (= 27 + 282). Every event 1 is a take-off (last
  update before it grounded with vy <= 0, next update airborne with vy 244..320). In 26 of 27 jumps an event 3 follows
  **in the next client frame** (next reliable block, same 16 ms tick, before the first airborne 0x64) and the real
  landing then sends nothing (20 of the 26 send no further notice until they are on the ground again). In 1 of 27 (take-off at
  t 244.359) the event 3 came at the real landing, 0.61 s later. Event 2 is followed by an event 3 once the body is on
  the ground again; on stairs and edges the pair repeats many times within one frame-time (more than 100 pairs during
  one stair run).
- **Verdict:** layout and codes CONFIRMED (PROVEN-CAPTURE); the timing of 3 is DIFFERENT from what "landed" suggests:
  after a jump the ground test usually still finds the floor on the next frame, so the 3 is sent right after the 1 and
  the actual landing is not reported.
- **Replacement text:** "1 = jump take-off; 2 = the body lost the ground without a jump (frequent on stairs and edges);
  3 = ground contact regained. After a jump the 3 normally comes one frame after the 1 (26 of 27 in tdm1), so a landing
  after a plain jump usually has no notice. A server that wants landing events (fall damage) takes them from the 0x64
  trailer (`on_ground` 0 -> 1 and the vertical speed before it), not from event 3."

### 2.4 C2S §2.3: `UP_SPRINT_STATE` (0xca)

- **Spec:** `[sprinting:bool1][stopped:bool1]`, sent on change; (1, 0) started, (0, 1) cancelled, (0, x) stamina path.
- **Evidence:** 56 messages: transitions (0,1) to (1,0) x24, (1,0) to (0,1) x17, (1,0) to (0,0) x7, (0,0) to (0,1) x7,
  the first (0,1) right after spawn (seq 467). The `sprinting` bit of the last 0xca equals the `SPRINT` bit of every
  following 0x64 (2,337 of 2,337).
- **Verdict:** CONFIRMED (PROVEN-CAPTURE). (0, 0) is the stamina-end form; it is always followed later by (0, 1).

### 2.5 C2S §3.3 / §3.4 and Appendix A11: respawn wait and `UP_RESPAWN_REQUEST`

- **Spec:** the 7000 ms wait is armed only by server 0xd7 sub 2 or 0xdb sub 6; then 0x89 for `mode_class` 1.
- **Evidence:** no death occurred (no server 0x6d, no 0x67 other than `[1]`, health never below 77.85). The reference stack sent
  no 0xd7 and no 0xdb. The client sent no 0x89 / 0xc5 / 0xd2 / 0x60 / 0x61.
- **Verdict:** UNDECIDED (no death). DC-C2S-10's respawn timing and GMD-3's arming cannot be checked with tdm1.

### 2.6 C2S §4.1: `UP_FIRE` (0x82)

- **Spec:** "one message per pellet"; layout `[wkind:u8][weapon_id:u16][ammo_id:u16][weapon_pos:vec96][origin:vec96]
  [aim:raw128][yaw:f32][dir:vec96][seed:u8][moving:bool1][wflag:bool1][mode1:bool1][frac:u8][shot_no:u32][time_ms:u32]
  [has_extra:bool1]... [flag_c:bool1][last:bool1][has_hit:bool1] if has_hit: [hit_dist:f32][hit_obj:objref16]
  [stat:u32]"; `seed` "rand() in 2..255"; most meanings PLAUSIBLE / UNPROVEN (U1).
- **Evidence:** 83 messages, all 623 bits, all parse exactly (0 bits left). Head (seq 4436):
  `0b 82 00 91 00 91 00 76 d1 24 c5 11 7a 46 44 09 e5 dd 44 4d 25 25 c5 79`. Weapons 145 (60, ammo id 145), 164 (22,
  ammo id 164), 7 (1, ammo id 9: the grenade, see 2.11). Field values:
  - `wkind` 0 in 83 of 83 (the spec's "returns 0 in both weapon method tables" holds);
  - `last` = 1 in 83 of 83: these weapons fire one message per shot;
  - `has_hit` = 0 in 83 of 83, `has_extra` = 0, `flag_c` = 0, `moving` = 0, `wflag` = 0, `mode1` = 0;
  - `seed` 4..255 (inside 2..255);
  - `origin` = the latest 0x64 trailer position + `eye_offset` (median distance 0.0); `weapon_pos` about 10.9 units
    from `origin`; `dir` a unit vector; `aim` a unit quaternion whose forward axis is within 0.009 (median, 0.042 max) of
    `dir` (the view rotation; `dir` adds the spread);
  - `yaw` = the view yaw of the 0x64 stream (body yaw + `aux[1]`), unwrapped (values up to 7.8 rad);
  - `frac / 255` follows the 0x64 `weapon_f` float of the same moment (0 on the first shot of a burst, rising during a
    burst: 0.047 / 0.049, 0.094 / 0.098, 0.145 / 0.146 ...): `frac = trunc(spread * 255)` of the weapon's current
    spread value;
  - **`shot_no` is not a counter**: it is the bit pattern of an f32, 0x3f800000 = 1.0 for weapons 145 and 7 and
    0x3f99999a = 1.2 for weapon 164, constant per weapon;
  - `stat` (`local_player`+0x1fc) = 2, 4, 6 ... 166: it grows by exactly 2 per 0x82 across weapon changes;
  - `time_ms` = the 0x64 trailer clock x 1000 (same client clock).
- **Verdict:** layout CONFIRMED (PROVEN-CAPTURE). `shot_no` DIFFERENT (an f32 per weapon). Meanings settled for
  `origin`, `aim`, `yaw`, `frac`, `time_ms`; `stat` behaviour recorded. Multi-pellet weapons and `has_hit` = 1 not
  exercised (no shotgun, no target: solo session) -> V-C1 needs tdm2. Hits on world geometry carry `has_hit` = 0
  (PLAUSIBLY: only object hits fill `hit_obj`; a miss and a wall hit look the same on the wire).
- **Replacement text:** "`[shot_scale:f32]` weapon+0x20c (OBSERVED tdm1: 1.0 or 1.2, constant per weapon; formerly
  `shot_no:u32`)"; "`frac` = `trunc(a2 * 255)` where `a2` is the weapon's current spread (the float that 0x64 sends
  with `pu_flags` 0x0200)"; "`origin` = eye point (body position + `eye_offset`)"; "`yaw` = view yaw, unwrapped";
  "`stat` grows by 2 per fire message in tdm1 (meaning UNPROVEN)".

### 2.7 C2S §4.2: `UP_RELOAD` (0x83)

- **Spec:** `[ammo_id:u16]` weapon+0x9c, sent with the "notify" flag; "the weapon state becomes 9 (PLAUSIBLE
  'reloading')".
- **Evidence:** 4 reloads: `0b 83 91 00` (seq 5198, 8561) and `0b 83 a4 00` (seq 9502, 9880). The first two come in
  the same reliable block as the 30th 0x82 of a burst with weapon 145 (the clip ran out: an automatic reload), stat
  values 62 and 122. Each reload is followed after 100-250 ms by `0x76 [3][1][v]` (v = 30 for weapon 145, 15 for
  weapon 164, see 2.10) and by three 0x80 sounds 1, 2, 3 (or 2, 1, 3) over about 1.8 s.
- **Verdict:** CONFIRMED (PROVEN-CAPTURE). For DC-C2S-4: the automatic reload arrives **after** the last 0x82 of the clip
  (same block, later entry), so a clip mirror sees the shot before the reload.

### 2.8 C2S §4.3: `UP_WEAPON_SOUND` (0x80)

- **Spec:** `0x80: [sound_no:u8][weapon_id:u16][own_id:u8][weapon_pos:vec96]`; "`own_id` is filled by `engine_client`
  member +0x84 (the own engine client id)".
- **Evidence:** 22 messages, all 144 bits, exact parse; head `0b 80 03 91 00 00 cb db 75 c4 65 c3 55 44 74 7f cd c4`
  (seq 531). `own_id` = 0 in 22 of 22 and the server's `S2C_CLIENT_ID` gave id 0 (seq 268, `0c 00 00 00`).
  `sound_no` 3 after every draw of weapon 145 (7 of 7, 0.67-0.73 s later) and of weapon 164 (2 of 2, 0.25 s later);
  1, 2, 3 during each reload; 10 once, 0.19 s before the grenade's 0x4d (start of the throw); none after a draw of
  weapon 0. No 0x81 in the session.
- **Verdict:** CONFIRMED (PROVEN-CAPTURE), `own_id` meaning settled.

### 2.9 C2S §4.4: `UP_WEAPON_CHANGE` (0x7f)

- **Spec:** `[slot:u8][weapon_id:u16][aux:u16]`; "`aux`: a value from the new weapon (slot +0x94 or +0x7c; PLAUSIBLY its
  select animation or clip state; UNPROVEN)".
- **Evidence:** 20 messages, all 56 bits: slot / weapon / aux = 0 / 145 / 145 (10), 2 / 0 / 0 (7), 1 / 164 / 164 (2),
  3 / 7 / 9 (1). In the server's `GS_WEAPON_GIVE` (seq 410) the same weapons have `ammo_kind` 145, 0, 164, 9 in slots 0,
  2, 1, 3, and the client's 0x82 / 0x83 carry the same values as `ammo_id`. Every 0x7f is immediately preceded in the
  same block by `0x76 [3][2]` (20 of 20). Four 0x7f (slot 0) are sent at spawn (seq 457), after the server's two
  `GS_WEAPON_SWITCH` with `auto` = 1 and its two sets of four `GS_WEAPON_GIVE`.
- **Verdict:** CONFIRMED (PROVEN-CAPTURE); `aux` settled: it is the weapon's ammunition id.
- **Replacement text:** "`aux`: the new weapon's ammunition id (weapon+0x9c, the `ammo_id` of 0x82 / 0x83; OBSERVED
  tdm1, 20 of 20). The client sends `UP_WEAPON_EVENT [3][2]` (draw) right before it."

### 2.10 C2S §4.5: `UP_WEAPON_EVENT` (0x76 category 3) (U4)

- **Spec:** events 2, 3, 5 `[3][event]`; event 1 `[3][1][value]` "UNPROVEN"; 2 "draw / select (PLAUSIBLE)"; 5 "idle
  fidget (PLAUSIBLE)".
- **Evidence:** 24 messages: event 2 x20 (one with every 0x7f, see 2.9), event 1 x4 with `value` 30 / 30 (weapon 145)
  and 15 / 15 (weapon 164), each 100-250 ms after a 0x83. No event 3 and no event 5 (no 20 s idle with a weapon drawn).
  No category 5.
- **Verdict:** event 2 CONFIRMED (draw, PROVEN-CAPTURE). Event 1 = the reload animation, `value` constant per weapon
  (PLAUSIBLY the clip size). Events 3 / 5 UNDECIDED (not exercised).

### 2.11 C2S §4.6: `UP_THROW_RELEASE` (0x4d) and the throw's direction

- **Spec:** "no fields ... **Server answer:** the server creates the thrown object (... the client sends no direction
  here, the latest `UP_PLAYER_UPDATE` view angles are the PLAUSIBLE source)".
- **Evidence:** `0b 4d` at seq 3293 (t 168.328, 0.19 s after a 0x80 with sound 10 at 168.140); 203 ms later (seq 3310) one 0x82 with
  `weapon_id` 7, `ammo_id` 9, an origin, a `dir` (-0.70, 0.67, 0.24) and `has_hit` 0, followed in the same block by
  `0x76 [3][2]` and `0x7f [0][145][145]` (automatic switch back: it was the only grenade).
- **Verdict:** DIFFERENT (PROVEN-CAPTURE): the throw's ray does come from the client, as an ordinary 0x82 of the
  thrown weapon after the 0x4d.
- **Replacement text:** "The direction of the throw is the `UP_FIRE` (0x82) that follows the 0x4d with the thrown
  weapon's ids (OBSERVED tdm1: 203 ms later, `origin` = eye point, `dir` = throw direction). The server creates the
  projectile from that 0x82."

### 2.12 C2S §5.1: `UP_PLAYER_INFO` (0x72) (V-C2, U2, U12)

- **Spec:** layout of the 26 fields; sources `session_info` +0xa4 room slot (0x383 +0x12), +0x08 char_no (0x303 +0xe),
  +0x14 session index (0x301 +0x1e), +0x194 exp (0x303 +0x5e), +0x104 GP (0x303 +0x5a), +0x444 nation (0x303 +0x100).
- **Evidence:** seq 408, 643 bits, exact parse (0 bits left). Head:
  `0b 72 00 00 00 00 ea 03 00 00 04 00 00 00 00 00 00 00 00 00 00 00 01 00`. Values: `room_slot` 0, `char_no` 1002,
  `session_idx` 4, `exp` 0, `look` bytes [0,0,0,0,0,1,0,0,0,1,1,-1,1,0,-1], `text_a` / `text_b` empty, r1..r4
  [-1,0,0,-1], r5 0, `team` 1, r6 0, **r7 = 0x442f0000 (the bit pattern of 700.0 as an f32)**, `gp` 10000, `n` 4,
  loadout (145, 255, 255, 255), (164, ...), (0, ...), (7, ...) (item number = the weapon number the server uses in
  `GS_WEAPON_GIVE`; the three bytes 0xff = -1), `in_clan` 0, `word_d` 0, `nation` 0, `flag_e` 0, `flag_f` 0. Lobby
  cross-check on conn 4: 0x301 +0x1e = 4 (seq 132), 0x303 +0xe = 1002, +0x5a = 10000, +0x5e = 0, +0x100 = 0, +0x86
  (clan) = 0 (seq 146), 0x383 +0x12 = 0 (seq 195).
- **Verdict:** CONFIRMED (PROVEN-CAPTURE): fields 1-4, `gp`, `nation` and the clan bit equal the lobby values; the
  loadout entries are written as `n` says (U12: no null slot with this loadout). V-C2 passes for a non-member; the clan
  member branch is untested. U2: values recorded, meanings still UNPROVEN (one character only); r7 is PLAUSIBLY an
  f32. DC-C2S-5 / DSV-19 (identify by `char_no` / `room_slot`) would accept this client.
- **Replacement text:** "`[r7:u32]` profile record +0x1a0 (OBSERVED tdm1: 0x442f0000, an f32 bit pattern)";
  "loadout `item_no` = the weapon number of the weapon table (the `weapon` of `GS_WEAPON_GIVE`); `e1..e3` = 0xff when
  no attachment (OBSERVED tdm1)".

### 2.13 C2S §5.2: `UP_CLIENT_OPTIONS` (0x85) (U3)

- **Evidence:** `0b 85 02 01` (seq 408): `opt_a` 2, `opt_b` 1; sent in the same block as 0x72, right after it. The
  reference stack sent no 0x73, so the other trigger was not exercised.
- **Verdict:** CONFIRMED trigger and layout (PROVEN-CAPTURE); U3 (meanings) still open: toggle the options next time.

### 2.14 C2S §5.4: `UP_WORLD_READY` (0x87) (V-C5)

- **Spec:** sent once when the in-world test passes; "the server must already have sent the view object, shell 0x88 and
  a `life_state`".
- **Evidence:** exactly one 0x87 (seq 457, t 139.953). Before it the client had received 0x88 `[2]` (seq 268, link
  start), the view object (seq 406, block 36), 0x67 `[1]` (seq 410, block 37) and the object group that creates the
  own character (block 57, completed in seq 448 at 139.906, the last of these to arrive). 0x87 follows 47 ms after the
  0x67 and right after the character's create record; in the same block as the reliable 0x64 state form.
- **Verdict:** CONFIRMED (PROVEN-CAPTURE). V-C5 passes. The capture also shows that the view object id may be named
  before its create record arrives; the test waits for the record.

### 2.15 C2S §5.5: `UP_ROSTER_QUERY` (0x8c)

- **Spec:** `[pid:u32]`; "they pass 0xFFFFFFFF because they do not know the id. cshell+0x2b170 also sends one for every
  id queued in the roster's pending list".
- **Evidence:** one message `0b 8c 00 00 00 00` (seq 552, t 140.906) with **pid 0**, the client's own id. The reference stack's
  first two `GS_PLAYER_INFO` for pid 0 (seq 410, block 37) named `character` 408, whose create record arrived only in
  block 57 (seq 448). One second later the client asked for pid 0; the reference stack answered with a third 0x6b (seq 555,
  30 ms later); after that no further query.
- **Verdict:** layout CONFIRMED; trigger DIFFERENT in detail (PROVEN-CAPTURE): a 0x6b whose `character` the client does
  not know yet queues that pid, and the query carries the pid (S2C §3.2.1 "cshell+0x2bd40(pid) when character is none").
- **Replacement text:** "`pid` = the queued player id (OBSERVED tdm1: 0, the own id, about 1 s after a 0x6b whose
  `character` was not yet created on the client); 0xFFFFFFFF only from the name / object lookups."

### 2.16 C2S §5.7: `UP_LEAVE_MATCH` (0xbd) and the bye

- **Spec:** "no fields, R ... The engine disconnect (`C2S_BYE`) follows."
- **Evidence:** `0b bd` (seq 13820, t 297.218; the lobby link sends 0x394 at the same moment); `C2S_BYE` `06` in the
  next reliable block 16 ms later (seq 13826); the reference stack's `CTL_BYE_CODE` 5 x4 at 297.265 .. 297.562; the client's own
  transport bye `[1][CTL_BYE]` (datagram `c9 03`) x4 at 306.250 .. 306.546 (about 100 ms apart), 9.0 s after its
  `C2S_BYE`, when the lobby link shows it back on the room screen (0x306 / 0x388 at 306.234).
- **Verdict:** CONFIRMED (PROVEN-CAPTURE). New: the client's transport bye (4 copies) comes when its game screen closes,
  not right after `C2S_BYE`.

### 2.17 C2S §5.8: `UP_PERF_REPORT` (0xc3) off by default

- **Evidence:** the reference stack never sent server 0xc3 sub 1; the client sent no 0xc3 in 157 s.
- **Verdict:** CONFIRMED (PROVEN-CAPTURE in the negative): the reports are off until the server enables them. Whether
  the client accepts the enable with `pid` 0xFFFFFFFF (dc-dserver's `TODO_PERF_ENABLE_PID`) is untested.

### 2.18 C2S §6.1: `UP_CHAT` (0x68) (V-C4)

- **Spec:** `[text:zstr16][tag:zstr16]`, then `[scope:u8]` for chat modes 0 / 1.
- **Evidence:** two lines (seq 12410, 104 bits; seq 12635, 88 bits): text of 3 and 2 units, empty tag, `scope` 0; exact
  parse. (Text not reproduced.)
- **Verdict:** CONFIRMED for scope 0 (PROVEN-CAPTURE). Team chat (scope 1) and the radio (0xbe + 0x68) were not
  exercised.

### 2.19 Capture list C2S §12.2

| id | result |
|---|---|
| V-C1 | partly: single-shot weapons send one 0x82 per shot with `last` = 1; `seed` 4..255; no shotgun and no player target, so `has_hit` / `hit_dist` and pellets open (tdm2) |
| V-C2 | passes for a non-member (2.12); clan member open |
| V-C3 | passes for 0x83 and 0x7f and event 2; event 1 + value seen after reloads; event 5 not seen (no idle) |
| V-C4 | scope 0 passes; scope 1 and radio open |
| V-C5 | passes (2.14) |
| V-C6 | passes: 15 / s moving or not; 0xca (1, 0) / (0, 1) / (0, 0); 0x76 sub 1 events 1 and 3, with the timing of 2.3 |

### 2.20 UNPROVEN list C2S §12.1 (U1-U12)

| # | status after tdm1 |
|---|---|
| U1 | partly settled: `wkind` 0, `frac` = spread x 255, `shot_no` is an f32 (1.0 / 1.2 per weapon), `extra_*` / `flag_c` / `mode1` / `wflag` / `moving` always 0, `stat` +2 per shot, `aim` = view rotation (2.6). Still open: `extra_*` (needs a weapon that sets +0x1cd), `stat` meaning |
| U2 | values recorded (2.12); meanings open (needs a second character with another appearance) |
| U3 | values 2 / 1 recorded; meanings open |
| U4 | event 1 + value after every reload (value per weapon, PLAUSIBLY the clip size); events 3 / 5 open |
| U5 | open for the 2009 server. REFERENCE: 0x80 -> object-less shell object message (engine id 17) type 0x19 with the same `sound_no`; each 0x82 -> id 17 type 6; each jump -> id 17 type 0x19 sound 15 (section 3.15); tdm2 shows what another client receives |
| U6 | none of 0x71, 0x75, 0x96, 0x98 was sent (negative evidence only) |
| U7, U8 | not exercised (no use objects of the own team; no 0x5a) |
| U9 | n/a (no 0xac challenge sent by the reference stack; none answered) |
| U10 | partly settled (2.2 d): 0x0800 never, 0x1000 always, 0x2000 with repeat fire; 0x0004 / 0x0008 open |
| U11 | **closed**: 2,338 of 2,338 updates parse with the derived layout (2.2 c) |
| U12 | settled for this loadout: `n` = 4 entries written (2.12) |

## 3. GAME_MESSAGES_2009_S2C.md items (server bytes are the reference stack's)

Server -> client shell ids the client received, with counts (OBSERVED tdm1): 0x90 x2, 0x88 x2, 0x72, 0x9e, 0x9d, 0x7d,
0x6b x3, 0x6a, 0x67, 0x66 x16, 0xb2 x8, 0x7f x6, 0xa0 x6, 0x43, 0x44, 0xb5, 0x45, 0xca x48, 0xa2 x52 (unreliable),
0x74 x2, 0x68 x2, 0xa6 x2975 (178 reliable, 2797 unreliable); plus engine id 17 (object-less shell object messages)
x140 unreliable. **Every one whose layout the catalogue gives parses exactly** (0 bits left; `mode_class` 1 where a
layout depends on it); not checkable: the class data of 0xa6 (S2C §8.3) and 0x9d (not in the catalogue). Not received: 0x6d, 0x67 `[2]` / `[3]`, 0xd4, 0xd7, 0xdb, 0x46, 0x65, 0x3f, 0x99, 0x6c, 0xc3,
0xc8, 0xb3, 0x4a, 0x5e, 0xac, 0x73.

### 3.1 S2C §5.1 `GS_IDENTIFY_REQUEST` (0x72) -> C2S 0x72 + 0x85

- **Evidence:** `0d 72` (no body) at seq 406 (block 36); the client answers 0x72 then 0x85 in its block 35, in the same
  16 ms tick (seq 408).
- **Verdict:** CONFIRMED (PROVEN-CAPTURE, client reaction). DSV-12's 3000 ms fallback is never needed with this
  client: the answer is immediate.

### 3.2 S2C §5.1 `GS_GAME_ENDPOINT` (0x9e) and the TDM `mode_class`

- **Spec:** `[mode_class:u8]` then `[host:zstr8(256)][port:u32]`; 11.2 item 6 "The TDM `mode_class` number (read from
  the ROM at run time)".
- **Evidence:** `0d 9e 01 ...` (seq 406): `mode_class` 1, a 13-character host text, port 29917 (the game port the
  client was given). The intro block of 0x45 parses exactly with `mode_class` 1 (no caption, team bases present, no
  co-op triple), and the client went on to play.
- **Verdict:** REFERENCE-CONFIRMED (layout; value 1 for TDM, which matches GM §2's class of the TDM row). The client
  reaction that would prove class 1 on the client side (the 0x89 branch) did not occur (no death).

### 3.3 S2C §3.2 `GS_MATCH_SETTINGS` (0x7d) (V-S2C-4)

- **Evidence:** seq 406, 152 bits, exact parse: caption of 5 characters (the room owner's name), `own_team` **0**,
  `own_value` 0, `goal` 100, scores 0 / 0, `round_limit` 1, `round_no` 1. The lobby set the room goal to 100 (conn 4
  0x3035 type 8 value 100, seq 231 and 242).
- **Verdict:** REFERENCE-CONFIRMED (layout). `goal` = the room's goal; `own_team` 0 is corrected by the following own 0x6b
  (team 1); no client reaction is visible on the wire.

### 3.4 S2C §3.2.1 `GS_PLAYER_INFO` (0x6b)

- **Spec:** "`character:objref16` ... the player's world object (it must already exist on the client)"; `kind` "0 = new
  player with notice, 1 = add or refresh, 2 = update"; S2C §10 step 5 "each only after that player's character object
  exists on the receiving client".
- **Evidence:** three 0x6b for pid 0 (seq 410 twice in block 37, seq 555), 486 bits each, exact parse: `account_no`
  1002, `character` 408 (= the view object id of `S2C_VIEW_OBJECT` `07 98 01`, seq 406), the `look` block equal to the
  client's own 0x72 look bytes (the reference stack forwards them unchanged), `team` 1, `kind` **2** in all three, `has_group` 0.
  The create record of object 408 arrives only in block 57 (seq 448). The client sends `UP_ROSTER_QUERY [0]` 1.0 s
  later (2.15); the third 0x6b answers it.
- **Verdict:** layout REFERENCE-CONFIRMED. The ordering rule is CONFIRMED by the client's reaction (PROVEN-CAPTURE): a 0x6b
  whose `character` is not yet created makes the client queue the pid and ask again with 0x8c. The `look` pairing of
  C2S §5.1 (forwarded unchanged) is REFERENCE-CONFIRMED.
- **Replacement text (S2C §10 step 5):** add "If a 0x6b names a character the client has not created yet, the client
  sends `UP_ROSTER_QUERY [pid]` about 1 s later (OBSERVED tdm1); send the create record first, and answer 0x8c with
  the 0x6b of that pid."

### 3.5 S2C §4.1 `GS_PLAYER_STATE` (0x67), PD-S2C-2, and the in-world test

- **Spec:** states 1 / 2 / 3 / 6; "A server sends 0x67 with state 1 when a player spawns (including the first spawn
  that ends the loading screen, S12)"; DC-C2S-10 proposes state 3 before 0x87 and state 1 after.
- **Evidence:** one 0x67, `0d 67 01` (seq 410), **before** 0x87; the client then sent its 0x64 state form and 0x87
  (2.14) and played. No state 2 / 3 / 6 (no death).
- **Verdict:** state 1 as the S12 carrier CONFIRMED by the client's reaction (PROVEN-CAPTURE). States 2 / 3 and their
  timing (PD-S2C-2, DC-C2S-10) UNDECIDED. DC-C2S-10's "state 3 before 0x87" is not contradicted (the client was not
  given it), but it is now the untested path: the reference stack's state 1 before 0x87 is the tested one.

### 3.6 S2C §4.3 `GS_PLAYER_STATUS` (0x66) kinds

- **Evidence:** 16 messages, exact parse: kind 0x0f (max health) 100.0 x2, 0x10 (max armour) 100.0 x2, 0x17 100.0, 0x13
  with `b1` 1 then `b1` 0, kind 1 (ammo) x3 (`weapon` 0 / 7 / 7: 1000.0, 1.0, then 0.0 after the throw; `extra` 1), kind
  2 (health) 100.0, 99.06, 77.85, kind 3 (armour) 100.0, kind 0x18 0.0, kind 6 1.0. Head `0d 66 0f 00 00 00 00 c8 42`.
- **Verdict:** REFERENCE-CONFIRMED (layouts of kind 1 and the `[b1][b2][value:f32]` kinds). Kinds 6, 0x17 and 0x18 are
  "any other kind: nothing" in S2C §4.3 (the client ignores them); no client reaction to any 0x66 is visible.

### 3.7 S2C §4.4 weapons: `GS_WEAPON_GIVE` (0xb2) and `GS_WEAPON_SWITCH` (0x7f)

- **Evidence:** in block 37 the reference stack sends, twice: 0x66 kind 0x13, then four 0xb2 `[weapon][ammo_kind][ammo:f32]
  [class_pos 0][slot][-1 x4]` for slots 0..3 (weapons 145, 164, 0, 7; ammo 120, 45, 1000, 1), with 0x7f
  `[command][auto][ammo -1.0][last]` = (105, 0, 0), (251, 0, 1), (105, 1, 0) between them. Head
  `0d b2 91 00 91 00 00 00 f0 42 00 00 ff ff ff ff`. No 0xb3. The client answers with four `0x76 [3][2]` + 0x7f slot 0
  weapon 145 and later switches to exactly these slots / weapons and fires them with these ammunition ids.
- **Verdict:** 0xb2 slot filling CONFIRMED by the client's use of the slots (PROVEN-CAPTURE, though the client's own
  lobby loadout lists the same weapons); 0x7f with `auto` 1 drawing slot 0 PLAUSIBLE (the client's 0x7f follow it).
  `GS_WEAPON_EQUIP` (0xb3), which dc-dserver sends for the drawn weapon, was **not** exercised by this capture.

### 3.8 S2C §4.2 / MOV §12.4 `GS_MOVER_UPDATE` (0xa0) and `GS_VIEW_ANGLES` (0x6a)

- **Evidence:** six 0xa0, exact parse: flags 0x000d (body: a 37-character model name and 2 skins, empty container list,
  friction 5.0 twice), 0x0020 teleport seq 1 at (-979.0, 826.07, -1653.0) with half extents (15, 50, 15) (head
  `0d a0 20 00 01 00 c0 74 c4 9a 84 4e 44 00 a0 ce c4 00 00 70 41 00 00 48`), 0x0080 freeze 1, (0x44 and 0xb5), 0x0080
  freeze 0, then after 0x87 another teleport seq 2 at the same point and 0x0008 friction 5.0. No 0xa0 during the 157 s
  of play, none on crouching. 0x6a `[0][0, 5.356415, 0]` (seq 410).
- **Verdict:** 0x0020 CONFIRMED by the client's reaction (PROVEN-CAPTURE: `move_seq` echo 1 then 2, position);
  0x6a kind 0 CONFIRMED (the first 0x64 body yaw is 5.356415, bit-identical). 0x0001 / 0x0004 / 0x0008 / 0x0080
  REFERENCE-CONFIRMED (layout; the client kept playing; the freeze pair has no visible effect on the wire).

### 3.9 S2C §2.1 round flow at match start: 0x43, 0x44, 0xb5, 0x45 (V-S2C-4)

- **Evidence:** 0x43 (no body), 0x44 `[0][1][1][0]` (`round_secs` 0, `round_no` 1, `round_limit` 1, `keep_team` 0),
  0xb5 `[0][0]`, 0x45 2490 bits: `round_secs` 0, `show_intro` 1, **`first_round` 0**, `countdown_secs` 0, intro block
  with strings of 43 / 41 / 38 / 43 / 39 characters, map and radar bounds as fractions (0.067 .. 0.53), `radar_zoom`
  0.0, `radar_rot` 0.15, `radar_p3` 1024, `radar_p4` 1024, two team base points, then `round_no` 0. Exact parse with
  `mode_class` 1. Head of 0x45: `0d 45 00 00 00 00 01 00 00`.
- **Verdict:** REFERENCE-CONFIRMED (layouts, including the TDM branch of the intro block). Field meanings and the
  `first_round` value of the 2009 server stay UNPROVEN (the reference stack sends 0; S2C §10 step 11 and DSV-20 send 1; no
  client reaction separates them in a session without a 0xd4).

### 3.10 S2C §3.2 `GS_PING_TABLE` (0xa2), PD-S2C-4

- **Spec / default:** PD-S2C-4 "ping table 0xa2 every 5 s"; PD-S2C-1 "every server -> client game message reliable".
- **Evidence:** 52 x `0d a2 00 00 00 00 ff ff` (`[pid 0][ping 0]`, end marker), **all unreliable**, every 3.01 s (2.97 ..
  3.06 s) from t 143.015 to 296.578.
- **Verdict:** REFERENCE: 3 s, unreliable. Not 2009 evidence; the client shows no reaction either way. PD-S2C-4 is not
  contradicted by 2009 evidence (see section 6 for the replay-parity note).

### 3.11 S2C §3.2 `GS_OWN_FLAG` (0xca): the sprint permission

- **Spec:** `[flag:bool1]` "own record (cshell+0x2aca0) +0x1f0 := `flag` (meaning UNPROVEN)". MOV §7: "sprint test =
  `SPRINT` and the object returned by cshell+0x2aca0 has byte +0x1f0 set".
- **Evidence:** 48 x `0d ca 0x` (1 bit), 15-79 ms after each client 0xca whose `sprinting` bit changed (47 paired),
  carrying the new bit (24 x 1, 24 x 0). In the 22 updates that had `SPRINT` set but were sent **before** the echo
  `[1]` arrived (0-63 ms after the client's sprint start), the speed stayed exactly at the run plateau (for example
  226.9 and 255.4 at 62-63 ms, no rise at all); the first update after the echo is 60-100 units faster and the sprint
  plateau (about 1.39 x run, 344-358) follows. 326 `SPRINT` updates after an echo `[1]` reach up to 361.
- **Verdict:** DIFFERENT / settled (PROVEN-CAPTURE by the client's reaction; the causal step is PLAUSIBLE because the
  pre-echo window is short): 0xca is the server's **sprint permission**. Without `[1]` the client's sprint test fails
  and it runs at run speed although it reports `SPRINT`. The reference stack grants it on every change; when to refuse is
  server policy.
- **Replacement text (S2C §3.2 row 0xca):** "`GS_SPRINT_ALLOWED` (formerly `GS_OWN_FLAG`) `[allowed:bool1]`: own record
  +0x1f0, the byte MOV §7's sprint test requires. A server answers each `UP_SPRINT_STATE` whose `sprinting` bit changed
  with this bit (OBSERVED tdm1: the client reaches sprint speed only after `[1]` arrives)."

### 3.12 S2C §2.4 `GS_CHAT` (0x68)

- **Evidence:** two echoes (seq 12415, 12640), 47 ms after each of the client's lines: `sender_name` 5 units (the user's
  name), `text` 11 and 10 units, `sender_id` 0 (the own id), `team_only` 0, `whisper` 0. The reference stack's `text` is "name,
  space, colon, space, line" (5 + 3 + 3 and 5 + 3 + 2 units).
- **Verdict:** layout REFERENCE-CONFIRMED. Whether the 2009 client shows the sender name itself (so that the reference stack's
  prefix appears twice) is not visible on the wire: UNDECIDED (screenshot needed). dc-dserver sends the line without a
  prefix, as S2C §2.4 reads.

### 3.13 S2C §4.3 `GS_DAMAGE_TAKEN` (0x74): fall damage

- **Evidence:** two 0x74 + 0x66 kind 2 pairs (seq 6660, 7933), each 50-110 ms after the client landed from a fall
  (trailer vy -766 and -825 just before `on_ground` 0 -> 1): `from_dir` (0, 0.0094, 0) / (0, 0.212, 0), `unused` 5,
  `mode` 1, `ammo_kind` 0xffff, `dmg_kind` 0, `zone` 5, `part` 0, `damage` 1 / 21; health 99.06 / 77.85. Head
  `0d 74 00 00 00 00 3d ca 19 3c 00 00 00 00 05 01 ff ff 00 05 00 01 00 00`.
- **Verdict:** layout REFERENCE-CONFIRMED. REFERENCE: the reference stack derives fall damage from the 0x64 stream; that rule (and its
  numbers) is server policy and is not taken from the reference stack.

### 3.14 S2C §7.2 shell 0x88 / 0x90, and 0x9d

- **Evidence:** 0x90 `[1]` and 0x88 `[2]` in the first server block, before `S2C_VERSION` (seq 268); 0x90 `[0]` and 0x88
  `[3]` right after the client's 0x87 (seq 461). 0x9d `[u32 0]` once (seq 406): 0x9d is in no stage list of the S2C
  catalogue, so the client drops it.
- **Verdict:** 0x88 `[2]` before 0x87 CONFIRMED as sufficient for the in-world test (PROVEN-CAPTURE, with 2.14).
  Botclient finding O6 (0x88 before `S2C_VERSION` is harmless) now holds for the real client. 0x90 meaning and 0x9d
  stay UNPROVEN; a server need not send either.

### 3.15 S2C §8 shell object blobs (V-S2C-1) and the reference stack's effect relays (U5)

- **Evidence (create records of the object groups):** the own character (object 408, model kind 1) carries a 580-bit
  blob of **type 0x18** (a class-read type), not 0x01; type 0x17 (pickup) with `pickup_kind` 2 (flag) on two objects
  (116 bits each, exact with GM §4.4); **five type 0x37 mode zones** in this TDM world (409 / 1081 bits, exact parse
  with GM §4.3): `z_kind` 4, 4, 6, 6, 8, `z_team` 2, 2, 1, 2, 2, `z_class` 1, 1, 1, 1, 9, `z_a` / `z_b` read as floats
  -1.0, one `z_d` = 15.0 as a float; type 0x0b on 60 kind-9 objects; type 0x64 (the second consumer, S2C §8.4) on 16;
  0x39 on 3, 0x0d on 1, 0x13 on 1. Server 0xa6: note kind 0x18 on object 408 (2973, mostly unreliable, about one per
  tick) and kind 0x17 on the two pickups. Engine id 17 (unreliable): type 6 (432 bits) after each of the 83 0x82;
  type 0x19 after each 0x80 with the same `sound_no` (22) and after each jump with sound 15 (27); type 0x10 x3; type
  0x65 x4 (outside the dispatcher's range, ignored by the client).
- **Verdict:** REFERENCE: these are the reference stack's choices, accepted by the client without a visible problem. The layout of
  the type 0x18 character blob is not specified (class-read) and **could not be decoded** (580 bits; head after the
  type byte `01 00 00 0c 08 00 00 00`). GM §5.1's "players (type 0x01 / 0x18)" matches the reference stack's 0x18.

### 3.16 Reliability (V-S2C-5, PD-S2C-1)

- **Evidence:** the reference stack sends 0xa2 (52 of 52), 0xa6 (2797 of 2975) and engine id 17 (140 of 140) unreliable;
  every other game message reliable.
- **Verdict:** REFERENCE only (the 2009 server's choice stays unknown). PD-S2C-1 (all reliable) is not contradicted.

### 3.17 Capture list S2C §11.4

| id | result |
|---|---|
| V-S2C-1 | REFERENCE answer recorded (3.15): character type 0x18, flag pickups 0x17 kind 2, zones 0x37; 2009 server unknown |
| V-S2C-2 | spawn part recorded (3.5-3.8); kill and respawn not in tdm1 |
| V-S2C-3 | not in tdm1 (the user left with 0xbd before a match end) |
| V-S2C-4 | recorded (3.3, 3.9): goal 100 = room goal, `round_no` 1 in 0x7d / 0x44 and 0 in 0x45, `first_round` 0 |
| V-S2C-5 | REFERENCE answer recorded (3.16) |

## 4. MOVEMENT_2009.md items

### 4.1 MOV §12.1 authority model and §12.2 wire form

- **Spec (§12.1):** "The 2009 client simulates its own body locally every frame ... and is the authority for its own
  position: it does not wait for the server and does not replay inputs ... the server ... can only impose state with
  the mover update (12.4)".
- **Evidence:** the client sent absolute position and velocity at 15 Hz for 157 s (2.2); the reference stack sent no 0xa0 after
  t 140.078 (the spawn set) and never corrected the client; the client never waited for the server (one 438 ms local
  stall aside, 2.2 b). Its positions are
  self-consistent with its velocities (2.2 i).
- **Verdict:** CONFIRMED (PROVEN-CAPTURE for the client side; the reference stack simply accepted every update).
- **§12.2 wire form:** CONFIRMED, with the corrections of 2.2 (e) `yaw8c` truncation, (f) body angles = yaw only,
  (g) eye offset values, (h) bit 0x400 = fire, (i) client clock and `standing`. "which bits of its `state:u32` word
  carry the stance is UNPROVEN here" is settled: the word is the control word; `CROUCH` 0x20 is the crouch stance (it
  accompanies the eye drop to +20 in 52 of 52 crouched updates).

### 4.2 Validation plan MOV §16.2

| id | spec expectation | tdm1 result | verdict |
|---|---|---|---|
| V-M1 | "horizontal speed plateaus at 70 / 285 / 50 (times `1 - p` for the loadout)" | plateaus depend on the **drawn weapon**: weapon 0: run 257.1, sprint 358.5; weapon 145: run 230.7, sprint 318.0; weapon 164: walk 65.6, crouch 45.6, run 250.4, sprint 347.0; weapon 7: run 248.6, sprint 345.7. Forward, side and back runs reach the same plateau. Ratios for weapon 164 (walk 0.937, crouch 0.911, run 0.879 of 70 / 50 / 285) are not one common factor | DIFFERENT from the bare defaults; consistent with per-weapon modifiers. Needs dc-sim with the ROM weapon / character tables at run time to decide whether the bases (70 / 285 / 50) or the modifier rule differ |
| V-M2 | ramp 500 -> 3000 over about 0.42 s, first frame at 500 | from standstill to the run plateau in 0.27-0.34 s (nine starts, 15 Hz samples: e.g. 34, 84, 138, 183, 226 at 67 ms steps) | UNDECIDED: needs a dc-sim run with friction 5.0 (the reference stack's 0xa0 value) and the weapon factor; the samples are recorded here for that test |
| V-M3 | `vel.y` starts at 330, decreases by 1100 per second; apex about 49.5-50.9 | gravity **-1100.0 per s** (median of 103 airborne sample pairs, 10th-90th percentile -1100.1 .. -1100.0); take-off speed from the parabola through the first airborne sample 320-333 on flat ground (330.0, 330.5, 326.5, 324.6 ...); apex 47.9-50.5 | CONFIRMED (PROVEN-CAPTURE) for the gravity; take-off speed consistent with 330 within sampling error |
| V-M4 | second take-off speed about `330 * 0.3` unless 0.58 s passed | chained jumps only on stairs: one take-off 0.41 s after the previous one reached about 263 (0.80 x 330, apex 31.4), which matches `HOP_MIN + 0.41 s x HOP_RECOVER` = 0.3 + 0.49 = 0.79; another, 0.49 s after, showed no reduction (stair geometry makes the estimate unreliable) | UNDECIDED (no flat-ground chain); the one usable case agrees with the recovery rule, not with a flat 0.3 |
| V-M5 | decay `exp(-5t)` plus the stop rule | after releasing the move keys at full run the speed is 0.0 by the **second** sample in 7 of 7 cases (<= 0.134 s; first sample 28-213); `exp(-5t)` alone would leave about 116 after 0.134 s | CONFIRMED that the slide-to-stop rule (§6.8, `STOP_TIME` 0.1) governs the stop in the live client (this also answers §16.1 item 1 in part: the rule runs; its caller is still unknown) |
| V-M6 | step climbing up to 30.5 | not tested (no step test map) | UNDECIDED |
| V-M7 | 0xa0 traffic while crouching: presence or absence of a 0x0020 block | the reference stack sent no 0xa0 during the four crouch periods; the client crouched anyway (eye +33 -> +20, speed 45.6) | CONFIRMED that the crouch box needs no server message (REFERENCE run; client behaviour PROVEN-CAPTURE) |
| V-M8 | trailer decode: position = client position, `move_seq` = last 0xa0 seq | 2.2 (i) | CONFIRMED (PROVEN-CAPTURE) |
| V-M9 | eye height | `eye_offset` (0, 33, 0) standing, (0, 20, 0) crouched relative to the body position, box half height 50 (2.2 g) | settled on the wire (PROVEN-CAPTURE for the value; PLAUSIBLE that it is the render camera height); a screenshot is no longer needed for the offset |
| V-M10 | Rust collision vs logged client positions | the trailer positions of tdm1 (2,338 samples, ladder and stairs included) are the input for this test | UNDECIDED (needs the dc-sim run) |
| V-M11 | air control | not isolated | UNDECIDED |
| V-M12 (addendum B1) | mouse scale | no scripted input | UNDECIDED |

Ladder (MOV §7 "On a ladder: `s = 200`" and §6.3 "if `v.y > 200`, `v.y := 200`"): 69 ladder updates (`surface` 200)
climb at vy 201-208. CONFIRMED within one frame of integration (PROVEN-CAPTURE).

### 4.3 MOV §12.3 / §12.4 / §12.5

- §12.3 movement notices: see 2.3 (event 3 timing DIFFERENT).
- §12.4 teleport `seq` echo: CONFIRMED (3.8). The shell's local freeze message is not on the wire (nothing to check).
- §12.5 0x6a kind 0: CONFIRMED (3.8). 0x67 state 3 zeroing movement and 0x74 knockback: not observed (no state 3; the
  two 0x74 were vertical fall-damage directions with no visible horizontal effect).
- MOV §8.4 "the container list and `k` come only from the server's mover update" when connected: the reference stack sent
  friction 5.0 in 0xa0 (flags 0x0004 and 0x0008); with it the stop and ramp behave as in 4.2. REFERENCE.

## 5. GAME_MODES_2009.md items (TDM chapter)

| place | spec text | tdm1 evidence | verdict |
|---|---|---|---|
| GM §5.1 Teams | "two teams, ids 1 and 2" | own record team 1 (0x6b), client 0x72 `team` 1 | REFERENCE-CONFIRMED |
| GM §5.1 Goal / time | "the `goal` of 0x7d is shown as a number on the scoreboard header (classes 1 and 4). Display only" | 0x7d `goal` 100 = room goal 100 | REFERENCE-CONFIRMED (layout and value source); display not visible on the wire |
| GM §5.1 Extra messages | "none beyond S2C §10" | the reference stack also sent 0x9d, 0x90, 0x43, 0xb5, 0xca echoes, 0xa6, engine id 17 | REFERENCE; of these only 0xca matters to the client (3.11) |
| GM §5.1 Client -> server | "none extra; respawn request 0x89 after an armed wait" | nothing mode-specific was sent (no 0x59 / 0xd8 / 0xad / 0xc5 / 0xd2) even with five type-0x37 zones in the world: the kind-8 zone (the 0x59 target) belonged to team 2, so the use test (own team or 0xfe) failed for a team-1 player | CONFIRMED in the negative (PROVEN-CAPTURE); 0x89 UNDECIDED (no death) |
| GM §5.1 Objects | "players (type 0x01 / 0x18), pickups 0x17 kind 6, HiSec 0x17 kind 5" | player character type 0x18; pickups 0x17 kind 2 (flags) x2; zones 0x37 x5 | REFERENCE (3.15) |
| GM §5.1 Respawn / round end | "free (0x89 or server timer)"; "one round: 0x46, 0x65, 0x3f, 0x99" | no death, no round end | UNDECIDED |
| GM §4.1 round flow | match start 0x45 with `first_round` 1 | reference stack: 0x43, 0x44 `[0][1][1][0]`, 0xb5, 0x45 with `first_round` 0 | REFERENCE; the 2009 choice open |
| GM §4.2 respawn arming | "armed only by server 0xd7 sub 2 (with `a == b`) or 0xdb sub 6" | the reference stack sent neither | UNDECIDED; to test GMD-3 the next capture must run against dc-dserver (section 9) |
| GM §4.3 zone layout | 15-field type-0x37 record | five zone blobs parse exactly (409 or 1081 bits); `z_a`, `z_b`, `z_d` read as floats (-1.0, -1.0, 15.0) | REFERENCE-CONFIRMED (layout); `z_a` / `z_b` / `z_d` are PLAUSIBLY f32 |
| GM §4.4 pickup kind 2 | `[state:u32][team:u32][at_base:bool1]` | two flag pickups of 116 bits parse exactly | REFERENCE-CONFIRMED |
| GM §7 movement | no mode-specific movement | nothing mode-specific seen | consistent |

V-GM-1 .. V-GM-9 are all about other modes (round modes, bomb / flag object streams, FFA, bounty, infection, co-op,
hold-to-use, flag events); tdm1 settles none of them. Partial input for V-GM-2: in this TDM world the reference stack creates
five type-0x37 zones (kinds 4, 6, 8) and two flag pickups at load, i.e. it attaches mode objects of other classes to
the world's objects in TDM too (REFERENCE).

## 6. Proposed DC_DEFAULTS changes

Rows of `DC_DEFAULTS.md` (approved CD-nn, proposed DC-C2S-n / PD-S2C-n / DSV-n / BOT-n / CLI-n) that tdm1
contradicts or sharpens. Values from the reference stack are never proposed as rules; every change below rests on the real
client's own bytes or reactions.

| row | current text (short) | tdm1 evidence | proposed change |
|---|---|---|---|
| CD-63 (approved) | 10 ticks per second; "whether its interpolation stays smooth above 10 Hz is UNPROVEN until the live capture tests ... test 20 and 30" | the real client accepted the reference stack's 29.6 ticks per second (one id 8 group and one id 10 stream per tick, 4,652 each over 157.3 s) without an error or a stall, while it kept its own 15 Hz update rate (2.2 b); solo session, so no remote player was interpolated | add to the note: "OBSERVED tdm1: the 2009 client accepts about 30 ticks per second on the wire; its own player-update rate is 15 Hz regardless. Smoothness of a remote player at 30 Hz to be judged in tdm2." Default stays 10 until tdm2; the first test value becomes 30 |
| BOT-3 (proposed) | aux words and standing ids of 0x64 sent as 0 / -1 | the real client sends `aux[0]` = view pitch, `aux[1]` = view yaw offset (+-pi/4), `aux[2..5]` = 0, body angles `(0, yaw, 0)`, and real standing ids (2.2 f, i) | "the bot sends `aux[0]` = its pitch, `aux[1]` = 0 (no free head look), `aux[2..5]` = 0, body angles (0, yaw, 0); standing ids -1 / -1 (it has no world model index)". Also add `pu_flags` 0x0001 with `eye_offset` (0, 33, 0) and 0x1000, which the real client sets in every update |
| BOT-9 (proposed) | aim rotation "in the shared simulation's convention" (Euler order PLAUSIBLE) | `aim_q = Ry(body_yaw + aux[1]) * Rx(pitch)` in 1,472 of 1,472 updates, forward = rotated +Z, positive pitch looks down (2.2 f) | replace the evidence column with "OBSERVED tdm1"; the bot must build the quaternion in that order and sign |
| CLI-3 (proposed) | eye height "0.8 x the body's half height above the body centre" | the client reports its eye point at +33 standing and +20 crouched above the body position, with half extents (15, 50, 15) (2.2 g): 0.66 and 0.40 of that half height, and a crouch transition of about 0.2 s | "eye offset (0, 33, 0) standing, (0, 20, 0) crouched above the body position (OBSERVED tdm1), blended over about 0.2 s; replace by the model socket once dc-model exposes it" |
| CLI-8 (proposed) | the 2009 client accumulates yaw unbounded | body yaw -2.85 .. 9.50 rad in tdm1 | evidence column: add "OBSERVED tdm1" (no change of rule) |
| DC-C2S-10 (proposed) | before 0x87: 0x67 state 3 and 0x88 state 2; then the spawn set with 0x67 state 1 | the tested path (reference stack) is 0x88 `[2]` at link start, 0x67 `[1]` **before** 0x87, spawn set before and again after 0x87; the client accepted it (2.14, 3.5). State 3 before 0x87 was not exercised; death / respawn timing not exercised | keep the row, add "OBSERVED tdm1: state 1 before 0x87 is also accepted (reference stack form). The state-3-first path and the respawn timing are still to be shown with the real client (tdm2 against dc-dserver)" |
| DC-C2S-7 (proposed) | enable 0xc3 statistics after 0x87 | without an enable the real client sends no 0xc3 (157 s) (2.17) | no change; add "the client is silent until enabled (OBSERVED tdm1)"; the enable with `pid` 0xFFFFFFFF is untested |
| PD-S2C-4 (proposed) | ping table 0xa2 every 5 s | reference stack: every 3.0 s, unreliable (3.10); the client shows no reaction either way | no change (not 2009 evidence). Note for replay tests: an allowlist entry for 0xa2 timing and channel is needed when a dc-dserver run is compared with tdm1 |
| DSV-12 (staged) | identification wait 3000 ms | the client answered 0x72 in the same 16 ms tick (3.1) | no change (the deadline is a safety net only) |
| DSV-14 (staged) | movement check `1500 * max(dt, 0.05 s) + 64` units per update | the largest step in tdm1 is 0.48 of that limit (79 units in 67 ms; max horizontal speed 374, vertical -825 .. +320) | no change; add "OBSERVED tdm1: real-client movement stays below half of the limit" |
| DSV-19 (staged) | identity check by `char=` / `slot=` | `char_no` 1002 and `room_slot` 0 equal the lobby's 0x303 +0xe and 0x383 +0x12 (2.12) | no change; add the evidence |
| new (for the coordinator to number) | -- | 3.11: the client sprints only after the server's 0xca `[1]` | **sprint permission**: "the server answers every `UP_SPRINT_STATE` whose `sprinting` bit changed with 0xca `[sprinting]` (always allow); a Lua hook `movement.allow_sprint(pid)` may refuse (stamina rules are DeCombat policy)". Evidence: MOV §7 sprint test needs own record +0x1f0; S2C §3.1 0xca is its only writer; OBSERVED tdm1 speeds before / after the echo |
| new (for the coordinator to number) | -- | 2.3 / 3.13: the client accepts fall damage as 0x74 + 0x66 kind 2; event 3 of 0x76 is no landing marker | **fall damage**: "computed from the 0x64 trailer (`on_ground` 0 -> 1, the vertical speed of the last airborne update); off by default until a GAME_RULES spec fixes the curve; delivered as 0x74 (zone and direction per S2C §4.3) + 0x66 kind 2" |

No approved row is contradicted by 2009 evidence.

## 7. Recommended changes for dc-dserver (list for the dserver owner; no crate was edited)

Ordered by impact on a real-client session. "Current" refers to `crates/dc-dserver` as read on 2026-10-25.

1. **Echo the sprint permission (0xca).** Current: `Up::Sprint { .. } => {}` (ignored). Without the echo a real client
   never sprints faster than it runs (3.11). Send `[13][0xca][sprinting:1 bit]` to the sender on every change of its
   `sprinting` bit, reliable (new DC default of section 6).
2. **Throws: parse 0x4d and treat the following 0x82 of a thrown weapon as a projectile.** Current: 0x4d falls into
   `Up::Unknown`, and the grenade's 0x82 (weapon 7, ammo 9 in tdm1) reaches `on_fire` like a bullet with no hit. The
   thrown object, its flight and its explosion are DeCombat policy; the client supplies only the release (0x4d) and the
   ray (`origin` = eye point, `dir`) in the 0x82 that follows about 200 ms later (2.11). Also count the thrown item in
   the ammunition mirror (the client switched back to slot 0 on its own after its last grenade).
3. **Do not use `UP_MOVE_EVENT` event 3 as a landing.** After a jump the client sends 3 one frame after the take-off
   (2.3). Any landing logic (fall damage, landing sounds for others) should use the trailer's `on_ground` transitions.
4. **Keep the create-before-roster order** (current `place_in_world` already sends the object group, the view object,
   0x88 and then 0x72; good) and keep answering `UP_ROSTER_QUERY [pid]` with that pid's 0x6b (current
   `on_roster_query` does). tdm1 shows the client queries a pid when a 0x6b names an object it has not created (2.15).
5. **Send `GS_GAME_ENDPOINT` (0x9e)** `[mode_class][host:zstr8][port:u32]` once, before 0x7d and 0x45 (S2C §10 step
   1). Current: not sent. The real client accepted the reference stack's `[1][host][port]` (3.2). It makes the layout-deciding
   class explicit instead of relying on the client's own derivation from the lobby room.
6. **Verify the untested spawn forms in tdm2, keep a fallback.** dc-dserver sends 0x67 state 3 before 0x87 and
   `GS_WEAPON_EQUIP` (0xb3) for the drawn weapon; tdm1 only proves the reference stack's forms (state 1 before 0x87; 0xb2 for
   every slot plus 0x7f with `auto` 1). Acceptance signals to check: the client's 0x87 arrives, its 0x64 stream starts
   after state 1, and it sends `0x76 [3][2]` + `0x7f [0][weapon][ammo id]` after the spawn set. If either is missing,
   switch to the reference stack's form (both are spec-conforming).
7. **Movement:** keep DSV-14 as is (max 0.48 of the limit in tdm1); keep using arrival time for `dt` (the trailer
   `time` is the client clock, about 138 s ahead of the server's world clock in tdm1); keep ignoring updates with an old
   `move_seq` (the first two updates after a teleport still carry the previous seq).
8. **Facing for other clients:** take the third-person yaw from the body angles (`view[1]`, yaw only, unwrapped) as
   now; the head / aim direction is `body_yaw + aux[1]` and pitch `aux[0]` (2.2 f) if a later relay needs it.
9. **`UpFire::shot_no` is an f32** (1.0 / 1.2 per weapon, 2.6); do not use it as a counter. `stat` grows by 2 per
   message; `frac / 255` is the weapon's spread; `origin` is the eye point (body + (0, 33, 0) standing), which keeps
   DSV-11's 160-unit origin check safe.
10. **0x66 kinds:** only 0x0f, 0x10, 2, 3 (and 1 for ammunition) are needed; the reference stack's 6, 0x13, 0x17, 0x18 are not
    required (S2C §4.3: the client ignores 6, 0x17 and 0x18).
11. **Chat:** keep sending the typed line without a "name : " prefix (S2C §2.4); tdm2 should include a screenshot of a
    chat line to settle whether the 2009 client prints the sender name itself (3.12).
12. **Optional fall damage** through 0x74 + 0x66 kind 2 (new DC default of section 6); the client accepts that form.
13. **0xc3 statistics enable** (`TODO_PERF_ENABLE_PID` 0xFFFFFFFF): untested with the real client; if tdm2 shows no
    0xc3 sub 2 / 3 reports after the enable, try the client's own id (which also shows its statistics overlay).

## 8. Notes for sibling specs (outside this task's four specs)

- `UDP_TRANSPORT_2009.md` §5.1 / V3: the real client's `OOB_JOIN_ASK` (seq 266,
  `10 7f 33 01 5a 9e c1 12 43 de 09 25 8a c4 66 af 75 4e 5c 93 03 00 71 02`) is byte-identical to the bot's GUID-record
  form of botclient finding O1: V3 is settled for the real client (PROVEN-CAPTURE); DSV-5 can drop the second layout.
- `UDP_TRANSPORT_2009.md` §9.1: the client's own transport bye is `[1][CTL_BYE]` (datagram `c9 03`), 4 copies about
  100 ms apart, sent when its game screen closes (2.16).
- `UDP_SESSION_2009_A.md` §5.5: `C2S_RATE` was sent three times: `07 80 02` after `C2S_LOAD_STATE(0)` (seq 400) and
  twice later with one extra `u16` id each (seq 6721, 7985), which matches the part B form `[rate_8k][u16 ids]`.
- `UDP_SESSION_2009_B.md` §10.3: `yaw_c8` is truncated, not rounded (2.2 e); the `v` floats are `(0, body_yaw, 0)`.
- `dc-botclient` (for its owner; no edit made): `shell::yaw8c` rounds; the real client truncates toward zero.

## 9. Items that still need the two-player capture (tdm2: user + bot) or a dc-dserver capture

| item | what to do in the session | settles |
|---|---|---|
| a kill of the bot by the user | shoot the bot until it dies | 0x82 `has_hit` 1, `hit_obj`, `hit_dist` (V-C1, U1); the 0x6d the user's client receives (S2C §3.3.1); kill bookkeeping |
| a self-kill (fall from a height, or a grenade at the feet) | die once, then wait 10 s without input | 0x6d with `killer` = `victim` or 0xff; 0x67 `[2]` / `[3]` timing (PD-S2C-2, DC-C2S-10); whether the client sends 0x60 / 0x61; that **no 0x89** comes without arming (C2S §3.3) |
| respawn arming (GMD-3) | **against dc-dserver** with GMD-3 on, since the reference stack never arms: die and wait | the client's 7000 ms wait and `UP_RESPAWN_REQUEST` 0x89 timing after 0xd7 sub 2 |
| a shotgun shot | one shot at a wall, one at the bot | pellets per message, `last` only on the final pellet (V-C1) |
| the bot as a second player in the world | walk near it, watch it move | interpolation of a remote player at the server tick (CD-63 smoothness), 0x6b kind 0 join notice, 0x6c on leave |
| relays seen by the other client | fire, reload, switch, jump near the bot | which messages the bot receives for the user's 0x80 / 0x7f / 0x76 / 0x82 (U5; REFERENCE form in 3.15) |
| a full match to its end | play until the goal or the timer ends the round, watch the result screen | 0x46 / 0x65 / 0x3f / 0x99 order and fields (V-S2C-3, GM §5.1 round end) |
| team chat and one radio command | scope 1 line, one radio entry; take a screenshot of a chat line | V-C4, the radio pair 0xbe + 0x68, the chat name prefix question (3.12) |
| idle 20 s with a weapon drawn; a camera-mode change | stand still; switch view if the client offers it | `UP_WEAPON_EVENT` event 5 (U4), `pu_flags` 0x0004 / 0x0008 (U10) |
| toggle the two client options | change options between rounds | U3 |
| a clan member account | join with a clan character | 0x72 clan branch (V-C2) |
| flat-ground chained jumps, a step test, an air strafe | jump twice within 0.5 s on flat ground; walk into steps; strafe in the air | V-M4, V-M6, V-M11 |
| dc-dserver spawn forms | the tdm2 run against dc-dserver | 0x67 state 3 before 0x87, 0xb3 drawn weapon, 0x9e, 0xc3 enable (section 7 items 5, 6, 13) |

## 10. What could not be decoded

- The 580-bit type-0x18 blob of the player character's create record, and the class data of the 2,975 server 0xa6
  notes of kind 0x18 (class-read types: their layouts are not specified, S2C §8.2 / §8.3).
- Engine id 17 type 6 (432 bits, one per shot) and type 0x65 (ignored by the client): class-read / unknown layouts.
- The meaning of the 0x7d caption, the 0x45 intro strings and images (ROM-derived; not reproduced), of 0x90's bit and
  of 0x9d's word.
- Field meanings that need other client states: the `look` bytes, r1..r7 of 0x72 (one character only); 0x85's two
  options; the 0x82 `extra_*` block and `stat`; `pu_flags` 0x0004 / 0x0008.
- Everything about death, respawn, kills and the match end: none happened in tdm1.
