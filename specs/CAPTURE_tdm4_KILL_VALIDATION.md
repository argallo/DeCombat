# capval-kill: tdm4 capture validation of the combat flow (staging, addendum style)

- **Targets:** `specs/GAME_MESSAGES_2009_C2S.md` §2.1, §3.3, §3.4, §4.1, §4.4, §4.5, §4.6, Appendix A steps
  A8-A14, §12.1 (U1, U4, U5, U10); `GAME_MESSAGES_2009_S2C.md` §3.3.1, §3.4, §4.1, §4.3, §8.2 (types 0x06, 0x19),
  §10 steps 14-22, §11.2 items 2 and 7, §11.4 V-S2C-2 / V-S2C-3; `GAME_MODES_2009.md` §4.2; `MOVEMENT_2009.md` §5.1;
  `PROTOCOL_2009.md` 0x394 / 0x3004; DC_DEFAULTS rows DC-C2S-3, DC-C2S-10, PD-S2C-2, GMD-3, DSV-11.
- **Evidence:** `// OBSERVED: captures/20261025_002236_tdm4/capture.jsonl` (captap format 1) and
  the bot's own decoded view `.../20261025_002236_tdm4/logs/Bot1.events.jsonl` / `Bot1.report.json`. One new
  DISASM fact (section 4) from a read-only capstone pass over the user's own `Game/CShell.dll` (image base
  0x37100000) and the clean decompiler tree `CShell.dll/` (cited `cshell+0xRVA`).
- **Connections (from the `open` events and the lobby frames):**

  | conn | port | who | evidence |
  |---|---|---|---|
  | 2 | lobby (TCP) | the real 2009 client, account 1001, nick `player1`, team 1 | 0x38b record of 1001 with that name; 0x394 at t = 137.031 |
  | 4 | lobby (TCP) | our bot `Bot1`, account 1002 (nick `bot1`), team 2 | 0x300 / 0x301 of 1002; 0x3004 at t = 140.078 |
  | 5 | game (UDP) | the bot, peer port 55723 | the bot's own `udp_open` local port; first datagram t = 56.109 |
  | 6 | game (UDP) | the real 2009 client, peer port 55725 | hello blob of `player1`; client id 1 (`S2C_CLIENT_ID` = `0c010000`) |

  In-game ids: the bot is roster `pid` 0 with character object **408** (0x198); the real client is `pid` 1 with
  character object **189** (0xbd). Both received `GS_GAME_ENDPOINT` 0x9e with `mode_class` = **1**.
- **Grades:** **PROVEN-CAPTURE** = bytes the real 2009 client sent, or a client behaviour visible in them (2009
  evidence). **REFERENCE** = bytes the reference server sent (they show what the reference stack does, not what a 2009 server did;
  rule values are never taken). **REFERENCE-CONFIRMED** = an reference stack form whose acceptance by the 2009 client is shown by
  its next messages. **DISASM** = read in the disassembly of the user's ROM binary (ranks below decompiler output).
- **Decoders (scratch, outside the tree, nothing in `rust/` edited):** (a) a Rust binary that feeds every datagram of
  one (connection, direction) into a passive `dc_net_engine::Link` (check byte, reliable order, split reassembly, bye
  handling of our own crate) and prints each delivered message with the capture `seq` of the datagram that completed
  it; (b) a Python LSB-first bit reader (the bit order of `dc_wire::udp::BitReader`) for the field layouts of the
  specs; engine `BUNDLE` (id 14) entries unpacked. Cross-check: the decoder delivers to connection 5 exactly
  **1,827** shell messages with the same per-id counts as the bot's own log (`shell_counts` of `Bot1.report.json`).
  Every client 0x64 (1,104) and 0x82 (5) message parses with zero leftover bits.
- **ROM content:** none quoted. ROM table values were read at run time through `dc-bute` / `dc-rez` (a scratch
  binary) and are cited by section and key only; file names and engine-variable names carried in the server's file
  lists and `S2C_EVAR` messages are described by form only.
- **Offsets:** "bit N" counts from bit 0 of the delivered engine message (bits 0-7 = engine id 11 or 13, bits 8-15 =
  shell id, fields from bit 16; LSB-first). Hex heads are the first bytes of the delivered message (<= 24 bytes).
  "seq" is the capture event number of the datagram. The tap clock resolution is 15.625 ms (every `t` is a multiple of
  1/64 s); loopback, no loss, no resend.

## 0. Summary

