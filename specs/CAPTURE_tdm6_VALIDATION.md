# capval-tdm6: tdm6 capture validation (head shot, shotgun, own death and respawn, wall kills, HiSec, kill feed, shadow lobby)

- **Targets:** `specs/GAME_MESSAGES_2009_C2S.md` §3.3, §3.4, §4.1, §4.1.1, §4.4, §4.6; `GAME_MESSAGES_2009_S2C.md`
  §2.1.3, §3.3.1, §3.3.2, §4.1.1, §4.3.1, §8.2 (type 0x06); `HISEC_2009.md` §3.1, §3.2; `PROTOCOL_2009.md` lobby rows
  0x2000, 0x20b, 0x301, 0x303, 0x305, 0x30b / 0x30d, 0x321, 0x323, 0x325, 0x329, 0x353, 0x381, 0x389, 0x3004, 0x3014,
  0x3034, 0x3035, 0x3042, 0x3f01; `CAPTURE_tdm4_KILL_VALIDATION.md` (K1, K3, K8-K12); `CAPTURE_tdm4_WEAPON_FORMS.md` §3;
  `staging/bot-shooter_REFERENCE_FINDINGS.md` F1, F5, F6; DC_DEFAULTS rows DSV-30, DSV-32, DSV-33,
  DSV-34, HS-1..HS-5, DC-C2S-4 and the capture note DC-C2S-10'.
- **Fold state:** folded by spec-fold-8 (2026-10-25) into C2S 3.3 / 3.4 / 4.1 / 4.1.1 / 4.6 / 5.7, S2C 3.3.2 / 4.1.1 / 4.3.1 / 4.4.1 / 8.2, `HISEC_2009.md` 3.5,
  `UDP_SESSION_2009_B.md` 3.5.4, `UDP_TRANSPORT_2009.md` 9.1, `MOVEMENT_2009.md` 12.4 (each base spec's Appendix I); this record is kept in full. Not folded: the shadow
  lobby diff L1-L16 (section 10, a harness matter) and the bot-shooter F1 rewording (a staged file outside the specs).
- **Evidence:** `// OBSERVED: captures/20261025_065016_tdm6/capture.jsonl` (captap format 1;
  manifest: ROM fingerprint match, primary = reference stack, shadow = the Rust `dc-lobby-server`), the bot's own view
  `.../logs/Bot1.events.jsonl` / `Bot1.report.json`, the shadow diff `.../shadow_report.json` and the Rust shadow's own
  log `.../logs/shadow.log` / `shadow.pktlog` (our server). The capture's reference stack logs were not read.
- **Grades:** **PROVEN-CAPTURE** = bytes the real 2009 client sent, or a client behaviour visible in them (2009
  evidence). **REFERENCE** = bytes the reference server sent (what the reference stack does; rule values are never taken).
  **PLAUSIBLE** = a consistent reading that is not proven. **UNPROVEN** = open. Two helper labels: **ROM** = a value
  read at run time from the user's read-only client folder (cited by table key, value not reproduced unless it is a
  small code such as a node location); **SIM** = a segment test against our own collision model of the announced ROM
  world (`dc_sim::from_world::coll_model` per world model plus the blocker soup, `CollisionWorld::ray`), i.e. our
  geometry, not the reference stack's.
