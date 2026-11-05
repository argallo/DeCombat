# FIRETEAM_AI_DESIGN: server-side AI for the co-op Fireteam mode (mode class 5)

- **Scope:** how `dc-dserver` runs the co-op mode the 2009 client knows as mode class 5: what an AI character is on
  the wire, the AI inside the server (navigation, perception, behaviour, combat, waves, missions, difficulty,
  scoring), determinism and cost, the modding surface, the DeCombat rows FT-1..FT-22 and an implementation plan in
  waves with tests. Pure design: no crate is changed by this document.
- **Builds on:** `specs/GAME_MODES_2009.md` §1-§6 (cited `GM-MODES §n`), `GAME_MESSAGES_2009_S2C.md` (`S2C §n`),
  `GAME_MESSAGES_2009_C2S.md` (`C2S §n`), `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md` §1.5-§1.7 (`TDM2 §n`),
  `SESSION_B_S2C_addendum_dserver3.md` S3-2..S3-6 (`S3-n`), `CAPTURE_tdm4_KILL_VALIDATION.md` (`TDM4`),
  `MOVEMENT_2009.md`, `FMT_DAT85.md`, `rust/HOSTING_AND_MODS.md` §2-§3, `docs/MODDING_GUIDE.md`, the proposed
  rows GMD-15..GMD-18 of `DC_DEFAULTS.md`, and the crates `dc-dserver` (`chars.rs`, `modelobj.rs`, `stream.rs`,
  `game.rs`, `game/{combat,relay,round}.rs`, `gs.rs`, `up.rs`), `dc-sim`, `dc-world`, `dc-bute`, `dc-mod-lua`,
  `dc-botclient`. The sibling design `docs/VISIBILITY_CULLING_DESIGN.md` (written concurrently in Wave P, not read
  here) owns the line-of-sight service this design consumes (section 4.3).
- **Evidence of this document (new):** clean decompiler output `CShell.dll/{205f40,206260,8a420,
  8a760,b8870,c8030,1e4760,1e0d80,3a5f00}.c`; read-only capstone disassembly of the user's `Game/CShell.dll` at
  cshell+0x205fa0..0x2060ca and cshell+0x206548..0x206569 and one PE-DATA float (VA 0x374d08d8) (scratch scripts
  outside the tree, grade **DISASM**); scratch programs over `dc-world`, `dc-bute`, `dc-rez` and `dc-sim` that read
  the user's ROM read-only and printed counts, key names, timings and graph shapes (grade **OBSERVED: ROM table key**
  / **MEASURED**). No ROM value, ROM text, display name or world file name is reproduced; world classes and properties
  are named by `sha256(name)[:16]/length` as in `FMT_DAT85.md` §6.2. The scratch programs were never committed.
- **Status:** DESIGN DRAFT 1. No Fireteam capture exists (section 2.6). Rows FT-n are PROPOSED.
- **Spec-fold-7 note (2026-10-25):** the clean spec `specs/FIRETEAM_2009.md` re-derived F1-F6 under the two-person rule; where
  the spec FIRETEAM_2009 overturned this document the place carries a note "superseded by FIRETEAM_2009 section x" and the
  old text is kept. Implementers follow the spec where the two differ. Record: section 10.
- **Date:** 2026-10-25.

Grades as in the specs: **PROVEN** (decompiler output), **DISASM**, **PLAUSIBLE**, **UNPROVEN**; plus **MEASURED** (our own
scratch measurement on this machine) and **DESIGN** (our choice, a DeCombat default, FT-n).

## 0. Contents

1. New findings this design rests on
2. An AI character on the wire (task item 1), capture items V-FT-n
3. Data the server reads from the ROM for co-op
4. AI architecture in `dc-dserver` (task item 2)
5. Determinism and performance budget (task item 3)
6. Modding surface (task item 4)
7. DeCombat rows FT-1..FT-22 (task item 5)
8. Implementation plan in waves with tests (task item 6)
9. Open questions and risks

## 1. New findings this design rests on

| # | finding | evidence | grade |
|---|---|---|---|
| F1 | The optional tail of the type-0x18 character record (S3-2 row "if `has_npc_block`") lands in the character object at: rec+0x28 -> object +0x60 (third argument of the marker add), rec+0x98 -> +0xd0 (marker team), rec+0x9c -> +0xd4, rec+0xa0 -> +0xd8, the name -> +0xb4. The init takes the NPC branch when object +0x40 (`roster_bound`) is 0 (or the roster check fails) and +0x44 (`has_npc_block`) is set. | cshell+0x205f40 (object = record + 0x38, S3-2) | PROVEN |
| F2 | rec+0x9c is a **float fraction**: the NPC branch loads object +0xd4 as an f32, multiplies by the PE-DATA constant at VA 0x374d08d8 (= 100.0) and stores the rounded result as a 16-bit value at object +0x5ae. **Superseded by FIRETEAM_2009 section 1.3 step 3 (D2):** the helper cshell+0x3a5f00 truncates toward zero (C cast), it does not round; 0.29 shows 28, so a server sends `(percent + 0.5) / 100` (FT-W1). | DISASM cshell+0x2060b9..0x2060ca; constant read from the user's CShell.dll | DISASM |
| F3 | Shell 0xa6 `[0x18][obj][sub 0x24][v:f32]` updates the same field: only for an object with `has_npc_block` set, `round(v x 100)` -> object +0x5ae. So the server can change an NPC's percent value after the create. The reader of +0x5ae was not found by a text search of the clean tree (likely a virtual getter): that it is the fill of the NPC health bar of GM-MODES §6.2 is PLAUSIBLE, not proven. | cshell+0x206260 case 0x24; DISASM cshell+0x206548..0x206569 (`fld dword`, `fmul [0x374d08d8]`, float-to-int helper cshell+0x3a5f00) | DISASM (layout), PLAUSIBLE (meaning) |
| F4 | rec+0xa0 (object +0xd8) is a **kind byte** of the NPC: value 2 changes the radar entry kind from 6 to 8 and the overview-map entry kind from 1 to 3; value 4 sets the infected flag (+0x5bc) and, for the own view object, calls the infected-view helper. Any other value: radar kind 6, map kind 1. PLAUSIBLE: 2 = boss (GM-MODES §6.2 lists co-op radar icons for a boss; V-GM-7). **Superseded in part by FIRETEAM_2009 sections 1.3 step 4 and 1.5 (D1):** the kind is also read every frame by the marker update and gates the NPC health bar: only kinds 1 and 2 show it, 0 (and 4) hide it; a normal AI therefore gets kind 1, not 0 (FT-K1). | cshell+0x205f40 (the two `== 2` / `== 4` tests), radar add cshell+0xb8870 (4th argument = entry kind), map add cshell+0xc8030 (7th argument) | PROVEN (control flow), PLAUSIBLE (boss) |
| F5 | The client's character model table holds the `[Model<n>]` rows first and then the `[AIModel<n>]` rows of the ROM model table in one array (record size 0x49c, total count at table +0x15b4, plain count at +0x15ac). **Superseded in part by FIRETEAM_2009 section 1.8 (D3):** a third group of `[Overidable_Model<n>]` rows follows the AI rows (count at +0x15b8); +0x15b4 is plain + AI, not the end of the table. The skeleton lookup cshell+0x1e0d80 used for rec+0x24 answers 0xff for every index at or above the plain count. PLAUSIBLE: rec+0x14 is a row of this combined table, so an AI body row would be `plain_count + ai_row`; the reference stack sent 0 for players (S3-2). Not needed for rendering (the create's model block names the files, S3-1), so DeCombat sends 0 until V-FT-3 says otherwise. | cshell+0x1e4760 (row loops, `AIModel` rows indexed after `+0x15ac`), cshell+0x1e0d80 | PROVEN (table), PLAUSIBLE (rec+0x14 use) |
| F6 | In class 5 the client arms its own respawn wait only while the own roster record's +0x1f8 is above 0, and 0xd7 sub 2 `[a][b]` writes +0x1f8 := `b` (and re-arms when `a == b`). PLAUSIBLE: `b` = lives left; matches the ROM per-difficulty keys `LifeCount` / `MaxLifeCount` (section 3.2). **Superseded in part by FIRETEAM_2009 sections 2.1 and 2.2 (D4, D5; S5):** the re-arm also needs the own player state 2, 3, 5 or 6 (to a living player sub 2 only sets the count); 0x6b field 21 `m1` writes the same field +0x1f8, so lives need not travel only in 0xd7; `b` counts the current life (the client shows `b - 1` with CRes string 0x5a0). | TDM4 B1/B2 (DISASM cshell+0x60ac0), S2C §3.5 row 0xd7 | DISASM, PLAUSIBLE |
| F7 | Both ROM maps that carry the co-op key (map rows 13 and 18 of the ROM map table, GM-MODES §2 "2 maps") ship world files with AI objects (section 3.1): navigation volumes, regions, patrol / guard / use-object nodes, placed AI characters (most marked as templates), spawners and a trigger network. No other ROM world has any of these classes. | scratch program over `dc-world` (`class_names()`), 21 worlds | OBSERVED (counts) |
| F8 | The co-op `.GMS` files carry per-difficulty rows `Object0..2`, and the wave map's file carries spawn tables per round, AI health rates per round and difficulty, weapon drops per round, a co-op score table and an NPC name table (section 3.2). `ATTRIBUTES.rez` carries AI tables: brains, attribute templates, stimuli, goal sets, AI body rows and NPC weapon rows (section 3.3). | scratch program over `dc-bute` (`gms_file_with_vfs`, `ButeDoc`) | OBSERVED (keys) |
| F9 | The ROM navigation volumes form a usable graph: wave map 106 volumes, one component of 105 (the other is outside the play space); mission map 904 volumes, two large components (273 and 569, together 93 %) plus 22 small islands (elevated vantage points). Adjacency = axis-aligned boxes (`Dims` read as half extents) overlapping or touching within 2..24 units; building it took 0.02 ms / 0.5 ms. | scratch program over `dc-world` + `dc-sim` (release, i686, this machine) | MEASURED |
| F10 | `dc-sim` cost on the co-op worlds: 7.5-10.4 us per body step (64 running bodies, 300 steps of 1/30 s), 23-26 us per BSP ray (`CollisionWorld::ray`, eye-height rays between volume centres; 90 % blocked); collision world build 6 / 23 ms. | same scratch program | MEASURED |
| F11 | Nothing in the workspace runs co-op today: `dc_bute::fireteam_refusal(clan, mode)` refuses plain mode row 2 (the co-op row) with a message, `dc-dserver` has no class-5 code (`gs.rs` writes the 0x6d co-op values as 0), and `dc-mod-lua` accepts only the hook namespaces `lobby`, `combat`, `render`, `hud` (`manifest.rs` `HOOK_NAMESPACES`), so the hook names `coop.*` of GMD-15..18 would be refused. The content domains `ai_brains` and `missions` already exist (`MODDING_GUIDE.md` §4). | crate sources | PROVEN (our code) |
| F12 | No reference stack exists for co-op: the reference stack refuses Fireteam rooms (the refusal that `fireteam_refusal` ports from the harness expectations) and has no AI, and no 2009 Fireteam capture exists. Everything server-side in this document is DeCombat design; only the client's acceptance can be tested. | `dc-bute/src/lib.rs` `fireteam_refusal`; task brief | as stated |

