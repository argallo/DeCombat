# capval-rust3: the first real-client round on the Rust stack, item by item against the reference stack

- **Capture under test:** `// OBSERVED: captures/20261025_081704_rust3/capture.jsonl` (captap format 1;
  manifest: ROM fingerprint match, primary = **rust** (`dc-lobby-server` + `dc-dserver`, rust-primary build), no shadow,
  `udp_tap` late but every UDP link was captured from its first datagram). Our own logs of the same run (allowed in full):
  `logs/lobby.log`, `lobby.pktlog`, `dserver_dserver_c1_r0_g1.log`, `Bot1.events.jsonl`, `Bot1.report.json`.
- **Reference stack references (reference stack, real 2009 client; capture.jsonl only, their logs were not read):**
  `captures/20261025_065016_tdm6` (**the same world** as rust3) and `captures/20261025_002236_tdm4`. **Correction of
  the task premise:** tdm4 is **not** the same map. Its announced world file is 20,849,375 bytes; rust3 and tdm6 announce
  the 15,673,717-byte world whose sha256 begins `690f2a02e4df1c97` (our dserver log). Below **W1** = the rust3 / tdm6 world,
  **W2** = the tdm4 world. tdm4 is used only for forms that do not depend on the map and as a second world for the
  world-settings rule (section 3.3).
- **Fold state:** folded by spec-fold-8 (2026-10-25) into `UDP_SESSION_2009_B.md` 3.5.1 / 3.5.5 / 4.2 / 8.5 / 8.6, `UDP_SESSION_2009_A.md` 4.5 / 4.8 / 5.7,
  `RENDER_2009.md` 4.4, S2C 2.1.1 / 3.3.2 / 4.4.1 / 8.2 (row 0x26), C2S 4.1.1 / 4.6 / 5.7, `UDP_TRANSPORT_2009.md` 9.1, `MOVEMENT_2009.md` 2 / 12.4 (each base spec's
  Appendix I); sections 3.3 and 7.4 were corrected in place (a missing variable reads 0). The rest is kept in full.
  Branch `capval-rust3` (at master 2517136) used only as the path base of scratch
  programs; no file under `rust/` was edited.
- **Grades:** **PROVEN-CAPTURE** = bytes the real 2009 client sent, or a client behaviour visible in them. **REFERENCE** =
  bytes the reference server sent (its form; its rule values are never adopted, `REFERENCE_TESTING.md` section 1). **OURS** =
  bytes our Rust servers sent (OBSERVED from our own run). **ROM** = a value read at run time from the user's read-only
  client folder (cited by role, key hash or index, the value itself is not reproduced unless it is a small code or a
  ratio). **DISASM / EVIDENCE** = clean decompiler tree or read-only capstone disassembly. **SIM** = our own collision data
  (`dc-world` polygons of the ROM world). **PLAUSIBLE** / **UNPROVEN** as usual.