- **Decoders (scratch, outside the tree; nothing in `rust/` edited; on branch
  `capval-tdm6` at master c7c2e6a, no commits):** (a) a Rust binary over `dc_net_engine::Link` and
  `dc_botclient::capdecode::unbundle` that prints every delivered message with the capture `seq` of the datagram that
  completed it (the same replay as `dc-botclient`'s `capdecode` example, plus `seq`); (b) a Rust binary that replays
  each server link's object groups / streams (engine ids 8 / 10) into `dc_botclient::objects::ObjectTable` with the
  world box of the announced ROM world; (c) a ROM-gated Rust binary for `dc-bute` table reads and the SIM segment
  tests; (d) a Python LSB-first bit reader for the field layouts of the specs. Cross-checks: the bot link's decoded
  client shell counts equal `Bot1.report.json` `c2s_shell_counts` exactly (0x64 2,436, 0x82 810, 0x89 1, ...) and the
  server shell messages differ by one (4,217 decoded, 4,216 in the bot's report: one more 0xa6 note, PLAUSIBLY after
  the bot stopped logging); every one of the real client's 2,147 0x64 and 0x82 messages parses with 0 bits left.
- **ROM content:** none quoted. The announced world, item names, model names and file names are described by form
  only; ROM keys are cited by section and key.
- **Offsets / clocks:** "bit N" counts from bit 0 of the delivered engine message (LSB-first). `seq` = capture event
  number of the completing datagram; `t` = tap clock in seconds (resolution 15.625 ms). Lobby offsets are payload
  offsets after the 8-byte frame header (the convention of `PROTOCOL_2009.md`; captap's report offsets are 8 higher).
  The bot's own event clock is the tap clock minus 59.098 s (its first `fire` event at 114.355 = the 0x82 at
  t = 173.453, seq 751).

### Connections

| conn | port | who | evidence |
|---|---|---|---|
| 1 / 2 | auth / lobby (TCP) | the real 2009 client, account 1001, nick `player1` | 0x300 / 0x301, room host |
| 3 / 4 | auth / lobby (TCP) | our bot `Bot1`, account 1002, nick `bot1` | `Bot1.events.jsonl` `auth_ok`, `lobby_ready` |
| 5 | game (UDP) | the bot, peer port 55195 | the bot's `udp_open` local port; counts above |
| 6 | game (UDP) | the real 2009 client, peer port 55197 | hello blob of `player1` (seq 160); 0x72 / 0x85 at seq 723 |

In-game ids: the bot is roster pid 0, character object **252**, team 2; the real client is pid 1, character object
**140**, team 1. Both links got `GS_GAME_ENDPOINT` 0x9e with `mode_class` **1** (`0d9e01...`, seq 714). Room goal 100
(0x7d), no time limit. The bot ran `--shooter --hunt --respawn-after 0` **without a ROM world** (`spawn` event
`rom_collision false`, report `world_resolved null`, `rom_weapons null`, `hunt_decisions 0`): section 4.5.

## 0. Summary

| # | item | spec | verdict | grade |
|---|---|---|---|---|
| T1 | the user's **head shot**: 0x82 extra block, `extra_kind` = 53, hit point 33.3 above the bot's centre; reference stack 0x6d `hit_zone` 1, 0x74 `dmg_kind` 1, `damage` 210 | C2S §4.1.1, DSV-30, S2C §3.3.1 | **SETTLED: `extra_kind` is the skeleton node index** (ROM `[Skeleton0] Node53Location` = 1 head, `Node6Location` = 2 torso, `Node24/11Location` = 3) | PROVEN-CAPTURE (client) + ROM + REFERENCE |
| T2 | the user's torso hit (`extra_kind` 6, 15.1 above centre) | as T1 | reference stack `dmg_kind` 2, health -29.75 (node factor ratio vs tdm4's node 24 hits matches the ROM factors) | PROVEN-CAPTURE + REFERENCE |
| T3 | **shotgun**: 8 x 0x82 in one datagram | C2S §4.1, WEAPON_FORMS §3 | CONFIRMED one message per pellet = ROM `[Weapon125] VectorsPerRound`; `last` only on the 8th; `stat` +2 per pellet; `shot_scale` 1.2; no hit (bot about 900 units away) | PROVEN-CAPTURE |
| T4 | **melee** swing (knife) | C2S §4.1.1 ("melee UNDECIDED") | NEW: one 623-bit 0x82, weapon 0 / ammo 0, `weapon_pos` = `origin` exactly, no hit | PROVEN-CAPTURE |
| T5 | the user's **own death** (6 deaths) | S2C §4.1.1, §3.3.2 | the reference stack's kill block as in tdm4 (0x6d, 0xb5, 0x74, 0x66, 0x67 [3] then [2]); the client stops 0x64 at once | REFERENCE + PROVEN-CAPTURE |
| T6 | **self-armed respawn request** | C2S §3.3, §3.4, DSV-32, DC-C2S-10' | **CONFIRMED from the real client:** one `0b89` 7.015-7.031 s after the datagram with 0x67 state 2, 5 of 5 deaths, no arming message from the server; answered 16-63 ms later | PROVEN-CAPTURE (client) + REFERENCE (answer) |
| T7 | respawn set and the client's answer | S2C §4.1.1 | REFERENCE set as botduel F2 (+ the 5th slot); the client answers with the 0x64 state form and four 0x76 [3][2] + 0x7f | REFERENCE + PROVEN-CAPTURE |
| T8 | **the wall kills** | F1, DSV-34 | **the reference stack applied hits whose reported ray met a solid level wall 46-109 units from the muzzle (its own effect record ends there), and in 3 of 6 kills the user's whole body was hidden from the bot by solid level geometry; F1 needs rewording.** The obstacle is the level model itself (surface collision bit set), not a door, blocker, breakable or plain world model | REFERENCE + SIM |
| T9 | why the bot "fired through the wall" and "did not aim" | `dc-botclient` | the bot loaded no world: its line-of-sight test passes every line, and it could not decode the user's `pos48` position, so every one of its 810 reports aimed at a fixed point near the world origin, 1,411-1,771 units from the user | bot (OBSERVED) |
| T10 | effect relay stops after the granted rounds | S2C §8.2, DC-C2S-4 | REFERENCE: exactly 120 effect records per bot life (= the 0xb2 grant of 120), none afterwards, but later hits still did damage | REFERENCE |
| T11 | **HiSec drops** | HISEC §3, HS-1..HS-5 | 6 cases in 7 kills, reserved for the killer 15 s, then free and **never removed** (5 lay in the world at the end); one was picked up 32 s after its drop | REFERENCE (observed only; HS-1 is the user's decision) |
| T12 | **kill feed** fields | S2C §3.3.1, DSV-33 | 7 x 0x6d: `kill_kind` 1, `f_relation` 0, `feed_value` 2 throughout; `hit_zone` 3 for the bot's kills, 1 for the user's; **`streak_flag` 1 only on the first kill of the match** (DSV-33's "first kill since the spawn" is contradicted) | REFERENCE |
| T13 | weapon switches, reload, chat, purchase | C2S §4.2, §4.4, §6.1 | slots 0, 2, 3, 4 selected (slot 1 never); no 0x83, no 0x68 / 0xbe, no lobby chat; one shop purchase + equip of weapon item 125 | PROVEN-CAPTURE |
| T14 | grenades | C2S §4.6, DSV-39 | two throws (19 / 7 x 0x4d, then the release 0x82); the second blast hit the bot (0x74 area form, `dmg_kind` 1) | PROVEN-CAPTURE + REFERENCE |
| T15 | the exit sequence | S2C §2.1.3, `PROTOCOL_2009.md` 7 | 0xbd + lobby 0x394 [0] in one tick, `C2S_BYE`, server bye x 4, **plain bye x 4 9.8 s later** with 0x306 [1] on TCP; no result screen for the leaver | PROVEN-CAPTURE / REFERENCE |
| T16 | the bot's result set | S2C §2.1.3 | forfeit end exactly as tdm4 (0x6c + 0x46 +0.343 s, +2.735 s 0x47 / 0x6b / 0x65 / 0x48, +3.0 s 0x3f) | REFERENCE |
| T17 | shadow lobby diff | `REFERENCE_TESTING.md` §4 | 23 different, 9 missing, 6 extra: listed as L1-L16 | (diff) |

## 1. Timeline (t = tap clock; P = player 1 = the real client, B = the bot)

| t | seq | link | message (decoded) | grade |
|---|---|---|---|---|
| 165.968 | 150 / 152 | both | UDP links open (bot conn 5, client conn 6) | - |
| 173.437 | 733 / 734 | s2c P | first spawn set (0x67 [1], move_seq 1 at (588.0, 50.1, 932.0)), round start (0x43, 0x44, 0x45, 0xb5 [0][0]) | REFERENCE |
| 173.453 | 751 | c2s B | the bot's first 0x82 (of 810, every 150-156 ms until 351.797) | bot |
| 190.000 - 190.531 | 2938 - 3006 | s2c P | four 0x74 (`damage` 27 each); kill block: 0xd6 sub 0 / sub 2 [1][0], 0x6d (victim 1, killer 0), 0xb5 [0][1], 0x67 [3], 0x67 [2] | REFERENCE |
| 197.547 | 3754 | c2s P | `0b89` (7.016 s after seq 3006) | PROVEN-CAPTURE |
| 197.593 | 3756 / 3763 | both | respawn set (move_seq 3 at (50.0, 50.1, 830.0)); client: 0x64 `[0x0010][1]`, 4 x (0x76 [3][2] + 0x7f [0][145][145]) | REFERENCE / PROVEN-CAPTURE |
| 205.547 | 4833 | s2c | 0xd6 sub 2 [1][-1] (case 1 freed) | REFERENCE |
| 211.703, 211.953 | 5540, 5572 | c2s P | two knife swings (0x82 weapon 0) | PROVEN-CAPTURE |
| 219.312 - 219.750 | 6527 - 6591 | s2c P | four 0x74; kill block (case 2 reserved for pid 0), 0xb5 [0][2] | REFERENCE |
| 219.718 | 6588 | c2s P | the user's rifle hit on the bot, `extra_kind` 6 (sent 32 ms before his own kill block) | PROVEN-CAPTURE |
| 219.750 | 6592 | s2c B | 0x74 (`dmg_kind` 2, `damage` 30), 0x66 health 70.25, armour 94.75 | REFERENCE |
| 226.781 | 7352 | c2s P | `0b89` (7.031 s after seq 6591) | PROVEN-CAPTURE |
| 240.890 | 9357 | c2s P | grenade release after 19 x 0x4d (239.468 - 240.687) | PROVEN-CAPTURE |
| 246.547 | 10125 | c2s P | **the head shot**, `extra_kind` 53, `hit_dist` 785.7 | PROVEN-CAPTURE |
| 246.609 | 10128 / 10129 | both | kill block: 0x6d (victim 0, killer 1, `hit_zone` **1**), 0xb5 [1][2]; to B 0x74 (`dmg_kind` 1, `damage` 210), 0x66 health 0, 0x67 [3], [2]; case 3 reserved for pid 1 | REFERENCE |
| 251.578 / 251.609 | 10685 / 10688 | P (TCP / UDP) | lobby 0x3014; 0xd6 sub 1 [2][1] (the user picked up case 2) | REFERENCE |
| 253.640 / 253.703 | 10958 / 10963 | B | the bot's self-armed 0x89, then its respawn set (63 ms) | bot / REFERENCE |
| 253.765 - 254.250 | 10976 - 11054 | s2c P | four 0x74, kill block **without a new case**; 0xd6 sub 2 [3][-1] | REFERENCE |
| 261.265 | 11892 | c2s P | `0b89` (7.015 s) | PROVEN-CAPTURE |
| 297.078 - 297.562 | 16236 - 16297 | s2c P | four 0x74, kill block, case 4 | REFERENCE |
| 304.578 | 17032 | c2s P | `0b89` (7.016 s) | PROVEN-CAPTURE |
| 326.593 | 20047 | c2s P | grenade release after 7 x 0x4d | PROVEN-CAPTURE |
| 329.109 | 20354 | s2c B | the blast: 0x74 (`zone` 7, `dmg_kind` 1, `damage` 42), health 57.68 | REFERENCE |
| 331.000 - 331.500 | 20609 - 20672 | s2c P | four 0x74, kill block, case 5 | REFERENCE |
| 338.515 | 21388 | c2s P | `0b89` (7.015 s) | PROVEN-CAPTURE |
| 343.281 | 22081 | c2s P | 0x76 [3][2] + 0x7f [4][125][125] (the shotgun) | PROVEN-CAPTURE |
| 347.000 | 22624 | c2s P | **8 x 0x82 weapon 125** in one datagram | PROVEN-CAPTURE |
| 351.343 - 351.828 | 23227 - 23308 | s2c P | four 0x74, kill block split over seq 23299 - 23308 (0x67 [3] at 351.812, [2] at 351.828), case 6 | REFERENCE |
| 356.500 | 23792 / 23793 | P | 0xbd and lobby 0x394 [0] in the same tick (4.69 s after the last death: no 0x89) | PROVEN-CAPTURE |
| 356.515 | 23798 | c2s P | `C2S_BYE` | PROVEN-CAPTURE |
| 356.547 - 356.843 | 23805 - 23814 | s2c P | transport bye (`0e a7 ...`), 4 copies | REFERENCE |
| 356.843 - 362.578 | 23813 - 24161 | s2c B | forfeit result set (T16) | REFERENCE |
| 365.609 | 24345 | c2s B | the bot's `C2S_BYE` (3.03 s after 0x3f) | bot |
| 366.343 - 366.640 | 24364 - 24367 | c2s P | transport plain bye `c9 03`, 4 copies, 9.8 s after the server's bye | PROVEN-CAPTURE |

## 2. Hits by the real client (task item 1)

### 2.1 The head shot and the torso hit: the 0x82 extra block

Layout as C2S §4.1.1 (887 bits, 0 bits left in both). Values:

| field | torso hit (seq 6588, t 219.718) | head shot (seq 10125, t 246.547) |
|---|---|---|
| `weapon_id` / `ammo_id` | 145 / 145 | 145 / 145 |
| `origin` | (20.450, 83.010, -1425.658) = 0x64 pos + (0.098, 33, -0.019) | (-422.737, 90.207, -1305.198) = 0x64 pos + (0, 33, 0) |
| `dir` | (0.965, -0.054, -0.257) | (0.967, -0.009, -0.255) |
| `seed` / `frac` / `shot_scale` | 166 / 0 / 1.0 | 15 / 0 / 1.0 |
| `extra_obj` = `hit_obj` | 252 (the bot) | 252 |
| **`extra_kind`** | **6** | **53** |
| `extra_a` (hit point) | (337.062, 65.177, -1509.940) = `origin + hit_dist * dir` (error 6e-5) | (337.062, 83.379, -1505.183) (error 7e-5) |
| `extra_b` | (-1, 0, 0) | (-1, 0, 0) |
| `hit_dist` | 328.124 | 785.708 |
| hit point minus the bot's 0x64 position (367.0, 50.1, -1522.0) | (-29.94, **+15.08**, +12.06) | (-29.94, **+33.28**, +16.82) |
| `stat` | 6 | 10 |

Both hit points lie on the plane x = 337.06, 29.94 units in front of the bot's centre on the shooter's side, with the
surface normal (-1, 0, 0) (CORRECTION 2026-10-26, client-hitnodes: the normal of the entered face of the object box doubled in size, whose half-extent 15 x 2 explains the 30-unit offset; C2S 4.1.1): the same 30-unit offset as the four tdm4 hits (K3). The head shot is 33.3 units above the
centre (the bot's box half height is 50, so 83 % of the way up); the torso hit 15.1 above.

ROM (`[Skeleton<s>] Node<k>Location`, `dc-bute` at run time; location codes as DSV-30: 1 head, 2 torso, 3 arms,
4 legs):

| node | `[Skeleton0]` location | `[Skeleton1]` location | where the client reported it |
|---|---|---|---|
| 53 | **1** | 4 | tdm6 head shot, 33.3 above centre |
| 6 | **2** | 2 | tdm6 torso hit, 15.1 above centre |
| 24 | **3** | 3 | tdm4 hits 1, 2 (24-27 above centre); every bot report of tdm6 |
| 11 | **3** | 1 | tdm4 hits 3, 4 (24-27 above centre) |

(`[Skeleton0]` holds 4 head nodes (8, 9, 10, 53) and `[Skeleton1]` 15; 19 of the 33 body rows name skeleton 0.)

The reference stack's answers to the two hits (bot link, seq 6592 and 10129):

| | torso hit | head shot | tdm4 hits (K8, for comparison) |
|---|---|---|---|
| 0x74 `dmg_kind` | **2** | **1** | 3 (nodes 24, 11) |
| 0x74 `zone` (and the byte before it) | 2 | 2 | 2 |
| 0x74 `damage` | 30 | **210** | 27 |
| 0x74 `from_dir` magnitude | 0.2975 | **1.0000** | 0.2678 |
| 0x66 health after | 70.25 (-29.75) | 0.0 (from 70.25) | -26.775 per hit |
| 0x66 armour after | 94.75 (-5.25) | none sent (0xa6 sub 0x15 stays 95) | -5.25 per hit |
| 0x6d `hit_zone` | (no kill) | **1** | 3 |

Findings:

1. **`extra_kind` is the index of the skeleton node the client's ray hit** (closes DSV-30's PLAUSIBLE and the
   dserver-slice4 open point 1). PROVEN-CAPTURE for the client side: the 2009 client named node 53 for a hit in the
   head region and node 6 for a hit in the chest region, and both numbers are the ROM nodes of those locations in
   `[Skeleton0]`. REFERENCE agreement: for all six real-client hits seen so far (tdm4 24, 24, 11, 11; tdm6 6, 53) the
   reference stack's 0x74 `dmg_kind` equals the `[Skeleton0]` location of the named node (3, 3, 3, 3, 2, 1), and its 0x6d
   `hit_zone` equals it for both kills (tdm4 node 11 -> 3, tdm6 node 53 -> 1). The same holds for the bot's 24
   reports (node 24 -> `dmg_kind` 3, `hit_zone` 3). Node 53's location differs between the two skeleton rows (head in
   0, legs in 1), so the body row of the victim matters; the reference stack's results fit `[Skeleton0]` for the bot's body
   (PLAUSIBLE: which `[Model<n>]` row the reference stack gave the bot is not decoded here).
2. **The reference stack's damage scales with the node's ROM factor** (reference stack rule values, not taken): the health loss of the
   node-6 hit divided by that of a tdm4 node-24 hit equals the ratio of the two ROM `Node<k>DamageFactor` values; the
   armour loss (5.25) did not change with the node; for the head shot the 0x74 `damage` equals the base value of the
   ammunition times the node-53 factor and the armour was not reduced. DSV-30 already uses the node factor (its
   armour rule is ours).
3. **0x74 `damage` is not clamped to the remaining health** (210 sent while the victim had 70.25; tdm4's lethal hit
   carried 27 with 19.675 left), and **`|from_dir|` = min(1, that damage / 100)** (1.0 for the head shot, 0.2975 for
   29.75, 0.4232 for the 42.32 grenade loss of T14). DSV-33 says "`damage` = rounded health loss" and "`dmg_kind` 3":
   both differ from the reference stack form (staged correction; the client only tests `damage` > 0 and stores it, S2C §4.3).
4. No leg hit happened in tdm6 (the user hit the bot twice with the rifle and once with a grenade). Leg locations
   (code 4) stay unobserved; `[Skeleton1]` node 53 shows that one node number can be a leg on another body.

### 2.2 The user's other shots

| seq | t | weapon | form | note |
|---|---|---|---|---|
| 5540, 5572 | 211.703, 211.953 | 0 (ammo 0) | 623 bits, no extra, no hit | melee (T4) |
| 9357 | 240.890 | 7 (ammo 9) | 623 bits | grenade release (T14) |
| 14709 | 284.843 | 145 | 623 bits | a miss from the spawn point (359, 50.1, 1020) |
| 18622 | 317.656 | 145 | 623 bits | a miss from (398, 50.1, 891) |
| 20047 | 326.593 | 7 (ammo 9) | 623 bits | grenade release (T14) |
| 22624 x 8 | 347.000 | 125 (ammo 125) | 623 bits each | shotgun (section 3) |

`stat` runs 2, 4, 6 ... 32 over all 16 messages (+2 per 0x82, pellets included).

### 2.3 Melee (T4, new)

Both knife messages: `wkind` 0, weapon 0, ammo 0, **`weapon_pos` = `origin` exactly** (-488.6, 97.2, 236.2) = the
0x64 position + (0, 33, 0) (a rifle's `weapon_pos` lies about 10.9 units from `origin`), `dir` = the view forward,
`frac` 0, `shot_scale` 1.0, `time_ms` 248 ms apart, `last` 1, `has_extra` 0, `has_hit` 0, no 0x80 sound and no 0x76
around them; preceded by 0x7f [2][0][0] at 198.672. The bot was 1,850 units away, so this is the swing at nothing.
Replacement for C2S §4.1.1 "melee UNDECIDED": "A melee swing that hits nothing is one 623-bit `UP_FIRE` with weapon
and ammunition 0, `weapon_pos` equal to `origin` (the eye point) and `last` 1 (OBSERVED tdm6, two swings); the hit
form of a swing is not yet captured."

## 3. The shotgun shot (task item 2; cross-check of CAPTURE_tdm4_WEAPON_FORMS §3)

Selection: 0x76 [3][2] + 0x7f **[4][125][125]** at 343.281 (slot 4; the reference stack's 0xb2 granted weapon 125 with
`ammo_kind` 125, ammunition 28.0, `class_pos` 1, slot 4 in every spawn set); 0x80 [4][125][own_id 1][pos] at 343.672
and 347.328 (select / pump animation sounds; none per pellet).

The shot: **eight 0x82 in one reliable block of one datagram** (seq 22624, t 347.000), 623 bits each, 0 bits left:

| pellet | `dir` | angle to the aim forward | `seed` | `last` | `stat` |
|---|---|---|---|---|---|
| 1 | (0.1202, -0.0385, -0.9920) | 2.06 deg | 48 | 0 | 18 |
| 2 | (0.1154, -0.0551, -0.9918) | 2.33 deg | 19 | 0 | 20 |
| 3 | (0.0451, -0.0316, -0.9985) | 2.35 deg | 73 | 0 | 22 |
| 4 | (0.1177, -0.0686, -0.9907) | 3.00 deg | 198 | 0 | 24 |
| 5 | (0.1270, -0.0373, -0.9912) | 2.42 deg | 127 | 0 | 26 |
| 6 | (0.0834, -0.0211, -0.9963) | 0.30 deg | 170 | 0 | 28 |
| 7 | (0.0776, -0.0181, -0.9968) | 0.67 deg | 52 | 0 | 30 |
| 8 | (0.1140, -0.0134, -0.9934) | 1.77 deg | 71 | **1** | 32 |

Common to all eight: `weapon_pos` (360.477, 264.053, -576.727), `origin` (365.296, 268.234, -565.821) = the 0x64
position + (0, 33, 0), `aim` (0.001, 0.999, -0.014, 0.043) (forward (0.086, -0.027, -0.996)), `yaw` -3.2278, `frac`
0, **`shot_scale` = 0x3F99999A = 1.2**, `time_ms` 346339, `moving` / `wflag` / `mode1` / `flag_c` 0, no extra, no hit.
Pairwise pellet angles up to 4.72 deg.

Verdict: **CONFIRMED** "one 0x82 per pellet" for a multi-pellet weapon (the open point of WEAPON_FORMS §3 and C2S
§4.1.1): count = ROM `[Weapon125] VectorsPerRound` (8, read at run time); the pellets of one shot share every field
except `dir`, `seed`, `last` and `stat`; `last` = 1 only on the final pellet; `stat` counts messages, not shots;
`frac` was 0 (the first shot of the weapon); `shot_scale` 1.2 is the weapon's constant (C2S §4.1 "1.0 or 1.2"). No
pellet hit: the user stood on an upper floor at (365.3, 235.2, -565.8) and the bot at (410, 50.1, -1460), about 900
units away; the user died 4.8 s later (the user's report: "couldn't get close enough").

## 4. The user's own death and respawn (task item 3)

### 4.1 The kill block (REFERENCE)

Six deaths, all by the bot, each after exactly **four** 0x74 150-170 ms apart (the bot's 150 ms fire interval; time
from the first to the lethal hit 0.44-0.49 s). Order on the user's link (seq 3006, one reliable block): 0xa6 type
0x18 on 140 sub 0x20, two file lists and a variable (the case), **0xd6 sub 0**, **0xd6 sub 2 [case][0]**, 0xa6 type
0x17 on the case sub 2 [16], **0x6d**, **0xb5**, **0x74**, 0xa6 sub 0x14 / 0x15, **0x66 kind 2 (0.0)**, **0x66 kind
3**, **0x67 [3]**, corpse files and variables, **0x67 [2]**, then unreliable notes and two type-0x06 records. This is
the tdm4 order (K9, K10) seen from the victim's side, now with a real 2009 client as the victim. The last death (seq
23299 - 23308) was spread over four datagrams: 0x67 [3] at 351.812 and 0x67 [2] at 351.828.

0x74 to the user, every hit: `0d749f7380bd9dae22bc3d2d853e0201ffff0302001b000000` (first bot life) and
`0d744f2594bd5bf227bcc8e2833e0201ffff0302001b000000` (second): `from_dir` = the bot's reported shot direction x
0.2678, `unused` 2, `mode` 1, `ammo_kind` 0xFFFF, `dmg_kind` 3, `zone` 2, `part` 0, `damage` 27 (the tdm4 form,
F5). Note for the user's report: the damage indicator of the 2009 client is drawn from `from_dir`, i.e. from the
bot's claimed direction toward the world origin, which was 60.8-143.6 deg away from the true direction from the bot
to the user (T9): the hits "came from" where no bot was.

### 4.2 The client while dead and its respawn request (PROVEN-CAPTURE)

| death (datagram with 0x67 [2]) | last 0x64 before | client messages while dead | `0b89` | delay | reference stack's 0x67 [1] |
|---|---|---|---|---|---|
| 190.531 (3006) | 190.484 | `C2S_RATE` 640 + one sound id (190.531) | 197.547 (3754) | **7.016 s** | 197.593 (+46 ms) |
| 219.750 (6591) | 219.718 | `C2S_RATE` + one id (220.109) | 226.781 (7352) | **7.031 s** | 226.828 (+47 ms) |
| 254.250 (11045) | 254.203 | `C2S_RATE` + one id (254.250) | 261.265 (11892) | **7.015 s** | 261.281 (+16 ms) |
| 297.562 (16297) | 297.500 | `C2S_RATE` + one id (297.578) | 304.578 (17032) | **7.016 s** | 304.640 (+62 ms) |
| 331.500 (20672) | 331.453 | `C2S_RATE` + one id (331.672) | 338.515 (21388) | **7.015 s** | 338.578 (+63 ms) |
| 351.828 (23308) | 351.812 | 0xca [2], `C2S_RATE` + one id | none: 0xbd at 356.500 (4.67 s) | - | - |

- **The 2009 client arms its own wait on 0x67 state 2 and sends exactly one `UP_RESPAWN_REQUEST` 0x89** (`0b 89`, 16
  bits, reliable) 7.015-7.031 s later (strictly more than 7000 ms plus at most two tap ticks), in `mode_class` 1, with
  **no 0xd7, 0xdb or 0xd4 from the server** at any time (none on either link). This upgrades C2S §3.3 / §3.4 and
  S2C §4.1.1 from DISASM + bot behaviour to **PROVEN-CAPTURE** and confirms DSV-32 from the real client's side: its
  250 ms guard (no request earlier than 6,750 ms after the death) is met by a margin of 265-281 ms; a server timer of
  exactly 7000 ms would still race it (C2S §3.4).
- No 0x64 is sent while dead (the last update is 16-62 ms before the kill datagram, the next one is the reliable
  state form `0b 64 10 00 01` in the datagram after the respawn set): C2S §2.1's gate "only while the state is 1"
  holds.
- The `C2S_RATE` (engine id 7) at each death carries the rate 640 and one u16 sound id (`07 80 02 01 01`, `07 80 02
  03 01`, ...), consistent with `UDP_SESSION_2009_A.md` §5.5 (rate unchanged, a tracked-sound acknowledgement
  pending); PLAUSIBLE: the death sound.
- The client left during its sixth wait (4.67 s): no request was sent; DSV-32 needs no special case for it.

### 4.3 The respawn set (REFERENCE) and the client's answer (PROVEN-CAPTURE)

Order on the user's link (seq 3756, one block; the same five times): 0x6a [0][angles], 0xa6 x 2, **0x67 [1]**, 0x66
kind 0x13, five 0xb2 (slots 0-4: weapons 145, 164, 0, 7, 125) with four 0x7f between them, **0xa0 flags 0x0009**,
**0xa0 flags 0x0020** (teleport, `move_seq` 3, 4, 5, 6, 7 at (50.0, 50.1, 830.0), (529.0, 50.1, 953.0), (359.0,
50.1, 1020.0), (398.0, 50.1, 891.0), (291.0, 50.1, 986.0): a different spawn point each time), 0x66 kind 0x13, the
five 0xb2 / four 0x7f again, 0x66 kinds 0x0f and 0x10, **0x6b** (refresh), **0x67 [1]** again, 0xa6 notes, 0x66 kind
1 for slots 2 and 3, 0x66 health, armour, kinds 0x18 and 6. As botduel F2, with the fifth slot.

The client's answer in the next datagram (seq 3763, 7365, 11900, 17047, 21402): the reliable 0x64 state form
`[0x0010][1]`, then **four** pairs 0x76 [3][2] + 0x7f [0][145][145] (one per 0x7f the server sent in the first half
of the set, as WEAPON_FORMS §2 saw at the first spawn), then its 15 Hz 0x64 stream from the new teleport position.

## 5. The wall kills (task item 4)

### 5.1 What the bot reported

All 810 bot 0x82 are the 887-bit form with `extra_obj` = `hit_obj` = 140 (the user), `extra_kind` 24, `extra_b`
(-1, 0, 0) and a **fixed claimed hit point** (3.617, 25.573, -15.0) in the first bot life (origin (367.0, 83.1,
-1522.0), `hit_dist` 1551.26) and (4.2, 25.6, -15.0) in the second (origin (410.0, 83.1, -1460.0), `hit_dist`
1502.0); `origin + hit_dist * dir` = `extra_a` exactly in every message. The bot's body yaw (its 0x64 view block)
was -13.6 deg / -15.7 deg (toward that point) while it fired and returned to its spawn yaw (91 deg / 120 deg) after
each kill; it never turned toward the user.

### 5.2 What the reference stack did with the reports (REFERENCE)

The reference stack answers a report with engine-17 type-0x06 effect records (S2C §8.2 layout; 432 bits):

| answer | count | records |
|---|---|---|
| applied hit (a 0x74 to the user followed within 0-78 ms) | 24 (6 kills x 4) | in the first 120 rounds of a bot life: two records, `[140][252][145][145][a 6][b 0x0f11 / 0x0f01][c 0]` with `from` = the bot's `weapon_pos` and `to` = **the claimed hit point** (3.9, 25.6, -16.0) with the claimed normal (-1, 0, 0), then `[0][252][145][145][10][0x4bf0 / 0x4be0][0]` ending at a wall; after round 120: none |
| refused (no damage) | 786 | in the first 120 rounds: one record `[0][252][145][145][10][0x4f10 / 0x4f00][0]`, `from` (369.5, 78.8, -1512.3), `to` **(341.7, 79.1, -1417.0)** with normal (0, 0, -1) (first life; second life (397.9, 81.4, -1417.0)); after round 120: none |

The reference stack's own trace of the reported ray therefore stops at a wall 109 units (first life) / 46 units (second life)
from the muzzle in **every** answer, hits and refusals alike, and the damage decision does not depend on it.

SIM check of that wall: our collision ray from the bot's `origin` along the reported ray hits the level model at
(341.46, 79.06, -1416.10), normal (0, 0, -1), surface flags 0x67001081 (collision bit 0 set), t = 0.070 (second
life: (397.7, 81.4, -1416.1), t = 0.030): within 1 unit of the reference stack's record end. This is also a cross-check of
dc-sim's level geometry against the reference stack's tracer.

### 5.3 The six kills, segment by segment (SIM)

The user's position is his own 0x64 trailer position at the shot (the 0x64 trailer, not the stream); the bot's is its
0x64 position (it never moved within a life). Each lethal set of four applied hits was tested from the bot's
`origin` (eye) and its `weapon_pos` to 30 points of the user's body (heights -45 .. +45 around the 0x64 position,
+-15 units sideways) and, for kills 1, 5 and 6, to 180 points of the volume +-50 vertical, +-40 / +-20 horizontal:

| kill (lethal hit) | user at (first -> lethal applied hit) | bot -> user | reported ray | user's body visible from the bot | obstacle |
|---|---|---|---|---|---|
| 1 (190.531) | (646.0, 47.4, -1356.4) -> (643.4, 47.6, -1499.0) | 280-326 units | crosses the level wall at 109 | **0 of 30 / 0 of 180 points** | level wall plane x = 493.9, normal (-1, 0, 0), flags 0x67001081 |
| 2 (219.750) | (20.4, 50.0, -1425.6), standing | 361 | crosses at 109 | 14 of 30 | partly: level faces at x = 259.6 (flags 0x67001001) and others; the user's own ray to the bot passed clear (he hit it at 219.718, SIM clear) |
| 3 (254.250) | (287.9, 50.1, -1762.9) -> (348.3, 50.1, -1633.5) | 187-328 | crosses at 46 | **30 of 30** | none: a clear line (the bot had just respawned facing away) |
| 4 (297.562) | (-185.4, 104.0, -1431.8) -> (-220.5, 50.1, -1531.8) | 596-635 | crosses at 46 | 5, 5, 9, 22 of 30 | partly: slanted level faces (flags 0x67001001) |
| 5 (331.500) | (903.4, 62.8, -1293.9) -> (917.9, 68.1, -1395.5) | 512-523 | crosses at 46 | **0 of 30 / 0 of 180** | the same level wall plane x = 493.9 |
| 6 (351.812) | (539.7, 300.3, -1411.0) -> (488.5, 250.1, -1538.0) | 200-258 | crosses at 46 | **0 of 30 / 0 of 180** | a level slab y = 179.9 (normal (0, -1, 0)): the user was on the floor above the bot |

Findings:

1. **The reference stack accepted shots whose reported segment crosses a solid** (24 of 24 applied hits: every reported ray
   met the level wall 46-109 units from the muzzle, and the reference stack's own effect record ends there), **and in kills 1,
   5 and 6 the victim was completely hidden from the shooter** by solid level geometry (SIM: no clear line to any of
   180 sample points of the body volume). The user's "killed through a wall" is literally what happened in those three
   kills; in kills 2 and 4 the user was partly exposed, in kill 3 fully.
2. **Obstacle class:** the level model (the world model no object names, `WorldFilter::level`), polygons with
   collision bit 0 set. A per-model SIM pass over all 63 world models and the 42 blocker polygons found that on all
   1,080 segments of kills 1, 5 and 6, and on the eye-to-eye segments of all 809 tested reports, the **first** solid is
   the level model; no door, blocker polygon, breakable, plain world model or other named model lies before it (a
   breakable model lies behind the level slab on 12 kill-6 segments). So it is **not** "an obstacle missing from our
   collision model": the geometry is there, the same surface class stops the reference stack's own effect trace, and the reference stack
   applies the damage anyway.
3. **What the reference stack tests instead is UNPROVEN.** Checked and rejected on the 810 reports: a trace of the reported ray
   (refused and applied alike stop at the wall); a line of sight from the shooter's eye to the victim in the level
   model (18 of 23 tested applied hits blocked; one refused report had a clear line); a distance threshold (applied at
   187-635 units, but 67 refusals at 250-635); the bearing of the victim relative to the claimed ray or to the bot's
   body yaw (overlapping ranges: in the first bot life applied from 60.8 deg off the claimed ray, refused up to 69.0 deg,
   41 refusals above 60.8 deg; second life applied from 71.7, refused up to 82.5); the victim's distance to the
   reference stack's trace end (overlap). All 24 applied hits came in uninterrupted runs of four that started when the user
   came within about 640 units of the bot. Not decodable here; a capture with a moving, aiming shooter is needed
   (V-WALL-1, section 10).
4. **F1 of `bot-shooter_REFERENCE_FINDINGS.md` must be reworded.** Its observation (botduel1: 120 reports through walls
   at 2,330-2,520 units, no damage, one effect record ending at the wall) stands, but its conclusion "the reference stack traces
   each reported shot against the world itself and stops it at the first wall" is true only of the **effect record**.
   Proposed text: "F1: the reference stack's effect record of a reported shot stops at the first wall along the reported ray;
   the damage decision is not that trace and not a line-of-sight test in the level model (tdm6: 24 hits applied
   through the wall the record stopped at, three kills on a fully hidden victim). botduel1's refusals have another,
   UNPROVEN cause." DSV-34 (DeCombat traces the reported ray from `origin` along `dir`) stays our rule; the reference stack is
   simply weaker here and must not be used as its reference.

### 5.4 Why the bot's line of sight "passed" (T9)

Not because an obstacle is missing from dc-sim: the bot had **no world at all**. Its `spawn` event says
`rom_collision false`, its report `world_resolved null`, `rom_weapons null`, and `hunt::line_clear` is not consulted
without a world (`shooter.rs`: "`None` = every line counts as clear"). Without the world box it also could not decode
the user's `pos48` stream positions (our replay of the bot link with the ROM world's box decodes 2,348 position
changes of the user's object 140), so its target position stayed at the world origin: every claimed hit point is that
origin plus the bot's box offset, 1,411-1,771 units from the user, and the bot's yaw pointed there. With no ROM weapon
row it never reloaded (810 shots, 0 x 0x83). Consequences for `dc-botclient` (bot-human worker): refuse `--shooter`
(or at least `--hunt`) without a loaded world, or decode positions from 0xa0 / stream entries with the server's box
before aiming; aim at the decoded target; reload by the ROM clip.

### 5.5 The effect relay stops after the granted rounds (T10)

Exactly **120** bot reports per bot life got effect records (life 1: reports 1-120 of 354; life 2: 1-120 of 456), the
granted 0xb2 ammunition of slot 0 being 120.0; reports 121 onward got no record, hit or miss. Yet 20 of the 24
applied hits came after round 120 (kills 2, 4, 5, 6). REFERENCE: the reference stack mirrors the shooter's ammunition only for
the effect relay and still applies damage from an empty gun. DC-C2S-4 (DeCombat refuses reports beyond the clip and
stock mirror) is ours and differs on purpose.

## 6. HiSec drops (task item 5; REFERENCE, observed only, no value adopted)

| case key | dropped (0xd6 sub 0 + sub 2 [key][owner]) | owner | case object, create position | countdown (0xa6 type 0x17 sub 2) | freed (sub 2 [key][-1]) | end |
|---|---|---|---|---|---|---|
| 1 | 190.531, the user's death 1 | 0 (the bot, the killer) | 258 at (640.1, 98.3, -1502.9) | 16, 15 ... 0, one per second | 205.547 (+15.0 s) | still in the world at the end |
| 2 | 219.750, death 2 | 0 | 269 at (24.1, 20.3, -1429.9) | 15 ... 0 | 234.734 (+15.0 s) | **picked up by the user** at 251.609 (0xd6 sub 1 [2][1], object removed; lobby 0x3014 to the user 31 ms earlier, seq 10685), 31.9 s after the drop |
| 3 | 246.609, the bot's death | 1 (the user) | 272 at (367.1, 20.3, -1521.9) | 15 ... 8, then 0 | **254.250, at the owner's death** | still in the world |
| (none) | 254.250, death 3 | - | - | - | - | no case: case 3 was still reserved at the kill |
| 4 | 297.562, death 4 | 0 | 283 at (-217.9, 20.3, -1529.9) | 15 ... 0 | 312.562 | still in the world |
| 5 | 331.500, death 5 | 0 | 277 at (917.1, 18.3, -1404.9) | 15 ... 0 | 346.453 | still in the world |
| 6 | 351.812, death 6 | 0 | 281 at (494.1, 220.3, -1535.9) | 15 ... 11 (the user left) | - | still in the world |

Findings (reference stack behaviour, for the record; the DeCombat rule is HS-1's project decision of about one case per match):

1. **6 cases in 7 kills**: every kill dropped a case unless a case was still **reserved** (kill at 254.250); free cases
   lying in the world did not stop a new drop. The drops are at the victim's position (x and z within 10 units of the victim's last 0x64 position; 30 units below
   its centre in four of six, PLAUSIBLY the floor point), the case keys count from 1, and 0xd6 sub 0 + sub 2 come in the kill block before 0x6d (both links).
2. **Reservation**: owner = the killer's pid for 15 s (countdown sent each second, 16 at the first drop, 15 later),
   then owner -1 with the countdown at 0; a reserved case is **released at once when its owner dies** (case 3).
3. **No despawn**: none of the six case objects was removed except by the pickup; at the end five cases lay in the
   world. With one drop per kill and no despawn the map fills with cases: this is what the user saw. HS-4's despawn
   (T = 30 s) and HS-1's one-case target both differ from the reference stack on purpose.
4. Other type-0x17 objects (pickup kinds other than 5: the victim's dropped items) are created at every death
   (mostly parked at (-9999.9, -9999.8, -9999.9)) and are not cases: only the six objects with 0xd6 keys and
   countdown notes are counted here (pickup kind from the blob not decoded: PLAUSIBLE identification by the 0xa6 type
   0x17 sub 2 countdown).

## 7. The kill feed (task item 6; REFERENCE, both links identical)

| t | seq (P link) | victim | killer | `kill_kind` | weapon | `hit_zone` | `f_relation` | `f_feed2` | `keep_hud` | `f_feed6` | `streak_flag` | `feed_value` | `no_count` | revenge | coop | then 0xb5 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 190.531 | 3006 | 1 (player1) | 0 (bot1) | 1 | 145 | 3 | 0 | 0 | 0 | 0 | **1** | 2 | 0 | none | 0, 0 | [0][1] |
| 219.750 | 6591 | 1 | 0 | 1 | 145 | 3 | 0 | 0 | 0 | 0 | 0 | 2 | 0 | none | 0, 0 | [0][2] |
| 246.609 | 10128 | **0 (bot1)** | **1 (player1)** | 1 | 145 | **1** | 0 | 0 | 0 | 0 | 0 | 2 | 0 | none | 0, 0 | [1][2] |
| 254.250 | 11045 | 1 | 0 | 1 | 145 | 3 | 0 | 0 | 0 | 0 | 0 | 2 | 0 | none | 0, 0 | [1][3] |
| 297.562 | 16297 | 1 | 0 | 1 | 145 | 3 | 0 | 0 | 0 | 0 | 0 | 2 | 0 | none | 0, 0 | [1][4] |
| 331.500 | 20672 | 1 | 0 | 1 | 145 | 3 | 0 | 0 | 0 | 0 | 0 | 2 | 0 | none | 0, 0 | [1][5] |
| 351.812 | 23299 | 1 | 0 | 1 | 145 | 3 | 0 | 0 | 0 | 0 | 0 | 2 | 0 | none | 0, 0 | [1][6] |

All 151 bits, head `0d6d0100019100031020...` (first), `0d6d0100019100030020...` (bot kills), `0d6d0001019100010020...`
(the user's kill). For the user to compare with what the screen showed (S2C §3.3.1 effects): six lines "bot1 [weapon
145 icon] player1" without a head-shot mark and one line "player1 [weapon 145 icon] bot1" **with** the head-shot mark
(`hit_zone` 1 sets the head-shot flag and adds 1 to the killer's head-shot counter); the first kill of the match uses
the second form of the kill announcement (`streak_flag` 1); `f_relation` 0, so no team relation argument; the
team score counters 0 : 1 ... 1 : 6 (team 1 = player1). Screenshot evidence is still needed for the drawn form.

Finding: **`streak_flag` is 1 only on the first kill of the match** (tdm6 7 kills: the bot's first kill after its
respawn at 254.250 had 0, and the user's first kill at 246.609 had 0). This agrees with tdm4 (one kill), botduel3
(the killer never died: "first kill of a life" and "first of the match" coincide) and botduel4 ("only on the first
kill"). DSV-33's "1 on the killer's first kill since its spawn" is contradicted (staged). PLAUSIBLE meaning: a
"first kill of the round" banner form.

## 8. Weapon switches, reload, chat, purchase, grenades (task item 7)

- **Switches** (PROVEN-CAPTURE; each 0x7f directly after 0x76 [3][2] in the same block): besides the 24 spawn pairs
  (four per spawn), slot 2 [0][0] at 198.672, 285.656, 338.828 (knife); slot 0 [145][145] at 217.437, 292.218, 342.922
  and automatically in the release block of each last grenade (240.890, 326.593); slot 3 [7][9] at 229.437, 318.234;
  slot 4 [125][125] at 343.281. **Slot 1 (weapon 164) was never selected.** `aux` = the ammunition id in every case
  (WEAPON_FORMS §2 confirmed for 125 / 125).
- **Reload:** no 0x83 (the user never emptied a clip). UNDECIDED from the real client, as in tdm4.
- **Chat:** no 0x68 / 0xbe on the game link and no lobby chat frame on TCP: no team chat in this capture.
- **Purchase (context for the user's "could not buy the grenade pack"):** the only shop traffic was one 0x328 ->
  0x329 ok (136.203) and one 0x324 -> 0x325 equip of weapon item 125 (139.297). No request for a second item appears
  on the wire, so the level refusal was made by the client itself (PLAUSIBLE: the client checks the rank requirement
  locally). The capture-seed worker's account seeding addresses it.
- **Grenades** (T14): two throws: 0x80 [10][7] then 19 x 0x4d (239.468 - 240.687, 62-78 ms apart) and the release
  0x82 (weapon 7, ammo 9, 623 bits) at 240.890; 0x80 [10][7] then 7 x 0x4d and the release at 326.593. Both releases
  carried 0x76 [3][2] + 0x7f [0][145][145] in the same block (C2S §4.6 confirmed). The reference stack created the projectile
  (blob type 0x13, dims (6, 6, 8)) 32-47 ms after each release and sent 0x66 kind 1 [7][9][0.0][3][1] to the thrower;
  the second blast at 329.109 (2.52 s after the release, as tdm2 / tdm4) hit the bot: 0x74 `from_dir` magnitude
  0.4232, `unused` 7, `mode` 1, `ammo_kind` 0xFFFF, **`dmg_kind` 1**, `zone` 7, `part` 0, `damage` 42; health 57.68
  (no armour change). tdm4's self-hit by a grenade carried `dmg_kind` 0: the area form differs by victim (PLAUSIBLE:
  0 for the thrower himself, 1 for another player). The first blast hurt nobody.

## 9. The exit sequence and the result screen (task item 7)

The real client (PROVEN-CAPTURE): 0xbd and lobby 0x394 [0] in the same tick (356.500; lobby 0x3042 [1001][0][0] to
both accounts at once), `C2S_BYE` at 356.515; the reference stack's transport bye `0e a7 00 00 00 00` four times 356.547 -
356.843; the client's **plain bye** `c9 03` four times **366.343 - 366.640, 9.8 s after the server's bye** (tdm1
8.7 s, tdm4 7.1 s), in the same tick as its lobby 0x306 [1], 0x388 -> 0x389, 0x38a -> 0x38b (366.343 / 366.687);
later 0x306 [12] (371.515) and 0x304 -> 0x305 (372.703), then the TCP close (the user quit). The leaver got no 0x46,
0x65 or 0x3f, so the result screen was never shown to the real client (as tdm4).

The bot (REFERENCE, the forfeit end of S2C §2.1.3): 0x6c [1][0] + 0x46 [3][win 2][0][0] at 356.843 (+0.343 s after the
0xbd); 0x47 [0], 0x6b (bot record), 0x65 [pid 0][obj 252][exp_before 0][exp_gain 52][0][gp_gain 40][0][0], 0x48 [3]
at 359.578 (+2.735 s); lobby 0x3004 (exp 52, GP 10040, popup 1) + 0x345 to the bot at 359.547; 0x3f [0][2] at
362.578 (+3.000 s); the bot's `C2S_BYE` at 365.609 (BOT-16, 3.03 s), the reference stack's bye code 5. Identical to tdm4 in
order and within 32 ms in timing (tdm4: +0.344, +2.703, +3.031 s); `result_value` 2 = the winning team again.

## 10. Rust shadow lobby versus the reference stack (task item 8; `shadow_report.json`)

Summary: 98 expected server frames, 66 identical, 23 different, 9 missing, 6 extra, 0 errors. Shadow diffs are leads
(`REFERENCE_TESTING.md` §3: the client reacted to the reference stack, not to the shadow). Grade of the reference stack side: REFERENCE; the
shadow side is our server (`dc-lobby-server`, stack-verify build).

| id | frame (conn) | difference (payload offset: reference stack -> shadow) | reading | action |
|---|---|---|---|---|
| L1 | 0x2000 `server_list_push` (1, 3) | entry 0 and entry 1 name text (+0x1a, +0xda) differ; entry +0xa4 max grade 99 -> 0 (both entries); conn 3 entry 1 +0x94 current users 1 -> 0 | the server names are our own text; a max grade of 0 in our list | DeCombat value (OK-DIFF for the names); check that a max grade 0 does not hide the server from the client's grade filter (PROTOCOL_2009 §3.2 logs it only: PLAUSIBLE harmless) |
| L2 | 0x20b `server_pick_reply` (1, 3) | +0xe 16-byte session key | issued value | allowlist (varies by design) |
| L3 | 0x301 `lobby_login_reply` (2, 4) | +0x1c SYSTEMTIME milliseconds 0 -> real; +0x26 counter 0 -> 1; +0x42 / +0x46 list 3 / 4 versions 0 -> 1 | the counter 0 makes the client reset its price cache; versions 1 make it fetch lists 3 / 4 (0x35a / 0x35c, which `dc-lobby-server` handles) | allowlist the time; keep our versions (D) |
| L4 | 0x3f01 (2, 4), extra | our messenger relay push | DeCombat extension, ignored by the 2009 client (PROTOCOL_2009 §1.2, §6) | allowlist (extra by design) |
| L5 | 0x353 `list_reply_0` (2) | length 70 -> 30: count 20 -> 0 | our highlight list 0 is empty | DeCombat choice (shop presentation); no change needed |
| L6 | 0x303 `character_info_reply` (2, 4) | +0x7a combined rank 1 -> 0 | rank value of a fresh account | check the rank rule (CD rows); low priority |
| L7 | 0x323 `equipment_reply` (2, 4) | +0x12 slot 0 item 145 -> **144**; +0x8a seven function-slot items -1 -> **0** | different default primary (our default-issue rule, CD-18); empty function slots sent as 0 instead of -1 | send -1 for empty function slots (the client keeps `n % 9000`, and 0 is a real number there: PLAUSIBLE display risk); the primary differs by design |
| L8 | 0x321 `inventory_reply` (2, 4) | record 0 +4 item 145 -> 144; five records +0x10 FILETIME start 0 -> the purchase time | as L7; we fill the start time | allowlist the time; item follows L7 |
| L9 | 0x389 `room_info_reply` (2, 4) and 0x381 `room_list_reply` (4) | +0x80 kill-cam option 1 -> 0 (0x381 record +0x72 the same) | our room default for the third option is 0 | check the room-option default against the 0x38c the client sent (room creation); PLAUSIBLE our default |
| L10 | 0x30b, 0x30d `private_state_reply` / `_end_reply` (2), missing | the shadow logged "message 0x30a / 0x30c not handled by the core server" | **lobby gap**: the client sent 0x30a [1] (88.250) and 0x30c (160.109) around the shop visit | implement both replies (+6 u32 result; PROTOCOL_2009 §3.4 row 0x30b / 0x30d) |
| L11 | 0x329 `shop_buy_reply` (2) | +0x12 FILETIME end, 3 bytes | time-derived | allowlist |
| L12 | 0x325 `equip_reply`, 0x305 `logout_reply` (2) | frame length 22 -> 18 (reference stack body 0xe, ours 0xa) | both satisfy ">= 0xa" | none (allowlist the length) |
| L13 | 0x3014 `inbox_stale_push` (2), missing | the reference stack pushed it at the HiSec pickup (251.578) | the shadow is not told about game events | expected in shadow mode; in the Rust-primary stack this belongs to HS-6 |
| L14 | 0x3035 type 5 (players) (2), extra at the bot's join | the shadow pushes the player count; the reference stack did not | PROTOCOL_2009 type 5 is a valid field | none (OK-DIFF) |
| L15 | the bot leaving (2, 4) | reference stack at 365.609: 0x3042 [1002][0] to both, 0x3035 type 7 [0] to both, then 0x3034 [1002] (366.265); shadow at the bot's TCP close: 0x3034, 0x3035 type 5 [1], 0x3035 type 7 [0], to the user only; no 0x3042 | the reference stack reacted to its game server's end report, the shadow only to the TCP close | expected in shadow mode (no game report); in Rust-primary mode check that 0x3042 {id, 0} reaches the room before 0x3034 |
| L16 | 0x3004 + 0x345 to the bot (4), missing | the reference stack pushed the bot's progress at 359.547 | the shadow got no match result | expected in shadow mode |

## 11. Proposed spec changes (for the spec-fold worker)

- **C2S §4.1 / §4.1.1:** "`extra_kind` = the index of the skeleton node the client's ray hit (PROVEN-CAPTURE tdm6: 53
  for a head hit, 6 for a chest hit; the ROM `[Skeleton<s>] Node<k>Location` of the victim's body row gives its
  location)". Add the melee form (2.3) and the multi-pellet form (section 3) and remove "multi-pellet UNDECIDED" /
  "melee UNDECIDED".
- **C2S §3.3 / §3.4, S2C §4.1.1:** the timing line "no capture contains a death of the real client" becomes "tdm6: five
  deaths of the real client, one 0x89 each 7.015-7.031 s after the datagram with 0x67 state 2 (PROVEN-CAPTURE), no
  arming message; the reference stack answered 16-63 ms later".
- **S2C §3.3.2:** `streak_flag` 1 only on the match's first kill; `hit_zone` = the node location (1 for the tdm6 head
  shot).
- **S2C §4.3.1:** `dmg_kind` = the location of the named node (1 head, 2 torso, 3 arms) for a weapon hit, 1 for a blast
  on another player, 0 for a self-blast or a fall; `damage` is the hit's damage before clamping to the remaining
  health; `|from_dir|` = min(1, damage / 100).
- **S2C §8.2 (type 0x06, reference stack note):** the reference stack stops relaying effect records after the shooter's granted rounds are
  spent; an applied hit's first record carries the claimed hit point and normal, not a point of the reference stack's own trace.
- **HISEC §3 (reference stack note, not a rule):** section 6 findings 1-3.
- **bot-shooter_REFERENCE_FINDINGS F1:** reworded as in 5.3 item 4.

## 12. Not decoded, UNPROVEN, follow-ups

- **UNPROVEN:** the reference stack's damage acceptance rule (5.3 item 3); the pickup kind of the type-0x17 blobs (6 item 4);
  which body row / skeleton the reference stack used for the bot (2.1 item 1); the meaning of the sound id in the death-time
  `C2S_RATE`; how the 2009 client drew the kill feed and the damage indicator (screenshots needed).
- **Not exercised:** a leg hit, a melee hit, a reload by the real client, a pellet hit, team chat, the real client on
  the result screen (it left first), a death of the real client inside its own 7 s wait at a round end.
- **Capture to take next (V-WALL-1):** the Rust-primary stack (dc-dserver traces the reported ray, DSV-34) with a bot
  that has the ROM world loaded and aims at the decoded target; the user stands behind the x = 493.9 wall of this world
  near the bot's first spawn: no hit should be applied by DeCombat, and the bot should not fire (its own line of sight).
  For the reference stack question, one more reference stack capture with a bot that walks while firing at a fixed point would
  separate the remaining hypotheses.