## 2. An AI character on the wire (task item 1)

### 2.1 Create form

An AI is a character world object exactly like a remote player (TDM2 §1.5, S3-1..S3-5): a kind-1 model object with
the type-0x18 blob, a model block, an animation block of three trackers, the flag block, the compact position, a
rotation for viewers, the held weapon as a second kind-1 object attached at socket 2, and `CF_DIMS`. The only
differences are inside the blob (FT-1):

| blob field (S3-2 / `modelobj::CharacterBlob`) | player (reference stack form) | AI (DeCombat) | why |
|---|---|---|---|
| `roster_bound` (rec+0x08) | 1 | **0** | an AI has no 0x6b roster record; the init then takes the NPC branch (F1) |
| `has_npc_block` (rec+0x0c) | 0 | **1** | the NPC branch adds the marker by itself (GM-MODES §6.2) |
| `pid` (rec+0x34) | engine client id | 0xff | no client id; 0xff is outside the roster range (S2C §3.1 note: ids below 256) and is never looked up on this branch (UNPROVEN, V-FT-1) |
| `rec_2c`, `rec_2d` | (1, 255) before the first spawn, (3, 2) after | (3, 2) at the create | an AI is created when it spawns; no pre-spawn form |
| `dims` (rec+0x58) | the player's half extents | the AI body's half extents (section 3.3: the placed object's `Dims` property, else the body model's stand box as for players) | same use |
| `infected` (rec+0xa4) | 0 | 0 (1 only for the infected kind, F4) | |
| tail `rec_28` (+0x60) | absent | 0 | passed to the marker add; meaning UNPROVEN (V-FT-2) |
| tail `team` (+0xd0) | absent | **2** (the AI side; humans are team 1, GMD-15) | the health bar is drawn only for marker entries whose team differs from the own team (cshell+0x8a420, GM-MODES §6.2) |
| tail `rec_9c` (+0xd4) | absent | **f32 bits of the health fraction, 1.0 at spawn** | F2 |
| tail `rec_a0` (+0xd8) | absent | 0 normal, **2 boss** (FT-14 boss rule), 4 never in co-op. **Superseded by FIRETEAM_2009 section 1.5 / 1.9 (D1):** 1 normal (0 shows no health bar), 2 boss (FT-K1) | F4 |
| tail `name` (zstr8, at most 256 bytes) | absent | the marker caption: the ROM co-op NPC name for bosses, the goal-set caption for others, else empty (FT-1) | the init converts it to a wide string for the marker (cshell+0x205f40 -> cshell+0x8a760) |

`modelobj::NpcBlock` already writes this tail (`rec_9c` is typed `u32`; the AI writer passes `f32::to_bits`).
Model block: the AI body row of section 3.3 resolved through the same asset path as players (`chars.rs`
`character_assets`, with an `[AIModel<n>]` row instead of `[Model<n>]`), the AI face row when the row names one,
and the NPC weapon row's held model at socket 2 (S3-3; the socket key is the one players use).

Flags: the spawned form 0x3803 at once (S3-4: visible), `flags2` / `uflags` as for players; `FLAG_YAW_ONLY` set so
the stream carries `yaw8` (S3-5). An AI is never anybody's view object, so every viewer gets `CF_ROT_SNAP` in the
create and no viewer is the owner.

### 2.2 Stream, animation, removal

- **Per tick:** one stream entry per AI per viewer, `uf` = `UF_POS | UF_YAW | UF_ANIM` like remote players
  (`stream.rs` `StreamEntry`), velocity from the dc-sim body, and the three trackers of DSV-28 computed from the AI's
  own control word (stand / walk pairs by name key in the AI rig). Run, crouch, death and fire clips are the same
  open item as for players (`TODO_DIRECTIONAL_ANIMS`); FT-21.
- **Visibility culling:** AIs pass through the same per-viewer filter as players once the visibility design lands
  (an AI a player cannot see costs no stream bits).
- **Death:** no 0x6d (F11; GM-MODES §6.2: 0x6d needs roster ids). The object is frozen in place for the corpse time
  (FT-4), then removed with the removal record of `S2C_OBJ_GROUP` (`stream::removal_group`) together with its weapon
  object. A death clip is used once the rig lookup finds one (FT-21). **Superseded by FIRETEAM_2009 section 1.7 (D7):** no client
  path removes an AI's marker, radar or map entry (only 0x6c / 0x65 for roster pids), so the entries would keep the handle of a
  deleted object; hide and reuse AI objects instead (FT-R1) until V-FT-6 shows removal is safe.
