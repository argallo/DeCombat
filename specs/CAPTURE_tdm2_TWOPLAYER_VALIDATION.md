# capval-twoplayer: capture validation of the two-player TDM round tdm2 (addendum, staging)

- **Targets:** `specs/UDP_SESSION_2009_B.md` (sections 3, 4, 5, 10), `GAME_MESSAGES_2009_S2C.md` (sections 3, 4, 8,
  10, 11), `GAME_MESSAGES_2009_C2S.md` (sections 2, 3, 4, 5, 9, Appendix A, 12), `PROTOCOL_2009.md` section 7,
  `MOVEMENT_2009_addendum_client.md` (V-M12), `UDP_REFERENCE_FINDINGS_botclient.md`, `DC_DEFAULTS.md` (DC-C2S-10,
  PD-S2C-2, DSV-8, DSV-13, DSV-21).
  **Date:** 2026-10-25. **Status:** STAGED, for the coordinator to fold.
- **Evidence:** `captures/20261025_001409_tdm2/capture.jsonl` (captap format 1, 24,562 events,
  244.5 s, ROM fingerprint match). Client -> server bytes of the real client (conns 2, 6, 7) are 2009 evidence,
  grade **PROVEN-CAPTURE**. Server -> client bytes come from the reference stack, grade **REFERENCE**; where the real
  client's next message proves it parsed them (or kept the session without an error) the grade is **REFERENCE,
  ACCEPTED**. Bot1's own decoded view (`logs/Bot1.events.jsonl`) is used as a cross-check only. The lobby and server
  debug logs of the folder were not read (timestamps of the launcher event log only, section 5).
- **Static evidence used to interpret reference stack bytes:** the clean decompiler output (`CShell.dll`)
  and read-only capstone disassembly of the user's own `Game/CShell.dll` (method-table slots read from the ROM
  binary, PE-DATA, addresses only). Cited `cshell+0xRVA`.
- **Tools (scratch, not in the tree):** our own Python decoders written from `UDP_TRANSPORT_2009.md` sections 2, 6-10
  and 15, `UDP_SESSION_2009_B.md` sections 3-5 and the two game-message catalogues: datagram `check8`, control /
  reliable / unreliable blocks, in-order delivery with the 8 out-of-order slots and split reassembly, the id 8 record
  loop with every change field, the id 10 stream, every shell id used below. Every datagram of the capture passed
  `check8`; no reliable or unreliable block was abandoned; every split message reassembled; **all 5,194 / 2,732 /
  972 object-group messages of conns 5 / 6 / 7 decoded with every record's length equal to its `rec_bits`**.
- **ROM content:** none quoted. Session file ids are given as numbers with a role (body model, texture, render style);
  file names, world names and strings that the session carries appear only as `<name>` or not at all. Player names
  `player1` and `bot1` are user-typed nicknames.
- **Scope split:** the parallel validators capval-transport / capval-game cover the solo capture tdm1 (transport and
  session basics, single-player game messages). This addendum covers only what the two-player round shows, plus two
  cross-checks against tdm1 that the mouse-yaw question needed (section 5).

## 0. Link map and what happened

| conn | what | evidence of identity |
|---|---|---|
| 1, 2 | real client auth / lobby TCP (account 1001, nick `player1`, host of room 0) | 0x371 / 0x38b records, 0x301 session index 2 |
| 3, 4 | Bot1 auth / lobby TCP (account 1002, nick `bot1`) | same, session index 4 |
| 5 | Bot1's UDP link (local port 49234) | hello (seq 149, 2088 bits) carries `bot1`; 0x72 (seq 205) `char_no` 1002, `session_idx` 4 |
| 6 | real client's UDP link (49236), 57.437 .. 168.33 s | hello (seq 153, 2088 bits) carries `player1`; 0x72 `char_no` 1001 |
| 7 | **real client again** (local port 63893), 170.78 .. 219.92 s | hello (seq 16619) byte-identical to conn 6's; 0x72 (643 bits) byte-identical to conn 6's; TCP 0x392 (0, 1) at 170.781 on conn 2 answered with 0x393 + 0x3043 in the same millisecond |

Timeline in capture seconds: room start pushes 0x3043 to both accounts at 57.437; Bot1 in world at 58.77; the real
client in world at 63.70 (its file-list check took 5.7 s); play until the real client left the match at 155.84
(UDP 0xbd + TCP 0x394); the reference stack ended the round for the remaining team (0x46 at 156.17, 0x3f at 161.92); the real
client re-joined the still-running match at 170.78 (conn 7), stood for 32 s, left again at 210.48, returned to the
lobby and logged out at 222.38. **There was no kill and no death in this capture**: the real client fired 11 times
and none of the shots reported a hit (section 3.1, caused by the frozen yaw of section 5); a grenade at its own feet
did 53 damage and left it at 46.8 health (section 3.2).

## 1. The remote player object (V-S2C-1; B §3.3, §3.4, §4.1-§4.4; S2C §8.2)

### 1.1 Spec text under test

> S2C §8.2: "For a TDM server the relevant entries are PLAUSIBLY: 0x01 on every player character's create record (the
> client object that the roster binds with 0x6b) ... Which type the 2009 server attached to which object is UNPROVEN
> until a capture (V-S2C-1, section 11)."

> B §3.3 / V-B3: "creates of the player models: kind byte 1, model block counts within caps, `create_pos` equals the
> first `pos48` within one cell".

### 1.2 Bot1's character as created on the real client's link (conn 6), REFERENCE, ACCEPTED

Datagram seq 794, t = 63.656, one `S2C_OBJ_GROUP` of 18,664 bits; the record starts at message bit 5728,
`rec_bits` = 3668. Record head (24 bytes, LSB-first bit order as on the wire):
`8b 05 98 01 c1 00 00 00 00 00 00 00 00 00 00 00 00 44 02 18 01 00 00 0c`.

| field (B §3.3 / §4.2 order) | value | note |
|---|---|---|
| `cf` | 0x058b = `CF_NEW` 0x0001, `CF_POS` 0x0002, `CF_FLAGS` 0x0008, marker 0x0080, `CF_ATTACH` 0x0100, `CF_ROT_SNAP` 0x0400 | no `CF_ANIM`, no `CF_DIMS`, no `CF_SCALE`, no `CF_COLOR` |
| `obj` | 408 (0x0198) | Bot1's object; the same id on every link |
| `kind_byte` | 0xc1: `obj_kind` 1 (`OBJ_MODEL`), blob bit 0x40, 16-bit blob length 0x80 | |
| `create_pos` | (0.0, 0.0, 0.0) | **not** the position: the real position is the `pos48` below |
| `blob_bits` | 580 (u16) | shell object type byte 0x18 + 572 bits, section 1.5 |
| model block | `model_count` 2; entry 0: model file 16 (body model), `sub_count` 2 with sub file 17 (child model), `tex_count` 5: 10, 11, 12, 12, 12 (two body skins, then one mark texture three times), `style_count` 3: 13, 14, 15; entry 1: model file 20 (face model), `tex_count` 1: 21, `style_count` 2: 13, 13 | file ids of this session's `S2C_FILE_LIST`; every count within the caps of B §3.4 |
| flag block | `flags` 0x3803, `flags2` 0x0318, `uflags` 0x00401020, `flags3` 0 | bit 0x0800 (yaw only) set, bit 0x0100 (full precision) clear |
| position | `pos48` (31680, 34090, 36512), `vec72` velocity 0 | compact form (flag 0x0100 clear) |
| rotation | `quat128` (0, -0.7716, 0, 0.6361) | yaw-only quaternion, yaw -1.763 rad = the yaw of Bot1's 0x6a |
| attachments | one entry: child 409, socket 2, offset (0, 0, 0), rotation (0, 0, 0, 1); then the 0xFFFF end; then the 32 mask pairs of a kind-1 object, all zero | |

The same record went to conn 7 at 178.047 (seq 17353, bit 138610, identical bits). The real client parsed both (no
error, no disconnect; it kept sending updates and its own 0x87), so the layout is ACCEPTED; that it *rendered* the
model is the user's observation, not visible on the wire.

### 1.3 The weapon child object

Object 409 (conn 6, same datagram, bit 9428, 2629 bits): `cf` 0x05ab (`CF_NEW`, `CF_POS`, `CF_FLAGS`, `CF_ANIM`,
marker, `CF_ATTACH`, `CF_ROT_SNAP`), `kind_byte` 0x01 (kind 1, no blob), `create_pos` (0, 0, 0), model block 1 entry:
model file 23 (a weapon model), `sub_count` 1, `tex_count` 1: 22, `style_count` 1: 13; animation block: main tracker
only, `anim` 0, `tsel` 0; flags 0x0003 / 0x0010 / 0x00000010 / 0; the parent's `pos48` and rotation; an empty
attachment list plus 32 zero mask pairs. Head (the record is not byte-aligned in its datagram; its bits repacked from the record start): `ab 05 99 01 01 00 00 00 00 00 00 00 00 00 00 00 00 c1 05 40 10 58 00 44`.
The parent names it in its attachment list (child 409, socket 2). The weapon a player holds is shown by changing
this child's model (`CF_FILES` 0x0800 change records, section 3.4).

