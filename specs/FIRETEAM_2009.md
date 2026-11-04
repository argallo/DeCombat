# FIRETEAM_2009: what the 2009 client expects for AI characters and co-op rooms (clean-room)

- **Scope:** client-side only. What the 2009 EU-test client renders, reads and expects for AI-controlled characters
  and for rooms of mode class 5 (the co-op mode): the character create form with its NPC tail, the health bar and
  icons, removal, animation and weapon expectations, the co-op messages of the room and round, the respawn rule,
  the result screen, the ROM data a co-op server needs and which of it the client reads (the mod surface), and the
  capture items that would settle what static analysis cannot. Nothing about how a server's AI behaves.
- **Author role:** spec author under the two-person rule (`specs/README.md`, `REFERENCE_TESTING.md` section 2).
  The findings F1-F6 of `docs/FIRETEAM_AI_DESIGN.md` were re-derived here from the binaries, not taken over;
  every point where this reading differs is listed in section 5 and staged in
  `staging/fireteam-spec_DESIGN_DIFF.md`.
- **Builds on:** `GAME_MESSAGES_2009_S2C.md` (cited `S2C §n`), `GAME_MESSAGES_2009_C2S.md` (`C2S §n`),
  `GAME_MODES_2009.md` (`GM-MODES §n`), `UDP_SESSION_2009_B.md` (`SESSION_B §n`), `FMT_DAT85.md`, `FMT_LTB.md`,
  `FMT_CRES_UI.md`.
- **Evidence of this file:** clean decompiler output `CShell.dll/<rva>.c` (cited `cshell+0x...`);
  read-only capstone disassembly of the user's `Game/CShell.dll` from a scratch script outside the tree (cited
  `DISASM cshell+0x...`); read-only scratch programs over our crates `dc-rez`, `dc-bute`, `dc-world` and `dc-model`
  that printed key names, section counts, class hashes and model structure counts of the user's ROM (cited
  `OBSERVED: ROM`, nothing copied); the CRes string table of the user's `Game/CRes.dll`, cited by resource id only.
  No capture of a co-op room exists and the reference stack has no AI, so nothing here is PROVEN-CAPTURE or
  REFERENCE-CONFIRMED.
- **Names:** all field, element and function names are ours. Engine and HUD singletons use the names of
  `S2C §1` (`hud_marker`, `hud_radar`, `hud_overview_map`, `hud_target_name`, `hud_respawn_token`,
  `hud_coop_result`, `local_player`, `roster`, `conn_mgr`, `engine_client`). ROM strings are given as key or
  section names where the task allows it (attribute keys, section families), as `sha:<sha256[:16]>/<len>`
  (`FMT_CRES_UI.md` convention) for entry names and world class names, and as CRes resource ids for texts.
- **Grades:** **PROVEN** = read in clean decompiler output with the control flow and the data both visible;
  **DISASM** = read in our own disassembly where decompiler output hides the operands (x87 code, pushed arguments, jump
  tables); **OBSERVED: ROM** = printed from the user's ROM by our crates; **PLAUSIBLE** = an inference that fits
  every piece of evidence but is not shown by it; **UNPROVEN** = open; **(proposed)** = a DeCombat rule suggestion
  that is not in `DC_DEFAULTS.md`.
- **Status:** DRAFT 1 (2026-10-25). Not validated against a capture (none exists; section 4 lists V-FT-1..V-FT-16).

## 0. Contents

1. The AI character on the wire (create form, tail, kind byte, health value, HUD entries, updates, removal,
   animation, weapon, hits)
2. Co-op room and round messages (class 5)
3. ROM data for a co-op server and what the client reads of it
4. Validation items V-FT-n and grade summary
5. Where this spec differs from the design document

## 1. The AI character on the wire

### 1.1 The class and the create path (PROVEN)

An AI character is a character world object (engine kind 1, `kind_byte` 0xc1, SESSION_B §3.5.1) whose create blob
starts with type byte 0x18: the same client class as a player character (S2C §8.2.1: factory case 0x18,
constructor cshell+0x206db0, method table VA 0x374efea4). The create slot +0x10 (cshell+0x203c70) builds a local
record, stores the engine object handle in record +0x04, reads the record with cshell+0x2b6aa0 and calls the init
slot +0x0c (cshell+0x205f40). The init copies the record into the object member at object +0x38
(cshell+0x202650), so **object offset = record offset + 0x38** throughout this section.

### 1.2 Record layout and the NPC tail

The fixed part is the 572-bit record of S2C §8.2.1 (unchanged; widths DISASM cshell+0x2b6ac9..0x2b6c79, all
re-read for this spec). The two leading one-bit fields decide the path:

| bit | record | object | meaning (ours) |
|---|---|---|---|
| 1st | +0x08 | +0x40 | `roster_bound`: the character belongs to a roster player |
| 2nd | +0x0c | +0x44 | `has_tail`: the NPC tail follows at the end of the record |

The tail is read only when `has_tail` is non-zero, after the infected bit (DISASM cshell+0x2b6c79..0x2b6cda):

| # | width | record | object | our name | client use (sections below) |
|---|---|---|---|---|---|
| T1 | u8 | +0x28 | +0x60 | `npc_rank` | rank-icon index of the marker entry (1.4) |
| T2 | u8 | +0x98 | +0xd0 | `npc_team` | team of the marker, radar and map entries (1.4) |
| T3 | u8 | +0xa0 | +0xd8 | `npc_kind` | icon kind, infected flag, health-bar visibility (1.3) |
| T4 | u32 | +0x9c | +0xd4 | `npc_health` | f32 bit pattern; health fraction at create (1.5) |
| T5 | zstr8, at most 257 bytes (reader limit 0x101) | +0x7c | +0xb4 | `npc_caption` | marker caption (1.4) |