- **Damage taken by an AI:** 0xa6 `[0x18][obj][0x24][fraction:f32]` to every in-world client, coalesced to at most
  one per AI per 100 ms (FT-3). Hit effects (blood, impact) reuse the effect relays the slice already sends for
  players (type 0x06 shot effects, S3 relays).
- **0xa6 notes not used:** sub 0x11 (record refresh) is needed only if a field of the blob changes (DeCombat does not
  change any after the create); sub 0x1f / 0x25 (spawn blink, `roster_bound` only, TDM2 §1.7) do nothing for an NPC.

### 2.3 Hits, kills and points

- **Players hitting an AI:** the client's `UP_FIRE` carries `hit: (dist, hit_obj)` (`up.rs` `UpFire::hit`). The
  server resolves `hit_obj` to an AI the same way it resolves a player object today (`game/combat.rs on_fire`):
  validate range and line (DSV-29), apply damage with the ROM weapon row of the shooter, the hit zone and the
  difficulty's health scale (FT-12). That the 2009 client reports an AI object as `hit_obj` is PLAUSIBLE (the AI is a
  solid model object like a player); V-FT-4.
- **An AI killed:** points through 0xd7 only (GM-MODES §6.1): **sub 1** `[pid:u32][obj:objref16][x:u32][points:u32]`
  to every client, with `pid` the killer, `obj` the dead AI, `x` the points just gained (the own player's popup,
  cshell+0x8ad40(x)) and `points` the killer's new total (record +0x1f4 := `points`, an absolute value); **sub 3**
  `[pid][x:u8][points]` for points without an object (damage points, mission goals). FT-13.