### 1.4 player1's character as created on Bot1's link (conn 5) and the join refresh

| t | record | `cf` | content |
|---|---|---|---|
| 63.531 (seq 733) | create 189 | 0x818b (`NEW`, `POS`, `FLAGS`, marker, `ATTACH`, `DIMS`) | `kind_byte` 0xc1, `create_pos` 0, blob 580 bits type 0x18, model block **one** entry (body model 16 + sub 17, 5 textures, 3 styles; no face), flags 0x0802 / 0x0318 / 0x00401020 / 0, `pos48`, attachment child 411 socket 2, `dims` (15, 50, 15) |
| 63.531 | create 411 (weapon child) | 0x01ab | weapon model 19 with texture 18, style 13; flags 0x0002 |
| 63.578 (seq 753) | change 189 | 0x2ba8 (`FILES`, `ANIM`, `FLAGS`, `POS_SNAP`, `ATTACH`) | flags -> 0x2802; model block **two** entries (the face model added); attachment list repeated |
| 63.578 | change 411 | 0x08ae | `CF_FILES`: weapon model 23 / texture 22 / style 13 (the drawn weapon) |
| 63.766 (seq 810, after player1's 0x87) | change 189 / 411 | 0x26a8 / 0x0008 | flags -> 0x3803 / 0x0003 |

The same three steps repeated for the re-join (object 411 re-created as player1's character at 178.016, weapon child
412; refresh at 178.109; flags 0x3803 at 178.187). Flag bits 0x0001 and 0x1000 are set when the player spawns and
0x2000 when its record is complete: PLAUSIBLE "visible" / "in play" (the client meaning of these bits is not traced).
The own character as created on its owner's link (conn 6, object 189, seq 794; conn 5, object 408, seq 250) carries
`CF_ANIM` (three trackers) and `CF_DIMS` (15, 50, 15), and no rotation for the view object (CD-67
agrees).

### 1.5 The type 0x18 shell blob: the character record

All 15 character blobs of the capture (creates and 0xa6 sub 0x11 repeats, section 1.7) are 580 bits: type byte 0x18,
then 572 bits. The class is the router's type 0x18 (factory case 0x18 -> cshell+0x220830 -> constructor cshell+0x206db0,
method table VA 0x374efea4, PE-DATA; `GAME_MODES_2009.md` §6.2). Its create slot +0x10 (cshell+0x203c70, PE-DATA slot
read from the ROM) reads the record with cshell+0x2b6aa0 and hands it to slot +0x0c (cshell+0x205f40), which copies
it into the object (cshell+0x202650). Read order of cshell+0x2b6aa0 (clean decompile; the width of the first read is
not visible in the C and is fixed by the capture: with 1 bit every one of the 15 records ends exactly at 572 bits):

```
[r08:u1]      record +0x08      observed 1 in all 15
[has_tail:u1] record +0x0c      observed 0 in all 15 (no tail)
[r10:u8] [r14:u8] [r20:u8]      +0x10, +0x14, +0x20   observed 0, 0, 0 (+0x24 := 0xff if r14 or r20 is 0xff,
                                                       else a lookup of r14, cshell+0x1e0d80)
[r2c:u8] [r2d:u8] [r34:u8]      +0x2c, +0x2d, +0x34   observed (1, 255, x) before the spawn, (3, 2, x) after it;
                                                       x = 1 for player1, 0 for Bot1
[r38:u64]                       +0x38                 observed 0
[r40..r54: 6 x u32]             +0x40 .. +0x54        observed r40 = 0x3f000000 (the bit pattern of 0.5) except
                                                       in Bot1's create to the two real-client links (0); rest 0
[box:vec96] [off:vec96]         +0x58, +0x64          observed (15, 50, 15) and (0, 0, 0)
[r70:u32] [r74:u32]             +0x70, +0x74          observed 0, 0
[r78:u1] [r79:u8] [ra4:u1]      +0x78, +0x79, +0xa4   observed 0, 0, 0
if has_tail: [r28:u8][r98:u8][ra0:u8][r9c:u32][name:zstr8(257)]   (absent in every player record)
```

Client use visible in cshell+0x205f40 (clean decompile): when +0x40 is non-zero and the connection check
cshell+0x1a0470 passes, the object takes the branch that attaches the chat effect to other players' characters;
otherwise, when +0x44 is non-zero, it adds a marker entry by itself (the NPC path of `GAME_MODES_2009.md` §6.2).
PLAUSIBLE therefore: +0x40 non-zero marks a roster player, and the tail (`has_tail` = 1, a name and three bytes and
a word) is the form for characters that have no roster record. All other field meanings: UNPROVEN.

### 1.6 Remote players in the object stream (B §4.3, §4.4)

Every `S2C_OBJ_STREAM` entry for a remote player had `uf` = 13 = `UF_POS` | `UF_YAW` | `UF_ANIM` (2732 / 2732
entries of object 408 on conn 6; 2733 + 1232 entries of 189 / 411 on conn 5), so the rotation travels as `yaw8`
(the object has flag 0x0800) and **every** entry carries an animation block of three trackers:

| tracker | `anim` values seen | `wset` | role (PLAUSIBLE) |
|---|---|---|---|
| main (no id) | 0 | (none) | base |
| 1 | standing Bot1: 96; player1: 96, 291, 101, 104, 98, 293, 107, 299, 296, ... | 0 | upper body pose (changes with the weapon and firing) |
| 2 | standing Bot1: 6; player1: 6, 42, 43, 38, 36, 37, 40, 41, 39, ... | 1 | lower body (stand / move directions) |

All trackers use the narrow form, `state0` = `state1` = 1, `tsel` 2 (10-bit `time8`, times growing by 32 ms per
entry). The weapon child objects never appear in the stream. Stream rate 29.4-29.6 messages per second on every link
(time entry last in each, B §9.1 confirmed for two links). Bot1 stood still: its `yaw8` stayed -71 (-1.756 rad, the
yaw of its 0x6a) in all 2732 entries; player1's `yaw8` moved with the server's notion of its yaw (section 5).

### 1.7 0xa6 notes to the type 0x18 object: what the 2009 client reads

The reference stack also addressed both characters with shell id 0xa6 `[note_kind:u8 = 0x18][obj:objref16][sub:u8]...`
(`GS_OBJECT_NOTE`, S2C §7.2 / §8.3), to every client, the owner included. The router passes the reader after `obj`
to the object's slot +0x30 = cshell+0x206260 (PE-DATA slot), which reads `sub:u8` and switches through a byte table
(DISASM cshell+0x2062a2..0x2062bf: `sub - 7` against 0x1e, byte table VA 0x373065cc, jump table VA 0x37306590).

| sub | handled by the 2009 client? | client reads | sent by the reference stack (count over the round, block) |
|---|---|---|---|
| 0x07 | yes | `[v:u8]` | - |
| 0x09 | yes | `[v:u8]` | - |
| 0x0c | yes | `[v:u64]` -> +0x70 | - |
| 0x11 | yes | the 572-bit character record again (member +0x38 slot +0x08) | at every join and spawn (R) |
| 0x17 | yes | `[text:zstr8(1023)]` | - |
| 0x18 | yes (only for an object that is not the own view object) | through cshell+0x202cd0 | - |
| 0x1b | yes | `[box:vec96][offset:vec96]` | 2 per character create (R) |
| 0x1d, 0x1e, 0x1f, 0x22, 0x25 | yes | nothing | 0x1f at each spawn (U), 0x25 once about 3.5 s after it (R) |
| 0x20 | yes | `[v:u64]` -> +0x530 | once, to the grenade victim (R; the reference stack wrote 80 bits, the client reads 64) |
| 0x24 | yes | `[v:u32]` | - |
| **0x08, 0x13, 0x14, 0x15, 0x1a** | **no** (table index 14 = the default exit) | nothing | 0x08 `[u8]` after each 0x87; 0x13 `[3]` at create; 0x14 / 0x15 `[u16]` (0 then 100) every ~2 s for each character (R, 47-88 each); 0x1a `[u8]` (666 x, U) |
| **0x40, 0x80, 0xc0** | **no** (above 0x25: default exit) | nothing | 0x40 `[3 x u8]` (50 x, U), 0x80 `[6 x f32]` = the six aux words of the shooter's 0x64 (593 x, U), 0xc0 `[6 x f32][3 x u8]` (91 x, U) |

So 1,594 of the 1,610 type 0x18 notes on the real client's link (2,004 of 2,028 on Bot1's) had no effect on the 2009 client. A server needs only
0x11 (record refresh), 0x1b (box) and, PLAUSIBLY, 0x1f / 0x25 (two latch bytes of the object, +0x539 / +0x53a with a
tick stamp: a spawn blink or protection window).

### 1.8 Verdicts and replacement text

- **V-S2C-1 SETTLED (REFERENCE, ACCEPTED):** player characters are kind-1 model objects with a **type 0x18** blob, not
  the inline type 0x01 record. Replacement text for S2C §8.2 last paragraph: "For a TDM server: every player
  character is a kind-1 model object (`kind_byte` 0xc1) whose blob is the type 0x18 character record (572 bits after
  the type byte, layout in the addendum capval-twoplayer §1.5), with a second kind-1 object for the held weapon
  attached at socket 2; weapon and item pickups are kind-1 objects with a type 0x17 blob (116 bits). Type 0x01 was not
  seen. OBSERVED: captures/20261025_001409_tdm2 (reference stack bytes the 2009 client accepted)."