- **Decoders (scratch, outside the tree):** (a) `scratchpad/cv6/target/debug/dump.exe` (a replay of every link through
  `dc_net_engine::Link` + `dc_botclient::capdecode::unbundle`, one line per delivered message with the `seq` of the
  completing datagram; built by capval-tdm6, reused unchanged); (b) a new scratch Rust package `scratchpad/r3dec`
  (path dependencies on the worktree's `dc-botclient`, `dc-wire`, `dc-world`, `dc-rez`, `dc-bute`, `dc-dserver`):
  `rec` (every `S2C_OBJ_GROUP` record with all fields of `UDP_SESSION_2009_B.md` section 3 / 4.2, every stream entry,
  `S2C_FILE_LIST`, `S2C_EVAR`, `S2C_SKY_TABLE`, `S2C_DIR_LIGHT`; 0 record-length errors on all six links), `wobj` (the
  317 objects of W1 and the 559 of W2 with class role, properties and linked world model), `near` (world-model polygons
  around a point), `clips` (clip name keys of the composed character model), `items` / `ammo` / `dmg` (ROM table reads);
  (c) Python readers `scratchpad/r3py/*.py` over the decoded messages. Cross-check: the bot link's decoded client shell
  counts equal `Bot1.report.json` `c2s_shell_counts` (0x64 1290, 0x82 21, 0x89 3, 0x7f 4, 0x76 4).
- **Names:** every ROM string (class names, property names, engine-variable names, file names, object names) is given
  as `h:sha256[:16]/length`; file names are hashed in their announced upper-case spelling. Labels in backquotes are ours.
- **Clocks:** `t` = tap clock (s); `seq` = capture event number of the completing datagram. Lobby offsets are payload
  offsets after the 8-byte frame header.

### Connections and ids

| capture | real 2009 client | bot | notes |
|---|---|---|---|
| rust3 | conn 5 (UDP, peer 56616), dserver client 0, roster pid 0, team 1, character object **7** (weapon child 8); lobby conn 2, account 1001 `player1` (new, seeded exp 225000 / GP 500000) | conn 6 (peer 56618), client 1, pid 1, team 2, character **5** (weapon child 6); `Bot1`, account 1002 | the bot loaded the ROM world (`world_resolved` ok) |
| tdm6 | conn 6, pid 1, team 1, character **140** | conn 5, pid 0, team 2, character **252** | as `CAPTURE_tdm6_VALIDATION.md` |

## 0. Summary: the user's report, cause, owner

| # | playtest report (rust3) | wire finding | responsible | grade |
|---|---|---|---|---|
| R1 | default weapon was another rifle | our lobby's 0x323 slot 0 = item **144**, the reference stack's = **145** for both accounts of every reference stack capture before any equip (tdm1, tdm2, tdm4, tdm6; the task's "144/164/0/7" is wrong: the reference stack sent 145/164/0/7); our 0xb2 grants 144 accordingly. ROM: 144 and 145 are two of the three unranked item-type-0 rows marked as default issue (the third is 160), 144 first in file order; CD-18 takes the first | `dc-lobby-server` CD-18 (`rom::pick_starter`), project decision staged | OURS + REFERENCE + ROM |
| R2 | bought 2 grenades; could not switch to the backpack weapon | the grenade pack went to equipment slot **3** (client 0x324 [uid 6][item 143][slot 3]) and was granted (0xb2 slot 3, weapon 143, 2 rounds); the shotgun went to equipment slot **10** (0x324 [uid 7][item 125][slot 10]) and was **not granted** (0xb2 slots 0..3 only). The reference stack grants equipment slot 10 as weapon slot **4** with `class_pos` 1 and a 0x7f for it. The client sent no 0x7f for slot 4 in rust3 | `dc-dserver` DSV-38 (slots 0..3) and the lobby's `loadout=` line (lobby-slots) | OURS + REFERENCE + PROVEN-CAPTURE |
| R3 | the sky rendered very oddly | we sent **no** `S2C_SKY_TABLE`, **no** sky world-model objects and **no** `S2C_EVAR` at all; the reference stack sent the sky table (one sky object), both sky world models, and 33 world-settings variables (sky portals, fog, sky fog, far clip) whose values equal the ROM world-settings object on two worlds (66 of 66) | `dc-dserver` (DSV-37 default `mode`, no EVAR / sky writer) | OURS + REFERENCE + ROM + DISASM |
| R4 | every kill showed the "first kill" animation | our 0x6d `streak_flag` = 1 on **6 of 7** kills (DSV-33 "first kill since the spawn"); the reference stack: 1 only on the match's first kill | `dc-dserver` DSV-33 (correction already staged by capval-tdm6, not yet implemented) | OURS + REFERENCE |
| R5 | damage seemed high | ours: 3 hits per kill (0x74 `damage` 43, 43, then 14 clamped), no armour (0x66 armour 0 at spawn); reference stack tdm6: 4 hits of 27 (weapon 145) with armour 100. The ROM base damage of 144's ammunition row is 1.37 x that of 145's (section 6) | measurement only; DSV-30 (project decision pending) | OURS + REFERENCE + ROM |
| R6 | the killed bot stayed standing and could still be hit | on a death we change nothing about the victim's character for the other players: it keeps flags 0x3803, its weapon attachment and the standing animation in every stream entry for the 7 s until the respawn; the reference stack hides the character (flags 0x0802, empty attachment list), creates a separate corpse object (type 0x26 blob, the body model, a death clip) and plays the death clip on the character's stream. The 2009 client kept hit-testing our standing model: 27 claims refused `victim_dead` | `dc-dserver` (DSV-28: "no death clips yet"; no death record) | OURS + REFERENCE + PROVEN-CAPTURE |
| R7 | stuck walking up a staircase | on one staircase of W1 the same client **walked** up in tdm6 (1.1 s, on the ground throughout) but was **blocked at the first riser and at three landings** in rust3 and needed 6 jumps (3.7 s). Every server input to the client mover that we compared is bit-identical (0xa0 body block, box, friction, flags, no correction teleport); only level geometry lies at that staircase (SIM). The cause is **UNPROVEN** (since answered by the missing step-height variable, which reads 0: 3.3 / 7.4 corrections of spec-fold-8; V-STAIR-2 open); the remaining differences are the missing engine variables and world objects | `dc-dserver` (candidates), experiment V-STAIR-1 | PROVEN-CAPTURE (both runs) + SIM |
| R8 | some world objects were missing | we created 4 world-file objects (2 zones, 2 flags); the reference stack created **70** world-file objects at world load on the real client's link (74 creates with the two characters and their weapon children): 21 named world models (12 breakable props, 7 plain world models incl. one visible solid model of 1,138 polygons over the whole map, 2 sky models), 6 sprites, 36 fall-death volumes, 1 use-target zone, 1 camera and 1 effect object, plus the same 2 zones and 2 flags. In tdm6 the real client **stood on** that large model's floors in 32 updates; in rust3 its box passed **through** that model's walls, floors and ceilings in 37 updates | `dc-dserver` DSV-37 default (`mode`) | OURS + REFERENCE + SIM |
| R9 | (tool) every other session does not open the client | out of scope here (tool-launch worker); rust3 opened the client, the client's first auth frame came 15.6 s after the capture start | `capture_session.py` | - |

Other findings (section 9): our 0x45 has no intro (no radar / map / mission data); spawns at the raw spawn-point height
(the client falls 36 units); no 0x66 kind 1 slot status after a spawn; the grenade (weapon 143) was never released
although the client sent 44 x 0x4d; our exit closes the link with a plain bye before the client's `C2S_BYE` (the client
accepted it); `S2C_DIR_LIGHT` equal to the reference stack's within float rounding; our visibility hide / show records reached a
remote character on the real client 17 times without a parse error (V-VIS-1 at parse level).

## 1. Default weapon (task item 1)

### 1.1 Lobby 0x323 and 0x321 for a fresh account

| capture (frame seq) | account | 0x323 payload +0x12 .. (slots 0, 1, 2, 3) | rest of 0x323 |
|---|---|---|---|
| tdm1 (seq 22, 148) | 1001, 1002 | **0x91 = 145**, 164, 0, 7 | - |
| tdm2 (29, 80), tdm4 (29, 80), tdm6 (29, 67) | 1001, 1002 | **145**, 164, 0, 7 | REFERENCE |
| rust3 (34, 97) | 1001 `player1` (created at 08:17:29), 1002 `bot1` | **0x90 = 144**, 164, 0, 7 | identical to tdm4 seq 29 except payload +0x12 (byte diff of the 166-byte payloads); empty function slots now -1 (the tdm6 L7 issue is fixed) |

0x321 (rust3 seq 36 vs tdm4 seq 31): record 0 item +4 (144 vs 145) and the FILETIME start fields of the five records
differ (time fields by design). So the reference stack showed item 145 as the primary of every account it served before any equip (whether those accounts
were fresh is not visible on the wire); ours gave 144 to the fresh rust3 accounts.

### 1.2 In-game grants

| capture | link, seq | 0xb2 (slot: weapon / ammo kind / rounds / `class_pos`) | 0x7f after the grants |
|---|---|---|---|
| tdm4 | conn 6, 815 | 0: 145/145/120/0; 1: 164/164/45/0; 2: 0/0/1000/0; 3: 7/9/1/0 | (WEAPON_FORMS section 2) |
| tdm6 | conn 6, 733 | the same + **4: 125/125/28/`class_pos` 1** | one per gun slot, `auto` 0: [105][0][-1][0], [251][0][-1][1], [91][0][-1][4], then [105][**1**][-1][0]; the whole set twice |
| rust3 | conn 5, 717, 2569, 7083, 8903, 11669 (every spawn) | 0: **144**/144/120/0; 1: 164/164/45/0; 2: 0/0/1000/0; 3: **143/143/2**/0; no slot 4 | one: [104][1][-1][0] |

The client answered with 0x76 [3][2] + 0x7f [0][144][144] (two pairs at seq 723, three at 7087), the PROVEN-CAPTURE
pattern of tdm6 (one pair per server 0x7f in the reference stack's set).

### 1.3 ROM (read at run time, `ITEMS.TXT` weapon rows)

Items 144, 145 and 160 are the `ItemType` 0 rows that carry the default-issue mark in their name row and have no rank
requirement (further marked rows of other primary item types: 146, 147, 161, 162, 163; secondary slot: 164, 165); 144
is the 135th weapon row in file order, 145 the 136th, 160 the 151st. `rom::pick_starter` (CD-18: "the row
marked as default issue, else ...") takes the first and so issues 144. Their shop-display rows differ (damage, fire
speed, accuracy, mobility, recoil, sight); the ammunition row of 144 has the higher `InstDamage` (section 6).

**Verdict R1:** the reference stack's choice (145) is observed on four captures; it is an reference stack rule value and is not adopted
here. Which default-marked row DeCombat issues is a project decision (staged: CD-18 note; the lobby-slots worker stages a
refinement of its own).

## 2. Backpack and the bought items (task item 2)

| step | rust3 (OURS / PROVEN-CAPTURE) | tdm6 (REFERENCE / PROVEN-CAPTURE) |
|---|---|---|
| purchase 1 | 0x328 product 1301 (seq 62, t 49.313) -> 0x329 ok uid 6 | 0x328 product 758 (seq 110) -> 0x329 ok uid 11 |
| equip 1 | client 0x324 **[uid 6][item 143][slot 3]** (seq 64) -> 0x325 ok; lobby log "equip: uid 6 item 143 slot 3" | client 0x324 [uid 11][item 125][slot 10] (seq 112) -> 0x325 ok |
| purchase 2 | 0x328 product 758 (seq 66) -> 0x329 ok uid 7 | - |
| equip 2 | client 0x324 **[uid 7][item 125][slot 10]** (seq 68) -> 0x325 ok | - |
| dserver | "player info ... loadout 5" (five items on the `user` line) | - |
| grants | slot 3 = weapon 143 (ammo kind 143 = the ROM ammunition row of weapon 143, checked), 2 rounds; **item 125 not granted** | slot 4 = weapon 125, `class_pos` 1, 28 rounds, plus 0x7f [91][0][-1][4] |
| client selections | 0x76 [3][2] + 0x7f for slots **1, 2, 0, 3** (seq 4913, 5025, 5100, 5178; t 172.25 - 174.27); **no 0x7f for slot 4**, no other selection message | 0x76 [3][2] + 0x7f [4][125][125] at 343.281 (seq 22081) |

The client chose the equipment slots itself (slot 3 for the throwable, slot 10 for the shotgun) in its 0x324. The lobby
stored both. The dserver granted only slots 0..3 (DSV-38: "slots 0..3; first claim of a slot wins"), so the client had
nothing in weapon slot 4 and its backpack key produced no message (PLAUSIBLE: the client ignores an empty slot; whether
the user pressed it is not visible). **Verdict R2:** equipment slot 10 must reach the dserver as weapon slot 4 with
`class_pos` 1 (reference stack form, accepted) and get its 0x7f command (the reference stack sent one per gun slot, the last with `auto`
1); owners: lobby-slots (the `loadout=` line must carry slot 10) and dserver-fixes (DSV-38 slot range).

Note (not in the report, section 9.4): the grenade 143 was selected at 174.266 and the client sent 0x80 [10][143]
(176.235) and 44 x 0x4d in three bursts (176.422 - 177.203, 179.391 - 180.703, 181.703 - 183.141) but **no release
0x82**; the user died at 183.141 with it selected. tdm6's two throws (weapon 7) each ended with the release 0x82.

## 3. The sky (task item 3)

### 3.1 What the reference stack sent at world load (tdm6, conn 6)

| seq / t | record | content |
|---|---|---|
| 211 / 167.25 (link) | 21 x `S2C_EVAR` | server-configuration variables (none sky-related) |
| 676 - 678 / 172.937 | **108 x `S2C_EVAR`**, in the three datagrams before `S2C_LOAD_WORLD` (seq 678) | includes the **33 world-settings variables** of 3.3 (sky portals, fog, sky fog, far clip, panning sky, wind, shadows) |
| 701 / 173.234 | `S2C_PRELOAD` x 8 | subs 0, 2 (x5, model files), 1, 5 |
| 701 / 173.234 | **`S2C_SKY_TABLE`** (888 bits) | block (5120, 0, -6656, 5632, 512, -6144, 5375.744, 255.744, -6400.256, 5376.256, 256.256, -6399.744), count 30, ids [**251**, then 29 x 0xFFFF] |
| 714 | `S2C_VIEW_OBJECT` [140] | |
| 722 / 743 | object groups | the sky world models: object **251** `h:ce598e8553e6ffde/12` and object **108** `h:bb4f543a63b30867/12`, both kind 2 (`OBJ_BRUSH`), `cf` 0x000b, flag words **0x0001 / 0x0020 / 0 / 0** (compact position, no full-precision bit), position = the centre of the linked world model's box, 448 bits |
| 722 / 173.375 | `S2C_DIR_LIGHT` | (0.32442, -0.80297, 0.50000, 314.2875, 314.2875, 271.15, 0.4) |

tdm4 (W2) has the same structure: EVARs at seq 746 - 750 before `S2C_LOAD_WORLD`, sky table at seq 788 (block centred
on (-8192, -3584, -7936), ids [1, 29 x 0xFFFF]), `S2C_DIR_LIGHT` at seq 807.

### 3.2 The sky table block is the sky object's two boxes (REFERENCE + ROM, two worlds)

ROM, W1: the sky-world-model objects (class `h:689cfa21cab3e0cb/17`) are file objects 107 and 316; object 316 (sent as
id 251) has position (5376, 256, -6400), `h:e62b876f06d008cf/7` (sky dims) (256, 256, 256) and `h:0ead0667b1ddb2dc/13`,
`h:244ff605c604fbca/13`, `h:2f349b60cd95d9c8/13` (inner percent x / y / z) 0.001. The 12 floats of the block are exactly

```
floats 0..2  = pos - sky_dims              floats 3..5  = pos + sky_dims
floats 6..8  = pos - sky_dims * inner      floats 9..11 = pos + sky_dims * inner     (per axis)
```

and the same holds for W2 (its only sky object at (-8192, -3584, -7936), dims 256, inner 0.001). This closes RENDER_2009
open item 14 for floats 0..5 (the outer sky box) and confirms M1 for 6..11 (the inner box the eye is mapped into); the
table lists that object's id. With two sky objects in W1 the reference stack listed the one with the higher file index (both have
`h:39c3be1198a2fddf/5` index 0): which one a server should list is UNPROVEN (one case).

### 3.3 The world-settings variables (REFERENCE form, ROM values; client readers DISASM)

Of the 116 distinct variable names the reference stack sent (138 messages, identical name sets in tdm4 and tdm6), **33 are the
properties of the world-settings object** (class `h:9819e9e19f000398/15`, one per world) under the client's names, and
**all 33 values equal the ROM property on both worlds (66 of 66)**, including every one that differs between W1 and W2
(far clip, fog near / far and colour, sky fog near / far, vertical fog bounds, wind, all-sky-portals). Mapping (client name = property name
unless stated):

| our label | variable | world-settings property | client reader |
|---|---|---|---|
| `far_z` | h:12462a1852d9fd08/4 | same | cshell+0x1a6900 via cshell+0x1a6840 (applied only when > 50) |
| `fog_enable` / `fog_near_z` / `fog_far_z` | h:b42e4232c66d28c4/9 / h:63cb48901b40a07e/8 / h:068c8f33716eac9c/7 | same | cshell+0x1a6900 |
| `fog_r` / `fog_g` / `fog_b` | h:6a3aa136d4a2c341/4 / h:cb7cae2a3c3069b3/4 / h:ccd55fe4a2191454/4 | h:5c589613c2ea1c49/8 components x / y / z | cshell+0x1a6900 |
| `sky_fog_enable` / `sky_fog_near` / `sky_fog_far` | h:cd9c0c6e0d40407e/12 / h:10eb8275f09917ef/11 / h:3885263f3d410645/10 | same | cshell+0x1a6900 |
| `vfog_on`, `vfog_min_y`, `vfog_max_y`, `vfog_density`, `vfog_max`, `vfog_max_y_val`, `vfog_min_y_val` | h:bdaa1c3a0acb61d2/7, h:0f3985ea5b0ad7d7/11, h:beba61a2f9598f93/11, h:32a829d79925912a/14, h:f3ff5de293b2a0ab/10, h:6f109a2668d368ba/14, h:6c282e532eb27396/14 | h:e3d72aecf9aec8aa/4, h:14939ed720e89b77/8, h:a814417a42df5d75/8, h:93dc00c3fca0f025/11, h:6b352f224b5283be/7, h:8a3a6921ff5b9858/11, h:5ec6c1a5d9dde9bf/11 | cshell+0x1a6900 (the client copies them to differently named console values) |
| `shadow_alpha`, `shadow_min_color`, `shadow_max_dist` | h:8c715da35076fc3a/22, h:ab4e0277a61f7ff4/34, h:96b2fe9873f40d91/28 | h:7323b5b4af48b597/11, h:27d2303e2d9fe403/23, h:d8080b0c9b3fa1c4/17 | cshell+0x1a6900 |
| `all_sky_portals` | h:5f483004cf36e238/13 | same | cshell+0x1a9b00 |
| `wind_x` / `wind_y` / `wind_z` | h:ce343f3c97fa692f/5 / h:2da14a1d9093769e/5 / h:b2c9e2e19a05a5c7/5 | h:d151346fe7eea3c6/4 components | cshell+0x1a9b00 |
| `pan_sky`, `pan_sky_offset_x`, `pan_sky_offset_z`, `pan_sky_scale_x`, `pan_sky_scale_z`, `pan_sky_texture` | h:6a903ff899d357cb/6, h:84acf69a55bfeee7/13, h:233fd63107dadf13/13, h:63551d8b8bbd9b38/12, h:86f2263cada6b175/12, h:c2e7db2c84e5fe9c/13 | same | no literal in the clean C (UNPROVEN reader) |
| `ai_max_number`, `sound_filter`, `music_mood_min` | h:98d690c4f4059827/11, h:56e6fb14b55882de/17, h:0365666c46f4de53/12 | same | sound filter: cshell+0x1f6fb0, cshell+0x1fadd0; others none found |

EVIDENCE (clean C, cshell+0x1a6900 and cshell+0x1a6840): at world entry the client copies each named server variable
into a console value with `"<name> %f"`; the read is preceded by a zero default, so a variable the server never sent is
applied as **0** (PLAUSIBLE: the read leaves the default when the variable is missing; **EVIDENCE since aa-triage-world-physics**, correction folded by
spec-fold-8: the engine's server-variable float read, client slot +0x134 = engine+0xd760, itself stores 0 into the output before the lookup and returns not-found). cshell+0x1a6900 skips the fog
block and runs "fog off" when a world flag (`+0x52b8` bit 0x10) is clear. With our stack (no EVAR at all) the client
therefore ran `all_sky_portals` 0 (the world wants 1), fog off with near / far 0, sky fog 0 and kept its default far
clip; the three world-settings properties the reference stack did **not** send (`h:734640ab2889ac43/12`,
`h:efefe954f78c519c/19`, `h:4bfc9635c6295c1c/15`) are read by the same function and get the same zero / default
treatment in both stacks.

The other 83 variables are server configuration (hit-box radii, AI and difficulty factors, respawn waits, body lifetime,
damage and heal scales, run / jump / swim / ladder speeds, step height, gamma, screen size, bind address, ...). Their
values are reference stack rule values and are not reproduced. Client readers were found only for: the step height
`h:eda10d3a6476dd56/15` (cshell+0x1fe6c0; DISASM: the caller presets 30.0 and pushes the value to the physics only when it is not 30.0,
and the shell sets 30.0 at start-up, cshell+0x1a3910; **corrected (spec-fold-8, from aa-triage-world-physics item 10):** the read through engine+0xd760
overwrites the preset with 0 when the variable is missing, so an absent step height reads 0, is pushed, and the client's step height becomes 0: absent and 30 do
**not** behave the same (`MOVEMENT_2009.md` 2; the first text here said they did), `h:86456a7c0366ace6/8`
(cshell+0x2b2330, an inside-box test over an object list, default 0), the world-north value `h:18e91a6281020c4a/10`
(cshell+0x1a6900 and HUD functions), gamma, bit depth, cursor, vsync and bandwidth (display / options code), a timing
flag. The run and jump speed names have no literal in the clean CShell / Engine / ClientFx C.

### 3.4 What we sent (rust3, conn 5)

`S2C_PRELOAD` [0], [1] (seq 699), `S2C_DIR_LIGHT` (seq 699: the same seven floats as the reference stack's to 1e-6), the
first object group (seq 708) with 8 creates: **no `S2C_SKY_TABLE`, no sky world model, no `S2C_EVAR`** (also none on
the bot link; `Bot1.report.json` `sky_count` null, `evars` []).

**Verdict R3:** the client got neither a sky object to draw into the sky portals (RENDER_2009 section 5.3: the sky pass
draws only sky objects named by the table) nor the world's sky / fog settings (`all_sky_portals` forced to 0, fog off).
The "very odd" sky is consistent with portals that nothing is drawn into (PLAUSIBLE; a screenshot would confirm).
Proposed rules (staged, `capval-rust3_DC_DEFAULTS.md` P3-P5): the 33 world-settings EVARs from the ROM object, the sky
table from the sky object's boxes, and the sky world models as kind-2 records with flags 0x0001 / 0x0020.

## 4. Kill messages (task item 4)

| t | seq (user link) | victim | killer | weapon | `hit_zone` | **`streak_flag`** | killer's previous event | reference stack rule (match's first kill) |
|---|---|---|---|---|---|---|---|---|
| 146.875 | 1837 | 0 (user) | 1 (bot) | 144 | 3 | **1** | match start | 1 |
| 162.188 | 3704 | 1 | 0 | 144 | 1 (node 53) | **1** | user respawned 153.906 | 0 |
| 183.141 | 6347 | 0 | 1 | 144 | 3 | **1** | bot respawned 169.25 | 0 |
| 198.594 | 8175 | 0 | 1 | 144 | 3 | 0 | bot's 2nd kill of its life | 0 |
| 207.578 | 9187 | 1 | 0 | 144 | 4 | **1** | user respawned 190.156 | 0 |
| 222.172 | 10930 | 0 | 1 | 144 | 3 | **1** | bot respawned 214.625 | 0 |
| 231.391 | 11963 | 1 | 0 | 144 | 4 | **1** | user respawned 205.61 | 0 |

All 151 bits, `kill_kind` 1, `f_relation` 0, `feed_value` 2, no revenge, both links identical (OURS). tdm6 (REFERENCE):
`streak_flag` 1 only at 190.531, the match's first kill, 0 on the other six (CAPTURE_tdm6 section 7). **Verdict R4:**
our DSV-33 rule ("first kill since the spawn") produced 6 of 7 first-kill banners; the staged capval-tdm6 correction
("1 on the first kill of the match") has not reached the build used for rust3.

## 5. Death presentation to the other players (task item 5)

### 5.1 The reference stack (tdm6, user link conn 6; the user killed the bot at 246.609, the bot respawned at 253.703)

| seq / t | record | content |
|---|---|---|
| 10117 / 246.531 | stream, bot 252 | standing (main clip 0, trackers 96 / 6) |
| 10126 / 246.547 | stream | tracker 1 on clip 110 (`h:c61810e9e24f396e/7`, 100 ms): the hit reaction [corrected by spec-fold-9: the rifle fire one-shot, `UDP_SESSION_2009_B_addendum_anims.md` A1], 62 ms before the kill block |
| 10128 / 246.609 | shell | 0xa6 notes, 0xd6 (HiSec case), **0x6d** (`streak_flag` 0), 0xb5; no 0x67 and no 0x6b to the killer |
| 10134 / 246.609 | **change of the victim's character** | `cf` **0x0188** (`CF_FLAGS` + `CF_ATTACH`): flags **0x0802** / 0x0318 / 0x00401020 / 0 (the pre-spawn "hidden" form of `UDP_SESSION_2009_B.md` 3.5), **empty attachment list** (the weapon child is dropped) |
| 10134 / 246.609 | **corpse create** | object 265, kind 1, `cf` 0x25bb, blob **type 0x26** (219 bits), model block = the victim's body model, child model, its five textures and three styles, one tracker on clip **86** (`h:5a6e14da8d04b144/4`, 1,766 ms; narrow form `state1` 0), flags 0x1803 / 0 / 0x00600820 / 0, position = the victim's last position |
| 10134 / 246.609 | other creates | the victim's dropped items (type 0x17 pickups, mostly parked at (-9999.9, -9999.8, -9999.9)), the HiSec case 272 |
| 10134 .. 10964 | stream of 252 | 208 entries during the 7.1 s: the same position, main tracker on clip 86 |
| 10964 / 253.703 | respawn | change `cf` 0x2ba8 (model files again, three trackers, flags **0x3803**, attachment of a **new** weapon child 269, created in the same tick) |
| 265.422 | removal | the corpse 265 was removed 18.8 s after its create (the same 18.8 s for corpses 260 and 274); corpse clips were 86, 87 or 88 |

The victim itself got the same character change on its own link (tdm6: 0x0588 / 0x0188 with flags 0x0802 at each of
the user's six deaths) plus its 0x67 [3] / [2].

### 5.2 Ours (rust3, user link conn 5; the user killed the bot at 162.188, the bot respawned at 169.25)

| seq / t | record | content |
|---|---|---|
| 3704 / 162.188 | shell | 0x6d only (0x74 / 0x66 / 0x67 [3] [2] went to the victim's link, seq 3703, as the reference stack) |
| 3692 .. 4524 | stream of 5 | **211 entries**, position (481.1, 54.2, 632.1), `uf` 13, yaw 25, **standing clips 0 / 96 / 6** with running times; no flag change, no attachment change, no corpse, no removal |
| 4544 / 169.25 | respawn | spawn change `cf` 0x468a (flags 0x3803, the park point for this viewer), then the show record 0x0282 (seq 4554) |

The same holds for the other two bot deaths (207.578 - 214.625, 231.391 - 238.438).

### 5.3 The standing corpse and the refused claims (PROVEN-CAPTURE)

The real client kept drawing and hit-testing the bot as a live standing character: after the kill at 162.188 it sent
**20** more hit claims on object 5 (162.297 - 167.922, `hit_dist` growing from 27.4 to 166.8 while the user backed
away), after 207.578 six (209.328 - 211.313) and after 231.391 one (231.547). These are exactly the 27 `victim_dead`
refusals of our log. In tdm6 (reference stack) the real client sent **no** hit claim on the dead bot (its next claim on 252 came
after the respawn). **Verdict R6:** to the other players a dead character must stop looking alive: hide it (flags
0x0802, attachments cleared) and show a corpse (type 0x26 kind-1 object with the body files and a death clip) or play a
death clip in its stream entries; restore with the respawn record. The clip choice and the corpse lifetime are rules of
their own (the reference stack's 18.8 s and three clips are recorded, not adopted). Owner: dserver-fixes (DSV-28 extension,
staged P6).

## 6. Damage (task item 6, measurement only)

No capture has the same weapon on both stacks: every rust3 shot used weapon 144, every reference stack rifle shot (tdm4, tdm6)
weapon 145. Both columns as observed:

| hit (node named in `extra_kind`, ROM `[Skeleton0]` location) | rust3 weapon 144: 0x74 `damage` / health after (OURS) | tdm6 weapon 145: 0x74 `damage` / health, armour change (REFERENCE) |
|---|---|---|
| node 24 (location 3), bot on user | **43** (10 hits; `|from_dir|` 0.43), health 100 -> 56.8 -> 13.6 -> lethal **14** (clamped, 4 hits) | **27**, health -26.775, armour -5.25 (24 hits) |
| node 11 (3), user on bot | 43, 43 | (tdm4: 27) |
| node 53 (1, head) | lethal 14 (clamped; 288 before clamping by DSV-30) | 210 (not clamped), armour unchanged |
| node 6 (2, torso) | - | 30, health -29.75, armour -5.25 |
| kills 2 and 3 (nodes 39, 55, 43 and 58, 13, 58; the kill feed named location 4 for both) | 43 / 43 / lethal 14 each | not observed |
| `dmg_kind` | always 3 | the node's location (1 / 2 / 3) |
| armour at spawn (0x66 kind 3) | **0** | **100** |
| hits per kill (rifle on rifle) | **3** (0.31 s at the bot's 150 ms interval) | **4** (0.44 - 0.49 s) |

ROM (read at run time): the ammunition row of weapon 144 has 1.371 x the `InstDamage` of weapon 145's; nodes 24 and 11 have the
same ROM node factor. So our 43 is DSV-30's InstDamage x node factor with no armour; the
reference stack's node-24 health loss (26.775) is 0.85 x 145's base x the same factor, and its armour loss 0.15 x 145's base
(measured ratios, not a rule). Rifle-for-rifle the user met 1.37 x base damage (the 144 issue of R1) and no armour
(DSV-30 gives armour only from armour items). No value is recommended here (project decision pending).

## 7. World objects (task item 7)

### 7.1 Counts

W1 holds 317 objects (ROM: 100 player spawns, 57 directional lights, 38 fall-death volumes, 25 trigger volumes, 24
effect objects, 12 breakables, 11 item spawns, 10 plain world models, 10 lights, 6 sprites, 6 explosions, 5 mode
controllers, 2 sky world models, 2 flag zones, 2 flags, 1 each of camera, camera point, world settings, use trigger,
startup command, sun light, sound effect) and 63 world models (the level model has 28,709 polygons).

Creates at world load on the **real client's link** (tdm6 seq 722 + 743: 74 records; rust3 seq 708: 8):

| `obj_kind` | world-file role (class hash) | blob | `cf` | flag words | tdm6 | rust3 |
|---|---|---|---|---|---|---|
| 0 | flag zone (`h:0d6a598a1816da49/15`) | type 0x37, 409 bits | 0x0003 | - | 2 | 2 (`z_class` byte differs, DSV-37 `-zoneclass own`) |
| 0 | use-target zone (`h:86ea234676d3a48e/15`) | type 0x37, 1,081 bits | 0x0483 | - | 1 | - |
| 0 | camera (`h:03494b0d1f803522/6`) | type 0x0d, 96 bits | 0x048b | 0x0100 / 0 / 0 / 0 | 1 | - |
| 0 | effect object (`h:dc800a8131725ee2/9`) | type 0x64 + 15 bytes of text | 0x000b | 0 / 0 / 0x00800000 / 0 | 1 | - |
| 1 | capture flag (`h:e576ae011fa1c731/8`) | type 0x17 kind 2, 116 bits | 0x01a3 | - | 2 | 2 (same model / texture / style files) |
| 1 | characters + weapon children | type 0x18 / none | 0x058b, 0xa1ab / 0x01ab, 0x01af | | 4 | 4 (weapon model = the third-person model of weapon 144 vs 145) |
| 2 | breakable props (`h:ebbd45154c24defa/9`) | - | 0x000b, full precision | 0x3101 / 0 / **0x1e000121** (6) or **0x14000121** (6) / 0 | 12 | **0** |
| 2 | plain world models (`h:33b5bf07771368ec/10`) | - | 0x000b, full precision | 0x3101 / **0x0040** (translucent) / 0x00000001, 0x0a000001 or 0x28000001 (6); 0x3101 / 0 / 0x00000001 (1: the large model) | 7 | **0** |
| 2 | sky world models (`h:689cfa21cab3e0cb/17`) | - | 0x000b, compact | 0x0001 / 0x0020 / 0 / 0 | 2 | **0** |
| 3 | sprites (`h:ad52a1d2eb46a655/11`) | - (file id: one sprite file, `h:2193bb6974af49d3/30`) | 0x049b | 0x0081 / 0x0002 / 0x00000001 / 0, scale (0.5, 0.5), positions = the 6 sprite objects within 0.8 | 6 | **0** |
| 9 | fall-death volumes (`h:df93348241153dc0/11`) | type 0x0b, 578 bits (identical in all), code 7 | 0x000b | 0 / 0 / 0x00400005 / 0 | 36 | **0** |
| | **total** | | | | **74** | **8** |

Kind-2 positions are the centre of the linked world model's box (e.g. the large model's box centre, not the object's
own position property). Not created by the reference stack at all: player / item spawns, triggers, lights, explosions, sounds,
mode controllers, the startup command, world settings (their content travels as EVARs, 3.3), camera point.

### 7.2 The five objects the real client never saw (REFERENCE, mode-dependent)

The bot's link (tdm6 conn 5, joined 6 s earlier) also got 2 plain world models, 2 fall-death volumes and 1 more plain
world model (`h:fd72e339fd3e47b7/15`, `h:69943d3e5bdfd55f/15`, `h:35d434eadccdc4fd/10`, `h:2df70e5323febaea/14`,
`h:1188f6deacb83a3b/10`), with flag changes at 167.406 and **removals** at 167.422 / 167.484, before the real client
joined. ROM (command texts read at run time, not reproduced): three of them are named by the world's startup-command
object (class `h:892748f74ce86ab8/14`: switch off / make non-solid); the other two (the second object of that
collision pair and the second large world model, 569 polygons) are named by command texts of trigger objects that the
mode controllers fire at their start (visible / solid messages that swap the two large models). So the reference stack's TDM
object set = the world's objects of the classes of 7.1 minus what the world's own start-up commands switch off; how a
"switch off / invisible" message becomes a removal on the wire, and the exact command-to-object mapping, is UNPROVEN
(the large model the real client got is the one these commands make visible and solid). (The bot link's flag words carry 0x0400 in every object, a per-link bit
absent on the real client's link: PLAUSIBLE "client is not the host / is a bot", UNPROVEN.)

### 7.3 The missing large world model (SIM + PROVEN-CAPTURE)

The plain world model `h:bd0b670539272464/10` (object 219 in tdm6; flags 0x3101 / 0 / 0x00000001, i.e. visible and
solid; 1,138 polygons: 520 floors, 137 slopes, 393 walls, 88 ceilings; box over the whole map, 26 below to 858 above
the floor) is part of the reference stack's set and missing from ours. Tested against the users' 0x64 positions (box 15 x 50 x 15):

| capture | updates whose box overlaps that model's polygons | of those, standing on one of its floors |
|---|---|---|
| tdm6 | 37 (e.g. 343.562 - 343.703 at (127 - 147, 228.1, 228 - 246)) | **32** |
| rust3 | 37 (198.063 - 216.344, e.g. 214.797 - 216.203 standing at (62.9, 48.0, 324.8) on the level floor **inside** 3 of its floors, 3 ceilings and 6 walls) | **0** |

So in rust3 the user walked through solid geometry the reference stack's client collides with, and could not see it: this is
the "world objects missing" of the report, together with the 12 breakable props (drums and boxes) and the 6
translucent world models. The 36 fall-death volumes are also missing (no fall death in our world).

### 7.4 The staircase (R7; PROVEN-CAPTURE in both runs, cause UNPROVEN)

The staircase at x 228 -> 37, z 1066 -> 902 rises in 16-unit steps from y 50.1 to 250.1 (centre heights). SIM: within
25 units of the four sample points (169, 82, 1045), (109, 130, 1040), (36.5, 162, 1011), (209, 50, 1050) only polygons
of the level model lie (no world model, no reference stack-created object box).

| run | t | movement on the staircase |
|---|---|---|
| tdm6 | 178.953 - 180.093 | **walked up**: 18 updates, 15 - 21 units per update, `on_ground` 1 throughout, heights 50.1, 66.1, 82.1, 98.1 ... 250.1; control 0xa01 / 0xb01, sprint (0x400a01) from 179.484 on |
| rust3 | 190.953 - 195.188 | sprinting at 305 units/s, **stopped at x = 209.0** (y 50.1) for 6 updates with control FWD + sprint and horizontal motion <= 2.1 per update; jumped (0x76 [1][1] at 191.485), **stopped again at x = 169.0** (y 82.1), 109.0 (130.1), 36.5 (162.0), 36.5 (194.1), 27.8 (226.0): **6 jumps** to reach 250 |

Equal on both links (byte compare): the five 0xa0 body blocks (flags 0x000d / 0x0009), the teleport boxes (15, 50, 15),
the friction record (0xa0 0x0008, 5.0), the own character's flag words (0x2802 -> 0x3803 / 0x0318 / 0x00401020 / 0);
no correction teleport was sent in rust3 (`move_seq` 4 throughout the climb). The step height was read here as not the cause
(3.3, first text: "absent and 30 behave the same"); **corrected (spec-fold-8):** an absent step-height variable reads 0, so its absence is the cause
(aa-triage-world-physics item 10: the same block on the SNOWVALLEY staircase in rust4, none in tdm4; `MOVEMENT_2009.md` 2). Remaining differences: the missing EVAR set (3.3) and the missing world objects
(none at this staircase). Other blocked spots of rust3 (e.g. 179.0 - 181.7 at (121.0, 74.7, 84.1), x pinned at 121.0)
also lie in level geometry only. **Verdict R7: UNPROVEN.** Experiment V-STAIR-1: the Rust stack with the world-settings
EVARs and the full world-object set, the user walks (not sprints, then sprints) up this staircase; if it still blocks,
send the full reference stack-form EVAR name set with DeCombat values one group at a time.

## 8. Hit acceptance (task item 8)

| shooter -> victim | claims (0x82 with a hit object) | accepted (0x74 sent) | refused | evidence |
|---|---|---|---|---|
| user -> bot (conn 5 c2s) | 37 | 9 (3 kills x 3) | 27 `victim_dead` (5.3), 1 `shooter_dead` (seq 10927 at 222.172, the claim behind the bot's lethal claim seq 10926 of 222.156, DSV-43) | PROVEN-CAPTURE claims; OURS verdicts |
| bot -> user (conn 6 c2s) | 14 | 14 (all node 24) | 0 | `Bot1.report.json` `hit_claims` 14 |
| total | **51** | **23** | **28** | = the dserver line "51 claims, 23 accepted (0 rewound); refused shooter_dead 1, victim_dead 27" |

The user's 39 other 0x82 were misses (no hit object). Every accepted user claim came while the bot was shown by the
visibility filter (9.6). Nodes named by the user: 11, 13, 39, 40, 43, 44, 53, 54, 55, 58, 60, 61, 64, 3, 5, 6.

## 9. Other differences (task item 9)

### 9.1 Join and spawn sequence (user link)

| step | rust3 (OURS) | tdm6 (REFERENCE) |
|---|---|---|
| at the link | 0x90 [1], 0x88 [2], version, client id, one file list (seq 165) | the same, then **21 EVARs** and single-entry file lists (seq 211 - 219), 0x90 [0], 0x88 [3] at 167.406 |
| world | file acks, `S2C_LOAD_WORLD` (seq 669) | **108 EVARs**, `S2C_LOAD_WORLD` (seq 678) |
| after the world | preload [0] [1], `S2C_DIR_LIGHT` **before** the view object (seq 699) | preload x 8, **sky table** (701), shell 0x72 / 0x9e / 0x9d / 0x7d, view object (714), first group + `S2C_DIR_LIGHT` **after** the view object (722) |
| 0x7d | caption empty, `own_team` 1, `round_no` 0 | caption = the room host's nick (the user), unused byte 1, `own_team` 0, `round_no` 1 |
| spawn set | 0x6b x2, 0x6a, 0x67 [1], 0x66 0x0f / 0x10 / 0x13, 4 x 0xb2, one 0x7f, 0xa0 0x000d / 0x0020, 0x66 2 / 3, 0x6b, 0x43, 0x44, 0xb5, **0x45 without intro**, 0xc3 | the same core, plus 0x66 kind 0x17, the grants twice with a 0x7f per gun, 0x66 kind 1 for slots 2 / 3 (`extra` 1), kinds 0x18 and 6, 0xa6 notes, **0x45 with the intro** |
| spawn height | at the spawn point's raw height (y 86.0); the client fell 36 units to 50.1 in 0.27 s (seq 742 - 772) | at the floor (y 50.1) |
| 0x44 | `[0][1][1][0]` | the same |

0x45 (`GS_MATCH_BEGIN`): ours 114 bits, `show_intro` 0, `first_round` 1; the reference stack's 2,450 bits with `show_intro` 1: the
mission caption and text, the large-map image with its two corners, the radar image pair with corners, zoom 0,
rotation 0.34, 1024 / 1024, and the two team bases, i.e. the room line's `cmt` / `obj` / `large` / `radar` strings, which our
dserver already logs ("room settings"). Without it the HUD has no radar map and no mission intro (PLAUSIBLE; the user did
not report it).

### 9.2 Shell message totals on the user link (whole session; rust3 96 s in the world, tdm6 183 s)

| id | rust3 | tdm6 | note |
|---|---|---|---|
| S 0xa6 (object notes) | 1 | 4,010 | health / armour notes and others on the character objects |
| S 0x66 kinds 1 / 6 / 0x17 / 0x18 | 0 / 0 / 0 / 0 | 14 / 6 / 1 / 6 | kind 1 (slot status) has a client handler; 6 / 0x17 / 0x18 have none (S2C 4.3) |
| S 0xa2 (ping table) | 19 | 62 | |
| S 0xd6 (HiSec) | 0 | 18 | no case dropped in 7 kills (HS-1) |
| S 0xc3 (`GS_PERF_STATS`) | **70** | 0 | ours; the real client answered with **69 x C 0xc3** (`UP_PERF_REPORT`), none in tdm6: accepted DeCombat extension |
| S2C id 0x11 (effect relay) | 125 | 307 | |

### 9.3 `S2C_DIR_LIGHT`

Ours (seq 699, before the view object) and the reference stack's (seq 722, after it) carry the same seven floats (difference
< 2e-7 in the first component). DSV-40's "before the view object" is our order; the reference stack sent it after the view
object on both worlds (tdm4 seq 807 after 799); the client accepted both (staged note).

### 9.4 The grenade that was never thrown

Section 2 note. Differences from tdm6 around a throw: weapon row 143 (`ShotsPerClip` 1, its own ammunition row) vs 7;
0xb2 rounds 2 vs 1; no 0x66 kind 1 for slot 3 after the spawn (the reference stack sends `[7][9][1.0][3][extra 1]`). The client
held (0x4d) three times for 0.8 / 1.3 / 1.4 s without a release. Cause UNPROVEN (the user may simply not have
released before dying; V-NADE-1: throw 143 and 7 on the Rust stack).

### 9.5 Visibility records (V-VIS-1, parse level)

On the user link our server sent the bot's character **17** hide / show records `cf` 0x0282 (park point
(-32767.9, -65535.8, -32767.9) or the true position; seq 710 - 10646) and two spawn records 0x468a with the park point;
the bot's stream entries paused during each hidden window (154.86 - 157.24, 169.77 - 178.39, 191.13 - 194.67,
215.16 - 219.99). The real client parsed all of them (no session error, its 0x64 stream continued) and named the bot in
claims only while it was shown. ACCEPTED at parse level on a **remote** character (closes that part of V-VIS-1).
Whether the client placed the bot without a visible slide after a show record is not on the wire (no playtest report of
sliding).

### 9.6 Round end for the remaining player

Ours: 0x6c [0][0] at the leave (234.531), 0x46 10.0 s later (DSV-31 team-empty rule), 0x47 / 0x6b / 0x65 / 0x48 +3.0 s,
0x3f +3.0 s, the bot's `C2S_BYE` +3.0 s. Reference stack (tdm6): 0x6c [1][0] + 0x46 0.343 s after the leave, +2.735 s, +3.0 s.
Forms equal, timing by DSV-31 (ours). Lobby: our 0x3004 progress went to **both** accounts at 244.547 (the leaver
included); the reference stack's only to the bot.

## 10. Exit sequence (task item 10)

| step | rust3 | tdm6 |
|---|---|---|
| leave | 0xbd and lobby 0x394 [0] in one tick (234.531, seq 12318 / 12317); lobby 0x3042 [1001][0] to both accounts | the same (356.500) |
| server bye | **plain `CTL_BYE` `c9 03` x4** at 234.531 - 234.828 (seq 12323 - 12338), sent at the 0xbd, **before** the client's `C2S_BYE` | `CTL_BYE_CODE(5)` `0e a7 00 00 00 00` x4 at 356.547 - 356.843, after the client's `C2S_BYE` |
| client `C2S_BYE` | 234.547 (seq 12324) | 356.515 |
| client plain bye | `c9 03` x4 at 242.188 - 242.485, 7.66 s later, with lobby 0x306 [1], 0x388 -> 0x389, 0x38a -> 0x38b | 366.343 - 366.640, 9.8 s later, same lobby frames |
| lobby | 0x3004 (244.547), 0x306 [12] (246.344), 0x304 -> 0x305 (247.406), TCP close | 0x306 [12], 0x304 -> 0x305, close |

PROVEN-CAPTURE: the 2009 client accepted the early plain bye (it returned to the room and later to the lobby as after
the reference stack's code-5 bye). Our dserver log: "client 0 gone (Local, code 0)". Difference recorded (staged note): send
code 5 after the client's `C2S_BYE`, as `UDP_TRANSPORT_2009.md` 9.1 describes the host side.

## 11. Proposed spec changes (for the spec-fold worker)

- `RENDER_2009.md` 4.4 / M1 / open item 14: the sky table block is `[pos - dims, pos + dims, pos - dims*inner,
  pos + dims*inner]` of one sky world-model object (REFERENCE + ROM, two worlds, section 3.2); floats 0..5 = the outer
  box.
- `UDP_SESSION_2009_A.md` 4.8 / S8 and B 8.5: the content of `S2C_SKY_TABLE` (3.2); the reference stack sends it after the
  preload and before `S2C_VIEW_OBJECT`.
- `UDP_SESSION_2009_B.md` 8.6: "33 of the reference stack's 116 variables are the world-settings object's properties under the
  client's names (table of 3.3), values = the ROM property (66 of 66 on two worlds); the client copies them into console
  values at world entry (cshell+0x1a6900, cshell+0x1a9b00); sent before `S2C_LOAD_WORLD`".
- `UDP_SESSION_2009_B.md` 3.5.1 "World objects of the initial group": add the TDM real-client set of W1 (section 7.1)
  and the removal of the startup-command targets (7.2); kind-2 positions = the world model's box centre.
- `GAME_MESSAGES_2009_S2C.md` 8.2 type 0x26: "the corpse of a player: kind-1 object created at the death with the
  body's model files and one tracker on a death clip; the victim's character goes to flags 0x0802 with an empty
  attachment list in the same tick (REFERENCE tdm6)"; 0x6d / 0x74 forms: as already staged by capval-tdm6.
- `UDP_SESSION_2009_B.md` 4.2 / VIS-9: the 0x0202 hide / show form is ACCEPTED on a remote character (9.5).

## 12. Not decoded, UNPROVEN, follow-ups

- **UNPROVEN:** the staircase block (7.4, V-STAIR-1); the unthrown grenade (9.4, V-NADE-1); which sky object the table
  names when a world has two (3.2); the client behaviour for a missing world-settings variable (zero default, 3.3,
  PLAUSIBLE from the C; EVIDENCE since, 3.3 correction); the 0x0400 flag bit on the bot link (7.2); the readers of the panning-sky variables.
- **Not on the wire:** what the sky looked like (a screenshot of the same spot on both stacks would settle R3), whether
  the client slid a shown character.
- **Not exercised in rust3:** a reload by the real client (one 0x83 at 158.813, ammo 144: first real-client reload,
  not analysed here), team chat, the result screen for the real client (it left first).