- **A player killed by an AI:** 0x67 state 3 / 2 as today, then 0x6d with `killer` = 0xff (no killer: the client
  counts only the victim's death, S2C §3.3.1 step 6) and `weapon` = the AI's weapon number for the kill-feed icon;
  `coop_a` / `coop_b` = 0 until V-FT-6. Then 0xd7 sub 2 `[lives][lives]` with the victim's remaining lives (F6): the
  client arms its 7000 ms wait only when `lives` > 0 and then asks with 0x89 (TDM4 B2); with 0 lives it stays an
  observer. FT-5. **Superseded by FIRETEAM_2009 sections 1.7 and 2.3 (D6, D8):** the client reads +0x1f8 when it
  processes 0x67 state 2, so the new count must be sent **before** the death message (FT-L1), not after the kill; and in
  class 5 the kill-feed add returns at once, so the weapon field has no visible effect (0x6d still counts the death).
- **Damage to a player from an AI:** the same messages the slice sends for player damage (0xa6 sub 0x20 to the
  victim's character, 0x66 health, S3 / TDM2 §2), with `from_dir` from the AI's muzzle.

### 2.4 Mission and round messages (class 5)

The sequence of GM-MODES §6.3, made concrete (FT-15, FT-16):

1. Start-up as DSV-22 with `0x9e [5]`; players on team 1 (spawn points whose game-type filter names the co-op type,
   GAME_MODES row filter as DSV-16).
2. 0x45 with the co-op caption and the co-op tail `[x:u8][y:u8][z:bool1]` (meanings UNPROVEN, V-FT-9; DeCombat sends
   the difficulty index, 0, 0).
3. 0xc9 sub 1 at mission start: the goal list from the difficulty row's `MissionList<k>` texts (section 3.2) or one
   generated goal per wave, one timer = the difficulty's `TimeLimit`; 0xd7 sub 4 `[1][secs]` shows the mission timer
   on the round timer element.
4. 0xc9 sub 5 op 3 when a goal completes, op 1 to add a goal; sub 2 banner text at a wave or stage start.
5. 0xd7 sub 2 lives at every spawn (also the first, so the client's record starts with the right value).
6. End: 0x46 with the co-op block (`stage` 2 success, 1 failure; `c1..c6` per FT-13), 0x65 per player with the
   co-op block, 0x3f, 0x99.

### 2.5 Wire summary

| item | message | status in `dc-dserver` | grade of the client side |
|---|---|---|---|
| AI create | kind-1 model object + type-0x18 blob with NPC tail | blob writer exists (`modelobj::NpcBlock`); creates for AIs new | PROVEN layout (S3-2), F1-F4 |
| AI motion | `S2C_OBJ_STREAM` entry with `yaw8` + 3 trackers | exists for players | REFERENCE-accepted for players (TDM2 §1.6) |
| AI health | 0xa6 [0x18] sub 0x24 [f32] | new | DISASM (F3) |
| AI removal | `S2C_OBJ_GROUP` removal record | exists | PROVEN (B §5.1) |
| AI kill points | 0xd7 sub 1 / sub 3 | new | DISASM (S2C §3.5) |
| lives | 0xd7 sub 2 | new | DISASM (F6) |
| player killed by AI | 0x6d killer 0xff | exists (no-killer form) | PROVEN |
| mission | 0xc9 subs 1, 2, 4, 5, 8; 0xd7 sub 4 | new | PROVEN (S2C §6.2) |
| result | 0x46 / 0x65 co-op blocks | layouts exist with `mode_class` switch (`gs::round_result`, `gs::player_summary`) | PROVEN |

### 2.6 Capture items V-FT-n and the plan to get them

There is no 2009 Fireteam capture and none can be made against the reference stack (F12). Captures therefore prove only
that the 2009 client **accepts and displays** what DeCombat sends. Plan: once wave FT-W3 (section 8) runs, the
coordinator asks the user for one session (GO protocol of the user playtest notes): the user plays the real 2009
client against `dc-lobby-server` + `dc-dserver --coop-test` (a fixed test room: one AI of each kind standing at
known places, a scripted sequence of server messages on chat commands), recorded with `evidence/captap.py tap`;
the user takes a screenshot at each step list step. No agent launches the game client. Expected length 15-20
minutes. Items:

| id | what to look at / record | settles |
|---|---|---|
| V-FT-1 | the AI appears, is animated, has a marker with the caption; no client error with `pid` 0xff | 2.1 create form |
| V-FT-2 | marker look for `rec_28` 0 vs 1 | F1 rec_28 meaning |
| V-FT-3 | NPC kind 0 vs 2 (radar / map icon) vs 4 (infected look); `rec_14` = 0 vs `plain_count + ai_row` | F4, F5 |
| V-FT-4 | shooting an AI: `UP_FIRE` names the AI object in `hit_obj`; head / body zones | 2.3 hits |
| V-FT-5 | health bar on an AI after sub 0x24 with 1.0, 0.5, 0.1 | F3 meaning (health fill) |
| V-FT-6 | 0xd7 sub 1 popup and co-op score panel; 0x6d `coop_a` / `coop_b` effect on the panel | FT-13 field use |
| V-FT-7 | 0xd7 sub 2 with lives 2, 1, 0: respawn countdown, target-name element (`b - 1`) | F6 |
| V-FT-8 | 0xc9 sub 1 goal list, sub 5 state change, sub 2 banner, sub 8 scoreboard values | 2.4 |
| V-FT-9 | 0x45 co-op tail `x`, `y`, `z` | 2.4 step 2 |
| V-FT-10 | the 0x46 co-op result panel for `stage` 1 and 2 with distinct `c1..c6`; 0x65 co-op block | FT-13, FT-15 |
| V-FT-11 | AI corpse frozen 3 s then removed: does the body vanish cleanly | FT-4 |
| V-FT-12 | 0x99 after a co-op mission: does the client return to the room | 2.4 step 6 |

## 3. Data the server reads from the ROM for co-op

All of this is read at run time from the user's ROM (`dc-rez`, `dc-world`, `dc-bute`); nothing is copied into the
tree. Key names are cited as `OBSERVED: ROM table key`; world classes and properties by hash.

### 3.1 World objects of the two co-op worlds (F7)

| our role (proposed `dc_world::ObjectRole`) | class hash / length | geometry | wave map | mission map | properties the AI uses (hash / length) |
|---|---|---|---|---|---|
| `NavVolume` | `da1d7de44f7660e9`/8 | Box (centre + half extents) | 106 | 904 | `Dims` `2d7b3d0c6d75ff11`/4, `Region` `d3a008ef1335692d`/6 (name of a region object), `PreferredPath` `f3713901cc08c9fc`/13 (bool) |
| `NavRegion` | `b7321ac39fb70c9e`/8 | Point | 1 | 4 | name (referenced by volumes) |
| `PatrolNode` | `5d075fec0fbec2ea`/12 | Point | 8 | 183 | `Next` `1ff57a29d7c9d11b`/4 (name of the next node: a chain), `Action` `64cff1319d2fd2cb`/6 (two values seen) |
| `WatchPost` | `43c6e371089f5553`/11 | Point | 0 | 230 | `ReturnRadius` `2adf3cdfaf32469c`/12, `Node1..16` (names of use-object nodes) |
| `UseNode` (vantage / firing position) | `e07d42a5d27238d6`/15 | Point | 0 | 621 | `Radius` `6fe0661c82d10d34`/6, `Movement` `49ba63e89af69e4d`/8, `SmartObject` `3ead647c816eaeaf`/11 (two posture values seen: standing and crouched firing spots) |
| `AiCharacter` | `a58c4bb4deed79bb`/8 | Point + `Dims` | 12 (8 templates) | 266 (260 templates) | `Template` `0575f29df888a27e`/8, `ModelTemplate` `f406ea5ca130ffb5`/13 (an `[AIModel<n>]` / attribute template name), `Brain` `7e4d5c460c04457e`/5, `GoalSet` `f8a41698d9c7b8fd`/7, `Initial` `1f397413e4399207`/7 (a start instruction naming a patrol or guard node), `RightHand` `c969c8aa86333373`/9 / `LeftHand` `83665722ff70bf1a`/8 (NPC weapon / thrown-item row names), `MaxHitPoints` `b6973b50181c00cf`/12, `NPCType` `3ea56cf557ea64d9`/7 |
| `SpawnSite` | `89bb4efe4151b8b5`/7 | Point | 40 | 260 | name (referenced by spawn tables), `Target` `978354db0c00fc78`/6 (name of a template character) |
| trigger network | `6c64a55461d65e44`/12, `6d267057c5c0d640`/13, `4c7eb79939e993f1`/12, `6d0d76d5a79188c0`/14, `45c6a1b82cfa6603`/14, `9062c0280952f80f`/14, `a6c4f47d4fded3e2`/9 | Point / Box | 7 objects | 19 objects | command text properties (a small command language; section 4.7) |

The wave map's AI characters are all templates referenced by its spawn tables; the mission map's placed characters
are mostly templates that its spawners create when the trigger network says so, and six are live at load. The
player spawn points of both worlds carry the co-op game-type filter (the same class as `ObjectRole::PlayerSpawn`).

### 3.2 GMS rows of the co-op maps (F8)

`dc_bute::gms_file_with_vfs(vfs, rooms, map, mode = 2, clan = false)` returns the document. Sections and keys
(OBSERVED: ROM table key):

| section | keys | used for |
|---|---|---|
| `[MapInfo]` | `Map` | world file (as `world_for_room` already does) |
| `[Mission0]` | `StartTrigger`, `Controller`, `MissionMode` | mission start trigger name (mission map) |
| `[Object0]`, `[Object1]`, `[Object2]` | `TimeLimit`, `LifeCount`, `MaxLifeCount`, `AIDamageFactor`, `ScoreFactor`, `StartTrigger`, `EndTrigger`, `OffenceTeam`, `DefenceTeam`, `MissionList<k>`, radar / map picture keys, (wave map only) `RecoverRoundTotalHP`, `ShowCoopResultDelayTime` | one row per difficulty: room goal 0..2 = row index (FT-12) |
| `[AISpawnTable<n>]` (wave map, 20 rows) | `Round`, `Spawner<k>`, `LimitMaxAliveNum`, `SpawnAIName<k>`, `SpawnDelay<k>`, `SpawnNum<k>` | the waves (FT-10) |
| `[AIHealthRateRoundTable<d>]` (wave map, d = 0..2) | `Round<n>` | AI health scale per round and difficulty (FT-12) |
| `[WeaponSpawn<n>]` (wave map) | `Round`, `WeaponID`, `LifeTime`, `Position` | weapon pickups (type 0x17 kind 6) placed at named markers in given rounds (FT-10) |
| `[CooperativeModeScoreTable]` | `DamageScoreType_A..D`, `DamagePlusRateByBossDamage`, `DamagePlusRateByTBossDamage`, `DamagePlusByBossDeath`, `DamagePlusByTBossDeath`, `DamagePlusByEtcDeath`, `DamagePlusByHeadShotDeath`, `DamagePlusByCombo` | points (FT-13) |
| `[CooperativeModeNPCNameTable]` | `NPCNameBoss`, `NPCNameTriggerBoss<k>`, `NPCNameETC<k>`, `NPCGoalSetPrefix<k>` | boss identification and marker captions (FT-1, FT-14) |
| `[CooperativeModeDialogMsg]` | `Msg<nnn>` | banner texts (0xc9 sub 2) |
| `[MapExp]`, `[MapMoney]`, `[ScoreTable]` | as for every mode | experience / GP (CD-03 / CD-04) |

The key names suggest meanings; DeCombat **defines** what each value does (FT rows). A key a map lacks falls back to
the DeCombat default of its row. The small table `ATTRIBUTES\COOPMISSIONS.TXT` (one `[Mission0]` row) is not used by
this design (its role is unknown).

### 3.3 AI tables in `ATTRIBUTES.rez` (F8)

| table | sections / keys (OBSERVED: ROM table key) | used for |
|---|---|---|
| `MODELBUTES.TXT` | `[AIModel<n>]`: `Name`, `ModelFile`, `Skin0`, `Skin1`, `RenderStyle0..1`, `AIName`, `AIFace`, `EnemyClassMark`, `Mass`, `Armor`; `[AIFace<n>]` rows | AI body and face models for the create's model block (2.1) |
| `AIBUTES.TXT` | `[Brain<n>]` (19 rows): `Name`, `AttackRangedRange`, `AttackThrownRange`, `AttackMeleeRange`, `AttackChaseTime`, `AttackGrenadeThrowTime`, `AttackGrenadeThrowChance`, `AttackCoverCheckTime`, `AttackCoverCheckChance`, `AttackFromCoverCoverTime`, `AttackFromCoverUncoverTime`, `AttackWhileMoving`, `ChaseExtraTime`, ...; `[AttributeTemplate<n>]` (12 rows): `Name`, `RunSpeed`, `WalkSpeed`, `HitPoints`, `Armor`, `Accuracy`, `FullAccuracyRadius`, `AccuracyMissPerturb`, `Marksmanship`, `SeeEnemyDistance`, `HearEnemyWeaponFireDistance`, `HearEnemyFootstepDistance`, `HearAllyDeathDistance`, `Alignment`, ...; `[Stimulus<n>]`, `[SenseMask<n>]`, `[DamageMask<n>]` | behaviour timings (4.4), body speed, health, aim error and sense ranges (4.3, 4.5) |
| `AIGOALS.TXT` | `[GoalSet<n>]` (8 rows) listing goals; `[Goal<n>]` rows: `Name`, `Importance`, `DecayTime`, `UpdateRate`, `SenseTrigger<k>` | which behaviours an AI may run and their base priority (4.4) |
| `WEAPONS.TXT` | 32 weapon rows whose names carry the NPC suffix (the rows the AI hand properties name) | AI weapon damage, range, fire interval, held model (4.5) |

The AI character's `ModelTemplate` names both an `[AIModel<n>]` row (by `Name`) and an `[AttributeTemplate<n>]` row
(by `Name`); its `Brain` names a `[Brain<n>]` row; its `GoalSet` a `[GoalSet<n>]` row (all matched ASCII
case-insensitively, OBSERVED on both worlds). Attribute values that the placed object overrides (its own text
properties, empty in every ROM object) win over the template.

## 4. AI architecture in `dc-dserver` (task item 2)

### 4.1 Module layout

A new module tree beside `game/`, sans-IO and deterministic like the session (`game.rs` contract: messages and time
in, `GameOut` out):

```
dc-dserver/src/coop/
  mod.rs        CoopRules: class-5 flow (2.4), lives, end conditions; owns Director, Actors, Mission
  data.rs       CoopData: ROM reads of 3.2 / 3.3 into plain structs (one per room, Arc-shared per map)
  actor.rs      Actor: one AI (body PlayerState, health, weapon state, blackboard, behaviour state)
  nav.rs        NavGraph: volumes -> nodes and edges, A* with cached paths; fallback grid (4.2)
  sense.rs      Senses: sight / hearing / damage memory per actor (4.3), uses the SightService of the
                visibility design
  think.rs      utility selection and the behaviour primitives (4.4)
  aim.rs        shooting and grenades (4.5)
  director.rs   waves from the spawn tables, synthetic waves, weapon drops (4.6)
  mission.rs    goals, stages, the trigger-network subset (4.7)
  score.rs      points and the result blocks (4.9)
  wire.rs       create records, stream entries, 0xa6 / 0xd7 / 0xc9 builders (uses modelobj / stream / gs)
```

`Game` gains a `coop: Option<CoopRules>` built in `set_room` when `mode_class == 5` and co-op is enabled (FT-20).
`dc-dserver` gains the dependency `dc-sim` (feature `world`), already used by `dc-botclient` and `dc-client`; the
world is parsed once per map into an `Arc<CollisionWorld>` and an `Arc<NavGraph>` shared by all rooms on that map
(HOSTING_AND_MODS §2 "world data shared read-only").

### 4.2 Navigation

> Slice 1 as built (2026-11-05, DC_DEFAULTS FT-S11): the AI use the bots' navigation grid (`dc-nav`, the lattice of
> 4 below, grown on demand between each AI and its target) over the level model, not the ROM volumes yet; the
> blocker polygons are player clips and do not stop them. The volumes of 1-3 stay the plan for slice 2.

**Source: the ROM navigation volumes when the world has them** (both co-op worlds do, F9); a baked grid otherwise
(synthetic worlds, other ROM maps used by a mod for co-op, mod maps).

1. **Nodes.** Each volume is a node: an axis-aligned walkable slab (centre, half extents), its region name and the
   preferred-path flag. Patrol, guard and use nodes are attached to the volume that contains them (or the nearest
   within 64 units).
2. **Edges (load time).** Two volumes are linked when their boxes overlap or touch within `nav.link_gap` (8 units,
   FT-6) on every axis. The portal of the edge is the overlap rectangle of the two boxes in x / z at the higher floor;
   its centre is the waypoint. Each edge is validated once at load by stepping a dc-sim body of the largest AI box
   from one centre to the portal to the other centre (straight run at walk speed, at most 3 s simulated): an edge
   the body cannot traverse (a wall between touching boxes, a step higher than the mover climbs) is dropped and
   counted in the load log. Cost: about one body second per edge = 30 steps x 10 us = 0.3 ms; 1,628 edges = 0.5 s
   at load for the mission map on one core, done on the loader thread and cached per map (MEASURED basis F10).
3. **Islands.** Components with fewer than 8 volumes and no player spawn are vantage islands (snipers, F9). An AI
   placed or spawned on one never paths off it; it fights from there (4.4 `HoldPosition`).
4. **Fallback grid (no volumes).** Sample the world box on a 64-unit x / z grid: drop a dc-sim body from the top at
   each cell (`motion::drop_to_floor`), keep cells whose floor is within the world and not a kill or hazard volume,
   link 8-neighbours whose step succeeds as in 2. Mod maps may ship their own volumes (same class hash) to skip this.
5. **Queries.** A* over nodes with Euclidean heuristic; the result is a waypoint list (portal centres) followed by
   the target point; a path is reused while the target moves less than 128 units; at most `ai.path_queries_per_tick`
   (4) new searches per room tick, the rest wait one tick. Movement follows waypoints with the dc-sim body (4.8);
   a body that makes less than 8 units of progress in 1 s asks for a repath through the next edge removed for 5 s.

ROM-provided AI nodes were found and are used (task question answered: yes, `dc-world` sees them as unlabelled
classes today; FT-W1 adds the roles of 3.1).

### 4.3 Perception

Per actor, a small memory: the last known position and time of each player, an alert level and the last damage
source. Inputs:

- **Sight:** a player is seen when (a) within the attribute template's `SeeEnemyDistance` (DeCombat default 2,000
  units when absent), (b) inside a 120-degree horizontal cone around the AI's yaw, or any direction while alerted,
  (c) the line from the AI's eye to the player's eye is clear. The eye is at 0.8 x the box half height above the body
  centre (CLI-3, MOVEMENT §13.4). Test (c) is the **same service the visibility-culling design defines for the
  stream filter** (`SightService::clear(from, to)`, BSP ray through `dc_sim::CollisionWorld::ray` plus its coarse
  pre-filter); this design only requires that it is pure, deterministic and cheap to cache. Until that design lands,
  `coop/sense.rs` calls `CollisionWorld::ray` directly behind the same function signature.
- **Hearing:** a player's shot (`UP_FIRE`) within `HearEnemyWeaponFireDistance` (default 1,500) or an explosion within
  2x that range sets the shooter's last known position without a sight test; footsteps are not modelled (FT-7).
- **Damage:** being hit sets the attacker's position as known and the alert level to maximum.
- **Allies:** an AI that sees or hears an ally die within `HearAllyDeathDistance` (default 1,000) becomes alerted
  toward that position.

Scheduling: each actor re-tests each candidate player at most every 200 ms (staggered by actor index); a room spends
at most `ai.rays_per_tick` (48) sight rays per tick, oldest pairs first; results are cached per (actor, player) for
the 200 ms.

### 4.4 Behaviour: utility selection over a small set of primitives

DeCombat uses **utility scoring** (a public technique) rather than a behaviour tree: the ROM goal tables already
express "goal + importance + sense triggers", which maps directly onto scored options, and utility keeps the
mod surface small (one score hook). Every think tick (5 Hz per actor, staggered) the actor scores the behaviours its
goal set enables and runs the best until the next think; a running behaviour keeps a 15 % bonus (hysteresis).

| behaviour | enabled by | score (DeCombat default, FT-8) | does |
|---|---|---|---|
| `Idle` | always | 0.05 | stands, turns slowly |
| `Patrol` | a patrol start instruction or goal-set member | 0.2 while not alerted | walks the node chain (`Next`), pauses at nodes whose action is the look-around value |
| `Guard` | a guard start instruction | 0.3 while not alerted | holds near its guard node within `ReturnRadius`; picks one of its listed use nodes as its spot |
| `HoldPosition` | on an island or a use node of the sniper postures | 0.6 while it sees a target | stays, fires, crouches on the crouched posture |
| `Investigate` | alerted with a last known position and no sight | 0.4 decaying over `ChaseExtraTime` (default 10 s) | paths to the last known position, looks around 3 s |
| `Chase` | a target seen within 5 s | 0.5 + 0.3 x (distance > `AttackRangedRange` max) | paths toward the target until in range |
| `Attack` | target visible and within `AttackRangedRange` | 0.7 | faces, fires per 4.5, strafes when the brain's `AttackWhileMoving` is true |
| `TakeCover` | under fire, a use node or volume corner within 512 units that blocks the line to the attacker | 0.65 x (1 - health fraction) + 0.2 for `AttackCoverCheckChance` | paths there, crouches, peeks every `AttackFromCoverUncoverTime` |
| `Lob` | carries a thrown item, target within `AttackThrownRange`, timer `AttackGrenadeThrowTime` elapsed | `AttackGrenadeThrowChance` roll succeeds -> 0.9 for one think | stops, throws (4.5) |
| `Rush` | wave AIs (spawned by the director) | 0.55 when no target is known | paths to the nearest player's position (DESIGN: wave AIs always know where the players are, so a wave cannot be waited out by hiding) |

Goal-set mapping: the goal names of the actor's `[GoalSet<n>]` select the rows above by our own name table
(`patrol`, `guard`, `attack*`, `cover`, `chase`, `retreat` substrings, ASCII case-insensitive); a set that names
none gets the full default list. Score weights from `[Goal<n>] Importance` scale the default scores
(`score x clamp(importance / 50, 0.5, 2.0)`, FT-8). Brain keys named in the table are read as seconds, units and
chances by their key names; missing keys use the defaults in the table.

### 4.5 Combat: shooting and grenades with ROM weapon stats

- **Weapon:** the NPC weapon row named by the actor's right-hand property (OBSERVED: always a row of the NPC set),
  else the first rifle row of the room's weapon table. From the row: damage per hit, fire interval, clip and reload
  time, effective range, held model (the same keys `rom_tables.rs` reads for players; FT-9).
- **Shot:** hitscan from the muzzle (eye) to an aim point on the target's box; the aim point gets an angular error
  that grows with distance beyond `FullAccuracyRadius`, with target speed (`MaxMovementAccuracyPerturb`) and shrinks
  with `Accuracy` / `Marksmanship` (our formula, FT-9); the shot hits when the ray reaches the target box before the
  world (`CollisionWorld::ray` up to the box distance). Hit zone head / body / limb by the vertical position on the
  box. Damage = row damage x zone factor x the difficulty's `AIDamageFactor` x a mod hook. The shot effect relay and
  the damage messages to the victim are the slice's existing ones (2.3).
- **Grenade:** when the left-hand property names a thrown-item row: a server-side projectile on a ballistic arc
  (gravity of the movement parameters, speed from the row or 900 units/s), bounces off the BSP with half the speed, and
  explodes after the row's fuse (default 2.5 s) with the row's radius damage, line-of-sight checked per player. The
  2009 client needs a visible projectile and an explosion effect; which message forms the 2009 server used for
  thrown objects is UNPROVEN (a player's throw is the client's own; the slice only relays its sound, `gs::throw_sound`),
  so FT-W4 sends that relay with the AI as the thrower position plus the slice's existing shot / impact effect at the
  burst point, and the capture session records what the 2009 client shows (added to V-FT-11).
- **Melee and special kinds:** two of the wave map's brain rows indicate by their names a suicide blast and a gas
  cloud (OBSERVED: ROM brain names, not quoted); FT-11 maps them to a rush that deals radius damage at 64 units, and to
  a damage-over-time zone around the body. Their
  look on the 2009 client is whatever the AI body model shows; no effect message is invented.

### 4.6 Waves and spawners (the director)

- **Wave map (spawn tables present):** rounds run in the order of `Round`. A round lists spawners and up to N
  `(SpawnAIName<k>, SpawnDelay<k>, SpawnNum<k>)` triples: the director creates `SpawnNum` actors of the template
  named `SpawnAIName` (a template character of the world, 3.1), one every `SpawnDelay` seconds per triple, each at the
  next spawner of the round's list (round-robin, skipping a spawner within 256 units of a player or with an actor
  within 48 units). At most `LimitMaxAliveNum` (and at most `ai.max_alive`, FT-18) actors are alive; spawning pauses
  above it. The round ends when every scheduled actor has spawned and died; the next starts after `wave.break_secs`
  (10 s) with a 0xc9 sub 2 banner. Health of an actor = template health x `AIHealthRateRoundTable<difficulty>`
  `Round<n>` (last value repeats past the table). Weapon drops: in the listed rounds a type-0x17 kind-6 pickup of
  `WeaponID` at the named marker for `LifeTime` seconds. The last round cleared = mission success.
  **Superseded for the ROM wave map on 2026-11-05 (DSV-576..DSV-578):** its own trigger network drives the rounds:
  the server fires the start box at the match start, the world's round counter starts each round, a cleared round
  runs the kill counter's (unset) event of its number, and the difficulty's ending statements end the mission.