| # | item | spec | verdict | grade |
|---|---|---|---|---|
| K1 | `UP_FIRE` 0x82 layout (4 rifle hits, 1 grenade throw) | C2S §4.1 | CONFIRMED bit for bit (887 / 623 bits, zero leftover); DIFFERENT meanings: `shot_no` is a float 1.0, the extra block carries the victim object and the exact hit point | PROVEN-CAPTURE |
| K2 | one 0x82 per pellet, `last` | C2S §4.1 | CONFIRMED for a 1-vector weapon (one 0x82 per shot, `last` = 1 each) | PROVEN-CAPTURE |
| K3 | hit point `origin + hit_dist * dir` | C2S §4.1 | CONFIRMED exactly (error 0.000 against the extra block's point) | PROVEN-CAPTURE |
| K4 | 0x64 cadence and control bits while firing | C2S §2.1, MOVEMENT §5.1 | CONFIRMED 15 Hz; DIFFERENT: bit 0x400 is the fire key held, seen only at shots and during the grenade hold | PROVEN-CAPTURE |
| K5 | `UP_WEAPON_CHANGE` 0x7f | C2S §4.4 | CONFIRMED layout; `aux` = the new weapon's ammunition id; always after 0x76 [3][2] | PROVEN-CAPTURE |
| K6 | 0x76 categories | C2S §2.2, §4.5 | CONFIRMED cat 1 events 1/2/3, cat 3 event 2; no fire or reload event exists on the wire | PROVEN-CAPTURE |
| K7 | grenade: 0x4d and the throw | C2S §4.6 | DIFFERENT: 0x4d repeats (18 x) while the throw is held; the release is the single 0x82 | PROVEN-CAPTURE |
| K8 | damage to the victim: 0x74 + 0x66 | S2C §4.3, §10 step 14 | REFERENCE form recorded; `from_dir` is scaled by damage / 100; the reference stack's amounts are below the ROM `InstDamage` (rule values not taken) | REFERENCE |
| K9 | kill: 0x6d, 0xb5, no 0x4a, no 0xc8 | S2C §3.3.1, §10 steps 15, 17 | REFERENCE form recorded; `0x6d` fields decoded; team kills travel in 0xb5, 0x4a is never sent | REFERENCE (weakly CONFIRMED) |
| K10 | life states after death | S2C §4.1, PD-S2C-2, DC-C2S-10 | DIFFERENT from DC-C2S-10: reference stack sends 0x67 **3 then 2 in the same block** (0 ms apart) and nothing more for 104 s | REFERENCE |
| K11 | respawn arming 0xd7 sub 2 / 0xdb sub 6 | C2S §3.3, GAME_MODES §4.2, GMD-3 | none sent; **C2S §3.3 is wrong**: the 2009 client arms its own 7000 ms wait after 0x67 state 2 in mode classes 1, 4, 6, 7, 8 (5 and 9 conditionally) | DISASM (new) |
| K12 | the bot stayed dead | playtest report | EXPLAINED: the reference stack respawns only on request; a 2009 client would have sent 0x89 about 7.0 s after the death; the bot sends nothing | DISASM + REFERENCE |
| K13 | real client's own death | C2S §3.4 | none: health 100 -> 98 -> 38, no 0x67 after its spawn, no 0x89 / 0xc5 / 0xd2 | PROVEN-CAPTURE |
| K14 | yaw at the default resolution | C2S §2.1 | CONFIRMED: yaw moved over 43.96 rad in 675 of 1,103 updates; `yaw_c8` = truncation (1,103 / 1,103) | PROVEN-CAPTURE |
| K15 | round end, leave | S2C §10 steps 19-22, C2S A13 / A14 | client left with 0xbd + `C2S_BYE`; the bot got 0x6c, 0x46, 0x47, 0x6b, 0x65, 0x48, 0x3f (order and timing recorded); no bye to the bot | PROVEN-CAPTURE / REFERENCE |
| K16 | lobby 0x394 / 0x3004 | PROTOCOL 0x394, 0x3004 | DIFFERENT name: 0x394 phase 0 is also sent on a normal leave; 0x3004 only to the bot; stored K/D not visible on the wire | PROVEN-CAPTURE / REFERENCE |
| K17 | relays of 0x80 / 0x76 / shots | C2S §12.1 U5, S2C §8.2 | REFERENCE relay forms found: engine 17 type 0x19 (sounds) and type 0x06 (two impacts per shot) | REFERENCE |

## 1. Timeline of the combat window (t in seconds of the tap clock)

| t | seq | link | message (decoded) | grade |
|---|---|---|---|---|
| 57.484 | 213 | s2c bot | spawn set of the bot (0x6b, 0x6a, 0x67 = 1, 0x66, 0xb2, 0x7f, 0xa0 teleport at (-289.0, 661.4, 1775.0)) | REFERENCE |
| 62.812 | 815 | s2c client | spawn set of the real client, 0x67 = 1 | REFERENCE |
| 74.859 | 2779 | c2s client | 0x76 [3][2], 0x7f [slot 0][weapon 145][aux 145] (draws the rifle) | PROVEN-CAPTURE |
| 75.547 | 2886 | c2s client | 0x80 [sound 3][weapon 145][own_id 1][pos] (draw sound) | PROVEN-CAPTURE |
| 76.078 | 2965 | c2s client | 0x82 shot 1, hit object 408, `hit_dist` 1329.886 | PROVEN-CAPTURE |
| 76.078 | 2968 / 2969 | s2c both | 0xa6 notes on 408; to the bot 0x74 + 0x66 health 73.225 + 0x66 armour 94.75; engine 17 type 0x06 x 2 | REFERENCE |
| 76.359 | 3011 | c2s client | 0x82 shot 2 (hit 408, 1328.119) | PROVEN-CAPTURE |
| 76.391 | 3017 | s2c bot | 0x74, 0x66 health 46.45, 0x66 armour 89.5 | REFERENCE |
| 76.687 | 3062 | c2s client | 0x82 shot 3 (hit 408, 1326.353) | PROVEN-CAPTURE |
| 76.734 | 3071 | s2c bot | 0x74, 0x66 health 19.675, 0x66 armour 84.25 | REFERENCE |
| 77.031 | 3119 | c2s client | 0x82 shot 4 (hit 408, 1327.387) | PROVEN-CAPTURE |
| 77.031 | 3115 | c2s bot | last 0x64 of the bot (it stops in state 2, as a 2009 client would) | (bot) |
| 77.078 | 3123 / 3124 | s2c both | 0xd6 sub 0, 0xd6 sub 2, 0xa6 case object, **0x6d** (victim 0, killer 1, weapon 145), **0xb5 [1][0]**; to the bot also 0x74, 0x66 health 0, 0x66 armour 79, **0x67 3**, file list and variable messages, **0x67 2** | REFERENCE |
| 77.953 | 3270 | c2s client | 0x76 [3][2], 0x7f [3][7][9] (grenade) | PROVEN-CAPTURE |
| 81.656 | 3812 / 3813 | s2c both | 0xd6 sub 1 [case 1][player 1] (the real client took the dropped case) | REFERENCE |
| (84.1) | - | (c2s bot) | a 2009 client would send 0x89 here (section 4); the bot sent nothing | DISASM |
| 101.516 | 6695 | s2c client | 0x74 (2 points, no shooter), 0x66 health 98.0 | REFERENCE |
| 104.906 | 7194 | c2s client | 0x76 [3][2], 0x7f [3][7][9] | PROVEN-CAPTURE |
| 106.266 | 7396 | c2s client | 0x80 [sound 10][weapon 7] | PROVEN-CAPTURE |
| 106.453 - 107.953 | 7423 - 7657 | c2s client | 0x4d x 18 (no fields) | PROVEN-CAPTURE |
| 108.156 | 7683 | c2s client | 0x82 weapon 7 / ammo 9, no hit (the throw), then 0x76 [3][2], 0x7f [0][145][145] | PROVEN-CAPTURE |
| 110.672 | 8064 / 8065 | s2c both | engine 17 type 0x06 (explosion); to the client 0x74 (60 points) + 0x66 health 38.0 | REFERENCE |
| 137.031 | 11852 | c2s client | 0xbd `UP_LEAVE_MATCH`; lobby 0x394 [0] on the TCP link at the same tick | PROVEN-CAPTURE |
| 137.047 | 11857 | c2s client | `C2S_BYE` (engine id 6, 8 bits) | PROVEN-CAPTURE |
| 137.078 - 137.375 | 11863 - 11868 | s2c client | transport bye code 5, 4 copies | REFERENCE |
| 137.375 | 11867 | s2c bot | 0x6c [pid 1][0], 0x46 [3][2][0][0] | REFERENCE |
| 140.078 | 12031 | s2c bot | 0x47 [0], 0x6b (bot record, deaths 1), 0x65, 0x48 [3]; lobby 0x3004 to the bot | REFERENCE |
| 143.109 | 12211 | s2c bot | 0x3f [0][2] | REFERENCE |
| 144.156 - 144.469 | 12277 - 12302 | c2s client | transport plain bye, 4 copies (7.1 s after the server's bye) | PROVEN-CAPTURE |
| 181.234 | - | s2c bot | last datagram of the capture: the bot is still linked and still streamed | REFERENCE |

## 2. Client -> server: what the real client sent while approaching and shooting (PROVEN-CAPTURE)

Counts on the real client's game link (connection 6, t = 56.109 .. 144.469): 0x64 x 1,104 (1 reliable state form,
1,103 unreliable), 0xca x 50, 0x76 x 72, 0x4d x 18, 0x7f x 14, 0x80 x 6, 0x82 x 5, 0x8c x 2, 0x72, 0x85, 0x87, 0xbd
x 1 each. **Never sent:** 0x83 (no reload happened), 0x81, 0xcc, 0xd5, 0x89, 0xc5, 0xd2, 0x60, 0x61, 0x68, 0xc3.

### 2.1 `UP_FIRE` (0x82), C2S §4.1

Spec text (C2S §4.1): "Sender: `weapon` slot +0x174 ... loops over the pellets of one shot and calls slot +0x174 once
per pellet ... the last argument is 1 for the last pellet of the shot" and the layout `[wkind:u8][weapon_id:u16]
[ammo_id:u16][weapon_pos:vec96][origin:vec96][aim:raw128][yaw:f32][dir:vec96][seed:u8][moving:bool1][wflag:bool1]
[mode1:bool1][frac:u8][shot_no:u32][time_ms:u32][has_extra:bool1] if has_extra: [extra_obj:objref16][extra_kind:u8]
[extra_a:vec96][extra_b:vec96] [flag_c:bool1][last:bool1][has_hit:bool1] if has_hit: [hit_dist:f32][hit_obj:objref16]
[stat:u32]".

Evidence, field by field (bit offsets from the engine id; values of the four rifle hits / the grenade throw):

| field | bit | shot 1 (seq 2965) | shots 2, 3, 4 (seq 3011, 3062, 3119) | grenade (seq 7683) |
|---|---|---|---|---|
| `wkind` | 16 | 0 | 0, 0, 0 | 0 |
| `weapon_id` | 24 | 145 | 145 | 7 |
| `ammo_id` | 40 | 145 | 145 | 9 |
| `weapon_pos` | 56 | (-1458.155, 700.709, 1104.536) | (-1458.159, 700.699, 1104.522) | (-2464.091, 596.529, 2111.930) |
| `origin` | 152 | (-1468.163, 704.979, 1104.178) | same | (-2461.649, 608.604, 2114.002) |
| `aim` | 248 | (0.006, 0.500, -0.003, 0.866) | (0.006, 0.500, -0.004, 0.866) | (-0.000, 0.840, -0.543, -0.000) |
| `yaw` | 376 | 1.0465 | 1.0477 | 9.4255 |
| `dir` | 408 | (0.864, -0.013, 0.503), length 1.000000 | (0.865, -0.013, 0.501), (0.866, -0.015, 0.499), (0.866, -0.012, 0.500) | (-0.001, -0.911, -0.412) |
| `seed` | 504 | 119 | 104, 102, 201 | 22 |
| `moving`, `wflag`, `mode1` | 512, 513, 514 | 0, 0, 0 | 0, 0, 0 | 0, 0, 0 |
| `frac` | 515 | 0 | 0 | 0 |
| `shot_no` | 523 | 0x3F800000 | 0x3F800000 | 0x3F800000 |
| `time_ms` | 555 | 75401 | 75696, 76025, 76369 | 107484 |
| `has_extra` | 587 | 1 | 1 | 0 |
| `extra_obj` | 588 | 408 | 408 | - |
| `extra_kind` | 604 | 24 | 24, 11, 11 | - |
| `extra_a` | 612 | (-318.983, 687.455, 1773.264) | (-318.983, 687.931, 1769.757), (-318.983, 685.336, 1766.154), (-318.983, 688.635, 1768.312) | - |
| `extra_b` | 708 | (-1.000, 0.000, 0.000) | same | - |
| `flag_c` | 804 / 588 | 0 | 0 | 0 |
| `last` | 805 / 589 | 1 | 1 | 1 |
| `has_hit` | 806 / 590 | 1 | 1 | 0 |
| `hit_dist` | 807 | 1329.8864 | 1328.1189, 1326.3533, 1327.3867 | - |
| `hit_obj` | 839 | 408 | 408 | - |
| `stat` | 855 / 591 | 2 | 4, 6, 8 | 10 |
| total | | 887 bits, all consumed | 887 each | 623 bits, all consumed |

Hex heads: shot 1 `0b820091009100f744b6c4632d2f4423118a443885b7c4b0`, shots 2-4
`0b8200910091001645b6c4c02c2f44b2108a443885b7c4b0`, grenade `0b82000700090075011ac5dc211544e2fe034562da19c5b0`.

Findings:

1. **Layout CONFIRMED** bit for bit, both shapes (with extra + hit: 887 bits; no extra, no hit: 623 bits).
2. **One message per shot, `last` = 1 on each** (four semi-automatic shots 295 / 329 / 344 ms apart by `time_ms`).
   The user's ROM row of weapon 145 has one vector per round (key `[Weapon145] VectorsPerRound`), so "one per pellet"
   is consistent; a multi-pellet weapon was not fired (UNDECIDED for shotguns).
3. **Hit point CONFIRMED:** `origin + hit_dist * dir` equals `extra_a` with error 0.000 in all four hits (`dir` is a
   unit vector). The extra block of a hit therefore carries the hit object again (`extra_obj` = `hit_obj` = 408) and
   the hit point itself; `extra_b` = (-1, 0, 0) is PLAUSIBLY the surface normal at the hit (the shooter stood at lower
   x). **CORRECTION 2026-10-26 (client-hitnodes): not a constant and not the surface normal; it is the normal of the face of the doubled object box that the ray entered (C2S 4.1.1); the shooter at lower x made it -x.** `extra_kind` 24 / 11 is PLAUSIBLY a model part index (UNPROVEN; the reference stack's damage was the same for both).
4. **`shot_no` is not a counter:** all five carry 0x3F800000, the bit pattern of the float 1.0 (PLAUSIBLY a damage
   or spread scale of weapon+0x20c).
5. **`stat`** (local_player+0x1fc) grows by 2 per 0x82, the grenade throw included (2, 4, 6, 8, 10). Meaning UNPROVEN.
6. **`yaw`** is the controller yaw, unwrapped (9.4255 = 3pi for the grenade; the `aim` quaternion gives 3.1423, the
   same angle wrapped). It equals the yaw of `aim` (2 atan2(y, w)) in every message; it is **not** the body yaw of the
   0x64 view block, which lagged by 0.035 rad at the shots (section 5).
7. **`time_ms`** = client time x 1000: the 0x64 trailer `time` of the update before shot 1 is 75.327 / 75.395 s.
8. **`origin`** = the shooter's 0x64 position + the 0x64 `eye_offset` (0, 33, 0): (-1468.2, 672.0, 1104.2) + 33 in y.
9. **Geometry against the victim:** the bot's last reported position was (-289.0, 661.4, 1775.0). All four hit points
   lie on the plane x = -318.98, i.e. 30.0 units in front of the victim's centre on the shooter's side, 24-27 units above
   its centre, 2-9 units off in z. The reference stack's teleport box for the bot (0xa0 flag 0x0020) was 15 / 50 / 15, so the
   client's hit volume is not that box (PLAUSIBLY a model hit box). DSV-11's acceptance box (24 / 53 / 24 grown by
   48) accepts all four; its head test (top 20 %) classifies all four as body hits, which agrees with the reference stack's
   `hit_zone` 3 (not 1).
10. The server answered each 0x82 within one reference stack tick (0 / 32 / 47 / 47 ms).

Replacement text for C2S §4.1 (field list, meanings column):

> `shot_no:u32` -> `w20c_raw:u32`: the raw 32 bits of a float of weapon+0x20c (OBSERVED 1.0 = 0x3F800000 in every
> message of capture tdm4). `stat:u32`: OBSERVED +2 per `UP_FIRE`. `yaw:f32`: the controller yaw local_player+0x4c,
> not wrapped; equal to the yaw of `aim`. Extra block of a hit (OBSERVED): `extra_obj` = `hit_obj`, `extra_a` = the hit
> point (`origin + hit_dist * dir` exactly), `extra_b` = a unit vector against the shot (PLAUSIBLY the surface normal),
> `extra_kind` a small number per hit (24, 11 observed; PLAUSIBLY a model part). A miss without an extra block is
> 623 bits; a hit with both blocks 887 bits.

### 2.2 `UP_PLAYER_UPDATE` (0x64) while approaching and firing, C2S §2.1 and MOVEMENT §5.1

Spec text (C2S §2.1): "Unreliable form ... at least `1 / R` seconds have passed since the last unreliable send ...
**15.0** when it is missing"; flag table "0x2000 | the current weapon's slot +0x134 returns true | none"; MOVEMENT §5.1
"0x00000400 | (aim) | command 8 on, command 2 off, not sprinting, and a weapon test (cshell+0x1f4080) false".

Evidence: all 1,103 unreliable updates parse with the derived layout and zero leftover bits (C2S §12.1 U11 closed for
this capture: the base variant never ran). Flag words seen: 0x1521 (614), 0x1421 (213), 0x1121 (78), 0x1721 (48),
0x1021 (41), 0x1321 (36), 0x1221 (27), 0x3521 (27), 0x1621 (8), 0x3421 (6), 0x3321 (4), 0x3221 (1); the reliable
state form `[0x0010][1]` once at t = 62.953. Rate: 74-75 updates per 5 s window from t = 65 to 135 (15 Hz), also
during the shots (74 in 75..80 s). Around the four shots (the control word follows the optional blocks):

| t | seq | pu_flags | control | note |
|---|---|---|---|---|
| 75.922 | 2945 | 0x1321 | 0x200 | standing, `RUN` only |
| 76.000 | 2953 | 0x3321 | **0x600** | fire key down; 0x2000 set |
| 76.062 | 2964 | 0x3221 | **0x600** | (shot 1 sent at 76.078) |
| 76.125 | 2978 | 0x1321 | 0x200 | `weapon_f` 0.049 |
| 76.406 | 3018 | 0x3321 | **0x600** | after shot 2 (76.359) |
| 76.734 | 3072 | 0x3321 | **0x600** | after shot 3 (76.687) |
| 77.078 | 3122 | 0x3321 | **0x600** | after shot 4 (77.031) |
| 105.781 - 107.937 | 7328 - 7649 | 0x3421 / 0x3521 | **0xe01** (0xf01 once) / 0x400a01 | grenade held while walking (0x400 with `FWD`, `RUN`, `ANY_MOVE`); 0x400a01 = sprinting, fire bit suppressed (106.531 .. 106.797) |
| 108.000 | 7660 | 0x1521 | 0xa01 | released; the 0x82 of the throw follows at 108.156 |

Control words over the whole stream: 0x400a01 (354), 0xa01 (336), 0x200 (186), 0xa81 (57), 0xb01 (34), 0x400a81 (30),
0xe01 (27), 0xb00 (21), 0xa80 (17), 0x0 (16), 0x400b01 (14), 0x600 (5), 0xa02 (5), 0xf01 (1).

Findings:

1. **Bit 0x400 is the fire key held** (PROVEN-CAPTURE by correlation): it appears in exactly the five updates that
   bracket the four rifle shots (five because the first press spans two updates) and in the updates of the 2.2 s
   grenade hold (105.781 .. 107.937), and nowhere else (0x600 x 5, 0xe01 x 27, 0xf01 x 1). It never coincides with
   `SPRINT` 0x400000, as MOVEMENT §5.1 says ("not sprinting"): in the five hold updates 106.531 .. 106.797 the player
   sprinted (0x400a01), the bit was off and pu_flags 0x2000 stayed on. The MOVEMENT name "(aim)" should read "(fire held)"; command 8 is
   PLAUSIBLY the primary fire command.
2. **pu_flags 0x2000** ("weapon slot +0x134 true") is set in the same frames as 0x400 (38 updates: 76.00, 76.06,
   76.41, 76.73, 77.08 and 105.78 .. 107.94): PLAUSIBLY "the weapon is firing or charging".
3. **pu_flags 0x0200 `weapon_f`** after each shot: 0.049, then 0.038, then 0 (0.152, 0.042, 0.007 after the draw at
   75.45). PLAUSIBLY the recoil or spread value of the weapon decaying.
4. **No `JUMP` 0x10 and no `CROUCH` 0x20** in any of the 1,103 control words, although the client sent 26 jump
   notices (0x76 [1][1]): the jump bit lives for one mover frame and the 15 Hz sampler does not catch it. A server
   must take jumps from 0x76, not from the 0x64 control word. (No crouch was attempted in this capture: UNDECIDED.)
5. There is no fire bit in the 0x64 flags and no extra 0x64 per shot: shots travel only as 0x82.

Replacement text for MOVEMENT §5.1, row 0x400: "| 0x00000400 | (fire held) | command 8 on, command 2 off, not
sprinting, and a weapon test (cshell+0x1f4080) false; OBSERVED (tdm4) only while the fire key is held, together with
`pu_flags` 0x2000 |". Add to C2S §2.1 after the flag table: "OBSERVED (tdm4): `JUMP` never appears in a sent control
word; jumps are reported by 0x76 [1][1] only."

### 2.3 `UP_WEAPON_CHANGE` (0x7f), C2S §4.4

Spec text: "`[slot:u8][weapon_id:u16][aux:u16]` ... `aux`: a value from the new weapon (slot +0x94 or +0x7c;
PLAUSIBLY its select animation or clip state; UNPROVEN)".

Evidence (14 messages, 56 bits each, hex head `0b7f0091009100` for slot 0):

| slot | weapon_id | aux | count | the server's 0xb2 for that weapon (`[weapon][ammo_kind]...`) |
|---|---|---|---|---|
| 0 | 145 | 145 | 8 (4 at t = 62.828 answering the doubled spawn set) | `0db2 9100 9100 ...` (ammo kind 145) |
| 2 | 0 | 0 | 4 | `0db2 0000 0000 ...` (ammo kind 0) |
| 3 | 7 | 9 | 2 | `0db2 0700 0900 ...` (ammo kind 9) |

Every 0x7f is preceded in the same datagram by 0x76 [3][2] (14 of 14). The secondary weapon (slot 1, weapon 164) was
never selected.

Verdict: layout CONFIRMED; `aux` is the new weapon's ammunition id (3 of 3 weapons). Replacement text: "`aux`: the
ammunition id of the new weapon (OBSERVED tdm4: equal to the `ammo_kind` the server gave that weapon in 0xb2); always
sent right after `UP_WEAPON_EVENT` [3][2] in the same datagram."

### 2.4 `UP_MOVE_EVENT` / `UP_WEAPON_EVENT` (0x76), C2S §2.2 and §4.5

Evidence (72 messages, all 32 bits): [1][1] jump x 26, [1][2] fall x 3, [1][3] land x 29, [3][2] select x 14. No
category 3 events 1, 3 or 5 and no category 5. **There is no fire or reload event:** the firing produced none, and
no reload happened (0x83 was never sent, so `UP_RELOAD` stays UNDECIDED for this capture).

### 2.5 `UP_WEAPON_SOUND` (0x80), C2S §4.3

Evidence: 6 messages, 144 bits, `[sound_no][weapon_id][own_id][weapon_pos]`: sound 3 / weapon 145 / own id 1 at
63.500, 75.547, 83.797, 104.156 and 108.828 (each 0.5-0.7 s after a 0x7f to slot 0) and sound 10 / weapon 7 at
106.266 (early in the grenade hold). Verdict: layout CONFIRMED; `own_id` = the client id 1 (`S2C_CLIENT_ID`). These
are animation-key sounds of the select and hold animations, **not** per-shot sounds: no 0x80 accompanied the four
shots.

### 2.6 Grenade (0x4d and the throw), C2S §4.6

Spec text: "`UP_THROW_RELEASE` 0x4d, no fields, R ... when the throw key ... passes ... Meaning PLAUSIBLE: the release
frame of a throwing animation".

Evidence: 0x7f [3][7][9] at 104.906; fire held from 105.78 (control 0x400 and pu_flags 0x2000); 0x80 [10][7] at
106.266; **0x4d x 18** (16 bits, `0b4d`) from 106.453 to 107.953, 62-78 ms apart, with one 406 ms gap after the first that
matches the sprint of 106.531 .. 106.797 (fire bit suppressed, 2.2);
fire released at 108.000; **one 0x82** (weapon 7, ammo 9, no extra, no hit, `last` 1, `dir` (-0.001, -0.911, -0.412),
i.e. at the own feet as the capture guide asked) at 108.156, in the same datagram as the switch back to the rifle.

Verdict: DIFFERENT. 0x4d is sent repeatedly while a throwable is held (PLAUSIBLY on every pass of the hold animation's
key), and the release is the 0x82. Replacement text: "`UP_THROW_HOLD` (0x4d), no fields, R: sent on each pass of the
throw key while the fire key holds a throwable (OBSERVED 18 times in 1.5 s); the release itself is one `UP_FIRE` 0x82
with the throwable's weapon and ammunition ids, no hit block, `last` = 1, and the throw origin and direction.
**Server answer:** none for 0x4d; the server creates the thrown object from that 0x82." The reference stack answered the 0x82
with 0x66 kind 1 [weapon 7][ammo kind 9][ammo 0.0][slot 3][1] to the thrower (its grenade count) and, 2.47 s later,
the explosion (section 3.6).

### 2.7 Melee

The knife (slot 2, weapon 0) was selected four times and never used: no 0x82 with weapon 0. UNDECIDED.

### 2.8 The real client did not die (task item 3)

It received 0x67 only once (state 1 at 62.812) and sent none of 0x89, 0xc5, 0xd2, 0x60, 0x61. It took two damage
events: 0x74 at 101.516 (`damage` 2, no shooter, `dmg_kind` 0, type 5; health 98.0) and 0x74 at 110.672 from its own
grenade (`damage` 60, type 7; health 38.0). Its respawn timing relative to an arming message therefore cannot be
measured in this capture; section 4 gives the client-side rule from the binary.

## 3. Server -> client: the reference stack's reactions (REFERENCE unless marked)

### 3.1 Each hit, to the victim (bot link) and to everyone

Spec text (S2C §10 step 14): "To the victim: 0x74 `GS_DAMAGE_TAKEN` (4.3) and 0x66 kind 2 with the new health (kind 3
for armour)". S2C §4.3 layout of 0x74: `[from_dir:vec96][unused:u8][mode:u8][ammo_kind:u16][dmg_kind:u8][zone:u8]
[part:i8][damage:u32]`; 0x66 `[kind:u8][b1:u8][b2:u8][value:f32]`.

Order inside the reliable block of the first hit (bot link, seq 2969, block 652, 10 entries): 0xa6 note on 408 (sub
0x20, see 3.4), one `S2C_FILE_LIST` entry, **0x74**, two `S2C_EVAR` messages (names not reproduced), 0xa6 note sub
0x14 [73], 0xa6 note sub 0x15 [95], **0x66 kind 2**, **0x66 kind 3**; then unreliable: two engine-17 type 0x06
records (3.6). The real client's block at the same tick (seq 2968) has the same entries without 0x74 and 0x66. Hits
2-4 carry only the 0xa6 notes, 0x74 and the two 0x66.

0x74 to the bot, bit by bit (200 bits, all four hits; hex head of hit 1
`0d74adeb6c3ef13b67bb34f1093e0201ffff0302001b0000`):

| field | bit | hits 1-4 | the client's own damage events (101.516 / 110.672) |
|---|---|---|---|
| `from_dir` | 16 | (0.2314, -0.0035, 0.1347), (0.2317, -0.0034, 0.1342), (0.2320, -0.0040, 0.1336), (0.2318, -0.0033, 0.1340) | (0, 0.0200, 0) / (-0.0085, 0.5931, -0.0905) |
| `unused` | 112 | 2 | 5 / 7 |
| `mode` | 120 | 1 | 1 / 1 |
| `ammo_kind` | 128 | 0xFFFF | 0xFFFF / 0xFFFF |
| `dmg_kind` | 144 | 3 | 0 / 0 |
| `zone` | 152 | 2 | 5 / 7 |
| `part` | 160 | 0 | 0 / 0 |
| `damage` | 168 | 27 | 2 / 60 |

0x66 to the bot after each 0x74 (72 bits each; `[kind][0][0][value]`): health 73.225, 46.45, 19.675, 0.0 (kind 2,
bit 40) and armour 94.75, 89.5, 84.25, 79.0 (kind 3). Per hit: health -26.775 and armour -5.25, every time.

Findings:

1. **`from_dir` is the shot direction scaled by the health loss / 100**: |from_dir| = 0.2678 = 26.775 / 100 with the
   direction of the 0x82 `dir`; the fall event gives 0.02 = 2 / 100 straight up, the grenade 0.600 = 60 / 100. The
   client uses the magnitude: the damage-indicator sector grows by it, capped at 1.0 (S2C §4.3); knockback normalises
   it (MOVEMENT §6.9). A unit vector (what DeCombat sends now) lights the indicator fully on every hit.
2. **`unused` repeats `zone`** in all six 0x74 seen (2 / 5 / 7).
3. **`zone` is a damage type, not a body zone**: it equals the user's ROM key value `[Ammo145] InstDamageType` for
   the rifle hits and `[Ammo9] AreaDamageType` for the grenade (key names only; values read at run time). The four
   hits went to different model parts (`extra_kind` 24 and 11) and still carried the same `zone`.
4. **`damage`** is the rounded health loss (27 for 26.775; 2; 60). The grenade's 60 equals the user's ROM key value
   `[Ammo9] AreaDamage`, and the reference stack sent no armour change for it.
5. **Amount per rifle hit vs the ROM:** both the health loss and health + armour loss per hit are below the user's
   ROM key value `[Ammo145] InstDamage`; the hit distance 1326-1330 lies inside the first range band
   (`[Weapon145] Effectrange0`, factor `[Ammo145] Rangedamage0`), so range falloff does not explain it. The reference stack
   applies its own armour share and a further factor. These are reference stack rule values: **not taken**; DSV-11 stays the
   DeCombat rule (it would kill the bot with three of these hits, the reference stack needed four).
6. `ammo_kind` 0xFFFF in every 0x74: the reference stack never names the ammunition row, so the client's camera shake by
   ammunition row (S2C §4.3) does not run with the reference stack.
7. Health and armour also go to **every** client as 0xa6 notes on the victim's character object (type 0x18, sub 0x14
   = health percent u8, sub 0x15 = armour percent u8: 100 -> 73 / 95, 46 / 90, 20 / 84, 0 / 79, i.e. rounded), plus a
   damage note (sub 0x20, 128 bits, `0da6189801200400000000000000bd00`, ending with the attacker's object 0x00bd).

### 3.2 The kill

Spec text (S2C §10 steps 15-17): "Per player: 0x6d `GS_KILL` ... To the victim: 0x67 `[2]` (dead), later 0x67 `[3]`
... Per player: 0x4a `GS_TEAM_SCORE` with the new team kill totals. Optional to the killer: 0xc8 `GS_MULTI_KILL`."

Order inside the reliable block of the fourth hit (bot link, seq 3123, block 682, 25 entries): 0xa6 damage note on
408, two `S2C_FILE_LIST` entries and one `S2C_EVAR` (the case model, by form), **0xd6 sub 0**, **0xd6 sub 2
[case_key 1][owner 1]**, 0xa6 type 0x17 on object 414 sub 2 [16] (the dropped case's countdown, HISEC_2009 §3.1),
**0x6d**, **0xb5 [1][0]**, 0x74, 0xa6 sub 0x14 [0], 0xa6 sub 0x15 [79], 0x66 health 0.0, 0x66 armour 79.0, **0x67 [3]**,
one `S2C_EVAR`, five `S2C_FILE_LIST` entries, two `S2C_EVAR`s (the corpse, by form), **0x67 [2]**; then unreliable the
two type 0x06 records and 0xa6 sub 0x1d on 408 (48 bits). The real client's block (seq 3124) has the same entries
without 0x74, the two 0x66 and the two 0x67.

0x6d, bit by bit (151 bits, identical on both links; `0d6d0001019100031020000000000000000000`):

| # | field (S2C §3.3.1) | bit | value |
|---|---|---|---|
| 1 | `victim` | 16 | 0 (the bot) |
| 2 | `killer` | 24 | 1 (the real client) |
| 3 | `kill_kind` | 32 | 1 |
| 4 | `weapon` | 40 | 145 |
| 5 | `hit_zone` | 56 | 3 (not a head shot; 1 would be) |
| 6 | `f_relation` | 64 | 0 |
| 7 | `f_feed2` | 65 | 0 |
| 8 | `keep_hud` | 66 | 0 |
| 9 | `f_feed6` | 67 | 0 |
| 10 | `streak_flag` | 68 | 1 |
| 11 | `feed_value` | 76 | 2 |
| 12 | `no_count` | 84 | 0 |
| 13 | `has_revenge` | 85 | 0 |
| 15 | `f_revenge_done` | 86 | 0 |
| 16 | `coop_a`, `coop_b` | 87, 119 | 0, 0 |

Findings:

1. Layout CONFIRMED as a well-formed message of S2C §3.3.1 (151 bits, no field left over).
2. **Team kills travel in 0xb5 (`GS_ROUND_WINS`) [1][0]**, sent to both right after 0x6d. 0xb5 [0][0] was also sent at
   the match start (after 0x44, both links). **0x4a and 0xc8 never appear** in the capture on either link (nor in
   tdm1 / tdm2). The client counts the kill itself from 0x6d (S2C §3.3.1 step 6), so 0x4a is not needed for the HUD
   team counters; 0xb5 feeds the scoreboard values (cshell+0x92bb0).
3. The kill messages go out **before** the victim's own 0x74 / 0x66 / 0x67 of the same hit.
4. Acceptance by the real client (REFERENCE-CONFIRMED, weak): after this block it kept its 15 Hz 0x64 stream and its
   game session for 60 s without an error, sent no 0x8c roster query (it sends one when a roster lookup fails, C2S
   §5.5; it did so once at 63.266), and the user walked to the dropped case, which the reference stack registered (0xd6 sub 1
   at 81.656). No message of the client proves how the kill feed was drawn.

### 3.3 Life states of the dead bot: 0x67 3, then 2, then nothing

Spec text (PD-S2C-2): "player states on the wire: 1 alive, 2 dying, 3 waiting for respawn"; DC-C2S-10: "death: state 2,
state 3 after 2000 ms, respawn 7000 ms after the death".

Evidence: 0x67 [3] (`0d6703`) and 0x67 [2] (`0d6702`) in the **same** reliable block (seq 3123, t = 77.078), 3 first,
0 ms apart, with the corpse's file and variable messages between them. No 0x67, 0xd4, 0xd7 or 0xdb followed for the
remaining 104.2 s of the capture (to t = 181.234). The bot's own log: `life_states [1, 3, 2]`.

Verdict: DIFFERENT (REFERENCE). The reference stack leaves a dead player in state 2 until something else happens; it has no
server-side respawn timer. For the 2009 client both orders arm its own respawn wait (section 4): state 2 stores the
death time (local_player+0x440) and state 3 does not clear it (DISASM cshell+0x61975..0x619b4 versus 0x619b9..0x619e4).
DC-C2S-10's order (2 at once, 3 after 2000 ms) therefore stays valid.

### 3.4 What the server kept sending to the dead bot (task item 2)

From t = 77.1 until the capture ends (181.2), to connection 5:

| what | count / period | grade |
|---|---|---|
| engine 8 (reliable) + engine 10 (unreliable) | about 29.5 per second each, unchanged by the death or the result screen (1,769 in 77.1-137.0 s; 1,128 in 143.2-181.3 s) | REFERENCE |
| 0xa6 type 0x18 sub 0x14 / 0x15 on both characters | every 2.02 s each (51 times for 408, always 0 / 79 after the death) | REFERENCE |
| 0xa6 type 0x18 movement notes of the shooter (subs 0x1a, 0x80, 0xc0, 0x40) | 564, 504, 69, 18 while he played | REFERENCE |
| 0xa2 `GS_PING_TABLE` | every 3.0 s, **unreliable** (`0da20100000000000b00ffff` = pid 1 ping 0, pid 0 ping 11; later `0da200000c00ffff`) | REFERENCE |
| engine 17 relays of the other player (jump sounds, shots, explosion) | 29 | REFERENCE |
| the case countdown (0xa6 type 0x17 object 414 sub 2: 16, 15, 14 ...) and 0xd6 sub 1 at the pickup | 5 + 1 | REFERENCE |
| file lists / variables | 4 / 2 | REFERENCE |
| anything that changes its life state; a bye | none | REFERENCE |

The bot itself sent only acks and 11 `C2S_FILE_ACK` after its death.

### 3.5 Kill feed and announcements the real client received

0xd6 sub 0 (chat line of HISEC_2009 §3.2), 0xd6 sub 2 [1][1], the case object and its countdown notes, 0x6d (kill feed
line, the client's own kill / death counters), 0xb5 [1][0]. No 0xc8, no 0x4a, no 0x68 text, no 0x65 or 0x46 (it left
before the round end). Grade REFERENCE; acceptance as in 3.2 item 4.

### 3.6 Relays and shot effects (C2S §12.1 U5, S2C §8.2)

| client sent | the reference stack sent to every client, the sender included (engine 17 = shell object message without an object, unreliable) | count |
|---|---|---|
| 0x80 [s][w][id][pos] | type 0x19 `[k = s][w:u16][pid:u32][pos:vec96]`, 168 bits, e.g. `111903910001000000b4d7ccc3518cca431e0b1ec4` | 6 of 6 |
| 0x76 [1][1] (jump) | type 0x19 `[k = 0x0f][0:u16][pid:u32][pos:vec96]` | 26 of 26 (no relay of land / fall) |
| the grenade's 0x82 | type 0x19 `[k = 0x12][6:u8][pid:u32][pos:vec96][1:u8][0:u8]`, 176 bits | 1 |
| each rifle 0x82 with a hit | two type 0x06 records of 432 bits: `[408][189][145][145][6]...` with the muzzle point (= `weapon_pos`) and the hit point (within 0.8 units of `extra_a`), and `[0][189][145][145][20]...` with the muzzle point and a point 80-90 units farther along the ray | 4 x 2 |
| (explosion, 2.47 s after the throw) | type 0x06 `[0xFFFF][189][7][9][71]...` with the throw origin and the explosion point | 1 |
| 0x7f, 0x4d, 0xca | no shell or engine-17 message | - |

The type 0x19 records match the inline reader of S2C §8.2 (`[k]` then `[u16][u32][vec96]`, or for k = 0x12
`[u8][u32][vec96][u8][u8]`) exactly in length. Type 0x06 belongs to the class-read group, so its remaining fields stay
UNPROVEN. Whether the weapon change reaches the other client inside the object stream (engine 8 / 10) was not decoded.

## 4. Respawn: the 2009 client arms its own wait (correction to C2S §3.3, GAME_MODES §4.2 and GMD-3)

Spec text (C2S §3.3): "Arming happens only while `life_state` is 2, 3, 5 or 6, and only from two places:
cshell+0x5df50, called by the roster handler for server message **0xd7 sub-id 2** ... cshell+0x5dfe0, called from
cshell+0xbac00 (the reply path of server message 0xdb ...) ... Nothing else arms it. A server that never sends either
message never receives the respawn requests below". GAME_MODES §4.2 ("the wait is armed only by server 0xd7 sub 2 ...
or 0xdb sub 6 (C2S §3.3, PROVEN)") and GMD-3 repeat it.

Evidence (DISASM: capstone over the user's own `Game/CShell.dll`, image base 0x37100000; the routine lies in a gap
of the clean decompiler function table, whose function cshell+0x60a20 ends at cshell+0x60abf):

1. A scan of the whole `.text` section for stores to `local_player`+0x450 and +0x454 finds, besides the two known
   writers (cshell+0x5df8d / 0x5df97 and cshell+0x5e006 / 0x5e010), a **third arming site**: cshell+0x60b94..0x60baa
   (+0x44c := `GetTickCount()`, +0x450 := 7000, +0x454 := 1) and cshell+0x60bd6..0x60bec (the same with +0x450 :=
   1500). The other matches are unrelated objects or disarming stores (cshell+0x618fd, 0x62132, 0x62283, 0x623f9 write
   0).
2. Both sites belong to the routine **cshell+0x60ac0**, whose only reference is PE-DATA VA 0x374d32ec, i.e. **slot
   +0x70 of the derived `local_player` method table** VA 0x374d327c. It switches on the observer camera mode
   `local_player`+0x428 (jump table VA 0x37160d70: mode 0 -> cshell+0x60adc, 2 -> cshell+0x60ce0, 3 -> cshell+0x60cf6,
   4 -> cshell+0x60c0f, 1 -> no action).
3. **Mode 0** (the own view, which the player has when he dies; 0x67 state 1 sets it through cshell+0x60680(0)): with
   `life_state` 6 it only enters the free camera; otherwise it binds the own roster record, selects the killer camera
   (mode 4) when `roster`+0x10 (the last killer id, written by 0x6d, S2C §3.3.1 step 3) is not negative, else the
   follow camera (mode 2), and then, by `mode_class` (`client_shell`+0x16938):

   | `mode_class` | effect (DISASM cshell+0x60b2e..0x60c0e) |
   |---|---|
   | 1, 4, 6, 7, 8 | arm 7000 ms |
   | 5 | arm 7000 ms only when the own roster record +0x1f8 is above 0 |
   | 9 | arm 7000 ms only when `mode_rules`+0xb4 is 0 |
   | 10 | arm 1500 ms |
   | 2, 3, 11, 12, 13 and others | nothing |

   Arming also starts the mode-notice countdown (cshell+0x5c860 on `hud_mode_notice` with 7).
4. **The caller** is the per-frame update cshell+0x61e40 (DISASM cshell+0x62467..0x62494): every frame while
   `life_state` is 2, 3, 5 or 6 **and the death time `local_player`+0x440 is non-zero**, it calls slot +0x70. The death
   time is written by 0x67 state 2 (cshell+0x619d5..0x619de, the state-2 case of the derived handler's jump table VA
   0x37161b38) and cleared by state 1 (cshell+0x618e0); state 3 (case cshell+0x61975) does not touch it. After the first
   call the camera mode is 4 or 2, so the wait is armed **once per death**.
5. **The request** (re-read, unchanged from C2S §3.4; DISASM cshell+0x6203e..0x62132): for `mode_class` 1, 4, 5, 6, 9,
   `life_state` 2 / 3 / 5 / 6, armed, and `GetTickCount() - start > wait` (strictly greater, `jbe` skips), the client
   builds a message of only the shell id (`write(0x89, 8)`), sends it with flag 1 (reliable) and disarms.

Consequence for tdm4: both links got `mode_class` 1 in 0x9e (`0d9e01...`; the bot report says `mode_class 1`). A 2009
client in the bot's place would have armed in its first frame after the block of t = 77.078 (0x67 [3] then [2], 3.3)
and sent **one 0x89 (`0b89`, 16 bits, reliable) at about t = 84.1**. The bot implements no such timer, sent nothing,
and the reference stack (which has no server-side respawn timer, 3.3) left it dead for 104 s: this explains the user's
observation. **What the reference stack answers to 0x89 is UNDECIDED:** no capture so far contains a death of the real client
(tdm1, tdm2 and tdm4 were checked; the real client was only damaged).

Verdict: C2S §3.3 **DIFFERENT** (wrong by DISASM). Replacement text for C2S §3.3, second paragraph onwards:

> The client keeps its own respawn wait (`local_player`+0x44c start, +0x450 length, +0x454 armed). It is armed from
> three places, each only while `life_state` is 2, 3, 5 or 6:
>
> 1. **By the client itself after a death** (DISASM cshell+0x60ac0, `local_player` derived slot +0x70, a region
>    outside the decompiler function table). The per-frame update cshell+0x61e40 calls it every frame while `life_state`
>    is 2, 3, 5 or 6 and the death time `local_player`+0x440 (set by 0x67 state 2, cleared by state 1) is non-zero. In
>    observer mode 0 it starts the death camera (killer camera when the killer id `roster`+0x10 is known, else follow)
>    and arms 7000 ms for `mode_class` 1, 4, 6, 7 and 8, for 5 when the own roster record +0x1f8 is above 0, for 9 when
>    `mode_rules`+0xb4 is 0, and 1500 ms for 10; other classes are not armed. It runs once per death (the camera mode
>    leaves 0).
> 2. cshell+0x5df50, from server 0xd7 sub 2 when `a` equals the stored value (re-arms 7000 ms; layout as before).
> 3. cshell+0x5dfe0, from the 0xdb sub 6 path (re-arms 7000 ms).
>
> A server therefore needs **no** arming message in the respawn classes: about 7000 ms (class 10: 1500 ms) after the
> client processed `GS_PLAYER_STATE` 2 it sends its respawn request (3.4) on its own. In classes 2, 3, 11, 12 and 13 no
> request is ever sent.

Replacement for C2S Appendix A step A11: "**A11 Respawn.** The 2009 client requests the respawn itself in TDM: about
7000 ms after it processed 0x67 state 2 it sends C> `UP_RESPAWN_REQUEST` 0x89 once (3.3, 3.4). **[DC] DC-KILL-1**: the
server answers a due request with the A6 spawn set at once and respawns on its own only as a fallback when no request
came."

GAME_MODES §4.2, first sentence: "The client arms its own respawn wait after a death (C2S §3.3, DISASM cshell+0x60ac0):
7000 ms in classes 1, 4, 6, 7, 8 (5 and 9 under the client state of C2S §3.3), 1500 ms in class 10; 0xd7 sub 2 and
0xdb sub 6 re-arm it." The class table gains "10: after 1500 ms".

## 5. Yaw at the default resolution (control comparison for the tdm2 mouse report)

Spec text (C2S §2.1): "`yaw8c = round(yaw(q) * 40.425354)` ... of the rotation `q` built from the body angles".

Evidence (real client, 1,103 unreliable 0x64, t = 62.953 .. 136.969): every one carries the 0x0020 view block, the
0x0001 eye offset and pu_flags 0x1000; 807 also carry `aim_q` (0x0100). Bit layout of a typical update (seq 2953,
t = 76.000, 928 bits, `0b6421332b00000000d7708a3f00000000202a5e3c9d7b0f`): `pu_flags` bit 16 = 0x3321, `yaw8c` bit 32
= 0x2b, body pitch / yaw / roll bits 40 / 72 / 104 = 0.0 / 1.0816 / 0.0, `aux` bits 136-327, `eye_offset` bit 328,
`aim_q` bit 424, `weapon_f` bit 552, control word bit 584 = 0x600, trailer from bit 616.

| measure | value |
|---|---|
| body yaw range (unwrapped) | -1.7988 .. 10.4470 rad, first -1.660 at 62.953, last 7.256 at 136.969 |
| updates whose yaw changed | 675 of 1,102 steps; 701 distinct values; total turning 43.96 rad (about 7 turns); largest step 0.982 rad |
| per 5 s window | yaw moved in every window from 65 s to 135 s (for example 85-90 s: 2.171 .. 5.042; 115-120 s: 8.199 .. 10.447) |
| body pitch / roll | always 0.0; pitch travels only in `aim_q` (its pitch component spans about 1.6 rad) |
| aim yaw vs body yaw | equal within 0.005 rad in 935 updates; otherwise the body lags the view by up to 0.7854 rad (pi / 4) |
| `yaw8c` | **truncation toward zero** of `wrap(body yaw) * 40.425354` in 1,103 of 1,103 (rounding matches only 527); seq 2953: 1.0816 x 40.425354 = 43.72 -> sent 43 |

Verdict: the mouse turned the view continuously at the default resolution; the 0x64 encoding carries it in both the
body angles and `aim_q`. `yaw8c` truncation CONFIRMS capval-transport V18 (tdm1). Replacement in C2S §2.1:
"`yaw8c = trunc(wrap(yaw(q)) * 40.425354)` (toward zero; OBSERVED tdm1 and tdm4) ... The body yaw follows the view yaw
(`aim_q`, controller +0x48..+0x50) with a dead zone of pi / 4 (OBSERVED); a server that needs the exact view direction
reads `aim_q` (flag 0x0100), sent whenever the view changed." For the tdm2 report: a missing mouse turn there is not
explained by the 0x64 format; compare its 0x64 stream with these ranges.

## 6. Round end and leave, both links (task item 5)

Real client (connection 6), PROVEN-CAPTURE unless marked:

| t | seq | message |
|---|---|---|
| 137.031 | 11851 (TCP 2) | lobby 0x394, body `94030a00000000000000` = phase 0 |
| 137.031 | 11852 | `UP_LEAVE_MATCH` 0xbd (`0bbd`, reliable) |
| 137.047 | 11857 | `C2S_BYE` (`06`, 8 bits) |
| 137.078 .. 137.375 | 11863 .. 11868 | transport bye code 5, four copies about 100 ms apart (REFERENCE) |
| 137.031 | 11854 / 11855 (TCP) | lobby 0x3042 [1001][state 0][0] to both accounts (REFERENCE) |
| 144.156 .. 144.469 | 12277 .. 12302 | transport plain bye, four copies, 7.1 s after the server's bye (tdm1: 8.7 s, capval-transport V13) |
| 144.156 .. 144.516 | TCP 2 | back in the room: 0x306 [1], 0x388 -> 0x389, 0x38a -> 0x38b |

The real client received no 0x46, 0x65, 0x3f or 0x99: it left first.

Bot (connection 5), REFERENCE:

| t | seq | message (decoded) |
|---|---|---|
| 137.375 (+0.344 s after the 0xbd) | 11867 | 0x6c [pid 1][kicked 0] `0d6c0100`; 0x46 [round_secs 3][win_team 2][0][0] `0d4603000000020000` |
| 140.078 (+2.703 s) | 12031 | 0x47 [0]; 0x6b own record (kills 0, **deaths 1** at bit 265, kind 2); 0x65 [pid 0][obj 408][exp_before 0][exp_gain 43][exp_bonus 0][gp_gain 34][gp_bonus 0][flag 0] (201 bits, `0d65009801000000002b0000000000000022000000000000`); 0x48 [3] |
| 140.078 | 12032 (TCP 4) | lobby 0x3004 body `0430120000002b0000003227000001000000` = exp total 43, GP total 10034, popup 1; then 0x345 |
| 143.109 (+3.031 s) | 12211 | 0x3f [result_flag 0][result_value 2] (`0d3f0400000000`, 49 bits) |
| 143.1 .. 181.2 | - | stream continues at 29.5 per second; no 0x99, no bye; the lobby's 0x38b of 144.516 still lists bot1 in state 3 with exp 43 |

Findings: the reference stack ended the round 0.344 s after the only team-1 player left (`win_team` 2 for the remaining side)
and sent 0x46 in a single-round TDM match (S2C §11.2 item 7: it does). Its order is 0x6c, 0x46, then 0x47, 0x6b refresh,
0x65, 0x48, then 0x3f; no 0x41, no 0xb5 at the end, no 0x99. `round_secs` is 3 in both 0x46 and 0x48; `result_value` 2
equals the winning team (PLAUSIBLE; one sample). None of these was seen by a 2009 client, so nothing here is
REFERENCE-CONFIRMED.

## 7. Lobby result for both accounts (task item 5)

- **0x394** (PROTOCOL_2009 `match_join_failed`, "LIVE: 0 then 1 after a failed connection"): OBSERVED once with phase 0
  at the moment the player confirmed leaving a running match, in the same tick as the game-server 0xbd. Replacement
  text: "0x394 `match_leave_notice` C->S 0xa: +6 u32 phase. Sent with phase 0 when the player leaves a running match
  from the menu (OBSERVED tdm4) and with 0 then 1 after a failed connection (LIVE)." The reference stack answered with 0x3042
  [id][0][0] to the room (REFERENCE).
- **0x3004** went only to the bot (exp total 43, GP total 10034 = its 10,000 start + the 34 of 0x65, popup flag 1),
  in the same tick as its 0x65. The real client got none: it left before the result and its room record still shows exp 0.
- **Kill / death numbers stored by the lobby: not decodable from this capture.** No message carrying them (0x201,
  0x303, 0x379) was sent after the match on either link, and the statistics words of the room records (0x38b +0x46..
  +0x5d) are 0 for both accounts before and after the match. The game-server to lobby report is not tapped, and the
  reference stack's own logs may be used for timestamps only. On the game link, bot1's 1 death is visible in the reference stack's 0x6b of
  140.078, and player1's 1 kill only as the 0x6d (killer pid 1) and 0xb5 [1][0].

## 8. What dc-dserver and dc-botclient must do differently (task item 6)

Read-only comparison with `crates/dc-dserver/src/game/combat.rs` (master) and
`reports/dserver-slice2.md`: dc-dserver parses 0x82 with exactly the observed layout, validates
`origin + hit_dist * dir` against the victim's box grown by 48 units and the origin within 160 units of the shooter
(both accept the four tdm4 hits), applies the ROM `InstDamage` with its own armour rule, sends 0x74 (unit `from_dir`,
`zone` / `dmg_kind` / `mode` / `part` = 0, `ammo_kind` = the reported id, `damage` = the rounded full damage) and 0x66
health / armour to the victim, then 0x6d to everyone, 0xc8 to the killer on a streak, 0x4a to everyone and 0x67 state 2
to the victim; state 3 after 2000 ms; **the spawn set 7000 ms after the death on its own timer**; a 0x89 is accepted
only while dead and at least 7000 ms after the death.

### 8.1 dc-dserver

Required by 2009 client evidence:

| # | change | why |
|---|---|---|
| S1 | Make the client's respawn request the normal trigger: on 0x89 (0xc5 in classes 7 / 8, 0xd2 in class 10) from a dead player whose death is at least `respawn_ms - 500` old, send the spawn set at once; keep the own timer only as a fallback for clients that never ask (proposed 10,000 ms after the death); ignore a request from a living player without a warning. Never send 0xd7 sub 2 / 0xdb sub 6 for arming in TDM. | the 2009 client arms itself and asks once, strictly more than 7000 ms after it processed 0x67 state 2 (section 4); with a server timer of exactly 7000 ms the two race and the request arrives from a living player (logged as "too early" today). The reference stack does not respawn without a request (3.3). |
| S2 | Keep sending 0x67 state 2 at the death (the order 2 then 3 of DC-C2S-10 is fine); never send state 3 alone to a dying player. | the client's death time comes only from state 2; without it the client never arms and never asks (section 4 item 4) |
| S3 | Read jumps from 0x76 [1][1] / [1][3], not from the 0x64 control bit 0x10. | the bit was never set in 1,103 updates with 26 jumps (2.2) |
| S4 | Treat 0x64 control bit 0x400 as "fire held" (logging, anti-cheat plausibility), not as aim. | 2.2 |

Recommended (the 2009 client's own use of the field is PROVEN; the value form is the reference stack's):

| # | change | why |
|---|---|---|
| S5 | 0x74 `from_dir` = the unit shot direction x (health loss / 100). | the client adds the magnitude to its damage-indicator sector (capped at 1.0); a unit vector lights it fully on every hit (3.1 item 1) |
| S6 | 0x74 `zone` (and the byte before it) = the damage type of the reported ammunition, read at run time from the ROM key `[Ammo<n>] InstDamageType` (`AreaDamageType` for area damage); `damage` = the rounded health loss. Keep `dmg_kind`, `mode`, `part` as UNPROVEN constants (the reference stack uses 3 / 1 / 0 for a weapon hit, 0 / 1 / 0 otherwise). | the field is a damage type (3.1 item 3); the client's view punch is chosen by it (S2C §4.3) |
| S7 | Also send 0xb5 [team 1 kills][team 2 kills] after each kill and 0xb5 [0][0] at the round start; keep 0x4a. | the reference stack carries team kills only in 0xb5 (it feeds the scoreboard values, cshell+0x92bb0); 0x4a sets the HUD counters, which the client also counts itself from 0x6d |
| S8 | Relay sounds as engine 17 (shell object message without an object) type 0x19 to every client including the sender, unreliable: 0x80 -> `[k = sound_no][weapon:u16][pid:u32][pos:vec96]`; 0x76 [1][1] -> `[k = 0x0f][0:u16][pid:u32][pos:vec96]`; a throwable's release 0x82 -> `[k = 0x12][u8][pid:u32][pos:vec96][u8][u8]`. | the client's inline reader of type 0x19 is PROVEN (S2C §8.2) and the reference stack's records match it in length; today other players hear no draw or jump sounds |
| S9 | At the match end, refresh every player with 0x6b kind 2 (absolute kills / deaths) before 0x65. | the reference stack does (6); the result screen then shows correct totals whichever way the client counted |

Reference stack parity only (REFERENCE forms whose layout is not yet specified; implement after a spec): two engine-17 type 0x06
impact records per hitting shot (3.6); 0xa6 type 0x18 notes sub 0x14 / 0x15 (health / armour percent of every
character to every client after each hit and every 2 s) and sub 0x20 (damage note with the attacker's object); the
0x6d values `kill_kind` 1, `hit_zone` 3 for a body hit, `streak_flag` 1, `feed_value` 2, `f_relation` 0 (dc-dserver: 0,
0, 0, 0, 1; which ones the 2009 server used is UNDECIDED and the user's view of the kill feed should decide); the
round-end set 0x47 / 0x48 with `round_secs` 3 and 0x3f `result_value` = the winning team. No change needed: the hit
geometry (keep the 48-unit growth, the client's hit points lie 30 units from the victim's centre), the order of the kill
messages relative to the victim's 0x74 / 0x66, and the ping-table period (the reference stack's 3 s is a rule value).

### 8.2 dc-botclient (to respawn and to finish a match)

| # | change | exact form |
|---|---|---|
| B1 | **Respawn wait** (the trigger) | on shell 0x67 with state 2 (also when 0x67 state 3 came first in the same block): `start := now` (the tick that processed the message), `wait := 7000 ms` for `mode_class` 1, 4, 6, 7, 8; class 5 only when the own record's 0xd7-sub-2 value is above 0; class 9 only while the mode-rules phase is 0; class 10: 1500 ms; classes 2, 3, 11, 12, 13: no wait. Once per death. 0xd7 sub 2 with `a == b` and 0xdb sub 6 re-arm it (`start := now`, 7000 ms). |
| B2 | **Respawn request** | while `life_state` is 2, 3, 5 or 6, armed, and `now - start > wait` (strictly): send ONE reliable `C2S_SHELL_MSG` with shell id **0x89 and no fields** (bytes `0b 89`, 16 bits) for classes 1, 4, 5, 6, 9; 0xc5 (no fields) for 7, 8; 0xd2 `[0:u8]` for 10, only while a team mate is alive. Then disarm. For tdm4 (`mode_class` 1 from 0x9e): one `0b89` about 7000 ms after the 0x67 state 2. |
| B3 | Respawn | on 0x67 state 1: disarm and clear the death time, take the new teleport (0xa0 flag 0x0020, new `move_seq`) before the next 0x64, resume 0x64 at the update rate (the existing spawn path). |
| B4 | Stay silent while dead | no 0x64 in states 2 / 3 (already so: the bot's last 0x64 was 47 ms before its death block). |
| B5 | Leave after the result screen | on 0x3f: hold a few seconds (DeCombat choice), then send `C2S_BYE` (engine id 6) and return to the room (0x306 [1], 0x388, 0x38a, as the real client did at 144.156). The reference stack did not close the bot's link after 0x3f (38 s of stream, no bye), so the bot must. That the 2009 client itself disconnects after 0x3f is PLAUSIBLE only (S2C §2.1: the 0x3f handler sets the leaving latches; not yet captured). |
| B6 | `yaw8c` | truncate toward zero instead of rounding (`shell::yaw8c` uses `.round()`); 1,103 of 1,103 here, also tdm1. |
| B7 (optional) | firing bot | for kill tests in the other direction: 0x82 exactly as section 2.1 (887-bit form with the extra block: `extra_obj` = `hit_obj`, `extra_a` = the hit point, `extra_b` = a unit normal), preceded by 0x7f / 0x76 [3][2] on a weapon change. |

## 9. Proposed DC_DEFAULTS changes

| id | rule (DeCombat policy) | default | 2009 client evidence it must satisfy | config key | hook |
|---|---|---|---|---|---|
| DC-KILL-1 (refines DC-C2S-3 and the respawn part of DC-C2S-10) | A dead player is respawned when his respawn request (0x89; 0xc5 in classes 7 / 8; 0xd2 in class 10) arrives at least `respawn_ms - 500 ms` after his death; without a request the server respawns him `respawn_fallback_ms` after the death; a request from a living player is ignored silently. | `respawn_ms` 7000, `respawn_fallback_ms` 10000 | the client arms its own 7000 ms wait after 0x67 state 2 (DISASM cshell+0x60ac0, called from cshell+0x62494) and asks once after it (cshell+0x6203e..0x62132); OBSERVED tdm4: the reference stack never respawns without a request | `combat.respawn_ms`, `combat.respawn_fallback_ms` | `combat.on_respawn_request` |
| GMD-3 (correction) | Respawn arming: the server sends no arming message in classes 1, 4, 6, 7, 8 (the client arms itself); in class 5 it sends 0xd7 sub 2 as before (the client arms only when the stored value is above 0); class 9 as the client state allows; respawn by DC-KILL-1. | as stated | evidence column replaced by: "the client arms its own wait after state 2 (C2S §3.3 corrected, DISASM cshell+0x60ac0); 0xd7 sub 2 / 0xdb sub 6 re-arm it" | `modes.respawn_arm` | `mode.respawn_due` |
| DC-KILL-2 (refines DC-C2S-11 for the 0x74 form) | `GS_DAMAGE_TAKEN`: `from_dir` = unit shot direction x health loss / 100; `zone` and the byte before it = the ROM damage type of the ammunition (`[Ammo<n>] InstDamageType`, `AreaDamageType` for area damage, a DeCombat value for falls); `damage` = rounded health loss; `dmg_kind`, `mode`, `part` stay UNPROVEN constants. | as stated | the indicator adds the magnitude of `from_dir` capped at 1.0 (S2C §4.3); knockback normalises it (MOVEMENT §6.9); OBSERVED tdm4: the reference stack's `zone` equals those ROM keys | `combat.damage_dir_scale` | `combat.on_hit` |
| DC-KILL-3 | After every kill the server sends 0xb5 with the two team kill totals (0xb5 [0][0] at the round start); 0x4a is never sent (wording changed 2026-10-25 after dserver-slice4: the reference stack never sent 0x4a, DSV-33). | on | 0xb5 sets the scoreboard values (S2C §3.4); OBSERVED tdm4: the reference stack carries team kills in 0xb5 only | `combat.round_wins_as_team_kills` | none |
| DC-KILL-4 | Sound relays: 0x80 and the jump notice 0x76 [1][1] are relayed to every client of the match, the sender included, as engine-17 type 0x19 records (k = sound number, or 0x0f for a jump), unreliable. | on | inline type 0x19 reader (S2C §8.2); OBSERVED tdm4 relay forms | `relay.sounds` | `relay.on_sound` |
| DC-KILL-5 | At the match end every player gets 0x6b kind 2 with absolute kills / deaths of all players before 0x65. | on | the result screen reads the roster records (S2C §3.4); OBSERVED tdm4 refresh before 0x65 | none | none |
| DSV-11 (note only) | unchanged; the reference stack's per-hit amounts (below `InstDamage`) are reference stack rule values and are not adopted. | - | - | - | - |

## 10. Not decoded, UNDECIDED, follow-ups

- **Not decodable here:** the kill / death totals the lobby stored for player1 (1 kill) and bot1 (1 death): no lobby
  message carrying them was sent after the match (7). The reference stack's answer to a respawn request: no 2009 client has
  died in any capture yet.
- **Layouts beyond the heads (REFERENCE, UNPROVEN):** engine-17 type 0x06 records (two per hitting shot); 0xa6 type 0x18
  notes (subs 0x11, 0x13, 0x14, 0x15, 0x1a, 0x1b, 0x1d, 0x1f, 0x20, 0x25, 0x40, 0x80, 0xc0); the object stream (engine 8
  / 10) was not decoded for a weapon-change or animation relay.
- **Meanings still open:** `extra_kind` (24, 11), `stat` (+2 per shot), `w20c_raw` (1.0), 0x74 `dmg_kind` / `mode`,
  0x6d `kill_kind` / `streak_flag` / `feed_value`, 0x3f `result_value` (2 = winner, one sample), the 2-point damage at
  101.516 (no shooter, type 5).
- **Not exercised:** reload (no 0x83), melee, multi-pellet weapons, crouch, a head shot, the 2009 client's reaction to
  0x3f.
- **Capture to take next (V-KILL-1):** the real client is killed by a firing bot (B7) or by its own grenade at low
  health, waits without input, and plays on after the respawn: measures the 0x89 time after 0x67 state 2, the reference stack's
  answer and the second death's re-arming.