- Add to S2C §8.2 table a row: "0x18 | `OBJ_CHAR18` (our name) | class-read: record of cshell+0x2b6aa0 (§1.5 of the
  addendum) | factory case 0x18 -> cshell+0x220830".
- Add to S2C §8.3: "Type 0x18 objects read 0xa6 sub-ids 0x07, 0x09, 0x0c, 0x11, 0x17, 0x18, 0x1b, 0x1d, 0x1e, 0x1f,
  0x20, 0x22, 0x24, 0x25 (cshell+0x206260, DISASM jump table); every other sub-id is ignored."
- **V-B3 PARTLY FAILS for the reference stack:** kind byte 1 and the caps hold; `create_pos` is (0, 0, 0) for every character
  and weapon object (the true position is the first `pos48`). Replacement pass condition: "kind byte 1, model block
  counts within caps; `create_pos` is not checked (the reference stack writes zero); the first `pos48` lies inside the world
  box".
- **B §4.3 note to add:** "OBSERVED (reference stack, accepted): remote player characters carry flag 0x0800 and are streamed
  with `uf` = 13 in every entry: `pos48`, `yaw8` and a three-tracker animation block (main, upper body tracker 1 /
  weight set 0, lower body tracker 2 / weight set 1)."

## 2. A second player joins: roster and identity sequence (S2C §3.2.1, §10 steps 1-10; C2S §5.1, §5.2, §5.5, A2-A4; PROTOCOL_2009 §7)

### 2.1 Spec text under test

> S2C §10 step 5: "0x6b `GS_PLAYER_INFO` (3.2.1) for every player in the match including the joining one (kind 1;
> kind 0 for a player who joins a running match, to the others, so they get the join notice); each only after that
> player's character object exists on the receiving client."

> S2C §3.2.1 field 14: "0 = new player with notice, 1 = add or refresh (cshell+0x2a950), 2 = update (cshell+0x2b820,
> same argument set in another order), other = no record change".

> PROTOCOL_2009 §7 step 4: "... its 0x392 (0, 1) must be answered with 0x393 ok + 0x3043 (LIVE)."

### 2.2 Starting room (both accounts got 0x3043 at 57.437; the real client is the slower loader)

| t | link | message (head) | note |
|---|---|---|---|
| 57.437 | TCP conn 2 / 4 | 0x3043 to both, same millisecond | after 0x3035 {7, 2} and 0x3042 {id, 3} |
| 57.484 | 6 | `0d 90 01`, `0d 88 02`, S2, S3, file list | first reliable block of the link |
| 58.766 | 6 | `0d 6b 62 6f 74 31 00 00 ea 03 00 00 30 03 ...` (bot1, pid 0, kind 2, dead 0) | Bot1's spawn refresh, broadcast to every link, **4.4 s before this client loads the world** (S6 at 63.187) |
| 58.937 | 5 and 6 | `0d 90 00`, `0d 88 03` | broadcast when the world started |
| 63.531 | 6 | 0xa6 0x1b x2 / 0x13 for its own object 189, `0d 72`, 0x9e, 0x9d, 0x7d, `S2C_VIEW_OBJECT` 189 | |
| 63.531 | 6 c2s | `0b 72 00 00 00 00 e9 03 00 00 02 00 ...` (643 bits) and `0b 85 02 01` | same datagram |
| 63.531 | 5 | create 189 (pre-spawn form, §1.4) and 411; 0xa6 0x14 / 0x15 / 0x11 | the others see the joiner's object first |
| 63.578 | 5 | `0d 6b 67 61 6c 6c 6f 00 01 e9 03 00 00 7a 01 ...` **kind 0, dead 1**, then the same record **kind 2, dead 0**; change 189 / 411 (face model, drawn weapon) | **47 ms after the joiner's 0x72, 125 ms before its 0x87** |
| 63.594 | 6 | own record kind 2 dead 1; bot1 kind 2; 0x6a; `0d 67 01`; 0x66; 0xb2 / 0x7f x4 (twice); 0xa0 body; 0xa0 teleport seq 1; own record kind 2 dead 0; 0x43; 0x44; 0x45; 0xb5 | the whole spawn set **before** 0x87; no 0x67 state 3 |
| 63.656 | 6 | object group with the world objects, 408 + 409 (Bot1, §1.2), 411, 189 (own) | **after** both 0x6b that name 408 and 189 |
| 63.703 | 6 c2s | `0b 64 10 00 01` (reliable state form, state 1), `0b 87`, 0xca | exactly one 0x87, 109 ms after `0d 67 01` |
| 63.766 | 5 and 6 | flags 0x3803 on 189, 0xa6 0x08; to 6: 0xa0 teleport seq 2, 0x66 kinds 1, 2, 3 | the client echoes `move_seq` 2 from 63.797 |
| 64.172 | 6 c2s | `0b 8c 00 00 00 00`, `0b 8c 01 00 00 00` | roster queries for pid 0 and pid 1 |
| 64.234 | 6 | 0x6b bot1 kind 2, 0x6b player1 kind 2 | the answers, to the sender only |

### 2.3 Joining the running match (conn 7)