- **Mission map (spawners named by the trigger network):** section 4.7.
- **Any other world (synthetic or mod):** GMD-17's synthetic plan (5 waves of `4 + 2n`, boss in the last) over the
  world's player-spawn-free floor cells of the nav graph farthest from the players, using the first AI template or a
  default body row.

### 4.7 Missions and the trigger network

The mission map drives its stages through world objects that carry **command texts** (event counters, timed and
area triggers, caption / message / mission triggers and a camera path) plus the GMS start / end trigger names. The
command language is ROM data; its grammar is not documented in any allowed source. Two options (coordinator
decision, Q-1):

- **(A, first cut, DESIGN default):** do not interpret the commands. Run a DeCombat mission over the map's own data:
  one stage per navigation region in the order of the regions' first player-reachable volume along the nav graph from
  the player spawns; at stage start, activate the region's live characters and spawn every template whose spawner lies
  in the region (one actor per spawner); the stage goal ("clear the region") completes when they are dead; the goal
  list shows the difficulty row's `MissionList<k>` texts in order, one per stage, padded or truncated; the mission
  succeeds after the last stage. Hostage / item goals of the ROM texts are not modelled; their text is still shown.
- **(B, later):** a small interpreter written only from the command texts the user's ROM contains (verbs, argument
  forms and object names observed in the data; no outside grammar), covering message sends to triggers and spawners,
  delays, counters and timers. Every verb is a DeCombat behaviour with a DC row; unknown verbs are logged and
  ignored.