Tail size: 8 + 8 + 8 + 32 bits plus the string; the record with tail is `572 + 56 + 8 x (len + 1)` bits (the
string reader's terminator rule is the one of S2C §1). Grade: PROVEN (field order, targets), DISASM (widths).

**Path selection (PROVEN cshell+0x205f40, cshell+0x1a0470):** the roster branch runs when object +0x40 is
non-zero **and** the connection check cshell+0x1a0470 passes (connection object VA 0x3759e22c exists and its
state at +0x04 is neither 4 nor 5; PLAUSIBLE: true in every network session). Otherwise, when object +0x44 is
non-zero, the NPC branch runs. So an AI character is sent with `roster_bound` 0 and `has_tail` 1. A record with
both bits 0 (or with `roster_bound` 1 in a session) gets no NPC branch. Both branches end with the common tail of
the init (box and offset to member +0x544, a per-object array sized by a ROM table count, and cshell+0x205ce0 when
either bit is set).

Init writes for every character, before the branch (PROVEN): object +0x5ae := low 16 bits of record +0x70,
+0x5b0 := low 16 bits of record +0x74, infected flag +0x5bc := record +0xa4.

### 1.3 The NPC branch (PROVEN, DISASM where noted)

In this order (cshell+0x205f40, DISASM cshell+0x206054..0x20616a):

1. `npc_caption` is converted to a wide string with the ANSI code page (cshell+0x20e30: plain `MultiByteToWideChar`
   with code page 0; **no table lookup**: the name on the wire is the text shown).
2. Marker entry: `hud_marker` add (cshell+0x8a760) with (engine object, `npc_team`, `npc_rank`, name, 0, this
   object). See 1.4.
3. Health value: object +0x5ae := `trunc(npc_health_as_f32 x 100.0)` as a 16-bit value. DISASM: `fld dword
   [esi+0xd4]`, `fmul dword [VA 0x374d08d8]` (the PE-DATA f32 100.0, read from the user's file), call
   cshell+0x3a5f00. cshell+0x3a5f00 is the compiler's float-to-integer helper that rounds with `fistp` and then
   corrects the result toward zero (DISASM cshell+0x3a5f00..0x3a5f74), so the conversion **truncates** (C cast
   semantics). The 16-bit store overwrites the value copied from record +0x70.
4. Kind (`npc_kind`, object +0xd8):

   | `npc_kind` | radar entry kind | overview-map entry kind | other effect |
   |---|---|---|---|
   | 2 | 8 | 3 | none |
   | 4 | 6 | 1 | infected flag +0x5bc := 1; when this object is the own view object (never for an AI) the infected view helper cshell+0x26c070(1) |
   | any other | 6 | 1 | none |

   The kind is also read **every frame** by the marker update (1.5): only kinds 1 and 2 show the health bar.
5. Overview map add `hud_overview_map` cshell+0xc8030(object, object, name, `npc_team`, 0, 0, map kind), then
   cshell+0xc7cc0(object, 0) (the call that 0x6d makes with 1 to grey a dead player out, S2C §3.3; so 0 =
   PLAUSIBLY "alive").
6. Radar add `hud_radar` cshell+0xb8870(object, object, `npc_team`, radar kind).

Meaning of the kinds: PLAUSIBLE 2 = boss (its own radar and map icon kinds, and the co-op HUD layout has boss
icons, GM-MODES §6.2); 4 = infected look (the flag selects the infected variant of two character event handlers,
cshell+0x204c50 and cshell+0x204f60 choose cshell+0x204090 instead of cshell+0x2043e0; their effect is UNPROVEN);
1 = an ordinary NPC that shows a health bar; 0 = an NPC without a health bar. Note that in the NPC branch the
infected flag is set but cshell+0x201e40 (which the roster branch calls) is not, which only matters for the own
view object.

### 1.4 HUD entries of an AI: marker, name, rank icon (PROVEN)

`hud_marker` add (cshell+0x8a760):

- If an entry for the same engine object already exists (entry +0x34), only its name, flag (+0x30) and team
  (+0x28) are replaced; nothing else is created.
- **If the name is empty (`wcslen` = 0) no entry is created at all**: no caption and no health bar. The radar and
  map entries are still added (1.3 steps 5-6).
- Otherwise the entry gets: the engine object (+0x08), team (+0x28), `npc_rank` (+0x2c), the character object's
  16-bit health value (+0x38, through the class slot +0x74, see 1.5), the name (+0x3c), a caption element
  (cshell+0x88da0), two rank-icon elements (cshell+0x88a40 and cshell+0x88bf0) and the health-bar element
  (cshell+0x878b0).
- **`npc_rank` is a rank index**: both icon builders look it up in the ROM level table (singleton from
  cshell+0x269520, list of small-icon names at +0x15f0 filled by the level loader cshell+0x268810 from the
  `SmallIcon<n>` keys of the rank rows; the index goes through the level-to-icon map cshell+0x267dc0, out-of-range
  indices map to 0) and build two textures from that name with the suffixes the builders append (an own-team and
  an other-team variant, chosen by the marker's own team +0x2c and the entry team being 1 or 2). PROVEN for the
  table path; that it shows a rank emblem next to the name is PLAUSIBLE (the same lookup serves player markers).
- The health-bar element (cshell+0x878b0) takes its textures, rectangles, offset and alpha from the
  `[CooperativeMode]` section of `ATTRIBUTES\LAYOUT.TXT` (keys `NPC_HPBarBackTex`, `NPC_HPBarBackPos`,
  `NPC_HPBarBackRect`, `NPC_HPBarOffset`, `NPC_HPBarTex`, `NPC_HPBarRect`, `NPC_HPBarAlphaColor`); its maximum is 100
  (element +0x198, set by its constructor cshell+0x6d970).

Draw rule (cshell+0x8a420): an entry is drawn while its caption and icon elements exist and its screen position is
valid; the health-bar element is drawn only when `mode_class` = 5 **and** the entry team differs from the marker's
own team (+0x2c). So AI enemies need a team value different from the players' team (GM-MODES §6.2, unchanged).

Radar and overview map: both are keyed by the engine object; a second add for the same object updates team and
kind (cshell+0xb8870 when the key exists: entry +0x114 team, +0x04 kind, +0x128 the second object argument).

### 1.5 The health value and its update (PROVEN, DISASM)

- **Reader found:** class slot +0x74 (method table VA 0x374efea4 + 0x74 = cshell+0x206d30) returns the 16-bit
  value at object +0x5ae; slot +0x78 (cshell+0x206d40) returns +0x5b0 (DISASM, both two-instruction getters).
- Every frame the marker update (cshell+0x8b2e0, DISASM cshell+0x8b536..0x8b58b) finds the character object of
  each entry by its engine object (cshell+0x2046f0 over the character list VA 0x3759c424..0x3759c428), reads slot
  +0x74 into entry +0x38 and calls the bar setter cshell+0x6d800 with it; then it sets the bar's draw flag (bar
  slot +0x9c = cshell+0x18630, stores element +0x3c) to **1 when object +0xd8 is 1 or 2, else 0**. The bar's draw
  method (cshell+0x6d350) draws nothing while +0x3c is 0.
- Bar setter cshell+0x6d800 (DISASM cshell+0x6d8d9..0x6d8fa): acts only when the value changed; the filled
  width is `trunc(bar_width x value / 100)` for a value above 0 and empty for 0 or below; each change starts a short
  flash (element +0x19c, tick stamp +0x1a0).
- So **object +0x5ae is the health percent 0..100 of the NPC health bar** (PROVEN by the chain create -> +0x5ae ->
  slot +0x74 -> bar setter -> width). The create sends it as a fraction (T4, f32 1.0 = full); the update is shell
  0xa6 type 0x18 sub 0x24:

  `0xa6 [note_kind:u8 = 0x18][obj:objref16][sub:u8 = 0x24][fraction:u32 = f32 bits]`

  Handler cshell+0x206260 case 0x24 (DISASM cshell+0x206548..0x206569): only when object +0x44 (`has_tail`) is
  non-zero; reads 32 bits, loads them as f32, multiplies by the same 100.0 and stores the truncated value at +0x5ae.
  For an object without the tail the four bytes are **not read** (the sub-id is consumed, the rest of the message
  is left), so a server must send sub 0x24 only to NPC objects. Sub-id dispatch: byte table VA 0x373065cc indexed
  by `sub - 7`, sub 0x24 -> case 12 (DISASM, table bytes read from the user's file).
- Truncation trap: `0.29` as f32 times 100 gives 28.99... and shows 28. A server should send `(percent + 0.5) /
  100` or exact binary fractions (proposed FT-W1, section 4.3).
- Record refresh (0xa6 sub 0x11) re-reads the whole record into member +0x38 through the record class slot +0x08
  (record method table VA 0x374efcb8, slot +0x08 = cshell+0x2b6aa0; DISASM table read) **without** re-running the
  init: a refreshed `npc_kind` changes the bar visibility at once (read every frame), but `npc_health`, the name,
  team, rank and the radar / map icon kinds do **not** reach the HUD. Use sub 0x24 for health; the HUD entries
  keep what the create set. PROVEN (control flow).
- **Consequence for reuse** (gap G-3 of dserver-fireteam, folded by spec-fold-8): after the create a server can still change the kind byte (through sub 0x11, PROVEN above)
  and the health (sub 0x24); the caption, team, rank and icon kinds are fixed at the create, so a hidden-and-reused AI object (FT-R1, 1.7) keeps its first caption. Slice 1
  of `dc-dserver` keeps a slot's first caption and kind (FT-S3, `DC_DEFAULTS.md` dserver-fireteam rows, PENDING).

### 1.6 Other 0xa6 type 0x18 sub-ids that matter for an NPC (PROVEN cshell+0x206260)

| sub | NPC effect |
|---|---|
| 0x1b `[box:vec96][offset:vec96]` | box and offset to member +0x544, as for players (S2C §8.3) |
| 0x1d (no body) | clears the infected flag +0x5bc and runs two resets (cshell+0x202ed0, cshell+0x205df0(0)); the roster-only part is skipped for an NPC. PLAUSIBLE "cure" for a kind-4 NPC |
| 0x1e, 0x1f, 0x25 | roster-only (they test +0x40 and the connection check); no effect on an NPC |
| 0x24 | health percent (1.5) |
| 0x11 | record refresh (1.5) |

The S2C §8.3 table lists 0x1d and 0x22 as "nothing"; they read nothing but do act (0x1d as above, 0x22 sets object
+0x540 := 1, meaning UNPROVEN). Correction C-FT-1 (section 5.2).

### 1.7 Removal and death of an AI

- **No death message exists for an AI.** 0x67 is the own player's state; 0x6d addresses roster ids (u8 `victim`,
  `killer`) and skips unknown ones (GM-MODES §6.2); and in class 5 the kill-feed add (cshell+0x9d4a0, called by
  the 0x6d case of cshell+0x2c430) returns at once (PROVEN: first test `mode_class == 5`), so co-op has no kill
  feed rows at all. A death of an AI is therefore shown only by what the object stream carries (animation, then
  removal or hiding). PLAUSIBLE.
- **The client removes marker, radar and overview-map entries only in the 0x6c handler** (cshell+0x2c430 case
  0x6c calls cshell+0xb8540, 0xc7c70, 0x8aa00, 0x89ea0 for a known pid) and the marker entry also in the 0x65
  case (cshell+0x8aa00 for a known record). The character teardown (cshell+0x206ac0, from the destructor
  cshell+0x206f50) removes the object from the character list but calls none of the HUD removers (PROVEN: the only
  callers of cshell+0xb8540, 0xc7c70, 0xae010, 0x8aa00 and 0x89ea0 are cshell+0x2c430 and themselves). An AI
  has no pid, so after a kind-3 removal of an AI object its HUD entries stay, holding a handle of a deleted engine
  object; the marker code then queries the engine with that handle every frame (cshell+0x89310 -> engine client
  slot +0xa0). What the client does with a stale handle (skips, draws at a stale point, or faults) is UNPROVEN
  (V-FT-6). Until V-FT-6 is answered a server should not remove AI objects during a match but hide and reuse them
  (proposed FT-R1, section 4.3).
- **What "hidden" means for a character object** (gap G-4, folded by spec-fold-8) is open: the pre-spawn flag words 0x2802 and the spawned 0x3803 differ in 0x0001 / 0x1000,
  whose meanings are UNPROVEN (SESSION_B §3.5: PLAUSIBLE "visible" / "in play"), and whether the NPC marker, radar and map entries keep drawing for a hidden AI was not traced
  (V-FT-6). Slice 1 hides with the pre-spawn flag words, a snapped park position and no stream entries (FT-R1).
- The 0x99 level exit and the world unload end the match HUD; whether they clear these lists is not traced
  (UNPROVEN).

### 1.8 Animation, model, weapon, hits

- **Animation:** the type 0x18 class neither creates nor reads animation trackers; trackers travel in the
  engine's animation block of the create and in the object stream (SESSION_B §3.5.1, §4.3), exactly as for remote
  players. PROVEN negative for the class code read here (init, create, 0xa6 handler); PLAUSIBLE overall.
- **AI bodies and clips (OBSERVED: ROM, `dc-model`):** the 12 `[AIModel<n>]` rows of `ATTRIBUTES\MODELBUTES.TXT`
  name body models of which 9 have 53 nodes, socket 2 with the player weapon-socket key `c969c8aa86333373/9`, and the
  same single child clip model as the player bodies (`e3b947ced3e66980/27`, 1,130 clips, holding the stand and walk
  clip keys of SESSION_B §3.5.1). For these 9 rows the player 3-tracker layout and clip indices apply unchanged. Three
  rows differ: one body with 45 nodes, no sockets and its own child model of 27 clips (without the player stand clip
  of tracker 1); two bodies with 43 and 71 nodes whose child models have 102 and 148 clips, no blend sets, none of
  the player stand / walk keys and another key at socket index 2 (`63ad83c0cd2ff3bf/5`, `a39e9d9816a99fc0/4`). For
  these a server must take clip indices from that body's own child model and cannot assume socket 2 is a hand.
- **Model block:** the client renders what the create's model block names (SESSION_B §3.4, §3.5.1). The AI rows
  carry `ModelFile`, `Skin0`, `Skin1`, `RenderStyle0`, `RenderStyle1`, `AttachFace` and `AIFace` (an `[AIFace<n>]`
  row with its own model); the server builds the block from them and announces the files first (as for players).
  PLAUSIBLE; layout PROVEN for players only (REFERENCE-CONFIRMED, tdm2).
- **Record byte +0x14 and the model table (PROVEN cshell+0x1e4760, cshell+0x1e0d80):** the client's model table
  holds the `[Model<n>]` rows, then the `[AIModel<n>]` rows, then the `[Overidable_Model<n>]` rows (record size 0x49c;
  counts: plain at table +0x15ac, plain + AI at +0x15b4, override rows at +0x15b8). The skeleton lookup applied to
  record +0x14 answers only for plain rows (index below +0x15ac) and 0xff otherwise. No other client use of +0x14 was
  found. A server can send 0 (as the reference stack does for players); whether an AI should send `plain + ai_row` is
  UNPROVEN and has no visible effect found (V-FT-4).
- **Weapon:** the held weapon of a player is a separate kind-1 model object attached at socket 2 with no client
  class of its own (SESSION_B §3.5.1). The same form works for an AI only on bodies with a hand socket at index 2
  (the 9 rows above). PLAUSIBLE. Six `[Weapon<n>]` rows of `ATTRIBUTES\WEAPONS.TXT` carry the AI burst keys
  (`AIMinBurstShots`, `AIMaxBurstShots`, `AIMinBurstInterval`, `AIMaxBurstInterval`, `AIAnimatesReloads`; OBSERVED:
  ROM); the client does not read those keys (section 3.3). AI shots are shown with the type 0x06 shot effect with
  `shooter` = the AI's character object (S2C §8.2); that the client accepts a non-roster shooter is UNPROVEN
  (V-FT-8). Gap G-11 (folded by spec-fold-8): slice 1 sends the shot effect for every AI shot; in its live rounds 1-2 `dc-botclient` parsed them without an error
  (OBSERVED, our own client only), which does not answer V-FT-8.
- **Death of an AI** (gap G-5, folded by spec-fold-8): no death clip or death animation is named for an AI, and the client has no AI death message (1.7), so a dead AI is
  shown only by what the stream carries. The form the reference stack used for a dead **player** (a type 0x26 corpse object and the death clip on the character's stream,
  SESSION_B §3.5.5, §4.3) is the nearest model; whether it suits an AI is open. Slice 1: the body stands still for `corpse_ms`, then hides (FT-4 / FT-S5).
- **Damage to players:** 0x74 carries no attacker id (S2C §4.3), so damage from an AI uses it unchanged.
- **Hits on an AI:** the client reports object hits with `hit_obj` in 0x82 (C2S §4.1.1). That its trace hits an
  NPC character object with the player flag set (0x3803 after the spawn) is PLAUSIBLE (engine-level, the class is
  the same) and UNPROVEN (V-FT-7).

### 1.9 The AI create in one line (what a server sends; proposed FT-C1)

Kind-1 character object as for a remote player (SESSION_B §3.5, §3.5.1: model block from the AI row, three
trackers for the 9 player-skeleton rows, flags, attachment of the weapon child), with the type 0x18 record:
`roster_bound` 0, `has_tail` 1, the fixed fields as for a player except `infected` 0, and the tail `npc_rank`,
`npc_team` (different from the players' team, GM-MODES GMD-15 (proposed)), `npc_kind` 1 (or 2 for a boss; 0 only for
an NPC that should have no bar), `npc_health` = f32 1.0, `npc_caption` non-empty if the NPC shall have a caption and a
bar. No 0x6b roster record for an AI. Then 0xa6 sub 0x1b as for players, sub 0x24 after each damage.

## 2. Co-op room and round messages (mode class 5)

### 2.1 Mode class and room settings

| message | class 5 specifics | grade |
|---|---|---|
| 0x9e `GS_GAME_ENDPOINT` `[mode_class:u8 = 5][host][port]` | must come before every message whose layout depends on the class (0x7d, 0x45, 0x46, 0x65; S2C §5.1, §10); the ROM row is `[Mode2]` of `ATTRIBUTES\GAMEROOM.TXT` with `GameType` 5 and key `COOP` (OBSERVED: ROM) | PROVEN (S2C) |
| 0x7d `GS_MATCH_SETTINGS` | layout as in S2C §3.2 (no class-5 condition in the layout). In every class `goal` goes to the co-op panel cshell+0x76df0, which formats the `[CooperativeMode]` layout key `ModeImgTex` with `goal` as its one integer and loads that picture (PROVEN). The `COOP` row's goal range is 0..2 (OBSERVED: ROM, keys `GoalMin` / `GoalMax`), so in class 5 `goal` selects one of three mode pictures: PLAUSIBLY the difficulty | PROVEN (use), PLAUSIBLE (difficulty) |
| 0x45 `GS_MATCH_BEGIN` | the intro carries `mode_caption` (to `hud_coop_score`) and the class-5 tail `[x:u8][y:u8][z:bool1]`; `y` goes to `hud_health` cshell+0x96540 (an empty method in this build), `x` and `z` are read and unused (S2C §2.1.1) | PROVEN (S2C) |
| 0x6b roster record field 21 `m1:u8` | stored at record +0x1f8 (cshell+0x2a950), the **same field as 0xd7 sub 2 `b`** (S2C §3.2.1). So the initial respawn count can travel in the player's own 0x6b | PROVEN |

Default map of a co-op room (gap G-9, folded by spec-fold-8; OBSERVED in live round 1 of dserver-fireteam, `round1/dserver/*.log`: "no GMS of mode COOP names this world"):
the mode row's `DefaultMapID` for the co-op row names a map without a co-op GMS, so a new co-op room's default map has none. A server finds the GMS by the loaded world
(slice 1 does), not by the lobby's map; the lobby's default map for a new co-op room is a follow-up.

### 2.2 0xd7 `GS_COOP` sub-ids (re-read: cshell+0x2c430 case 0xd7, DISASM cshell+0x2d3a9..0x2d41a)

Layouts are those of S2C §3.5 (unchanged). Client effects in class 5:

| sub | layout | effect | grade |
|---|---|---|---|
| 1 | `[pid:u32][obj:objref16][x:u32][points:u32]` | record of `pid` +0x1f4 := `points`; for the own pid a floating popup (`hud_marker` cshell+0x8ad40) and the scoreboard value (cshell+0x91c90); `hud_coop_score` cshell+0x7ef20(`pid`, `points`) | PROVEN (S2C) |
| 2 | `[a:u8][b:u8]` | own record +0x1f8 := `b`; `local_player` cshell+0x5df50(`a`, `b`); `hud_multi_kill` reset cshell+0xa4c60; `hud_target_name` cshell+0xca6d0(`b - 1`) and cshell+0xca780(2) | DISASM (argument order) |
| 3 | `[pid:u32][x:u8][points:u32]` | as sub 1 without the record write | PROVEN (S2C) |
| 4 | `[running:bool1][secs:u32]` | round timer shown with `secs`, running or frozen | PROVEN (S2C) |
| 5 | none | `hud_multi_kill` reset | PROVEN (S2C) |

**Sub 2 is the respawn count:** cshell+0xca6d0 formats CRes string id **0x5a0** with the single integer `b - 1`
(DISASM cshell+0xca715..0xca72b: the value and 0x5a0 are pushed, the string loader cshell+0x1a0420 is called,
then the formatter) and shows it on `hud_target_name`; the string is a "respawns remaining" text with one integer
(read from the user's CRes.dll, not copied). In class 5 the target-name update cshell+0xcbea0 switches the element
to mode 1 (no count) when the stored `b - 1` is negative, else shows the count element. So `b` = the lives the
player has including the current one, and the text shows the respawns left after it.

**Re-arm:** cshell+0x5df50 (PROVEN) acts only when the own player state (`local_player`+0x30) is 2, 3, 5 or 6
(the dead and observer states of S2C §4.1) **and** `a` equals the own record's +0x1f8 (just written as `b`, so
`a == b`): it starts the 7000 ms respawn wait (+0x44c tick, +0x450 = 7000, +0x454 = 1), shows the 7.0 s countdown of
`hud_mode_notice` and resets `hud_respawn_token` to state 0.

### 2.3 Respawn rule for humans in class 5 (DISASM cshell+0x60ac0..0x60bcc)

When the own player dies (the death branch of the per-frame state machine at `local_player`+0x428, entered after
server 0x67 state 2, C2S §3.3), the client arms its wait by class. For class 5 it takes the own roster record
(cshell+0x2aca0); if there is none, or its +0x1f8 is 0 (unsigned compare), **no wait is armed and no respawn
request is ever sent**; otherwise 7000 ms (0x1b58) as in classes 1, 4, 6, 7, 8. After the wait the request is
`UP_RESPAWN_REQUEST` 0x89 (cshell+0x61e40, GM-MODES §4.2).

Consequences for a server (proposed FT-L1): the value at the moment the client processes 0x67 state 2 decides.
Send the count before the death message: 0x6b `m1` (at join) or 0xd7 sub 2 `[n][n]` with `n` = the lives left
counting the current one; on a death send `[n-1][n-1]` **before** 0x67 state 2. With `n - 1` = 0 the client stays
dead (observer) for the rest of the round; a later sub 2 `[k][k]` with `k > 0` while the player is dead re-arms the
wait (2.2), which is how a server can give a life back. That `LifeCount` / `MaxLifeCount` of the GMS rows (3.1) are
the intended source of `n` is PLAUSIBLE (names only).

### 2.4 Mission, wave and round HUD (class 5)

| message | what the client shows | grade |
|---|---|---|
| 0xc9 `GS_COOP_MISSION` sub 1 | goal list, mission timer, respawn-token HUD reset; its trailing `[a:u8][b:u8]` go to the scoreboard cshell+0x92140: +0x11c := `a`, +0x114 := `b` (the fields that 0x7d fills with `round_no` and `goal`, S2C §3.2); in class 5 with `a` > 0 six scoreboard elements are switched on and redrawn. PLAUSIBLE: `a` = the current stage or wave number, `b` = the goal (difficulty) | PROVEN (stores, condition), PLAUSIBLE (meaning) |
| 0xc9 sub 2 / 3 | banner text for `secs` / text note, each with the mode sound (S2C §6.2) | PROVEN (S2C) |
| 0xc9 sub 4, 0xd7 sub 4 | mission timer / round timer start and stop | PROVEN (S2C) |
| 0xc9 sub 5 | goal add, remove, state | PROVEN (S2C) |
| 0xc9 sub 6 | pickup note with one of three pickup sounds | PROVEN (S2C) |
| 0xc9 sub 8 `[a:i8][b:u8]` | scoreboard values; `a` != 0 also a multi-kill note | PROVEN (S2C) |
| 0x44 `GS_ROUND_BEGIN` | not class-specific; a co-op server may use it per wave (resets timer and kill feed) | PLAUSIBLE |

The client has no wave object: a wave exists for the client only as a goal list (0xc9 sub 1 / 5), the stage number
(0xc9 sub 1 `a`), texts (0xc9 sub 2 / 3) and timers (PROVEN negative for the handlers listed; GM-MODES §6.2).
Mission captions with a speaker exist in the co-op HUD layout (keys starting `CoopMissionCaption`, cshell+0x7f290);
which message drives them was not traced (UNPROVEN, V-FT-12).
Goal labels (gap G-6, folded by spec-fold-8): whether the difficulty row's `MissionList<k>` values are texts, CRes ids or picture paths is open (OBSERVED: ROM, value kinds
only: some look like paths, some like plain text). Slice 1 sends no goals and the wave number in 0xc9 sub 1 `a` (FT-S7).

### 2.5 0xdb and 0xd4 in co-op

- 0xdb `GS_RESPAWN_TOKEN` drives `hud_respawn_token` (S2C §3.5, C2S §7.6). Its element states (cshell+0xba5b0)
  carry layout key stems that read, in our words: 1 asking to use, 2 purchase, 3 cancel, 4 respawning, 5 cannot use,
  6 **everyone died**. Server sub 3 sets state 6; the client answers with C2S 0xdb sub 5 when the player presses the
  use key in state 1 or 6 with a token. PLAUSIBLE: the co-op "all players dead, use a respawn token?" prompt.
  DeCombat has no token rule (DC-C2S-9 (proposed): refuse); a co-op server that never sends 0xdb loses nothing
  visible (PLAUSIBLE).
- 0xd4 `GS_RESPAWN_TIMER` is not class-5 specific (S2C §7.1); not needed.

### 2.6 Result screen for co-op (PROVEN cshell+0x4a608.., DISASM cshell+0x4a72e..0x4a8a0)

0x46 in class 5 reads `[stage:u8][c1..c6:u32]` and for `stage` 2 the name list, title, text and panel time (S2C
§2.1.2, layout unchanged). Re-read details:

- `stage` selects the result banner of `hud_coop_result` through cshell+0x76ee0: 2 -> banner 0, 3 or 4 ->
  banner 1, 5 -> banner 2; other values leave the previous banner. The banners are loaded in that order from the
  `[CooperativeMode]` keys `Result_Complete_Tex`, `Result_Failed_Tex`, `Result_TimeOut_Tex` (cshell+0x77400). So
  `stage` 2 = mission complete, 3 / 4 = failed, 5 = timed out (PROVEN by key order; values 0 / 1 UNPROVEN).
- The six values go to panel fields through seven one-line setters (cshell+0x76ab0 -> +0xf8, 0x76a50 -> +0xe0,
  0x76a60 -> +0xe8, 0x76aa0 -> +0xf4, 0x76a90 -> +0xec, 0x76a70 -> +0xf0, and cshell+0x76a80 -> +0xe4 with
  `c1 - c5 - c4`), and the panel counts them up (cshell+0x79350..0x796a3). The panel's labels are the layout keys
  of kills, mission score, survival score (with a remaining-life multiplier picture), time score (minutes and
  seconds) and total; which field is which label is UNPROVEN (V-FT-11).
- 0x65 in class 5 adds `[c1:u32][n:u32] n x [bool1] [c2:u32][c3:u32][c4:u32]` to the record (S2C §3.4). The meaning of the `n x bool1` flags is unknown (gap G-8, no
  evidence); slice 1 sends `n` = 0.
- `win_team` of 0x46 in class 5 (gap G-7): the class-5 branch does not use it for a banner (S2C §2.1.2) and no visible effect of its value was found; slice 1 sends 1 on
  success, 2 otherwise (FT-S8).
- The final match result screen (screen 7, cshell+0x1326b0 / 0x130220) has a class-5 variant (a completion mark
  picture and CRes string 0x1896 when the co-op data flags completion); not traced further (UNPROVEN).

### 2.7 Class-5 switches found (index)

cshell+0x8a420 (NPC bar), 0x9d4a0 (no kill feed), 0xcbea0 (respawn-count text), 0x96b30 (`hud_health` extra
elements), 0x7caf0 (co-op score banner), 0x1326b0 / 0x130220 (result screen), 0x5df50 and 0x60ac0 (respawn),
0x2c430 and 0x49a40 (message layouts), 0x81930 (a co-op caption element). Consistent with GM-MODES §3, re-found
by a search of both forms of the class field.

## 3. ROM data for a co-op server and what the client reads of it

Counts and key names are OBSERVED: ROM (printed by our crates from the user's files on 2026-10-25); no value is
copied. Section families are written `[Name<n>]`.

### 3.1 Co-op maps, GMS files and per-difficulty rows

- `ATTRIBUTES\GAMEROOM.TXT`: one mode row with `GameType` 5 (`[Mode2]`, key `COOP`, goal range 0..2); two map rows
  carry a `GMS_COOP` key: `[Map13]` and `[Map18]`. Their GMS entries in `GMS.rez` are `sha:ecd3bcab112036bf/18`
  (Map13) and `sha:67a4c5c13a338c34/21` (Map18); their worlds in `WORLDS.rez` (from the GMS `[MapInfo]` key `Map`) are
  `sha:f586d0806e7fcdfb/22` and `sha:224beb1d9e5a38c6/21`.
- Both GMS files: `[MapInfo]` (`Name`, `Comment`, `MaxPlayer`, `Map`), `[Mission0]` (`MissionMode`, `StartObject`,
  `StartTrigger`, `Controller`, two team weapon keys, two money keys), **three per-difficulty rows `[Object0..2]`**
  (33 keys each, among them `TimeLimit`, `LifeCount`, `MaxLifeCount`, `AIDamageFactor`, `ScoreFactor`,
  `MissionList<n>`, the map and radar picture keys starting `LargeMap` / `RadarMap`, team spawn / win / comment keys,
  `StartTrigger`, `EndTrigger`, `OffenceTeam`, `DefenceTeam`), `[ScoreTable]`, `[CooperativeModeScoreTable]`
  (11 damage and death score keys), `[CooperativeModeNPCNameTable]` (19 keys: `NPCNameBoss`, `NPCNameTriggerBoss<k>`,
  `NPCNameETC<k>`, `NPCGoalSetPrefix<k>`), `[CooperativeModeDialogMsg]` (32 keys `Msg<nnn>` / `Cap<nnn>`),
  `[MapExp]`, `[MapMoney]`.
- The Map18 file adds 20 `[AISpawnTable<n>]`, 3 `[AIHealthRateRoundTable<n>]` and 5 `[WeaponSpawn<n>]` sections;
  the Map13 file has none of them (its AI comes from world objects, 3.4).
- `[AISpawnTable<n>]` keys (gap G-2 of dserver-fireteam, folded by spec-fold-8; OBSERVED: ROM key names and counts): `Round`, `Spawner<k>` (spawner object names; all 400
  references resolve), `LimitMaxAliveNum`, `SpawnAIName<k>` (AI character object names, 80 / 80 resolve; not `[AIModel<n>]` rows and not attribute templates), `SpawnDelay<k>`,
  `SpawnNum<k>` (4 triples per row). Slice 1: one wave per row in `Round` order, spawned at the row's spawners.
- Three difficulty rows match the goal range 0..2 of the mode row, so PLAUSIBLY `goal` (0x7d) = row index.
- The NPC name table is **not** an index on the wire: the tail carries the caption text itself (1.3 step 1). A
  server that wants the ROM names reads them from this table and sends the text (PLAUSIBLE use of the table).
- `LifeCount` and `MaxLifeCount` of the difficulty row (gap G-10): FT-5 reads `LifeCount`; what `MaxLifeCount` caps is not shown (names only, UNPROVEN). Slice 1 uses
  `LifeCount` only.

### 3.2 AI tables in `ATTRIBUTES.rez` (section families with counts)

| file | families (count) |
|---|---|
| `AIBUTES.TXT` (67 sections) | `[Brain<n>]` 19, `[AttributeTemplate<n>]` 12, `[Stimulus<n>]` 22, `[DamageMask<n>]` 5, `[SenseMask<n>]` 2, single sections for senses, rules, reset timers, attraction, text ids and editor defaults |
| `AIGOALS.TXT` (148) | `[Goal<n>]` 53, `[GoalSet<n>]` 8, `[SmartObject<n>]` 87 |
| `AIVOICE.TXT` (18) | voice type, situation and per-kind voice sections |
| `RELATIONDATA.TXT` (15) | `[RelationSet<n>]` 5, `[RelationData<n>]` 5, `[RelationalObject<n>]` 4, one collective section |
| `COOPMISSIONS.TXT` (1) | one mission section |
| `MODELBUTES.TXT` | `[AIModel<n>]` 12 (26 keys each: `ModelFile`, `Skin0/1`, `RenderStyle0/1`, `Skeleton`, `AIName`, `AttachFace`, `AIFace`, armour and class-mark keys), `[AIFace<n>]` 9; also `[Model<n>]` 33, `[Overidable_Model<n>]` 40 |
| `WEAPONS.TXT` | `[Weapon<n>]` 297, of which 6 carry the AI burst keys (1.8) |

### 3.3 What the client reads (the mod surface)

PROVEN by the literal file list of the clean tree (every attribute path the client names: 29 files plus the screen
layout) and the readers cited:

| data | client reads it? | consequence for a server / mod |
|---|---|---|
| `AIBUTES`, `AIGOALS`, `AIVOICE`, `RELATIONDATA`, `COOPMISSIONS` | **no** (no path or section literal in the client) | server-only; a mod may change them freely |
| world objects other than lights (AI volumes, nodes, spawners, triggers) | **no**: the client scans the object list for light classes only (`FMT_DAT85.md`, engine+0xcea90) | server-only data, but the world file is shared with the client: a mod changes the server's view of the objects (overlay), not the file |
| the co-op GMS | **only to find the world**: the GMS loader cshell+0xe8aa0 parses the whole file (it names every key of 3.1), but its two callers (cshell+0x5a910, cshell+0x5bea0) use a stack-local instance, take the world and mission name and destroy it before returning | everything except `[MapInfo]` `Map` is server-only; the pictures and bounds the client draws come from the server in 0x45 (S2C §2.1.1) |
| `MODELBUTES` `[AIModel<n>]`, `[AIFace<n>]` | yes: loaded into the client model table (cshell+0x1e4760: `Skeleton`, `AIName`, `AIOnly`, `AIIgnoreBody`, `UnalertDamageFactor`, face keys); no use of the AI-only keys found | the AI body files are client files (rendered from the create's model block); the row keys are effectively fixed |
| `WEAPONS` | yes (weapon table; its checksum feeds the 0xac integrity answer, S2C §3.2) | AI weapon rows must not change in the client copy; server-side overrides are free |
| `LAYOUT.TXT` `[CooperativeMode]` | yes (HUD: NPC bar, result panel, mission captions, mode pictures) | client presentation |
| rank table (`SmallIcon<n>` keys) | yes (marker rank icon for `npc_rank`) | the server only picks the index |
| CRes strings 0x5a0, 0x1896 | yes | fixed client texts |

Server-chosen values the client only displays (the mod surface on the wire): `npc_caption` (any text), `npc_rank`,
`npc_team`, `npc_kind` (0, 1, 2, 4), the health fraction, lives (`b`), points, goals, timers, texts and the six
result values.

### 3.4 World objects of the two co-op worlds (OBSERVED: ROM, `dc-world`)

Class names by `sha256[:16]/len` with our role words. Map13 world: 3,017 objects in 38 classes; Map18 world: 399
objects in 32 classes. Only AI-related and gameplay classes are listed; the rest are lights, sounds, effects and
geometry helpers.

| class (hash) | our role | Map13 | Map18 |
|---|---|---|---|
| `da1d7de44f7660e9/8` | AI volume | 904 | 106 |
| `b7321ac39fb70c9e/8` | AI region | 4 | 1 |
| `43c6e371089f5553/11` | AI guard node | 230 | 0 |
| `5d075fec0fbec2ea/12` | AI patrol node | 183 | 8 |
| `e07d42a5d27238d6/15` | AI use-object node | 621 | 0 |
| `a58c4bb4deed79bb/8` | placed AI character | 266 | 12 |
| `89bb4efe4151b8b5/7` | spawner | 260 | 40 |
| `8b9c643731c94119/7` | trigger | 151 | 26 |
| `6d267057c5c0d640/13` | target trigger | 14 | 3 |
| `86ea234676d3a48e/15` | activate trigger | 4 | 0 |
| `9062c0280952f80f/14` | mission trigger | 1 | 0 |
| `45c6a1b82cfa6603/14` | message trigger | 1 | 0 |
| `6d0d76d5a79188c0/14` | caption trigger | 1 | 1 |
| `4c7eb79939e993f1/12` | timer trigger | 1 | 0 |
| `6c64a55461d65e44/12` | event counter | 1 | 2 |
| `892748f74ce86ab8/14` | startup command | 4 | 0 |
| `6fe7e0eb8921d10c/14` | player start point | 32 | 8 |
| `0878a7b251301f5d/14` | game control object | 1 | 1 |
| `c25f64269d13d012/16` | object spawn point | 0 | 2 |
| `4713c0097e04864f/9` | poison gas volume | 0 | 13 |

No class with a firing-node role exists in either world (the use-object nodes are the closest, PLAUSIBLE). Property
layouts of these classes were not part of this task. The properties a server needs (gap G-1 of dserver-fireteam, folded by spec-fold-8; OBSERVED: ROM, by hash key and
count): the spawner's target property `978354db0c00fc78/6` names a placed AI character object on every spawner of the Map13 world (260 / 260) and on 11 of the 40 of the
Map18 world; the placed AI character's body-row property `f406ea5ca130ffb5/13` equals an `[AIModel<n>]` `Name` on every one (266 / 12); its right-hand property
`c969c8aa86333373/9` (the same key as the weapon socket name) equals a `[Weapon<n>]` `Name` (260 / 260 and 8 / 12); a real health-maximum property `b6973b50181c00cf/12` is on
all; a template bool `0575f29df888a27e/8` is set on 260 / 8. Slice 1 reads them by hash (`world::coop_objects`).

## 4. Validation items V-FT-n and grade summary

### 4.1 Capture items (none exists; the reference stack has no AI)

All need a DeCombat server sending the forms of this spec to the real client in a class-5 room, captured with
`evidence/captap.py` and the user's GO (REFERENCE_TESTING §4), plus screenshots for the visual items.

| id | what to do and record | settles |
|---|---|---|
| V-FT-1 | create one AI (1.9) and keep it still | the create form is accepted (no error 0x2c, no disconnect); caption, rank icon and bar shown |
| V-FT-2 | AIs with `npc_kind` 0, 1, 2, 4 side by side | bar hidden for 0 and 4 (1.5); boss radar / map icon for 2; infected look for 4 |
| V-FT-3 | sub 0x24 with 1.0, 0.5, 0.29, 0.0 | bar width and the truncation (1.5) |
| V-FT-4 | record byte +0x14 = 0 vs plain count + AI row | any visible effect (1.8) |
| V-FT-5 | `npc_rank` 0, 1 and a high index | the rank emblem next to the caption (1.4) |
| V-FT-6 | remove an AI object with a kind-3 record; separately only hide it | stale HUD entries: ghost icon, nothing, or a fault (1.7) |
| V-FT-7 | shoot an AI | 0x82 with `hit_obj` = the AI (1.8) |
| V-FT-8 | type 0x06 shot effect with an AI shooter | muzzle flash and tracer from the AI (1.8) |
| V-FT-9 | 0xd7 sub 2 with `b` = 3, 1, 0; die each time | respawn text `b - 1`; 0x89 after 7000 ms only while `b` > 0 at the death (2.2, 2.3) |
| V-FT-10 | 0x6b `m1` = 2 and no sub 2; die | the 0x6b path arms (2.1) |
| V-FT-11 | 0x46 class 5 with six distinct values and `stage` 2, 3, 5 | label of each value, banner per stage (2.6) |
| V-FT-12 | 0xc9 sub 1 with `a` 1, 2; sub 2 / 3 texts | stage display; caption element source (2.4) |
| V-FT-13 | 0x7d `goal` 0, 1, 2 | the mode / difficulty picture (2.1) |
| V-FT-14 | one of the non-player bodies (43 or 71 nodes) with its own clips | animation with three trackers vs one (1.8) |
| V-FT-15 | AI with an empty name | no caption and no bar, radar icon still shown (1.4) |
| V-FT-16 | 0xdb sub 3 to a dead player | the "everyone died" token prompt (2.5) |

### 4.2 Grade summary

| item | grade |
|---|---|
| tail order, widths and object offsets (1.2) | PROVEN / DISASM |
| path selection (`roster_bound` 0 + `has_tail` 1) | PROVEN |
| health conversion x 100 with truncation (create and sub 0x24) | DISASM |
| +0x5ae is the NPC bar percent (reader slot +0x74, bar setter) | PROVEN (chain), DISASM (operands) |
| bar visible only for kinds 1 and 2, only in class 5, only for other-team entries | DISASM / PROVEN |
| kind 2 icons 8 / 3, kind 4 infected flag | PROVEN |
| kind meanings (normal, boss, infected) | PLAUSIBLE |
| empty name = no marker entry | PROVEN |
| `npc_rank` = rank-icon index | PROVEN (lookup), PLAUSIBLE (what it looks like) |
| sub 0x11 refresh does not re-run the init | PROVEN |
| HUD entries removed only by 0x6c / 0x65 | PROVEN; effect of stale entries UNPROVEN |
| no kill feed in class 5 | PROVEN |
| class-5 respawn armed only while own +0x1f8 > 0; 0x6b `m1` and 0xd7 sub 2 `b` write it | DISASM / PROVEN |
| `b - 1` shown as respawns remaining (CRes 0x5a0) | DISASM (call), OBSERVED (text meaning) |
| re-arm needs `a == b` and a dead or observer state | PROVEN |
| result `stage` 2 / 3-4 / 5 = complete / failed / time-out banner | PROVEN (key order) |
| six result values: field targets | DISASM; labels UNPROVEN |
| 0x7d `goal` = mode picture index; = difficulty | PROVEN / PLAUSIBLE |
| client does not read the AI tables, the GMS beyond world and mission name, or non-light world objects | PROVEN |
| AI body structure counts, class counts | OBSERVED: ROM |
| animation, weapon child, hits, shot effect for AI | PLAUSIBLE / UNPROVEN |

### 4.3 DeCombat proposals raised here (proposed; not in DC_DEFAULTS)

- **FT-C1:** the AI create form of 1.9.
- **FT-W1:** send health as `(percent + 0.5) / 100` (or an exact binary fraction) so the truncation shows the
  intended percent.
- **FT-L1:** lives in 0x6b `m1` at join and 0xd7 sub 2 `[n][n]`; on a death send the new count before 0x67 state 2.
- **FT-R1:** do not remove AI objects during a match; hide (flags) and reuse them until V-FT-6 shows that removal is
  safe.
- **FT-K1:** normal AI `npc_kind` 1, boss 2; 0 only for a deliberately bar-less NPC.

## 5. Where this spec differs from the design document

### 5.1 Against findings F1-F6 of `docs/FIRETEAM_AI_DESIGN.md` section 1

| finding | agreement | difference found here |
|---|---|---|
| F1 tail -> object offsets, path selection | agrees on every offset and on the branch condition | adds the widths (u8, u8, u8, u32, string of at most 257); `rec+0x28` is a rank-icon index into the level table (the design leaves its meaning open, V-FT-2 there); an empty name creates no marker entry; the NPC branch also runs for `roster_bound` 1 when the connection check fails (only outside a network session) |
| F2 health conversion | agrees on x 100.0 and the target +0x5ae | the helper truncates toward zero; the design says "rounded" |
| F3 sub 0x24 | agrees on layout, the `has_tail` condition and the target | the reader **was found** (class slot +0x74 = cshell+0x206d30, read every frame by cshell+0x8b2e0 and fed to the bar setter), so "health bar fill percent" is PROVEN, not PLAUSIBLE; for objects without the tail the 32 bits are not read |
| F4 kind byte | agrees on 2 -> radar 8 / map 3, 4 -> infected, else 6 / 1 | the kind also gates the **health bar**: only 1 and 2 show it (DISASM cshell+0x8b565..0x8b58b, cshell+0x6d350). The design's create table sends 0 for a normal AI, which shows **no** bar; send 1 |
| F5 model table | agrees on record size, order Model then AIModel, and the skeleton lookup answering 0xff for AI rows | a third group (`[Overidable_Model<n>]`) follows the AI rows (count at +0x15b8); +0x15b4 is plain + AI, not the end of the table |
| F6 class-5 respawn | agrees: armed only while own +0x1f8 > 0; sub 2 writes `b`, re-arms when `a == b` | the re-arm also needs own state 2, 3, 5 or 6; 0x6b `m1` writes the same field (initial lives without any 0xd7); the meaning of `b` is shown by CRes 0x5a0 (respawns remaining = `b - 1`), so it is a count including the current life, upgraded from PLAUSIBLE |

### 5.2 Corrections to sibling specs (for a later spec-fold)

| # | spec and place | correction | evidence |
|---|---|---|---|
| C-FT-1 | S2C §8.3 type 0x18 table, row "0x1d, 0x1e, 0x1f, 0x22, 0x25: nothing" | they read nothing but act: 0x1d clears the infected flag and resets (all objects), 0x22 sets +0x540, 0x1e / 0x1f / 0x25 are roster-only; 0x24 reads `[f32]` only for tail objects and sets the NPC bar percent | cshell+0x206260 |
| C-FT-2 | S2C §8.2.1 tail row | name the tail fields and their uses (section 1) | 1.2-1.5 |
| C-FT-3 | GM-MODES §6.2 "Which value feeds the bar's fill is UNPROVEN" | settled: object +0x5ae (create tail x 100, 0xa6 sub 0x24), visible for kinds 1 / 2 only | 1.5 |
| C-FT-4 | GM-MODES §6.2 "Kills of AI" | add: no kill-feed rows at all in class 5 | cshell+0x9d4a0 |
| C-FT-5 | GM-MODES §4.2 and S2C §3.5 row 0xd7 sub 2 | add the dead-state condition of the re-arm, the 0x6b `m1` path and the CRes 0x5a0 text | 2.2, 2.3 |
| C-FT-6 | S2C §2.1.2 co-op block | `stage` 2 / 3-4 / 5 banners; the result field targets of 2.6 | cshell+0x76ee0, cshell+0x77400 |

### 5.3 Gaps found by the first co-op implementation (dserver-fireteam slice 1; folded by spec-fold-8, 2026-10-25)

`FIRETEAM_2009_addendum_gaps.md` (now a stub) listed eleven places where this spec did not say what a server needed. Each is folded where it belongs; what slice 1 did is a
DeCombat choice (FT-S rows of the dserver-fireteam section of `DC_DEFAULTS.md`, PENDING), not 2009 evidence.

| gap | folded into | evidence grade |
|---|---|---|
| G-1 spawner and AI character properties | 3.4 (paragraph under the table) | OBSERVED: ROM (hash keys, counts) |
| G-2 `[AISpawnTable<n>]` keys | 3.1 | OBSERVED: ROM (key names, counts) |
| G-3 state a server may change after the create | 1.5 | PROVEN (1.5) |
| G-4 what "hidden" means for a character | 1.7 | none (UNPROVEN, V-FT-6) |
| G-5 no AI death clip | 1.8 | none |
| G-6 `MissionList<k>` value kinds | 2.4 | OBSERVED: ROM (value kinds only) |
| G-7 `win_team` of 0x46 in class 5 | 2.6 | S2C §2.1.2 |
| G-8 the `n x bool1` flags of 0x65 | 2.6 | none |
| G-9 the co-op row's default map | 2.1 (paragraph under the table) | OBSERVED: live round 1 log |
| G-10 `LifeCount` vs `MaxLifeCount` | 3.1 | names only |
| G-11 AI shooter in type 0x06 | 1.8 (weapon bullet) | OBSERVED (our client only); V-FT-8 open |

No grade of this file was changed; the gaps add notes, not findings.