| t | link | message | note |
|---|---|---|---|
| 170.781 | TCP conn 2 | c2s 0x392 (0, 1); s2c 0x393 ok, 0x3042 {1001, 3} (to both accounts), 0x3043 | same millisecond: PROTOCOL §7 step 4 confirmed again |
| 170.828 | 7 | `0d 90 00`, `0d 88 03`, S2, S3, file list | a running world: 0x90 0 and 0x88 3 at once (a starting world gave 1 and 2 first) |
| 172.47 .. 176.52 | 7 | 0xa6 [0x18][408] 0x14 / 0x15 every 2 s | before the world load; ignored by the client (§1.7) |
| 178.016 | 5 | create 411 (player1's new character, pre-spawn form) + 412 | the id 411 was freed at 156.172 (section 4) and is re-used |
| 178.047 | 7 | 0x72 request, 0x9e, 0x9d, 0x7d, view object 411, object group with 408 / 409 **before** any 0x6b | |
| 178.047 | 7 c2s | 0x72 (643 bits, byte-identical to conn 6's) + 0x85 `0b 85 02 01` | |
| 178.109 | 5 | `0d 6b 67 61 6c 6c 6f 00 01 e9 03 00 00 36 03 ...` kind 0 dead 1, then kind 2 dead 0 | 62 ms after 0x72, 63 ms before 0x87, 7.33 s after 0x3043 |
| 178.109 | 7 | own record kind 2 dead 1, bot1 kind 2, 0x6a, `0d 67 01`, 0x66, weapons, 0xa0 x2, own record kind 2 dead 0, 0x43, 0x44 `0d 44 00 00 00 00 01 00 00 00 01 00 00 00 00`, 0xb5 | **no 0x45** for a joiner of a running round |
| 178.172 | 7 c2s | `0b 64 10 00 00` (state form, state 0), `0b 87` | one 0x87, 63 ms after `0d 67 01` |
| 178.187 | 5 and 7 | flags 0x3803 on 411; 0xa0 seq 2; 0x66 | |

No 0x8c was sent on conn 7, where every 0x6b named an object that already existed on the client; on conn 6, where
both 0x6b records arrived before the objects they name (58.766 / 63.594 vs the creates at 63.656), the client queried
both pids. PLAUSIBLE: the client's lookup by object (cshell+0x29fb0) failed and asked (C2S §5.5).

### 2.4 The real client's own identity messages (PROVEN-CAPTURE)

`UP_PLAYER_INFO` 0x72 parsed with the layout of C2S §5.1 to the last bit (643 bits incl. ids, 0 left), identical on
both links:

| field | value | cross-check |
|---|---|---|
| `room_slot` | 0 | the host's slot |
| `char_no` | 1001 | account / character number of 0x303 |
| `session_idx` | 2 | equals 0x301 +0x1e of the same login (bytes 30..31 of the 0x301 body) |
| `exp` | 0 | new character |
| `look` | 15 bytes `00 00 00 00 00 01 00 00 00 01 01 ff 01 00 ff`, both strings empty | the reference stack forwarded these 15 bytes unchanged into player1's 0x6b look block (C2S §5.1 "PLAUSIBLE pairing": confirmed for the reference stack) |
| `r1`..`r5` | -1, 0, 0, -1, 0 | |
| `team` | 1 | the lobby team |
| `r6`, `r7` | 0, 0x442e0000 | |
| `gp` | 10000 | 0x303 GP |
| `n` + loadout | 4: (145, ff, ff, ff), (164, ff, ff, ff), (0, ff, ff, ff), (7, ff, ff, ff) | the same four weapon numbers the reference stack then gave with 0xb2 |
| `in_clan` | 0 | |
| `word_d`, `nation`, `flag_e`, `flag_f` | 0, 0, 0, 0 | |

`UP_CLIENT_OPTIONS` 0x85 = `[opt_a:u8 = 2][opt_b:bool1 = 1]` (25 bits), right after 0x72 in the same datagram, on
both links. `UP_ROSTER_QUERY` 0x8c carries real pids (0 and 1), not 0xFFFFFFFF.

### 2.5 Verdicts and replacement text

- **Correction to S2C §3.2.1 field 14 (clean decompile cshell+0x2b820):** kind 2 looks the pid up and, when there is
  no record, calls the add function cshell+0x2a950 itself. Replacement: "2 = add or update without a notice
  (cshell+0x2b820 updates the record of `pid` in place and falls back to cshell+0x2a950 when it has none)".
- **OBSERVED order (reference stack, accepted)** for S2C §10 steps 5-10 / C2S A3-A6: the joiner receives kind 2 for itself
  (first with `dead` = 1, again with `dead` = 0 after its spawn set) and kind 2 for every other player; every other
  client receives the joiner's kind 0 (notice, `dead` = 1) directly followed by kind 2 (`dead` = 0), in the
  starting room as well as in a running match, after the joiner's 0x72 and before its 0x87. Kind 1 was never sent.
  The spawn set and `GS_PLAYER_STATE` 1 come **before** 0x87 (the client accepted state 1 as the in-world condition
  d). DC-C2S-10's state 3 before 0x87 stays a valid DeCombat choice (both pass the in-world test).
- **S2C §10 step 5 condition reinforced:** sending 0x6b before the character create made the 2009 client ask with
  0x8c; the correct order (conn 7) did not. Keep the rule "each only after that player's character object exists on
  the receiving client".
- **S2C §10 step 11 note:** the reference stack sent 0x45 only to the players of the round's first spawn; a joiner of a
  running round got 0x43, 0x44 (`round_secs` 0, `round_no` 1, `round_limit` 1) and 0xb5 and no 0x45. Both are
  accepted; which one the 2009 server used is still open.
- **0x7d as the reference stack filled it (not a rule source):** `lobby_caption` = the receiving player's own nickname,
  `unused` = 1 for the room host and 0 for the guest, `own_team` 0 for both, `goal` 100, scores 0 / 0,
  `round_limit` 1, `round_no` 1.
- **C2S §5.1 / V-C2 (second client):** fields 1-4 equal the lobby values (session index = 0x301 +0x1e); the client
  re-sends a byte-identical 0x72 when it joins again.

## 3. Shots, hits, the self-inflicted damage and the weapon relays (C2S §4, §9, U5; S2C §4.3, §10 steps 14-18; DC-C2S-10, PD-S2C-2)

### 3.1 Spec text under test

> C2S §4.1: "A miss is a message with `has_hit = 0` ... **Server answer:** ... to the victim the damage message
> (server 0x74 ...), to everyone the health / kill / kill-feed messages ..., and to the other clients the shot effect
> so they see muzzle flash and tracer (shell object messages ...; which effect id the 2009 server used is UNPROVEN)."

> C2S §12.1 U5: "which server message relays 0x80 / 0x81 / 0x76 / 0x7f / 0xcc to other clients".

### 3.2 The real client's 11 shots (PROVEN-CAPTURE)

Every `UP_FIRE` parsed with C2S §4.1 to the last bit (623 bits each). Common to all 11: `wkind` 0, `last` 1,
`has_hit` **0**, `has_extra` 0, `flag_c` 0, `moving` 0, `wflag` 0, `mode1` 0; the field C2S §4.1 calls `shot_no` held
0x3f800000 (the bit pattern of 1.0) in all 11, and in tdm1 0x3f800000 for weapon 145 and 0x3f99999a for weapon 164:
PLAUSIBLY a per-weapon float, not a counter; `stat` went 2, 4, 6, ... 22 (+2 per message; tdm1 the same).

| t | weapon / ammo | `dir` (x, y, z) | heading atan2(x, z) | `yaw` field | `frac` |
|---|---|---|---|---|---|
| 101.187, 101.437 | 0 / 0 | (0.580, 0.027, 0.814), (0.578, 0.099, 0.810) | 35.5°, 35.5° | 2426654.75 | 0, 0 |
| 109.547 | 7 / 9 (grenade) | (0.209, -0.933, 0.293) | 35.5° | 2426654.75 | 0 |
| 111.469 .. 155.766 (8 shots) | 145 / 145 | x 0.528 .. 0.587, y -0.488 .. -0.088, z 0.695 .. 0.821 | 34.9° .. 37.2° | 2426654.75 | 25 .. 63 |

Head of the first: `0b 82 00 00 00 00 00 84 cb 76 44 8c 54 4e 44 63 73 b7 44 84 cb 76 44 8c` (seq 6737). The `yaw`
field is the controller's yaw in radians, unwrapped (tdm1: 5.05, 6.35, 7.80 ...); here it is the stuck value of
section 5, and every shot left in the same horizontal direction. **No shot could hit Bot1, so no hit, no damage to
Bot1, no 0x6d and no 0x4a occur in this capture.**

### 3.3 The server's answer to a miss: the shot effect relay (REFERENCE, ACCEPTED)

For each of the 10 bullet shots the reference stack sent one engine `S2C_SHELL_OBJ_MSG` (id 17, no object) of effect type
0x06 to **every** link, the shooter's included, in the **unreliable** block, 0-47 ms after the 0x82. Head (seq 6744):
`11 06 ff ff bd 00 00 00 00 00 6e 10 5e 01 84 cb 76 44 8c 54 4e 44 63 73`. Type 6 is a static client object (router
factory case 6 returns the object at VA 0x3759c4c8; its method table VA 0x374f03a4 is set by cshell+0x2160c0;
create slot +0x10 = cshell+0x2c6770, PE-DATA). Its read order (DISASM cshell+0x2c6770..0x2c68f6) matches all 11
relays bit for bit (8 + 416 bits):

```
[type:u8 = 0x06]
[first:objref16]    observed 0xFFFF (pistol shots, grenade blast) or 0x0000 (rifle shots)
[shooter:objref16]  the shooter's character (189)
[weapon:u16][ammo:u16]                      as in the 0x82
[a:u8]              observed 110, 71, 20, 10          (cleared by the client when a global flag is set and a is 4 or 6)
[b:u16]             observed 0x5e10, 0x4f10, 0x4f00, 0x59f0
[c:u8]              observed 1
[from:vec96]        the 0x82 weapon_pos, exactly (for the grenade blast: the throw origin)
[to:vec96]          the reference stack's own trace end: origin + 104.0 * dir for the pistol shots, 314 .. 1340 units for
                    the rifle shots, the blast point for the grenade
[normal:vec96]      unit vectors: (0, 1, 0) on floors, (0, 0, -1) on walls
[d:u8][e:u8][f:u8][g:u8]   observed 0x00, 0xff, 0xff, 0xff
```

Meanings PLAUSIBLE: `first` = what the trace hit (0 the world, 0xFFFF nothing), `a` / `b` impact and surface
selectors. Because the client reported no hit point, `to` and `normal` came from the server's own world trace.

### 3.4 The grenade at the own feet (PROVEN-CAPTURE client side, REFERENCE server side)

| t | dir | message |
|---|---|---|
| 107.437 | c2s | 0x76 cat 3 event 2, `0b 7f 03 07 00 09 00` (slot 3, weapon 7, aux 9) |
| 107.484 | s2c 5, 6 | `CF_FILES` on the weapon child 411: model 25 / texture 24 |
| 108.687 | c2s | `0b 80 0a 07 00 01 96 3a c6 c3 ...` (sound 10, weapon 7, own id 1, position) |
| 108.719 | s2c 5, 6 | id 17 type 0x19 `[0x0a][7][1][pos]` |
| 108.875 .. 109.344 | c2s | 0x4d `UP_THROW_RELEASE` x 8, about one per player-update frame |
| 109.547 | c2s | 0x82 weapon 7 / ammo 9, dir down (0.209, -0.933, 0.293); 0x76 cat 3 event 2; 0x7f back to (0, 145, 145) |
| 109.578 | s2c 5, 6 | create 413: kind 1, blob 32 bits `13 07 09 01` (type 0x13), model 25 with texture 24 and no style, `cf` 0x85bf, flags 0x1201 / 0x0100 / 0x00400020, scale (2, 2, 2), `pos48` with velocity (324.5, -1498.7, 454.4), rotation, `dims` (6, 6, 8); id 17 type 0x19 with `k` 0x12 (`[u8 6][u32 1][vec96][u8 1][u8 0]`); `CF_FILES` back to model 23; to the thrower 0x66 kind 1 (slot 3, ammo 0) |
| 112.062 | s2c 5, 6 | removal record [413]; id 17 type 0x06 with weapon 7 / ammo 9, `first` 0xFFFF, `to` = the blast point |
| 112.062 | s2c 6 only | 0xa6 [0x18][189] sub 0x20; `0d 74 9c 65 04 bf df 25 d2 3c a8 2f fa bd 07 01 ff ff 00 07 00 35 00 00` = `from_dir` (-0.517, 0.026, -0.122), `unused` 7, `mode` 1, `ammo_kind` 0xFFFF, `dmg_kind` 0, `zone` 7, `part` 0, `damage` 53; `0d 66 02 00 00 85 30 3b 42` = health 46.797 |

The real client survived; no 0x6d, no 0x67 2 / 3 and no respawn followed. **Not observed in tdm2:** player states 2,
3, 6; `UP_RESPAWN_REQUEST` 0x89 / 0xc5 / 0xd2; 0xd7 sub 2; 0xdb sub 6; 0xd4; 0x6d; 0xc8; 0x4a; a respawn 0xa0 after
7000 ms. DC-C2S-10 and PD-S2C-2 stay as proposed (tdm1's self-kill is their
capture).

### 3.5 Weapon and movement events and their relays (U5 DECIDED from conn 5)

| client event (PROVEN-CAPTURE, counts on conns 6 / 7) | what the other player's link (conn 5) received | to the sender too? | block | layout |
|---|---|---|---|---|
| 0x80 `[sound_no:u8][weapon_id:u16][own_id:u8][weapon_pos:vec96]` (3 / 0) | id 17 type 0x19, 32-46 ms later, 3 of 3 | yes | U | `[0x19][sound_no:u8][weapon_id:u16][player:u32][pos:vec96]` (168 bits): the client message with `own_id` widened to 32 bits; the 2009 inline reader of type 0x19 (`[k:u8]`, then for `k` != 0x12 `[u16][u32][vec96]`, S2C §8.2) reads it bit for bit. Head `11 19 03 91 00 01 00 00 00 8b cb cc c3 ...` |
| 0x76 cat 1: event 1 jump x 13, event 2 fall x 3, event 3 land x 16 | id 17 type 0x19 with `k` 0x0f, `u16` 0, `player` 1, `pos` = the landing point: 13 relays for 16 landings, none for jumps or falls | yes | U | same type 0x19 form. Head `11 19 0f 00 00 01 00 00 00 e1 b3 45 43 ...` |
| 0x76 cat 3 event 2 (7 / 4, one with every 0x7f) | nothing of its own | | | |
| 0x7f `[slot:u8][weapon_id:u16][aux:u16]` (7 / 4) | `CF_FILES` (0x0800) change record of the weapon child object (model, texture, style of the new weapon), 3 of 3 changes during play, within 47 ms | yes | R (object group) | B §4.2 item 1 |
| 0x82 (11 / 0) | id 17 type 0x06, section 3.3 | yes | U | section 3.3 |
| 0x4d (8 / 0) | nothing of its own; the projectile object of the following 0x82 | | | |
| 0xca `[sprinting:bool1][stopped:bool1]` (34 / 0: (1, 0) x 16, (0, 1) x 17, (0, 0) x 1) | nothing | **only** the sender gets s2c 0xca `[flag:bool1]` = its `sprinting` bit, 16-47 ms later, 32 times | R | `0d ca 01` / `0d ca 00` |
| 0x81, 0x83, 0x76 cat 5, 0x76 cat 3 events 1 / 3 / 5, 0xcc, 0xd5 | not sent by the real client in this round | | | |

Correction to C2S §4.4: "`aux`: a value from the new weapon (slot +0x94 or +0x7c; PLAUSIBLY its select animation or
clip state; UNPROVEN)" -> "`aux`: the ammunition id of the new weapon (OBSERVED: (145, 145), (0, 0), (7, 9), the same
pairs as the server's 0xb2 `weapon` / `ammo_kind`)". Correction to C2S §2.3 server answer "none": "the reference stack echoes
`sprinting` back to the sender as 0xca (`GS_OWN_FLAG`, own record +0x1f0); not required by the client (meaning of
+0x1f0 UNPROVEN)".

Replacement for U5: "DECIDED (reference stack form, accepted by the 2009 client): 0x80 -> `S2C_SHELL_OBJ_MSG` type 0x19
`[sound_no][weapon_id][player:u32][pos]` to everyone; a landing (0x76 cat 1 event 3) -> type 0x19 `k` 0x0f with the
player's position; 0x7f -> `CF_FILES` on the attached weapon object; 0x82 -> type 0x06 shot effect (section 3.3);
0x76 cat 3 -> no relay; 0x81 / 0xcc -> not observed."

## 4. Leaving and the end of the match (V-S2C-3; S2C §2.1, §3.4, §6.1, §10 steps 19-22; C2S §5.7; PROTOCOL_2009 §7; DSV-13)

### 4.1 Spec text under test

> S2C §11.2 item 7: "Whether 0x46 is sent at the end of a single-round TDM match, and the order of 0x65 / 0x3f / 0x99
> at match end (section 10 steps 19-22 are a consistent proposal, not observed)."

> C2S §5.7: "After the send the client sets the two leaving latches ... **Server answer:** none; remove the player at
> once ... The engine disconnect (`C2S_BYE`) follows."

> PROTOCOL_2009 §7 step 3: "On a failure it returns to the waiting room ... and sends 0x394 (LIVE: twice, +6 = 0 then 1)."

### 4.2 First leave: the remaining team wins by forfeit

| t | link | message (head) |
|---|---|---|
| 155.844 | UDP 6 c2s | `0b bd` (`UP_LEAVE_MATCH`) |
| 155.844 | TCP 2 c2s | 0x394 `94 03 0a 00 00 00 00 00 00 00` (+6 = 0); the lobby pushes 0x3042 {1001, 0, 0} to both accounts at once |
| 155.859 | UDP 6 c2s | `06` (`C2S_BYE`) |
| 155.875, 155.969, 156.078, 156.172 | UDP 6 s2c | `CTL_BYE_CODE`(5), four copies about 100 ms apart (O11 of the bot findings, now with the real client) |
| 156.172 | UDP 5 s2c | `0d 6c 01 00` (pid 1, not kicked), `0d 46 03 00 00 00 02 00 00` (`round_secs` 3, `win_team` 2, wins 0 / 0), removal record [189, 411] |
| 158.891 | UDP 5 s2c | `0d 47 00 00 00 00`; Bot1's own 0x6b (kind 2); `0d 65 00 98 01 00 00 00 00 2b 00 00 00 ...` (pid 0, `objref16` **408**, `exp_before` 0, `exp_gain` 43, `exp_bonus` 0, `gp_gain` 34, `gp_bonus` 0, flag 0); `0d 48 03 00 00 00` |
| 158.891 | TCP 4 s2c | 0x3004 to account 1002: exp total 43, GP total 10034, popup flag 1; then 0x345 (balance) |
| 161.922 | UDP 5 s2c | `0d 3f 04 00 00 00 00` (`result_flag` 0, `result_value` 2) |
| 156 .. 168 | UDP 6 c2s | the leaver's link keeps acknowledging |
| 168.016 | TCP 2 c2s | 0x306 {1} (waiting room), 0x388, 0x38a |
| 168.016 .. 168.328 | UDP 6 c2s | `CTL_BYE` x 4 (the client closes its link when it is back in the waiting room) |

Not sent at all: 0x41, 0xb5, 0x4a, 0x99, a bye to Bot1. The reference stack kept the world running and accepted the re-join.
The leaver's account received **no** 0x3004.

### 4.3 Second leave (the round already over)

`0b bd` + TCP 0x394 {0} at 210.484, **no** `C2S_BYE`; the reference stack did nothing until the client's `CTL_BYE` x 4
(219.625 .. 219.922, together with TCP 0x306 {1}); then `0d 6c 01 00` and the removal record [411, 412] to Bot1 at
219.656. No 0x46 this time. At 222.375 the client logged out (0x304 / 0x305); the lobby told Bot1 (0x3034, 0x3035).

### 4.4 Verdicts and replacement text

- **V-S2C-3 SETTLED for a forfeit end (REFERENCE):** order `0x6c` + `0x46` + removal in one datagram; 2.7 s later `0x47`,
  the own 0x6b, one `0x65` per remaining player and `0x48`; 3.0 s later `0x3f`; no `0x99`. The 0x3f fields:
  `result_flag` 0, `result_value` 2 = PLAUSIBLY the winning team. A goal or timer end was not observed; the receiver
  of these messages was the bot, so the client's display of them is not covered by this capture.
- **Correction to S2C §3.4 (0x65):** the second field is not unused: the reference stack put the player's character object
  there (408). Replacement: "`[pid:u8][char:objref16]` (the player's character; the client does not read it further)".
- **PROTOCOL_2009 §3.7 / §7 (0x394):** add "LIVE (tdm2): the client also sends 0x394 with +6 = 0 in the same
  millisecond as `UP_LEAVE_MATCH` 0xbd, on every leave (2 of 2)". The name `match_join_failed` is too narrow;
  proposal: `match_exit` (gave up or left a match).
- **C2S §5.7:** "The engine disconnect (`C2S_BYE`) follows" -> "the engine disconnect follows either at once
  (`C2S_BYE`, first leave, 15 ms) or only when the client is back in the waiting room (transport `CTL_BYE` x 4 with
  TCP 0x306 {1}, second leave, 9.1 s later)". The reference stack removed the player only at the end of the link, never on
  0xbd; DeCombat's "remove at once on 0xbd" is the safer choice and stays.
- **DSV-13 / S2C §10 step 22:** the reference stack sent no 0x99 after a forfeit end; DSV-13's 0x99 hold is neither confirmed
  nor contradicted.
- **Results for both accounts:** only the account still in the match got 0x3004 (2.7 s after 0x46, in the same
  millisecond as its 0x65); the leaver got none. The values are the reference stack's and are not a rule source (CD-54).

## 5. The mouse-yaw anomaly: what the 0x64 stream shows

User report: during tdm2 the mouse could only look up and down, not left and right (the in-game resolution had been
set to 2560x1440 = the full monitor; the launcher marks the run copy's `Engine.exe` high-DPI aware; monitor at 125 %).

### 5.1 The stream (PROVEN-CAPTURE)

Conn 6, 63.703 .. 155.844: one reliable state form and **1361** unreliable `UP_PLAYER_UPDATE` messages (14.8 per
second; `R` = 15 of C2S §2.1 confirmed). **Every one parsed with the derived layout of C2S §2.1 with 0 bits left**
(also the 483 of conn 7 and the 2338 of tdm1: U11 / DC-C2S-2 settled, the base variant never appears). Flag words:
0x1021 (327), 0x1121 (84), 0x1221 (47), 0x1321 (26), 0x1421 (304), 0x1521 (492), 0x1621 (13), 0x1721 (38),
0x3021 / 0x3121 / 0x3521 / 0x3621 / 0x3721 (30); so the view block 0x0020 (`yaw8c` + `body_angles`) and the eye
offset 0x0001 are in every update, the aim block 0x0100 in 668, the weapon float 0x0200 in 129, the camera point
0x0400 in 874.

| quantity | over the round (conn 6) | conclusion |
|---|---|---|
| `body_angles` pitch, roll (`local_player`+0x68, +0x70) | 0.0 and 0.0 in all 1361 | the body angles carry only the yaw |
| `body_angles` yaw (`local_player`+0x6c) | **8 distinct values**: -1.659809 (63.703 .. 65.062, the yaw the server's 0x6a set), then from 65.141 on 2426654.0 (f32 bits 0x4a141c78), 2426654.25, 2426654.5, 2426654.75, 2426655.0, 2426655.25, 2426655.5 | one jump of +2,426,655.66 rad, then only multiples of 0.25 |
| `yaw8c` | 8 distinct values (-67, -5, 4, 14, 25, 35, 45, 55), 34 changes, every change a step of 10 or 11 units (0.25 rad) | the same steps |
| aim block `aim_q` yaw (rotation of `local_player`+0x48..+0x50) | **2 distinct values**: -95.1° in the first block, **35.5° in all 667 later blocks** (65.141 .. 155.844) | the controller yaw never moved again |
| aim block pitch | **414 distinct values**, -27.2° .. +84.3°, 636 changes | pitch worked all the time |
| 0x82 `yaw` field (`local_player`+0x4c) | 2426654.75 in all 11 shots | the stuck controller yaw |
| 0x82 `dir` heading | 35.5° (34.9° .. 37.2°) for all 11 shots | 2426654.75 rad folded into one turn = 35.5° |
| eye offset, camera point | eye (0, 33, 0) standing, y down to 5.5 crouched; camera = position + eye | normal |

The jump happened inside one update interval: update seq 1017 at 65.062 (client time 64.409) still had yaw
-1.6598 and pitch 0; update seq 1029 at 65.141 (client time 64.476, head
`0b 64 21 11 fb 00 00 00 00 78 1c 14 4a 00 00 00 00 0e 43 bc 3f db 0f 49`) had yaw 2426654.0 and, in the aux words
and the aim block, pitch 1.4708 rad = 84.3°: **both axes received an enormous delta in the same frame**, 1.44 s after
the first update and at the first mouse movement of the round. The pitch is clamped, so it was back inside its range
at once and kept full precision; the yaw is an unwrapped accumulator (tdm1 shows values above 2π: 9.50 rad), so it
stayed at 2.4 million radians. An `f32` between 2^21 and 2^22 has a spacing of 0.25: a per-frame yaw delta below
0.125 rad, that is below about 110 device units at the default scale of 0.001135 rad per unit
(`MOVEMENT_2009_addendum_client.md` B1), rounds to no change at all, and a fast flick moves the yaw by whole 0.25-rad
(14.3°) steps. That is exactly the 34 `yaw8c` steps of the round and the user's "could only look up/down".

Size of the delta: 2,426,655.66 rad / 0.001135 rad per unit = 2.138 x 10^9 units, 99.6 % of 2^31: a single corrupt
relative mouse delta near the 32-bit limit (a sentinel or an overflowed value), not a series of large movements.
Which component produced it is UNPROVEN.

### 5.2 Controls

- **Conn 7, the same client process re-joining 15 s after it left (32 s in the world):** `body_angles` yaw 15 distinct values between -0.61
  and -1.51 rad, `yaw8c` -24 .. -60 in steps of 1 to 20 units: no anomaly.
- **tdm1, same machine, default resolution:** body yaw -2.85 .. 9.50 rad with 1128 distinct values, aim yaw 1303
  distinct values over the whole circle, 0x82 `yaw` 5.05 .. 7.80 rad: no anomaly.
- **Server side:** the last server message that set the view was 0x6a at 63.594 (yaw -1.6598). The reference stack only
  mirrored the stuck yaw afterwards (its `yaw8` for player1 on conn 5 moved in the same 0.25-rad steps; its
  `CF_ROT_SNAP` records for object 189 on conn 6 likewise). Nothing on the wire caused the jump.
- **Launcher event log (timestamps only):** no launcher event between 63.600 and 155.867, so nothing the launcher
  logged coincides with the jump at 65.141.

### 5.3 Where the cause is

Client side, in the input path from the engine's mouse delta to CShell's yaw accumulator (`local_player`+0x4c and
+0x6c): one absurd delta entered once, after the spawn, at the first mouse movement. The trigger is environmental
and specific to that session (full-monitor windowed mode at 2560x1440 with the high-DPI layer, and any change the
launcher makes to the client's input handling, are the candidates to test first); the re-join in the same process and the default-resolution tdm1
were normal. Suggested check for the launcher owner: repeat without launcher changes that touch input (if there are any) and
with a window smaller than the monitor, and watch the 0x64 body yaw (any |yaw| above 1000 rad marks the event).
A DeCombat server can repair the client after the fact: `GS_VIEW_ANGLES` 0x6a kind 0 sets the controller's pitch,
yaw and roll (S2C §4.2), so sending it with the yaw folded into (-π, π] restores the precision (proposed row CV2-5).

### 5.4 Side result: `yaw8c` truncates (correction, PROVEN-CAPTURE)

`yaw8c` equals `trunc(yaw(q) * 40.425354)` in 1361 / 1361 updates of conn 6, 483 / 483 of conn 7 and 2338 / 2338 of
tdm1 (`q` the yaw-only rotation of the body yaw); `round(...)` matches only 1347, 2 and 957 of them. Corrections:
- `UDP_SESSION_2009_B.md` §10.3: "`yaw_c8 = round(yaw(q) * 40.425354)` ... Note the shell rounds where the engine's
  `yaw8` writer truncates." -> "`yaw_c8 = trunc(yaw(q) * 40.425354)` (toward zero, like the engine's `yaw8` writer;
  OBSERVED in 4182 updates of two captures)".
- `GAME_MESSAGES_2009_C2S.md` §2.1: "`yaw8c = round(yaw(q) * 40.425354)`" -> "`yaw8c = trunc(yaw(q) * 40.425354)`".
  (`yaw16c` never appeared: flag 0x0040 was never set.)

## 6. Capture validation items this round settles (and tdm1 could not)

| item | verdict from tdm2 | where |
|---|---|---|
| V-S2C-1 | **SETTLED** for player characters (kind-1 model object, type 0x18 record, attached weapon object) and confirmed for pickups (type 0x17) | §1 |
| V-S2C-2 | PARTLY: the spawn set of a second player and a self-damage set (0x74 + 0x66 kind 2); **kill, death states, respawn: not observed** | §2, §3 |
| V-S2C-3 | **SETTLED for a forfeit end** (order and 0x3f fields; no 0x99); goal / timer end not observed | §4 |
| V-S2C-4 | PARTLY: 0x45 goes only to the first spawn of a round; a joiner of a running round gets 0x43 / 0x44 / 0xb5 | §2.5 |
| V-S2C-5 | PARTLY: relays travel unreliable (id 17 types 0x06 / 0x19, 0xa6 subs 0x1a / 0x40 / 0x80 / 0xc0 / 0x1f, 0xa2); roster, state, status, weapon, round and result messages reliable | §1.7, §3 |
| V-C1 | PARTLY: one 0x82 per bullet with `last` = 1 (single-shot weapons only); `has_hit` never set (no shotgun, no player hit) | §3.2 |
| V-C2 | SETTLED for a second login and a re-join (fields 1-4 equal the lobby values, byte-identical 0x72 on re-join) | §2.4 |
| V-C3 | PARTLY: 0x7f layout and `aux` = ammunition id; 0x76 cat 3 event 2 with every change; no reload, no idle event | §3.5 |
| V-C5 | SETTLED on two more links (one 0x87 per link, 63-109 ms after 0x67 state 1) | §2 |
| V-C6 | PARTLY: rate 14.8-15.0 per second; 0xca (1, 0) / (0, 1); 0x76 cat 1 events 1, 2, 3 | §3.5, §5.1 |
| V-B3 | **FAILS as written** for the reference stack (`create_pos` is zero for characters); rest holds | §1.8 |
| V-B5 | **SETTLED**: a leaver's character and weapon object are removed in one kind-3 record; the id 411 was re-used by a later create | §2.3, §4.2 |
| V-B6 | **SETTLED** for `yaw_c8` (truncation, §5.4), the aim `quat128`, and the trailer `move_seq` echo (1 -> 2 within 31-63 ms of the second teleport, both links) | §5 |
| U5 (C2S §12.1) | **DECIDED** (reference stack form, accepted) | §3.5 |
| U11 (C2S §12.1) | **SETTLED**: all 4182 real-client updates of two captures use the derived layout | §5.1 |
| V-M12 | not usable (the yaw was broken in this round) | §5 |
| V-GM-1 .. 9 | none (TDM only) | |
| V-A (join of a running world) | 0x90 [0] and 0x88 [3] come first on a link that joins a running world (1 / 2 on a starting world) | §2.3 |

## 7. Proposed DC_DEFAULTS changes (PENDING; ids CV2-n until the coordinator numbers them)

Every value below is a DeCombat choice. Where it equals what the reference stack sent, the reason is that the 2009 client
accepted that form, not that the reference stack is a rule source (REFERENCE_TESTING.md §1).

| id | rule (DeCombat policy) | default | 2009 client evidence it must satisfy | config key | hook |
|---|---|---|---|---|---|
| CV2-1 | Player character create form: a kind-1 model object (`kind_byte` 0xc1) with the type 0x18 character record as blob (`r08` 1, `has_tail` 0, `r2c` / `r2d` = (1, 255) until the spawn and (3, 2) after it, `r34` = 1 for a team-1 player and 0 for a team-2 player (the two values the reference stack used for its two players; which property it follows is UNPROVEN), `r40` = the bit pattern of 0.5 (non-zero marks a roster player), box = the player half extents, every other field 0), the ROM character's model block (body + child, two skins and the mark texture, three styles; the face as a second model once the record is complete), a flag block (yaw-only bit 0x0800 always; 0x0802 before the record is complete, 0x2802 after, 0x3803 once spawned; `flags2` 0x0318, `uflags` 0x00401020, `flags3` 0), the compact position, rotation for remote viewers, `CF_DIMS` = the box on the owner's create and the pre-spawn create, and the held weapon as a second kind-1 object attached at socket 2. Replaces DSV-8 (plain object). Field meanings beyond the widths stay UNPROVEN. | as stated | record widths of cshell+0x2b6aa0 (572 bits, §1.5); the init branch on +0x40 (cshell+0x205f40); flag 0x0800 selects the `yaw8` stream (B §4.2 item 3); attachment block (B §4.2 item 8); all accepted by the 2009 client in tdm2 | `dserver.character.*` | `dserver.character_create` |
| CV2-2 | Remote player stream: every tick, every other player's character gets an entry with `UF_POS` + `UF_YAW` + `UF_ANIM` (`uf` 13): `pos48`, `yaw8` and an animation block of three trackers (main tracker clip 0; tracker 1 = upper-body clip, weight set 0; tracker 2 = lower-body clip, weight set 1; narrow form, `tsel` 2), the clips chosen from the reported control word and the held weapon. Replaces the `quat128`-on-change rotation of DSV-21. The weapon child is never streamed. | as stated | B §4.3 (`yaw8` when the object has flag 0x0800; `UF_ANIM` only for kind 1), B §4.4 animation block; reference stack form accepted (§1.6) | `dserver.stream.*` | `dserver.select_updates` |
| CV2-3 | Relays of a player's weapon and movement events to everyone, the sender included, in the unreliable block: 0x80 -> `S2C_SHELL_OBJ_MSG` type 0x19 `[sound_no][weapon_id][player:u32][pos]`; a landing (0x76 cat 1 event 3) -> type 0x19 `k` 0x0f with the player position; 0x82 -> type 0x06 shot effect `[first][shooter][weapon][ammo][a][b][c][from][to][normal][d][e][f][g]` with `to` = the reported hit point or the server's trace end and `normal` from the world collision (opaque bytes as the reference stack: a 71, b 0x4f10, c 1, d..g 0, 0xff, 0xff, 0xff, until their meaning is traced); 0x7f -> `CF_FILES` record of the weapon child. 0x76 cat 3, 0x4d: no relay. | on | type 0x19 inline reader (S2C §8.2, PROVEN); type 0x06 reader cshell+0x2c6770 (DISASM, §3.3); weapon child model change (B §4.2 item 1) | `dserver.relays.*` | `combat.on_fire_effect`, `dserver.on_weapon_change` |
| CV2-4 | Sprint acknowledgement: each `UP_SPRINT_STATE` change is answered to the sender with 0xca `[sprinting]`. | on | 0xca is read by the 2009 client into its own record +0x1f0 (S2C §3.2); reference stack form (§3.5); meaning UNPROVEN, no harm seen | `dserver.sprint_ack` | none |
| CV2-5 | View-angle repair: when a player's reported yaw (0x64 body yaw or 0x82 `yaw`) is not finite or its magnitude exceeds 1000 rad, the server sends `GS_VIEW_ANGLES` 0x6a kind 0 with the current pitch, the yaw folded into (-π, π] and roll 0, at most once per 2 s per player, and logs it. | on, 1000 rad, 2 s | 0x6a kind 0 sets the controller angles (S2C §4.2); tdm2 shows a client stuck at yaw 2.4e6 rad that lost all fine yaw control (§5) | `dserver.view_repair_max_rad`, `dserver.view_repair_interval_ms` | `dserver.on_view_anomaly` |
| CV2-6 | Forfeit: when every player of one team has left a running team round, the round ends at once with the other team as the winner (`win_team` = that team); an empty match with no player left ends as a draw. | on | `GS_ROUND_RESULT` shows a team banner only for 1 / 2 (S2C §2.1.2); the reference stack ended the tdm2 round this way (§4.2) | `modes.tdm.forfeit` | `mode.on_team_empty` |
| CV2-7 | Roster record kinds: the joiner gets kind 2 (add or update, no notice) for every record, its own included; every other client gets the joiner's record as kind 0 (notice) once, whether the round is starting or running; later refreshes and 0x8c answers use kind 2. | as stated | kind 2 adds a missing record (cshell+0x2b820 falls back to cshell+0x2a950, §2.5); reference stack order accepted (§2.2, §2.3) | `dserver.roster_kinds` | none |
| CV2-8 | A leaving player's character and its weapon child are removed in one kind-3 record, sent in the same datagram as 0x6c to every remaining client; both ids become reusable after that (CD-66). | as stated | B §5.1; tdm2 removal [189, 411] and re-use of 411 (§4.2, §2.3) | none | none |

Notes on existing rows:

- **DC-C2S-10:** the reference stack sent `GS_PLAYER_STATE` 1 (not 3) before 0x87 together with the whole spawn set, and the
  client accepted it. The row can stay (state 3 then 1 also passes the in-world test); no evidence for the death and
  respawn timeline was produced by tdm2.
- **PD-S2C-2:** only state 1 was observed; unchanged.
- **DSV-8:** superseded by CV2-1 once dserver slice 3 lands.
- **DSV-13:** a forfeit end without 0x99 was observed (reference stack); the 8000 / 2000 ms holds stay a DeCombat choice.
- **DSV-21:** the rotation part is superseded by CV2-2 (`yaw8` every entry); the stale-velocity part stays.
- **CD-49:** confirmed live once more (re-join of a running match: 0x392 (0, 1) -> 0x393 + 0x3042 {id, 3} + 0x3043).

## 8. dserver_changes: what `dc-dserver` sends today versus the form the 2009 client accepted in tdm2

Read-only comparison against `crates/dc-dserver/src` on master (slice 2: `objects.rs`, `stream.rs`, `gs.rs`,
`session.rs`, `shell64.rs`, `game.rs`, `game/relay.rs`, `game/combat.rs`, `game/round.rs`) and the slice 3 work in
progress in branch `dserver-slice3` (`chars.rs`: kind-1 character with a type 0x18 blob, three trackers and an
attached weapon, from the earlier bot-vs-reference-stack run). Priority: **P1** needed for the real client to see and
animate the other player, **P2** needed for weapons and effects of the other player, **P3** optional or cosmetic.

| # | prio | where | today | tdm2 form (accepted) | change |
|---|---|---|---|---|---|
| D1 | P1 | `session.rs` `create_spec` / `TODO_PLAYER_OBJ_KIND`, `objects.rs` `create_record` | kind 0 (`OBJ_PLAIN`), `cf` NEW + POS + FLAGS, flags 0x0100 (full precision), raw position, no model, no blob, no rotation, no attachment, no dims | `kind_byte` 0xc1, `create_pos` 0, 580-bit blob (type 0x18 record, §1.5), model block, flags 0x0802 / 0x2802 / 0x3803 with 0x0800 always, `flags2` 0x0318, `uflags` 0x00401020, `pos48` + `vec72`, `CF_ROT_SNAP` for remote viewers, `CF_ATTACH` (weapon child, socket 2, identity offset, then 32 zero mask pairs), `CF_DIMS` (box) on the owner's and the pre-spawn create | `create_record` must write the blob (8- or 16-bit length by size), `CF_ROT` / `CF_ROT_SNAP`, `CF_ATTACH` with the kind-1 mask block, `CF_DIMS`, `CF_ANIM` for the owner's create, and the flag words above (CV2-1; slice 3 `chars.rs` is the asset side) |
| D2 | P1 | `objects.rs` `write_model_block` | `sub_count` always 1, `style_count` always 0 | body entry: `sub_count` = 1 + child models (2), five textures, three styles; face entry: one texture, two styles; weapon: one texture, one style | write the child-model list and the style list (`sub_count` counts the entry itself, B §3.4) |
| D3 | P1 | `stream.rs` `write_entry`, `game/relay.rs` `stream_for` | `UF_POS` + `UF_ROT` (`quat128`, only on change), no animation | `uf` 13 in every entry: `pos48` + `has_vel`, `yaw8`, animation block with three trackers (main 0; tracker 1 upper body, weight set 0; tracker 2 lower body, weight set 1) | add a `yaw8` writer (truncation toward zero, transport §15.3) and an animation-block writer (B §4.4); set flag 0x0800 on characters; choose clips per tick (CV2-2; slice 3 has the clip keys) |
| D4 | P1 | new object per player (session / game) | none | a kind-1 weapon object (model, one texture, one style, main-tracker animation, flags 0x0002 -> 0x0003 at spawn, `flags2` 0x0010, `uflags` 0x00000010), created with the character and named in its attachment list | create, keep and remove it with the character (CV2-1, CV2-8) |
| D5 | P1 | `shell64.rs` `Shell64Update::yaw`, `game/relay.rs` `on_update` | takes the body yaw as sent (unwrapped; tdm2 had 2,426,654.75 rad) | the client's yaw is an unwrapped accumulator | fold into (-π, π] (in f64) before storing or streaming; a `yaw8` of an unfolded value overflows; add the CV2-5 repair (0x6a kind 0 when the magnitude exceeds 1000 rad) |
| D6 | P2 | `game.rs` `on_up` `Up::WeaponChange` | records the current slot only | `CF_FILES` record of the weapon child (model / texture / style of the new weapon) to every client | send it (CV2-3) |
| D7 | P2 | `game.rs` `on_up` `Up::WeaponSound`, `Up::Event` (ignored); `msg.rs` (no id 17 writer) | nothing relayed | `S2C_SHELL_OBJ_MSG` (engine id 17) type 0x19 `[sound_no][weapon_id][player:u32][pos]` for 0x80, type 0x19 `k` 0x0f with the position for a landing, to everyone incl. the sender, unreliable | add `msg::s2c_shell_obj_msg` and the two relays (CV2-3) |
| D8 | P2 | `game/combat.rs` `on_fire` | damage only, no effect for the other players | type 0x06 shot effect (424 bits, §3.3) to everyone incl. the shooter, unreliable, for every bullet (hit or miss) | add it; `to` = the reported hit point (`origin + hit_dist * dir`) or a world trace end, `normal` from `dc-world` collision (CV2-3) |
| D9 | P2 | `game/combat.rs` (throwing weapons) | a thrown grenade's 0x82 is treated like a bullet | projectile object (kind 1, blob `[0x13][weapon:u8][ammo:u8][1]`, grenade model, scale 2, dims (6, 6, 8), `pos48` + velocity), later removed with a type 0x06 blast effect and 0x74 / 0x66 to the players in range | a projectile path (later slice; the blast rule is DeCombat policy) |
| D10 | P2 | `session.rs` `on_link_closed` | removal record with the player object only | removal record [character, weapon child] in the same datagram as 0x6c | include the child (CV2-8) |
| D11 | P2 | `game/round.rs` | the round ends only by goal or timer | the reference stack ended it when the opposing team became empty (0x46 `win_team` = remaining team) | add the forfeit trigger (CV2-6) |
| D12 | P3 | `game.rs` `introduce` | joiner: kind 1 for everyone; others: kind 0 only in a running round, else kind 1; the joiner's own record `dead` = 0 | joiner: kind 2 for everyone (own first with `dead` 1, again with `dead` 0 after the spawn set); others: kind 0 (`dead` 1) then kind 2 (`dead` 0), also in a starting room | optional (kind 1 and 2 both add a record); CV2-7 |
| D13 | P3 | `game.rs` `on_up` `Up::Sprint` (ignored) | nothing | 0xca `[sprinting]` to the sender | optional echo (CV2-4) |
| D14 | P3 | `game.rs` `on_world_ready` | 0x45 to every joiner (`first_round` 1) | 0x45 only to the first spawn of the round; a later joiner gets 0x43 + 0x44 + 0xb5 | optional; both accepted |
| D15 | P3 | `gs.rs` `player_summary` | second field `objref16` none (0xFFFF) | the player's character object | optional (the client ignores it) |
| D16 | P3 | `game/round.rs` `end_round` | 0x41, 0x46, 0x65 x n, 0xb5, 0x3f at once; 0x99 8 s later | forfeit end: 0x6c + 0x46; 2.7 s later 0x47, own 0x6b, 0x65, 0x48 (`round_secs` 3); 3.0 s later 0x3f (`result_flag` 0, `result_value` = winning team); no 0x99 | optional: `result_value` = the winning team is the only field worth copying (PLAUSIBLE); the order is free |
| D17 | none | spawn order (DC-C2S-10) | 0x67 3 before 0x87, spawn set and 0x67 1 after it | spawn set and 0x67 1 before 0x87 | no change: both pass the in-world test |
| D18 | none | 0xa6 notes | none sent | 570-980 per minute per link, 99 % of them ignored by the 2009 client (§1.7) | if slice 3 adds 0xa6, send only sub 0x11 (record refresh after the spawn: `r2c` / `r2d` = 3 / 2), 0x1b (box) and optionally 0x1f / 0x25; never 0x08, 0x13, 0x14, 0x15, 0x1a, 0x40, 0x80, 0xc0. *Correction 2026-10-27 (remote-aim):* 0x80 / 0xc0 / 0x40 are the remote aim, handled by the type-0x18 subclass (cshell+0x26f60, GAME_MESSAGES_2009_S2C.md §8.3); send 0x80 (DSV-153) |

Lobby note (not dserver): the 2009 client sends TCP 0x394 {0} on every match leave (§4.4); the Rust lobby must treat
it as "left the match" (member back to the waiting room), not only as a failed connection.

## 9. What could not be decoded or settled from tdm2

1. **Kill, death and respawn** (0x6d, 0xc8, 0x4a, 0x67 2 / 3 / 6, 0x89 / 0xc5 / 0xd2, 0xd7 sub 2, 0xdb sub 6, 0xd4, the
   respawn 0xa0 after 7000 ms): none occurred; the frozen yaw made every shot miss and the grenade did not kill.
   DC-C2S-10 / PD-S2C-2 / the hit-to-kill flow need tdm1 (self-kill) and a new two-player capture with a working mouse.
2. **A hit on a player** (`has_hit` = 1, `hit_dist`, `hit_obj` = the other character) and the server's 0x74 to the
   victim from another player: not observed.
3. **Field meanings of the type 0x18 record** beyond widths (`r2c` / `r2d` / `r34` / `r40` / the tail) and of the
   type 0x06 effect bytes `a`, `b`, `c`, `d..g`: widths and values only; meanings UNPROVEN (the consumer functions
   cshell+0x205f40, cshell+0x202650 and the type 6 create path were read only as far as cited).
4. **Object flag bits** 0x0001, 0x0002, 0x1000, 0x2000 of characters and `flags2` 0x0318, `uflags` 0x00401020:
   values observed, client meaning not traced.
5. **The animation clip indices** (tracker 1 / 2 values) are indices into the ROM model's clip list; which clip each
   number is was not resolved here (slice 3 resolves clips by name key at run time).
6. **0xa6 subs 0x40, 0x80, 0xc0, 0x1a** carry the reference stack's copy of the shooter's 0x64 aux words and pitch; they are
   ignored by the 2009 type 0x18 class, so their layout does not matter for DeCombat and was not pursued further. *Correction 2026-10-27:* wrong for 0x40 / 0x80 / 0xc0 (the remote aim; GAME_MESSAGES_2009_S2C.md §8.3 correction).
7. **Engine id 17 type 0x65** (one message at 65.922 to both links, 312 bits, carrying a ROM effect name): above the
   dispatcher's range (types < 0x3d) and not 0x64, so the 2009 client ignores it; content not reproduced.
8. **Shell 0x9d** `[u32 0]` and 0x66 kinds 0x06, 0x17, 0x18 (sent by the reference stack at every spawn): not in any 2009
   handler list (S2C §4.3: "any other kind: nothing"); ignored.
9. **What produced the 2^31-sized mouse delta** (section 5): the wire shows the effect, not the cause.
10. **Whether the real client rendered** Bot1's model, animation and weapon: only the absence of errors is on the wire;
    the user's eyes are the evidence for rendering.