### 4.8 Bodies: dc-sim at the server tick

Each actor owns a `dc_sim::player::PlayerState` with its stand box and the room's `MovementParams`; every server
tick (30 Hz, CD-65) the actor's current behaviour produces a `TickInput` (forward / strafe / run / crouch bits and a
yaw toward the next waypoint or target) and the server steps the body exactly as `dc-botclient` steps its bot
(`motion.rs`: `TickInput { commands, yaw, pitch, ctrl_state: 1, .. }`). Speeds come from the attribute template:
`SpeedMods` scale = `RunSpeed` / the movement parameters' run speed (FT-6). Steering: turn rate 360 degrees / s,
arrival radius 24 units at waypoints. The body's position, velocity and yaw feed the stream (2.2); hits use its box.

### 4.9 Difficulty, scoring and the result screen

- **Difficulty** = the room goal (0..2; the ROM row's goal range, GM-MODES §2) = `[Object<goal>]` (FT-12): time limit,
  lives, AI damage factor, score factor, start / end trigger names, health-rate table `d = goal`. No room-size
  scaling beyond the ROM tables; `ai.max_alive` is the operator's cap.
- **Points** (FT-13): per hit, `DamageScoreType_A..D` by zone (head A, body B, arms C, legs D: our mapping); per
  kill, `DamagePlusByEtcDeath` (normal), `DamagePlusByBossDeath` / `DamagePlusByTBossDeath` (boss / trigger boss,
  FT-14), plus `DamagePlusByHeadShotDeath` for a head kill and `DamagePlusByCombo` for each kill within 4 s of the
  previous (MULTI_KILL_WINDOW_MS); boss damage points x `DamagePlusRateByBossDamage`; everything x `ScoreFactor`,
  rounded. Sent as 0xd7 sub 1 (kills, with the AI object) or sub 3 (damage and goal points), at most one per player
  per 250 ms (summed).
- **Result:** 0x46 co-op block `[stage][c1..c6]`: `stage` 2 success / 1 failure; `c1` total team points, `c2`
  waves or stages cleared, `c3` AI killed, `c4` player deaths, `c5` time bonus (0, ROM `ClearBonus_Time` decides
  later), `c6` the mission seconds used. The client shows `c1 - c5 - c4` in one cell (S2C §2.1.2), so the mapping is a
  guess to be fixed by V-FT-10. For `stage` 2 the name list = the players, title / text = the difficulty row's
  mission caption (`MissionList` first text) and empty, `panel_ms` = `ShowCoopResultDelayTime` or 3,000. 0x65 per
  player co-op block `[c1 = own points][n = 0][c2 = own kills][c3 = own points][c4 = own deaths]` (record fields
  +0x208 / +0x204 / +0x1f4 / +0x1fc, S2C §3.4). Experience and GP through CD-03 / CD-04 with the map's `[MapExp]` /
  `[MapMoney]`; a co-op objective bonus per cleared stage (CD-03 objective term).

## 5. Determinism and performance budget (task item 3)

### 5.1 Determinism

- The co-op state advances only in `Game::tick(now)` and in message handlers, in a fixed order: players' messages
  in arrival order, then actors in ascending actor id, each: senses (if due), think (if due), body step, aim.
- Randomness: one `xorshift64*` per room seeded from the room seed (`-seed`, as DSV-16), split per actor by
  `seed ^ (actor_id * 0x9e3779b97f4a7c15)`; no other source. All geometry goes through `dc-sim` (binary32, fixed
  operation order, MOVEMENT §15), so the same seed and the same input stream give the same actor digests on every
  target (FT-17). `CoopRules::digest()` hashes every actor's `PlayerState::digest()`, health, behaviour id and target.
- Think and sense times are tick counts, not wall time; the director's delays are rounded to ticks.

### 5.2 Budget

Per room tick at 30 Hz (33.3 ms), on the measured costs of F10 (i686 release, this machine; x86_64 is expected to
be equal or faster):

| item | count per tick (32 alive actors, 8 players) | unit cost | per tick |
|---|---|---|---|
| body steps | 32 | 10 us | 0.32 ms |
| sight rays (cap) | <= 48 | 25 us | <= 1.2 ms |
| shot rays | about 32 x 1/6 (fire interval) = 6 | 25 us | 0.15 ms |
| think (5 Hz, staggered) | 6-7 | about 5 us (scoring only) | 0.04 ms |
| path searches (cap) | <= 4 | about 50 us on 904 nodes (estimate) | <= 0.2 ms |
| stream entries | 32 x 8 viewers | bits only | (existing cost) |
| **total** | | | **about 2 ms = 6 % of one core per room** |

So one core carries about 12 busy co-op rooms with headroom; an idle room (no players) costs 0 (actors are not
stepped). FT-18 fixes the caps: `ai.max_alive` 32 per room, 48 rays, 4 path searches; when a room's measured AI time
exceeds 4 ms for 30 consecutive ticks the think rate halves for that room and the log says so (graceful degradation
instead of tick overrun). Memory: an actor is about 1 KB; a nav graph of 904 volumes about 100 KB, shared per map.
A release-mode test (`#[ignore]`, run by CI on demand) asserts the 64-actor synthetic room stays below 4 ms per tick.

## 6. Modding surface (task item 4)

Consistent with HOSTING_AND_MODS §3 (server-rules kind: lives on the server, clients need nothing, because the 2009
client renders whatever the server streams) and MODDING_GUIDE §4:

- **Content domains (exist):** `content.ai_brains` patches the `[Brain<n>]`, `[AttributeTemplate<n>]` and
  `[GoalSet<n>]` / `[Goal<n>]` rows by name (for example faster AIs, other sense ranges); `content.missions` patches
  the co-op GMS sections of 3.2 per map (spawn tables, health rates, score table, difficulty rows) and may add rows
  (a 25-round survival). Patched values are read through the same `CoopData` loader; the ROM stays untouched.
- **Hook namespace `coop` (new, FT-19):** capability `hooks.coop`, allowed for `server_rules` and `shared`, refused
  for `client_visual`; `dc-mod-lua` `HOOK_NAMESPACES` gains `"coop"`. Hooks (chain-of-responsibility `wrap` like the
  existing ones; contexts are plain tables):

| hook | when | context -> result | default |
|---|---|---|---|
| `coop.wave_plan` | mission start | `{map, difficulty, rounds = [...]}` -> rounds | the ROM spawn tables or GMD-17 |
| `coop.spawn` | before each actor spawn | `{template, spawner, round, health, weapon}` -> same (or nil to skip) | as built |
| `coop.ai_params` | actor creation | `{brain = {...}, attributes = {...}, goalset = {...}}` -> same | ROM rows |
| `coop.score_behaviour` | an actor's think, only for actors flagged by the mod (`ai_params` sets `scripted = true`) | `{behaviour, score, actor, target}` -> score | table of 4.4 |
| `coop.on_ai_damage` | a hit on an actor | `{damage, zone, attacker, actor}` -> damage | as computed |
| `coop.on_ai_kill` | an actor's death | `{killer, actor, points}` -> points | FT-13 |
| `coop.score` | any points award | `{player, kind, points}` -> points | FT-13 |
| `coop.objective` | stage / goal changes and on a 1 Hz timer | `{goals = [...], stage, time}` -> goals (custom objectives: hold an area, kill a named actor, survive N s) | 4.6 / 4.7 |
| `coop.on_end` | end check | `{success, reason}` -> `{success, reason}` | FT-15 |

- **Budget for hooks:** the sandbox limit of 10,000,000 instructions per call (MODDING_GUIDE §5) is far too large
  for per-think calls; `coop.score_behaviour` runs with a per-call limit of 100,000 and a per-room per-second budget
  of 2,000,000 instructions; over budget, the default scores apply for that second and the mod gets one log line
  (FT-19). Other `coop.*` hooks run at most a few times per second and keep the default limit.
- **New maps:** a shared mod that ships an original world with the navigation volume class (3.1), spawners and its
  own GMS-shaped `content.missions` rows gets waves without code; without volumes the fallback grid applies.

## 7. DeCombat rows FT-1..FT-22

The full rows (rule, default, evidence, config key, hook) are staged in
`staging/fireteam-design_DC_DEFAULTS.md`. They refine GMD-15..GMD-18: GMD-15 (teams) and GMD-18
(end) stay, with FT-1 / FT-15 as their wire detail; GMD-16 (behaviour) is replaced by FT-6..FT-9; GMD-17 (waves)
becomes the fallback of FT-10 for maps without spawn tables.

| id | short title |
|---|---|
| FT-1 | AI create form (type-0x18 blob with the NPC tail; team 2; pid 0xff; health fraction; kind 0 / 2; caption) |
| FT-2 | AI object ids, weapon objects and the alive cap |
| FT-3 | AI health updates by 0xa6 sub 0x24, coalesced 100 ms |
| FT-4 | AI death: no 0x6d; body frozen 3,000 ms, then removal |
| FT-5 | Player killed by an AI: 0x6d no-killer form; lives by 0xd7 sub 2; respawn on 0x89 while lives remain |
| FT-6 | Navigation: ROM volumes + validated edges; fallback grid; body speeds from the attribute template |
| FT-7 | Perception: sight cone / range / ray; hearing of shots; ally deaths; 200 ms re-test; ray cap |
| FT-8 | Behaviour: utility scores of 4.4, 5 Hz think, hysteresis, goal-set mapping |
| FT-9 | AI shooting: NPC weapon row, aim-error formula, zones, AI damage factor |
| FT-10 | Waves from the ROM spawn tables; break 10 s; weapon drops; fallback GMD-17 |
| FT-11 | Special AI kinds (explode, gas) |
| FT-12 | Difficulty = room goal = `[Object<goal>]` + health-rate table |
| FT-13 | Points and result blocks |
| FT-14 | Boss identification (NPC name table) and NPC kind 2 |
| FT-15 | End conditions and the result timing |
| FT-16 | Players: team 1, co-op spawn filter, start with fewer than the ROM minimum |
| FT-17 | Determinism: seeded per-room RNG, fixed order, digest |
| FT-18 | Budgets and graceful degradation |
| FT-19 | Lua `coop.*` hooks and their budgets |
| FT-20 | Enabling co-op: config gate, lobby refusal lifted only when enabled |
| FT-21 | AI animation clips |
| FT-22 | Mission maps: option A (region stages) until the command interpreter is decided |

## 8. Implementation plan in waves with tests (task item 6)

Two-person rule (specs/README): the new client-side facts F1-F6 go into the specs first (W0) by a spec author; the
implementers of W1+ work from the specs and this design, not from the disassembly. `dc-dserver` work starts after
`dserver-slice4` merges (concurrent owner of the crate).

| wave | owner (model) | crates | work | tests (all loopback, no game window) |
|---|---|---|---|---|
| FT-W0 | spec author | `specs` | fold F1-F6 into S3-2 / S2C §3.5, §8.3 (0xa6 sub 0x24 row) and GM-MODES §6.2; add V-FT-1..12 to GM-MODES §9 | the checks; spec review |
| FT-W1 | - | `dc-world`, `dc-bute` | roles of 3.1 (hash rows, `ObjectRole::{NavVolume, NavRegion, PatrolNode, WatchPost, UseNode, AiCharacter, SpawnSite}`), typed accessors; `CoopGms` (3.2) and `AiTables` (3.3) readers with defaults; synthetic writers for both in `dc-world::synth` / test fixtures | synth worlds with volumes, nodes, spawners round-trip; ROM-gated test: both co-op worlds report the F7 counts, both GMS documents parse with 3 difficulty rows, the wave map has spawn tables; non-co-op worlds report 0 AI objects |
| FT-W2 | - | `dc-dserver`, `dc-lobby-server` | class-5 flow without AI (2.4): `0x9e 5`, 0x45 co-op tail, 0xc9 sub 1 / 4 / 5, 0xd7 sub 2 lives / sub 4 timer, 0x46 / 0x65 co-op blocks, end by time or deaths; `modes.coop.enabled` (FT-20) lifts the lobby refusal | `synth_client` unit tests on a synthetic room: message order, layouts bit-exact against hand-written expected frames from the specs; lives: death with 1 life -> sub 2 [0][0], no respawn on 0x89; lobby test: refused when disabled, accepted when enabled |
| FT-W3 | - | `dc-dserver` (`coop/actor.rs`, `wire.rs`, `nav.rs` volumes only) | actors as NPC character objects (FT-1), stream + animation, dc-sim bodies, `Idle` / `Patrol` / `Guard`, hits from `UP_FIRE` on actor objects, health 0xa6 0x24, death + removal, 0xd7 sub 1 points; `--coop-test` room for V-FT | unit: create record bits for an NPC equal a hand-built blob (580 + tail bits); `dc-botclient` gains a reader for the NPC tail and counts AIs; botclient shooter vs a standing AI on a synth world: hit -> sub 0x24 with the expected fraction -> kill -> 0xd7 sub 1 -> removal; determinism: two runs, same seed -> same digest every tick |
| FT-W4 | - | `dc-dserver` (`sense.rs`, `think.rs`, `aim.rs`, `nav.rs` full) | perception, utility behaviours, shooting with NPC rows, grenades, cover from use nodes, edge validation, fallback grid | synth world "two rooms and a door": AI chases a botclient through the door, loses sight, investigates; cover test: AI under fire moves to a blocking node; aim test: hit rate at 256 / 1024 units within the formula's band over 1,000 seeded shots; ROM-gated: nav graph of both worlds builds, F9 component counts after validation logged; perf `#[ignore]` test: 64 actors < 4 ms / tick |
| FT-W5 | - | `dc-dserver` (`director.rs`, `score.rs`) | waves from spawn tables, health rates, weapon drops, difficulty rows, points, result blocks, GMD-17 fallback | synth GMS with 3 rounds: spawn counts, delays and alive cap per tick; round transition and banner; result block values for a scripted match; ROM-gated: wave map loads all rounds; full botclient-driven match on a synth world: 2 botclients survive 3 waves -> stage 2 result |
| FT-W6 | after Q-1 | `dc-dserver` (`mission.rs`) | mission map option A (region stages), later option B | synth world with 3 regions: stage order, activation, goal completion; ROM-gated: the mission map yields its region stages |
| FT-W7 | - | `dc-mod-lua`, `dc-dserver`, `docs/MODDING_GUIDE.md` | `coop` namespace, `hooks.coop` capability, the hooks of section 6, per-call budgets | kind tests (refused for `client_visual`), a sample mod doubling AI health and adding a round, budget exhaustion falls back to defaults deterministically |
| FT-W8 | coordinator + user | live | V-FT-1..12 capture session (2.6) with the user's GO; fold results into specs and FT rows | capture-derived allowlist; replay of the capture against `dc-dserver --coop-test` |

## 9. Open questions and risks

- **Q-1 (coordinator):** may a worker derive the command grammar of the trigger network (4.7 option B) from the
  command texts in the user's ROM alone? The texts are allowed ROM data, but the language resembles an engine
  scripting language whose documentation lives only in forbidden SDK trees; a worker must not consult those. Until
  decided, option A.
  **Decided 2026-11-05 (USER DIRECTION, "Fire Team must be playable as the maps are built"): option B**, the grammar
  read only from the ROM's own command texts; rows DSV-570..DSV-575 of `DC_DEFAULTS.md`, code `script.rs` and
  `doors::run_world`.
- **Q-2 (user):** should co-op rooms start with fewer humans than the ROM row's minimum (the co-op row fixes min =
  max players)? FT-16 proposes yes (1 or more), since DeCombat servers will often be small.
- **Q-3 (user):** the wave map's special kinds (explode / gas) have no client effect message we know; is "damage only,
  no special visual" acceptable for the first release?
- **R-1:** the meanings of the ROM AI keys are taken from their names only; DeCombat's behaviour defines them. A
  community expecting the 2009 feel may find differences; the capture plan cannot fix this because no 2009 Fireteam
  traffic exists.
- **R-2:** F3 (health bar fill) and F4 (boss icon) are DISASM / PLAUSIBLE; if V-FT-5 shows another bar source, FT-3
  changes but nothing else does. *Spec-fold-7:* the bar source is PROVEN by FIRETEAM_2009 section 1.5 (S1 of the diff); the
  boss meaning of kind 2 stays PLAUSIBLE.
- **R-3:** the NPC weapon rows and AI body rows may name files the EU-test ROM lacks (DSV-24 found one face row
  missing); the asset resolver skips missing parts as for players and the actor falls back to the first resolvable
  `[AIModel<n>]` row.
- **R-4:** the visibility-culling design is written concurrently; section 4.3 depends only on a pure
  `clear(from, to) -> bool` service, which both designs should name the same way when the coordinator integrates them.

## 10. Fold record (spec-fold-7, 2026-10-25)

Source: `specs/FIRETEAM_2009.md` sections 1-5. Nothing was deleted; each overturned place keeps its text and gains a
"superseded by FIRETEAM_2009 section x" note.

| finding / place | overturned by | note added at | spec section |
|---|---|---|---|
| F2 (rounded) | D2 | section 1 row F2 | 1.3 step 3 |
| F4 (kind byte) | D1 | section 1 row F4; 2.1 create table row `rec_a0` | 1.3 step 4, 1.5, 1.9 |
| F5 (model table end) | D3 | section 1 row F5 | 1.8 |
| F6 (lives) | D4, D5 (S5 for the meaning of `b`) | section 1 row F6 | 2.1, 2.2 |
| 2.2 death (remove the AI object) | D7 | 2.2 "Death" bullet | 1.7 |
| 2.3 player killed by an AI (sub 2 after the kill, kill-feed icon) | D6, D8 | 2.3 bullet | 1.7, 2.3 |
| F1, F3 | not overturned (FIRETEAM_2009 5.1 agrees; F3's meaning upgraded to PROVEN, S1) | R-2 note | 1.2-1.5 |

The FT rows of section 7 are unchanged here; FT-W1, FT-K1, FT-L1, FT-R1 and FT-C1 of FIRETEAM_2009 4.3 (proposed) are the corrected rules for the coordinator.
