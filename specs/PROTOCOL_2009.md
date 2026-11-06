# PROTOCOL_2009: lobby, auth and clan wire specification (clean-room)

- **Title:** PROTOCOL_2009, the TCP protocol between the 2009 client shell and its auth and lobby servers
- **Scope:** everything the 2009 client (EU-test 0.5.2494, CShell.dll) sends to or accepts from the auth server
  and the lobby server, from start-up to the moment the client leaves for the game server. The UDP game
  connection is a separate spec. Server-side rules (who may do what, how values are computed) are NOT part of
  this spec. They are proposed as DeCombat defaults in
  `staging/spec-protocol2009_CAK_DEFAULTS_proposed.md` (rows CDP-nn).
- **Evidence (only these kinds):**
  - **PROVEN**: the field reads or writes were read in the decompiler output of the 2009 CShell.dll
    (`CShell.dll/<rva>.c`, rva = VA - 0x37100000). The rva of the sender or handler is in the
    evidence column. This audit re-read every sender (`FUN_3728fb10(frame id)` + `FUN_3728fb40(payload, size)`)
    and the handlers named in the rows.
  - **LIVE**: behaviour observed with the real 2009 client against our own local harness server (2026-10-19 ..
    2026-10-20 sessions lobby2009, lobby3, lobby5, pvp1, gameserver4; captures kept in the private harness output tree). LIVE
    shows that a layout is accepted and displayed. It does not prove the meaning of every field.
  - **HARNESS**: `lobby_harness` (two scripted clients).
  - **PROVEN-CAPTURE** (added by the fourth spec fold): bytes the real 2009 client sent on its lobby connection in a packet capture
    (`OBSERVED: captures/<id>/capture.jsonl`, ids tdm1 20261025_000014, tdm2 20261025_001409, tdm4 20261025_002236), or a
    client behaviour visible in them. 2009 evidence, never downgraded. Server pushes in those captures are the reference server's (REFERENCE:
    recorded for comparison, never a rule).
  - Public knowledge: Win32 FILETIME / SYSTEMTIME, UTF-16LE.
- **Status:** draft 1. Every message row has an evidence column. Ids or fields with no 2009 evidence are listed in
  Appendix B. The ids are given only where the layout could not be proven.
- **Date:** 2026-10-24

Conventions. The CShell rvas are written as `cs+0xRVA`. "Payload offset +N" counts from the start of the inner
header (section 1.2), so the first field is at +6. Integers are little-endian x86. `u32`/`i32` are 4 bytes,
`u16` 2 bytes. `wstr[n]` is a fixed array of n UTF-16LE code units, NUL-padded. `astr[n]` is a narrow (byte)
array. "C->S" is client to server. The names (`login_request`, `room_create` ...) are this project's names. Only
the numeric id, the sizes and the offsets are wire facts. Message boxes are cited by their CRes string id
(`res 0x452`). The text stays in the user's ROM.

---

## 1. Transport and framing

### 1.1 Socket

- Plain TCP over IPv4, with no encryption, compression or checksum. The client keeps two connections:
  - the **auth** connection, opened at start-up from the launcher's auth address and port. The client closes it
    after a successful `server_pick_reply` (0x20b);
  - the **lobby** connection, opened (`cs+0x33070`) to the IP / port of the chosen server entry
    (entry +0x46 / +0x88, section 3.2).
- A receive buffer cuts complete frames (frame check `cs+0x18fad0`). Each frame becomes window message 0x8100.
  A closed lobby socket becomes 0x8101 and, unless the client is already shutting down, a shutdown box
  `res 0x29e` (dispatcher `cs+0x42df0`). PROVEN.
- The lobby connection stays open during a match (LIVE: pvp1 / gameserver sessions). There is no keep-alive
  message in the lobby flow (LIVE).

### 1.2 Frame (FrameHeader8) and inner header (InnerHeader6)

```
frame        := i32 frame_id | i32 body_len | body[body_len]      (FrameHeader8)
body         := u16 msg_id   | u32 msg_len  | fields...           (InnerHeader6, then packed fields)
```

| fact | value | evidence |
|---|---|---|
| frame check | complete when `len >= 8` and `len >= body_len + 8` | PROVEN cs+0x18fad0 |
| writer | stores body_len, refuses `body_len > 0x8000`, sends `8 + body_len` bytes | PROVEN cs+0x18fb40 |
| max body | 0x8000 bytes | PROVEN cs+0x18fb40 |
| frame id setter | `cs+0x18fb10(frame_id)` before every send | PROVEN (all 86 senders) |
| inner header | every sender fills `msg_id` and `msg_len`; `msg_len == body_len` | PROVEN (sender table, Appendix A) |
| fields | packed, no padding (offsets in this spec are exact) | PROVEN (sender stack layouts) |
| reader | handlers read through the body getter `cs+0x18fb30` (body start), so the first field is +6 | PROVEN cs+0x42df0 |
| dispatch | on the **frame id** (getter `cs+0x18f970`) | PROVEN cs+0x42df0 |

**Request / reply pairing.** Requests are even ids. Replies are `request + 1`. Server pushes are in the 0x2000 and
0x3000 ranges. The dispatcher handles only the ids listed in Appendix A. Other ids are dropped silently, so a custom
push id is safe (LIVE: id 0x3f01 is ignored).

**Inner-id overrides (wire facts).** In four cases the inner `msg_id` differs from the frame id:

| frame id | inner msg_id | sender | meaning |
|---|---|---|---|
| 0x324 | **0x368** | PROVEN cs+0x18f7b0 | function item equip (section 4.3) |
| 0x324 | **0x36a** | PROVEN cs+0x18f890 | function item unequip |
| 0x356 | **0x35a** | PROVEN cs+0x18ed10 (type 3) | list-3 version request |
| 0x356 | **0x35c** | PROVEN cs+0x18ed10 (type 4) | list-4 version request |

A server must therefore dispatch C->S frames on the **inner msg_id** (or on frame id + msg_id) and not on the
frame id alone. Server -> client frames should carry the same id in both headers.

**Strings.** All lobby text is `wstr`. A few logging / GM fields are narrow (marked).

**Time values.** FILETIME fields are 8 bytes and in **local wall-clock time** (LIVE: a UTC value showed a 7 h offset
in PDT). 0 = permanent. SYSTEMTIME is 8 x u16.

### 1.3 Generic reply head

Most replies begin with `+6 u32 result` (non-zero = success) and often `+0xa u32 reason` or `u16 reason`. Rows
below give the exact width per message.

---

## 2. Auth server

### 2.1 Flow (LIVE)

```
C 0x206 file_check ............ S 0x207 file_check_reply      -> login screen
C 0x200 login_request ......... S 0x201 login_reply
                                S 0x2000 server_list_push      (unprompted; -> server select after 3 s)
  (reason 0x0003001e = no character -> nickname screen:
   C 0x204 / S 0x205 name check, C 0x20e / S 0x20f referrer check, C 0x202 / S 0x203 create, then S 0x2000)
C 0x20a server_pick ........... S 0x20b server_pick_reply     -> client closes auth socket, opens lobby
```

### 2.2 Messages

| id | our name | dir | size | layout (payload offsets) | evidence | status |
|---|---|---|---|---|---|---|
| 0x206 | `file_check` | C->S | 0x26 | +6..+0x25 u32[8] supplied by the caller: three reserved words (sent as 0) then five file checksums | PROVEN cs+0x191fb0; values LIVE | PROVEN |
| 0x207 | `file_check_reply` | S->C | >= 0xa | +6 u32 ok (0 -> shutdown box `res 0x442`; non-zero -> login screen) | PROVEN cs+0x38ce0 | PROVEN, LIVE |
| 0x200 | `login_request` | C->S | 0x2054 | +6 wstr[4096] login token; +0x2006 wstr[17] typed text (LIVE: carries the **password box** text); +0x2028 wstr[20] adapter address text; +0x2050 i32 partner number | PROVEN cs+0x191e30; LIVE | PROVEN, LIVE |
| 0x201 | `login_reply` | S->C | >= 0x2e | +6 u32 ok; +0xa u32 reason (see below); +0xe u32 sub-error code; +0x12 u32 account number (echoed in 0x300); +0x16 u32 exp; +0x1a u32 clan id; +0x1e u32 kills; +0x22 u32 deaths; +0x26 i32 privilege (2 = operator); +0x2a u32 day count (reason 0x89 only) | PROVEN cs+0x42450 | PROVEN, LIVE |
| 0x2000 | `server_list_push` | S->C | pad to 0xf16 | +6 u32 result (6 = list); +0xa u32 reason; +0xe u32 sub-error (reason 4); +0x12 u32 count (0 -> error 5); +0x16 count x `server_entry` (0xc0, section 3.2). The client copies the first 0xf16 bytes (20 entries). | PROVEN cs+0x38a20, cs+0x13eda0, copy cs+0x36280 | PROVEN, LIVE |
| 0x2001 | `auth_alive_push` | S->C | header | only logged by the client | PROVEN cs+0x42df0 | PROVEN |
| 0x2002 | `bonus_event_push` | S->C | header | sets a bonus-event flag in the shell state (+0x420 = 1) | PROVEN cs+0x356b0 | PROVEN |
| 0x204 | `name_check` | C->S | 0x28 | +6 wstr[17] name | PROVEN cs+0x1921f0 (from cs+0x12e550) | PROVEN, LIVE |
| 0x205 | `name_check_reply` | S->C | >= 0xa | +6 u32 free (non-zero = name available) | PROVEN cs+0x34de0 | PROVEN, LIVE |
| 0x20e | `referrer_check` | C->S | 0x28 | +6 wstr[17] name typed into the nickname screen's second (referrer) field; sent on that field's confirm command (0x2712 of cs+0x12e550) | PROVEN cs+0x1925b0 | PROVEN |
| 0x20f | `referrer_check_reply` | S->C | >= 0xc | +6 u32 ok: non-zero -> the referrer is accepted (screen flag +0x5e41 = 1, "accepted" texture); 0 -> flag cleared, and when `+0xa u16 reason == 0x38` the "not found" texture is shown. Handled only on the nickname screen (0xe). | PROVEN cs+0x35b80 -> cs+0x12cf70 | PROVEN |
| 0x202 | `character_create` | C->S | 0x2c | +6 i32 face value (6000 + model index); +0xa wstr[17] name | PROVEN cs+0x1922d0 | PROVEN, LIVE |
| 0x203 | `character_create_reply` | S->C | >= 0x16 | +6 u32 ok; +0xa u32 code (0xe / 0xf -> `res 0xfa3`); +0xe u32 account number; +0x12 u32 level. No screen change on success: the server follows with 0x2000. | PROVEN cs+0x34e60 / cs+0x12e9d0; LIVE | PROVEN, LIVE |
| 0x20a | `server_pick` | C->S | 0xa | +6 u32 server id (sent automatically for the first suitable entry) | PROVEN cs+0x1924a0; picker cs+0x13bf70 | PROVEN, LIVE |
| 0x20b | `server_pick_reply` | S->C | >= 0x1e | +6 u32 ok; +0xa u32 reason (both only logged); +0xe u8[16] session key (stored only when ok). On ok: auth socket closed, lobby socket opened to entry +0x46 / +0x88, then 0x300. Handled only on screen 0xb. | PROVEN cs+0x34f60, cs+0x13dff0 | PROVEN, LIVE |

**0x201 reason (u32 = high word, low word).** High word 3 with low word 0x1e: no character -> nickname screen. Low
word 0x39 -> `res 0x560`; 0x25 -> `res 0x443`; 0x30 / 0x31 / 0x32 -> text boxes; 0x3a -> sub-error at +0xe
(0x7593 -> `res 0x1874`); 0x89 -> uses the day count at +0x2a; anything else -> `res 0x44f`. PROVEN cs+0x42450.

**Client-side name filter (no packet).** Names of 4..12 characters that pass the client's word filter (cs+0x12e550,
filter at cs+0x2fea70, list in the ROM) are the only ones sent. A refused name never reaches the server. LIVE.

**Face value.** 0x202 sends 6000 + n. The character info (0x303 +0x56) must return the same raw value. The
client subtracts 6000 again (cs+0x3f2c0). LIVE: sending n alone crashed the Shop screen.

---

## 3. Lobby server

### 3.1 Login flow (LIVE, PROVEN senders)

```
C 0x300 lobby_login ............ S 0x301 lobby_login_reply   (client always continues with 0x342)
C 0x342 item_sync .............. S 0x343 item_sync_reply     (+0xe > 0 -> 0x342 again, else 0x302)
C 0x302 character_info ......... S 0x303 character_info_reply (client continues with 0x322; 0x366 if +0x104)
C 0x322 equipment .............. S 0x323 equipment_reply     (on screens 0xb/1/0xe -> 0x320)
C 0x320 inventory .............. S 0x321 inventory_reply     (-> 0x3c0 if clan, 0x30e(1) if observer, 0x344)
C 0x344 balance ................ S 0x345 balance_reply
  optional: 0x350 / 0x352 / 0x354 / 0x356 / 0x35a / 0x35c (version-gated by 0x301), 0x310 (room settings)
C 0x376 channel_list ........... S 0x377
C 0x372 channel_enter .......... S 0x373                     -> lobby screen 15
C 0x380 room_list .............. S 0x381                     (client then sends 0x370)
C 0x306 location(0)              (no reply)
C 0x370 channel_users .......... S 0x371
C 0x374 channel_leave .......... S 0x375
C 0x3b0 server_list ............ S 0x3b1;  C 0x3b2 server_move ... S 0x3b3
```

### 3.2 server_entry (0xc0 bytes; in 0x2000 and 0x3b1)

| off | type | field | evidence |
|---|---|---|---|
| +0 | u32 | server id (sent back in 0x20a / 0x3b2; stored in the shell state +0xc) | PROVEN cs+0x13dff0 |
| +4 | wstr[33] | server name (stored in the shell state +0x40) | PROVEN cs+0x13dff0 |
| +0x46 | wstr[33] | lobby IPv4 address text | PROVEN cs+0x13dff0 (connect) |
| +0x88 | u32 | lobby TCP port | PROVEN cs+0x13dff0 (connect) |
| +0x8c | u32 | open flag (only entries with 1 are listed by 0x3b1) | LIVE; cs+0x13f0b0 |
| +0x90 / +0x94 | u32 | capacity / current users (load % = cur*100/cap; client thresholds 31 / 81 / 96) | LIVE |
| +0x98 / +0x9c | u32 | operator capacity / operator current | LIVE (shown) |
| +0xa0 / +0xa4 | u32 | min / max grade | PROVEN cs+0x13eda0 (logged) |
| +0xa8 / +0xac | f32 | min / max kill ratio (0 = none) | LIVE (shown) |
| +0xb0 | u32 | min paid-grade | LIVE |
| +0xb4 / +0xb8 | f32 | exp / GP rate (display) | LIVE |
| +0xbc | u32 | clan-only server (copied to the shell state +0x12c; switches room lists to clan layout) | PROVEN cs+0x13dff0 |

The automatic pick (cs+0x13bf70) takes the first connectable non-clan entry below 95 % load, else the first
connectable entry. If there is none, the client shows a shutdown box `res 0x475`. LIVE.

### 3.3 Login and character messages

| id | our name | dir | size | layout | evidence | status |
|---|---|---|---|---|---|---|
| 0x300 | `lobby_login` | C->S | 0x1a | +6 u8[16] session key (from 0x20b / 0x3b3); +0x16 u32 account number | PROVEN cs+0x18ab20 | PROVEN, LIVE |
| 0x301 | `lobby_login_reply` | S->C | >= 0x4a | +6 u32 ok (0 -> `res 0x452`, disconnect); +0xe SYSTEMTIME server time (16 bytes); +0x1e u32 lobby session index (shell state +0x14; the game server needs it later); +0x22 u32 venue flag (shell +0x1a0); +0x26 u32 counter (-> cs+0x36330: 0 resets the price cache); +0x2a u32 shop-view version (> held -> 0x350); +0x2e / +0x32 / +0x36 u32 versions of lists 0 / 1 / 2 (> held -> 0x352 / 0x354 / 0x356); +0x3a u32 room-settings key, +0x3e u32 room-settings version (-> cs+0x3c7e0: missing or older -> 0x310); +0x42 / +0x46 u32 versions of lists 3 / 4 (-> 0x35a / 0x35c). The client always sends 0x342 next. | PROVEN cs+0x420d0 | PROVEN, LIVE |
| 0x342 | `item_sync` | C->S | 6 | header only | PROVEN cs+0x18dbf0 | PROVEN, LIVE |
| 0x343 | `item_sync_reply` | S->C | >= 0x12 | +0xe i32 more (> 0 -> 0x342 again; else 0x302) | PROVEN cs+0x35c40 | PROVEN, LIVE |
| 0x302 | `character_info` | C->S | 6 | header only | PROVEN cs+0x18acf0 | PROVEN, LIVE |
| 0x303 | `character_info_reply` | S->C | 0x108 | table 3.3a. Also accepted **unprompted** (refreshes own clan name / mark). | PROVEN cs+0x3f2c0 | PROVEN, LIVE |
| 0x322 | `equipment` | C->S | 6 | header only | PROVEN cs+0x18adb0 | PROVEN, LIVE |
| 0x323 | `equipment_reply` | S->C | 0xa6 | +6 u32 ok; +0x12 i32[15] item number per slot (-1 = empty); +0x4e i32[15] slot flags (bool: the slot holds a free-pass weapon, the flag cs+0x225220 matches against a record's +0x5d); +0x8a i32[7] function-slot item numbers (the client keeps `n % 9000`). Slots 4..9 and 14 drive the character model with `n % 1000`. On ok while on screen 0xb / 1 / 0xe -> 0x320. | PROVEN cs+0x229ca0 | PROVEN, LIVE |
| 0x320 | `inventory` | C->S | 6 | header only | PROVEN cs+0x18ae70 | PROVEN, LIVE |
| 0x321 | `inventory_reply` | S->C | 0x12 + 0x34*n | +6 u32 ok; +0xe i32 count; +0x12 count x `inventory_record` (table 3.3b). On ok: wait box closes, server-select step done, then 0x3c0 when clan id != 0, 0x30e(1) when the observer flag is set, then 0x344. | PROVEN cs+0x42340, cs+0x229130 | PROVEN, LIVE |
| 0x344 | `balance` | C->S | 6 | header only (after login and on the panel's refresh) | PROVEN cs+0x18e310 | PROVEN, LIVE |
| 0x345 | `balance_reply` | S->C | >= 0x16 | +6 u32 ok; +0xe u32 credit balance; +0x12 u32 prepaid balance (purchase dialogs default to prepaid and refuse client-side when it is short) | PROVEN cs+0x337b0; LIVE | PROVEN, LIVE |
| 0x366 | `notice_fetch` | C->S | 6 | sent when 0x303 +0x104 != 0 | PROVEN cs+0x18dcb0 | PROVEN, LIVE |
| 0x367 | `notice_reply` | S->C | 0x418 | +0x12 wstr[513] text (box); +0x414 u32 more. Also accepted unprompted (LIVE). | LIVE | LIVE |
| 0x3003 | `grade_push` | S->C | 0xa | +6 u32 grade -> shell state +0x440 (the same field as 0x303 +0xf8); refreshes screens 0x14 / 0x16 | PROVEN cs+0x36690 | PROVEN |
| 0x3004 | `progress_push` | S->C | 0x12 | +6 u32 exp total; +0xa u32 GP total; +0xe u32 popup flag. In a match or result screen the three values are parked in the waiting room (+0x5960..+0x5968) and applied later. Otherwise: if the rank of the new exp is above the rank of the stored exp, a rank-up flag is set and, when the popup flag is set, box `res 0x185f` is shown with the new rank name. Then GP -> shell +0x104 and exp -> shell +0x194. | PROVEN cs+0x40580; LIVE | PROVEN, LIVE |

**Table 3.3a, 0x303 `character_info_reply` (0x108 bytes).** Fields not read by cs+0x3f2c0 are marked reserved:
send 0.

| off | type | field (what the client does with it) |
|---|---|---|
| +6 | u32 | ok (0 -> only logged) |
| +0xe | u32 | character number (shell +0x8) |
| +0x12 | wstr[17] | reserved (not read) |
| +0x34 | wstr[17] | character name |
| +0x56 | i32 | face value, 6000 + n (-1 = none) |
| +0x5a | u32 | GP (shell +0x104) |
| +0x5e | u32 | exp (shell +0x194; the rank follows from it) |
| +0x62 / +0x66 / +0x6a / +0x6e / +0x72 | u32 | headshots / kills / deaths / wins / losses (shell +0xd4 / +0xfc / +0x100 / +0x110 / +0x114) |
| +0x76 | u32 | reserved |
| +0x7a | u32 | combined rank (shell +0x10c) |
| +0x7e / +0x82 | u32 | reserved |
| +0x86 | u32 | clan id (shell +0x124) |
| +0x8a | wstr[17] | clan name |
| +0xac | u32 | clan member role (shell +0x128; 5 = clan leader, see 0x3064) |
| +0xb0 | u32 | clan option word (shell +0x1ac) |
| +0xb4..+0xd3 | u32[8] | reserved |
| +0xd4 / +0xd8 | u32 | clan emblem / clan background |
| +0xdc | u32 | inbox-unread flag (non-zero -> inbox marked stale) |
| +0xe0 / +0xe4 | u32 | name colour id / clan-name colour id |
| +0xe8 | u32 | room-badge flag (bool, shell +0x424) |
| +0xec | FILETIME | chat muted until (shell +0x448; see 0x3053) |
| +0xf4 | u32 | reserved |
| +0xf8 | u32 | grade (shell +0x440) |
| +0xfc | u32 | reserved |
| +0x100 | u32 | nation index (shell +0x444; 0 = none) |
| +0x104 | u32 | pending notice (non-zero -> client sends 0x366) |

**Table 3.3b, `inventory_record` (0x34 bytes, 13 u32; parser cs+0x229130).**

| off | field |
|---|---|
| +0 | unique item id (instance) |
| +4 | item number (catalogue number, looked up in the ROM item tables, cs+0x25c010 / cs+0x35c010) |
| +8 | parent unique id (the weapon an attachment sits on; 0 = none) |
| +0xc | in-use flag (for function items of type 0x15: active) |
| +0x10 | FILETIME start |
| +0x18 | FILETIME end (0 = permanent) |
| +0x20 | use kind |
| +0x24 | remaining count (non-zero -> counted item) |
| +0x28 | default slot kind (1 primary, 2 secondary, 3 melee, 4 throw, 5 head, 6 back; LIVE) |
| +0x2c | function word |
| +0x30 | remaining period |

### 3.4 Server, channel and location messages

| id | our name | dir | size | layout | evidence | status |
|---|---|---|---|---|---|---|
| 0x3b0 | `server_list` | C->S | 6 | header only | PROVEN cs+0x18f0e0 | PROVEN, LIVE |
| 0x3b1 | `server_list_reply` | S->C | 0x12 + 0xc0*n | +6 u32 ok; +0xe u32 count; +0x12 count x `server_entry` | PROVEN cs+0x34f10, cs+0x13f0b0 | PROVEN, LIVE |
| 0x3b2 | `server_move` | C->S | 0xa | +6 u32 server id | PROVEN cs+0x18f010 | PROVEN, LIVE |
| 0x3b3 | `server_move_reply` | S->C | >= 0x1e | +6 u32 ok (0 -> `res 0x3f5`); +0xe u8[16] session key. The lobby address comes from the chosen entry. If the server changed, the client reconnects (cs+0x32f80) and sends 0x300. | PROVEN cs+0x34fb0, cs+0x13e200 | PROVEN, LIVE |
| 0x376 | `channel_list` | C->S | 6 | header only | PROVEN cs+0x18d110 | PROVEN, LIVE |
| 0x377 | `channel_list_reply` | S->C | 0x12 + 0x5a*n | +6 u32 ok; +0xe i32 count; +0x12 count x `channel_entry`: +0 u32 id, +4 wstr[33] name, +0x46 u32 open, +0x4a u32 capacity, +0x4e u32 current, +0x52 u32 operator capacity, +0x56 u32 operator current | PROVEN cs+0x350f0, cs+0xfb880 (0x5a stride); fields LIVE | PROVEN, LIVE |
| 0x372 | `channel_enter` | C->S | 0x4e | +6 u32 account number; +0xa u32 channel id; +0xe wstr[32] channel name | PROVEN cs+0x189db0 | PROVEN, LIVE |
| 0x373 | `channel_enter_reply` | S->C | >= 0x12 | +6 u32 ok (0 -> box: channel full); +0xe u32 my in-channel id (shell +0x10; must equal my row's +4 in 0x371, which is then highlighted). Ok -> lobby screen 15. | PROVEN cs+0x32e30, cs+0xfacd0 | PROVEN, LIVE |
| 0x374 | `channel_leave` | C->S | 6 | header only (Exit / change channel / change server) | PROVEN cs+0x18d050, cs+0x122410 | PROVEN, LIVE |
| 0x375 | `channel_leave_reply` | S->C | >= 0xa | +6 u32 ok -> server select (0xb), or channel select (0xc) after the lobby's change-channel command | PROVEN cs+0x35010 | PROVEN, LIVE |
| 0x306 | `location` | C->S | 0xa | +6 u32 where: 0 lobby, 1 waiting room, 2 shop, 3 tuning, 0xb result, 0xd / 0xe messenger shown / hidden. No reply. | PROVEN cs+0x18ac20 | PROVEN, LIVE |
| 0x304 | `logout` | C->S | 6 | header only | PROVEN cs+0x18d210 | PROVEN |
| 0x305 | `logout_reply` | S->C | >= 0xa | +6 u32 ok -> client shutdown path | PROVEN cs+0x35160 | PROVEN |
| 0x308 | `option_set` | C->S | 0xe | +6 u32 index; +0xa u32 flag | PROVEN cs+0x18da20 | PROVEN |
| 0x309 | `option_set_reply` | S->C | >= 0xa | +6 u32 result (printed only) | PROVEN cs+0x35a20 | PROVEN |
| 0x3038 | `room_option_push` | S->C | 0xe | +6 u32 character number; +0xa u32 option mask -> waiting room | PROVEN cs+0x38890 | PROVEN |
| 0x30a | `private_state` | C->S | 0xa | +6 u32 state (1 shop, 3 tuning, 4 clan) | PROVEN cs+0x18b6b0 | PROVEN |
| 0x30c | `private_state_end` | C->S | 6 | header only | PROVEN cs+0x18b780 | PROVEN |
| 0x30b / 0x30d | `private_state_reply` / `private_state_end_reply` | S->C | >= 0xa | +6 u32 result (printed only) | PROVEN cs+0x35c90 / cs+0x35cc0 | PROVEN |
| 0x30e | `observer_mode` | C->S | 0xa | +6 u32 on (sent with 1 after 0x321 when the observer flag is set) | PROVEN cs+0x18de40, cs+0x42340 | PROVEN, LIVE |
| 0x30f | `observer_mode_reply` | S->C | >= 0xa | +6 u32 result | dispatcher case cs+0x42df0 | PROVEN (case) |
| 0x312 | `player_report` | C->S | 0xe4c | +6 u32 kind (0 plain, 1 with in-match statistics block, 2 with free text); +0xa u32 / +0xe u32 reported player keys (from the report window; +0xe is the key the window also passes to its lookup); +0x12 wstr[17] reported name; +0x34 wstr[601] report text; +0x4e6: kind 2 -> u32 text length, then +0x4ea wstr[0x4b0] free text (copied with a 0x4b0-unit limit); kind 1 -> 13 u32 statistics words (+0x4e6..+0x519). No reply. | PROVEN cs+0x273790 -> cs+0x18e9d0: the builder zeroes 0x393 dwords, sets id 0x312 / size 0xe4c, copies the window name into a 17-unit field at +0x12 and the window text into a 601-unit field at +0x34; HARNESS lobby_harness::test_lb5gm sends the 601-unit text | PROVEN |
| 0x313 | (none) | S->C | | the dispatcher reads the body and does nothing | PROVEN cs+0x42df0 | PROVEN |

### 3.5 Room settings and version lists

| id | our name | dir | size | layout | evidence | status |
|---|---|---|---|---|---|---|
| 0x310 | `room_settings_fetch` | C->S | 0xe | +6 u32 held key (0 = none); +0xa u32 held version | PROVEN cs+0x18ec30 | PROVEN, LIVE |
| 0x311 | `room_settings_reply` | S->C | **0x10a** exactly | +6 u32 result; +0xa u32 reason; +0xe u32 key (the 0x301 +0x3a value); +0x12 u32 version; +0x16 u32 count N (<= 30); +0x1a N x {u32 map index, u32 mode bit mask}. The client stores the whole 0x10a bytes by key (cs+0x3db40) and applies them. A mode appears in the create-room / waiting-room lists only if its bit is set. **Without 0x311 the create-room mode list is empty** (LIVE). | PROVEN cs+0x3dbf0, cs+0x3db40 | PROVEN, LIVE |
| 0x3055 | `room_settings_push` | S->C | 0xe | +6 u32 key; +0xa u32 version -> same check as 0x301 +0x3a/+0x3e (may cause 0x310) | PROVEN cs+0x3c890 -> cs+0x3c7e0 | PROVEN, LIVE |
| 0x3054 | `shop_view_version_push` | S->C | 0xa | +6 u32 version; > held -> 0x350(held) | PROVEN cs+0x36420 | PROVEN, LIVE |
| 0x3056 / 0x3057 / 0x3058 | `list_version_push_0/1/2` | S->C | 0xa | +6 u32 version; > held -> 0x352 / 0x354 / 0x356 | PROVEN cs+0x42df0 (inline) | PROVEN, LIVE |
| 0x3059 | `price_counter_push` | S->C | 0xa | +6 u32 counter -> cs+0x36450 / cs+0x36330: 0 resets the price cache; newer -> 0x358 when the shop screens 0x14 / 0x16 are open | PROVEN cs+0x36450 | PROVEN |
| 0x305a / 0x305b | `list_version_push_3/4` | S->C | 0xa | +6 u32 version; > held -> 0x35a / 0x35c (framed as 0x356). 0x305a first refreshes screen 0x16. | PROVEN cs+0x42df0 (inline) | PROVEN, LIVE |

### 3.6 Lobby lists and lobby pushes (screen 15)

| id | our name | dir | size | layout | evidence | status |
|---|---|---|---|---|---|---|
| 0x380 | `room_list` | C->S | 6 | header only | PROVEN cs+0x18ca00 | PROVEN, LIVE |
| 0x381 | `room_list_reply` | S->C | 0x12 + 0x7e*n | +6 u32 ok; +0xe u32 count; +0x12 count x `room_record` (table 3.6a). The client then sends 0x370. | PROVEN cs+0x332a0, cs+0x12a530 | PROVEN, LIVE |
| 0x370 | `channel_users` | C->S | 6 | header only | PROVEN cs+0x189f20 | PROVEN, LIVE |
| 0x371 | `channel_users_reply` | S->C | 0x12 + 0x42*n | +6 u32 ok; +0xe i32 count; +0x12 count x `lobby_user_record` (table 3.6b) | PROVEN cs+0x1220e0 | PROVEN, LIVE |
| 0x3021 | `lobby_user_add_push` | S->C | 0x48 | +6 one `lobby_user_record` | PROVEN cs+0x34650 -> cs+0x122230 | PROVEN, LIVE |
| 0x3022 | `lobby_user_remove_push` | S->C | 0xa | +6 u32 in-channel id | PROVEN cs+0x346a0 -> cs+0x1222b0 | PROVEN, LIVE |
| 0x3031 | `lobby_room_add_push` | S->C | 0x84 | +6 one `room_record`. The client **appends** it and re-sorts, with no de-duplication (cs+0x126c80): send it once per room. | PROVEN cs+0x346f0 -> cs+0x12a8b0 | PROVEN, LIVE |
| 0x3032 | `lobby_room_remove_push` | S->C | 0xa | +6 u32 room number (removes the first row with it) | PROVEN cs+0x34740 -> cs+0x126130 | PROVEN, LIVE |
| 0x3035 | `room_update_push` (lobby form) | S->C | >= 0x58 | +6 u32 room number; +0xa u32 field type; +0xe u32 value; +0x16 wstr text. The lobby updates the row with that number. The types are as in section 3.7. | PROVEN cs+0x34830 -> cs+0x12ae60 | PROVEN, LIVE |
| 0x3a0 | `chat_send` | C->S | 0xf8 | +6 wstr[121] text (lobby and waiting room) | PROVEN cs+0x18a0d0 | PROVEN, LIVE |
| 0x3051 | `chat_push` | S->C | 0x38 + text | +6 u32 sender id (not read); +0xa u32 name length; +0xe u32 name colour id (> 0 -> colour table); +0x12 wstr[17] name; +0x34 u32 text length; +0x38 wstr text (copied up to length + 1). **The client does not print its own line: the server must echo it to the sender.** Screens 9 and 15. | PROVEN cs+0x334b0 -> cs+0x192f0; LIVE | PROVEN, LIVE |
| 0x3052 | `ticker_notice_push` | S->C | 0x12 + text | +6 u32 period (s); +0xa u32 repeat count; +0xe u32 text length; +0x12 wstr text. On a lobby screen it scrolls in the notice bar `count` times. On screen 0 (in match) the text goes to the in-match notice manager (cs+0x73970). | PROVEN cs+0x34270 | PROVEN, LIVE |
| 0x3065 | `broadcast_push` | S->C | 0x2a2 / 0x2dc | type 1 (player broadcast, 0x2a2): +6 u32 1; +0xa u32 seconds on air; +0xe u32 order; +0x12 u32 serial (an older or equal serial is dropped); +0x16 u32 sender account number (own -> confirmation line `res 0x1904`); +0x22 u32 sender colour index; +0x26 wstr[17] name; +0x48 wstr[100] message; +0x110 wstr[201] colour runs. Type 0 (system, 0x2dc): +0xa u32 seconds; +0xe u32 kind (1 / 2 colour); +0x12 wstr[101] text; +0xdc wstr[257] link. Only the lobby screen shows the text. | DECOMPILER cs+0x38190 -> cs+0x1202d0, ticker cs+0x6cd20 / cs+0x6ce30; LIVE (modern build) | PROVEN, LIVE |

**Table 3.6a, `room_record` (0x7e bytes; reader cs+0x12a530 / cs+0x12a8b0).**

| off | type | field |
|---|---|---|
| +0 | u32 | room number (0-based, shown as `%03d`) |
| +4 | wstr[33] | title. On clan servers: two wstr[16] team names (home, away). |
| +0x46 | u32 | reserved (not read) |
| +0x4a | u32 | locked (password) flag |
| +0x4e | u32 | host in-channel id |
| +0x52 | u32 | map index (row of the ROM room table) |
| +0x56 | u32 | mode index |
| +0x5a / +0x5e | u32 | current / max players (`%d / %d`) |
| +0x62 | u32 | state: 0 waiting, 1 starting, 2 / 3 in game (labels `res 0x161b` / `res 0x445` / `res 0x161c`) |
| +0x66 | u32 | goal |
| +0x6a / +0x6e / +0x72 | u32 | room options 1..3 (mid-match join, friendly fire, kill cam; the order is that of 0x389 +0x78..+0x80) |
| +0x76 | u32 | weapon-limit option |
| +0x7a | u32 | host badge level |

The offsets are PROVEN (cs+0x12a530). The meanings of +0x66..+0x7a follow the field order of 0x389, and they are
checked only for map, mode, players and state (LIVE).

**Table 3.6b, `lobby_user_record` (0x42 bytes).**

| off | type | field |
|---|---|---|
| +0 | u32 | account number (0 hides the tooltip, cs+0x120250) |
| +4 | u32 | in-channel id |
| +8 | wstr[17] | name |
| +0x2a | u32 | exp (rank icon via cs+0x2683d0) |
| +0x2e | u32 | clan id |
| +0x32 / +0x36 | u32 | clan emblem / clan background (LIVE) |
| +0x3a | u32 | name colour index (LIVE) |
| +0x3e | u32 | nation index (LIVE) |

### 3.7 Rooms (create, enter, waiting room)

```
lobby 15: C 0x382 room_create ........ S 0x383   -> waiting room 9
          C 0x384 room_enter ......... S 0x385   -> waiting room 9
waiting 9: C 0x306(1); C 0x388 room_info .. S 0x389; C 0x38a room_users .. S 0x38b; C 0x392(1,0)
           pushes 0x3033 / 0x3034 / 0x3035 / 0x3036 / 0x3041 / 0x3042 / 0x3044 / 0x3043
           ready: C 0x392(0,0|1) .. S 0x393     team: C 0x390 .. S 0x391
           settings (host): C 0x38c .. S 0x38d (+ 0x3035 to the room)
           leave: C 0x386 .. S 0x387 -> lobby 15
```

| id | our name | dir | size | layout | evidence | status |
|---|---|---|---|---|---|---|
| 0x382 | `room_create` | C->S | 0x62 | +6 i32 max players; +0xa wstr[33] title (empty on clan servers); +0x4c wstr[9] password ("" = open; the screen requires 4 characters); +0x5e i32 mode index. No map at create time. | PROVEN cs+0x18a560 (caller cs+0x1388c0) | PROVEN, LIVE |
| 0x383 | `room_create_reply` | S->C | >= 0x16 | +6 u32 ok; +0xa u16 code (0x8d -> `res 0x1061`, no more rooms); +0xe u32 room number; +0x12 u32 slot. On ok the client records room, slot, host = 1, max players, password and title, then goes to screen 9. Screen 8 only. | PROVEN cs+0x33320, cs+0x138ad0 | PROVEN, LIVE |
| 0x384 | `room_enter` | C->S | 0x9e | +6 i32 room number; +0xa wstr[9] password; +0x1c wstr[65] extra (sent empty) | PROVEN cs+0x18a710 (caller cs+0x121cf0) | PROVEN, LIVE |
| 0x385 | `room_enter_reply` | S->C | >= 0x12 | +6 u32 ok; +0xa u16 reason (0x2a -> `res 0x1013` wrong password; 0x8a -> `res 0x14be` kicked from this room; else `res 0xfb1`); +0xe u32 slot. Ok -> screen 9. Accepted on screens 8 / 0xf / 0x14 / 0x16 / 0x1c and forwarded to the room-list screen. | PROVEN wrapper cs+0x33370; body LIVE | PROVEN (wrapper), LIVE |
| 0x386 | `room_leave` | C->S | 6 | header only (also sent by the idle timer) | PROVEN cs+0x18b380, cs+0x15d4d0 | PROVEN, LIVE |
| 0x387 | `room_leave_reply` | S->C | >= 0xa | +6 u32 ok -> room = -1, lobby 15 | PROVEN cs+0x33400 | PROVEN, LIVE |
| 0x388 | `room_info` | C->S | 6 | header only (also triggered by 0x3036) | PROVEN cs+0x18a820 | PROVEN, LIVE |
| 0x389 | `room_info_reply` | S->C | 0x8c | table 3.7a | PROVEN cs+0x1684c0 | PROVEN, LIVE |
| 0x38a | `room_users` | C->S | 6 | header only | PROVEN cs+0x18b440 | PROVEN, LIVE |
| 0x38b | `room_users_reply` | S->C | 0x12 + 0xb8*n | +6 u32 ok; +0xe i32 count; +0x12 count x `room_user_record` (table 3.7b) | PROVEN cs+0x166660 | PROVEN, LIVE |
| 0x3033 | `room_user_add_push` | S->C | 0xbe | +6 one `room_user_record` | PROVEN cs+0x347e0 -> cs+0x166310 | PROVEN, LIVE |
| 0x3034 | `room_user_remove_push` | S->C | 0xe | +6 u32 in-channel id; +0xa u32 reason. Own id -> the reason is stored (shell +0x430) and the client returns to the lobby, which shows `res 0x1851` (reason 1) or `res 0x565` (reason 2) (cs+0x1295a0). Other id -> the row is removed. | PROVEN cs+0x34990 -> cs+0x165400; LIVE | PROVEN, LIVE |
| 0x3035 | `room_update_push` | S->C | >= 0x58 | +6 u32 room number (read by the lobby form only); +0xa u32 field type; +0xe u32 value; +0x12 u32 text length (not read); +0x16 wstr text (NUL-terminated; type 0xb copies 0x42 bytes). Types: 1 locked flag, 2 host (value = new host id; equals mine -> I am host), 3 map, 4 mode, 5 players, 6 max players, 7 room state, 8 goal, 9 / 0xa / 0xd room options (mid-match join, friendly fire, kill cam), 0xb title (text), 0xc password (text), 0xe weapon limit, 0xf host badge. | PROVEN cs+0x168680 (switch 1..0xf); LIVE for 2 / 3 / 5 / 7 | PROVEN, LIVE |
| 0x3036 | `room_info_stale_push` | S->C | header | client sends 0x388 | PROVEN cs+0x42df0 (inline) | PROVEN |
| 0x3041 | `room_team_push` | S->C | 0xe | +6 u32 in-channel id; +0xa u32 team (1 / 2) | PROVEN cs+0x349e0 -> cs+0x166c90 | PROVEN, LIVE |
| 0x3042 | `room_user_state_push` | S->C | 0x12 | +6 u32 in-channel id; +0xa u32 state (0 waiting, 1 ready, 2 starting, 3 in game); +0xe u32 sub-state (0 none, 1 shopping `res 0x456`, 2 idle `res 0x457`) | PROVEN cs+0x33f50 -> cs+0x1673a0 | PROVEN, LIVE |
| 0x390 | `team_change` | C->S | 0x12 | +0xe u32 destination team (1 / 2; clan servers: host only, 0). +6 / +0xa are not written (send-side garbage: ignore). | PROVEN cs+0x18b500; LIVE | PROVEN, LIVE |
| 0x391 | `team_change_reply` | S->C | >= 0xe | +6 u32 ok; +0xa u32 reason (0x27 -> `res 0x1063`) | PROVEN cs+0x15f070 | PROVEN |
| 0x392 | `room_user_flag` | C->S | 0xe | +6 u32 kind; +0xa u32 value: (0, 0/1) ready off / on; (0, 2) start (host) or join a running match; (1, 0) present; (1, 1) shopping; (1, 2) away (60 s idle, cs+0x1608b0) | PROVEN cs+0x18b5d0, cs+0x18b840, cs+0x15d680 | PROVEN, LIVE |
| 0x393 | `room_user_flag_reply` | S->C | >= 0xe | +6 u32 ok; +0xa u16 reason; +0xc i16 kind. Kind 3: reason 10 -> `res 0x41b`, 11 -> `res 0x444`, 0x8e -> join failed (server error). | PROVEN cs+0x33ef0 -> cs+0x15f0d0 | PROVEN, LIVE |
| 0x38c | `room_setting_change` | C->S | 0x16 | +6 i32 current mode (informational); +0xa u32 field type (as 0x3035); +0xe u32 value. Password form (0x58 bytes): +0xa type 1, +0x12 u16 length, +0x16 wstr password; +0xe not written. Pending marks (waiting room +0x5940 + type, one byte per type): a type 3 (map) or type 4 (mode) request sets the marks of types 3, 4, 6 and 8, the type 6 / 8 / 0xe requests set only their own; a request whose mark is set is not sent; each 0x3035 clears the mark of its own type, 0x38d {0} clears all. A server must therefore answer an accepted map or mode change with 0x3035 of all four types 3, 4, 6, 8 (the reference stack did, menu-frida6 / menu-frida8; a lobby that echoed only type 3 left the goal and max-player lists dead, rust5-auto / sweep-*). | PROVEN cs+0x18a8e0, cs+0x18a9c0; marks EVIDENCE cs+0x1614f0, cs+0x168680, cs+0x162370; echo REFERENCE (captures) | PROVEN, LIVE |
| 0x38d | `room_setting_change_reply` | S->C | >= 0xa | +6 u32 ok (0 -> `res 0x4e2`, game in progress). The change itself arrives as 0x3035. | PROVEN cs+0x162370 | PROVEN, LIVE |
| 0x38e | `room_kick` | C->S | 0x12 | two senders: cs+0x18f280 writes +6 u32 in-channel id, +0xa u32 character number; cs+0x18f1a0 writes +6, +0xa and +0xe u32 (the third word is a reason) | PROVEN cs+0x18f280, cs+0x18f1a0 | PROVEN |
| 0x38f | `room_kick_reply` | S->C | >= 0xe | +6 u32 ok -> `res 0x1849` "%s kicked"; ok 0 -> +0xa u32 reason: 0x7e -> `res 0x1850`, 0x7f -> 0x184f, 0x80 -> 0x184e, 0x81 -> 0x1846, **0x82 -> 0x1845 (the target is a GM)**, 0x83 -> 0x1844, any other -> 0x1847 | PROVEN cs+0x160340 | PROVEN |
| 0x394 | `match_exit` (formerly `match_join_failed`) | C->S | 0xa | +6 u32 phase. LIVE: 0 then 1 after a failed connection. **PROVEN-CAPTURE (tdm1, tdm2 2 of 2 leaves, tdm4): phase 0 is also sent on every deliberate leave of a running match, in the same millisecond as the game link's `UP_LEAVE_MATCH` 0xbd** (`94 03 0a 00 00 00 00 00 00 00`); the lobby must read it as "left the match" (member back to the waiting room), not only as a failed connection | PROVEN cs+0x18b010 | PROVEN, LIVE, PROVEN-CAPTURE |
| 0x396 | `room_invite_all` | C->S | 6 | header only (Invite button) | PROVEN cs+0x18d960 | PROVEN, LIVE |
| 0x397 | `room_invite_all_reply` | S->C | >= 0xa | +6 u32 ok -> box `res 0x509` | PROVEN cs+0x39060 | PROVEN, LIVE |
| 0x3023 | `room_invite_push` | S->C | 0x40 | +6 u32 room number; +0xa wstr[17] host name; +0x2c wstr[10] password. The client shows a 10 s invitation; Yes -> 0x384. | PROVEN cs+0x39110; LIVE | PROVEN, LIVE |
| 0x3037 | `voice_room_push` | S->C | >= 0x2a2 | +6 u32 room number; six strings at +0xe, +0x90, +0x116, +0x198, +0x21e, +0x2a0 (voice client) | PROVEN cs+0x348e0 | PROVEN; not needed |

**Table 3.7a, 0x389 `room_info_reply` (0x8c bytes).**

| off | type | field |
|---|---|---|
| +6 | i32 | ok |
| +0xe | i32 | room number (shown + 1) |
| +0x12 | wstr[33] | title |
| +0x54 | i32 | reserved (not read) |
| +0x58 | i32 | locked flag |
| +0x5c | i32 | host in-channel id (== mine -> I am host) |
| +0x60 / +0x64 | i32 | map index / mode index |
| +0x68 / +0x6c | i32 | players / max players |
| +0x70 | i32 | state (0 waiting, 1 starting, 2 / 3 in game) |
| +0x74 | i32 | goal |
| +0x78 / +0x7c / +0x80 | i32 | mid-match join / friendly fire / kill cam |
| +0x84 | i32 | weapon limit |
| +0x88 | i32 | host badge level |

The client then sends 0x38a.

**Table 3.7b, `room_user_record` (0xb8 bytes; cs+0x166660 / cs+0x166310).**

| off | type | field |
|---|---|---|
| +0 | i32 | character number (the 2nd word of 0x38e) |
| +4 | i32 | in-channel id |
| +8 / +0xc | i32 | rank word / class word |
| +0x10 | wstr[17] | name |
| +0x32 | i32 | clan id |
| +0x36 | i32 | team: 1 / 2 |
| +0x3a | i32 | state (as 0x3042) |
| +0x3e | i32 | sub-state (as 0x3042) |
| +0x42 | i32 | exp (rank icon) |
| +0x46..+0x5d | | statistics words (not displayed in the room) |
| +0x5e | wstr[17] | clan name |
| +0x80 | i32 | option word |
| +0x84..+0xa3 | | reserved |
| +0xa4 / +0xa8 | i32 | clan emblem / clan background |
| +0xac / +0xb0 | i32 | name / clan-name colour ids |
| +0xb4 | i32 | nation index |

State text (cs+0x163d00): `res 0x368` ready, `res 0x445` starting, `res 0x161c` in game. PROVEN.

**Client-side room rules (no server action needed, LIVE).** Quick join picks the first joinable room and sends a plain
0x384 (cs+0x129ac0). The idle kick is client-side (0x386 after 180 s / 120 s, 0x392(1,2) after 60 s). The host's
user popup has no kick entry. Host hand-over uses 0x38c with type 2 and the new host's id. A host alone in a room is
refused client-side unless the client's own config allows solo play (cs+0x35530).

---

## 4. Shop, inventory, inbox, coupons

The catalogue is the ROM's shop item list, read by both client and server. The client loads every product but
**shows only products listed in 0x351** (cs+0x258340). Sale kind 0 = GP, 1 = credit (LIVE).

### 4.1 Catalogue lists

| id | our name | dir | size | layout | evidence | status |
|---|---|---|---|---|---|---|
| 0x350 | `shop_view_fetch` | C->S | 0xa | +6 u32 held version | PROVEN cs+0x18ea90 | PROVEN, LIVE |
| 0x351 | `shop_view_reply` | S->C | 0x16 + 2n | +6 u32 result; +0xa u32 reason; +0xe u32 version; +0x12 u32 count; +0x16 u16[count] product ids -> shown flag = 1, version stored, shop screens refreshed | PROVEN cs+0x42ca0 | PROVEN, LIVE |
| 0x352 / 0x354 / 0x356 | `list_fetch_0/1/2` | C->S | 0xa | +6 u32 held version | PROVEN cs+0x18ed10 types 0 / 1 / 2 | PROVEN, LIVE |
| 0x353 / 0x355 / 0x357 | `list_reply_0/1/2` | S->C | 0x16 + 2n | as 0x351: +0x12 count, +0x16 u16 ids -> highlight lists 0 / 1 / 2 (manager +0x15f0 / +0x1600 / +0x1610), in packet order | PROVEN cs+0x2581c0 / cs+0x258240 / cs+0x2582c0 | PROVEN, LIVE |
| 0x35a / 0x35c | `list_fetch_3/4` | C->S | 0xa | +6 u32 held version; **frame id 0x356** | PROVEN cs+0x18ed10 types 3 / 4 | PROVEN |
| 0x35b | `list_reply_3` | S->C | 0x16 + 4n | +0x12 u32 count; +0x16 u32[count] -> manager +0x1620 (no reader in 2009) | DECOMPILER cs+0x42df0 case | PROVEN |
| 0x35d | `list_reply_4` | S->C | 0x16 + 8n | +0x12 count; +0x16 {u32 item number, u32 exp} -> rank requirement of credit cells | DECOMPILER cs+0x42df0 case | PROVEN |
| 0x358 | `price_fetch` | C->S | 0xa | +6 u32 held price version | PROVEN cs+0x18eb60 | PROVEN, LIVE |
| 0x359 | `price_reply` | S->C | 0x16 + 8n | +0xe u32 version; +0x12 u32 count; +0x16 {u32 product id, u32 price} -> price = sale price = value (the previous list restored first) | PROVEN cs+0x41350 -> cs+0x251590 | PROVEN, LIVE |

**Shop grid (client-side, OBSERVED on all 60 pages).** The client builds the cells itself from the ROM list and the
0x351 / 0x353 / 0x355 flags (cs+0x255860). The server only chooses which ids are shown and highlighted.

**Highlight lists** (PROVEN cs+0x255860, cs+0x157c00, cs+0x149f80, cs+0x141ef0; shop addendum section 6, applied 2026-10-24). List 0 (0x353) sets
cell flag +0x19, list 1 (0x355) +0x18, list 2 (0x357) +0x1a. The cell painters show +0x19 as the Hot mark, +0x18 as the New mark, +0x1a as the
Sale mark (mark textures bound in cs+0x141ef0). List 4 records (0x35d) set the cell's exp requirement +0x14 (values >= 100000000 set flag +0x1b).

### 4.2 Buy, equip, discard, attach

| id | our name | dir | size | layout | evidence | status |
|---|---|---|---|---|---|---|
| 0x328 | `shop_buy` | C->S | 0x22 | +6 u32 product id; +0xa u32 option (0 in every dialog seen); +0xe u32[5] unique ids to extend (0 = new) | PROVEN cs+0x18cf50 | PROVEN, LIVE |
| 0x329 | `shop_buy_reply` | S->C | >= 0x22 | +6 u32 ok; +0xa u16 reason (0x3b not enough money -> GP box `res 0x46e` / credit box `res 0x1873`; 0x3c -> `res 0x503`); +0xe u32 unique id; +0x12 FILETIME end; +0x1a u32 GP after; +0x1e i32 -1 (else an option item number). The client adds the item itself. For credit products it says "placed in your inbox". | PROVEN cs+0x336a0 -> cs+0x150c40 / cs+0x1588a0 | PROVEN, LIVE |
| 0x324 | `equip` | C->S | 0x12 | +6 u32 unique id; +0xa u32 item number; +0xe u32 slot | PROVEN cs+0x18b0e0 | PROVEN, LIVE |
| 0x325 | `equip_reply` | S->C | >= 0xa | +6 u32 ok (0 -> `res 0x506`); the client moves the item itself | PROVEN cs+0x34ab0 | PROVEN, LIVE |
| 0x326 | `unequip` | C->S | 0x12 | +6 u32 unique id; +0xa u32 item number; +0xe u32 slot | PROVEN cs+0x18af30 | PROVEN |
| 0x327 | `unequip_reply` | S->C | >= 0xa | +6 u32 ok (0 -> `res 0x4dd`) | PROVEN cs+0x34a40 | PROVEN |
| 0x334 | `item_discard` | C->S | 0xe | +6 u32 unique id; +0xa u32 item number | PROVEN cs+0x18a480 | PROVEN, LIVE |
| 0x335 | `item_discard_reply` | S->C | >= 0xa | +6 u32 ok (the item leaves the client inventory; no GP field) | PROVEN cs+0x32ee0 | PROVEN, LIVE |
| 0x336 | `attach` | C->S | 0x12 | +6 u32 option unique id; +0xa u32 option item number; +0xe u32 parent weapon unique id | PROVEN cs+0x18b1c0; LIVE | PROVEN, LIVE |
| 0x337 | `attach_reply` | S->C | >= 0xa | +6 u32 ok | PROVEN cs+0x34b10 | PROVEN, LIVE |
| 0x338 | `detach` | C->S | 0x12 | +6 / +0xa / +0xe u32 (same order as 0x336) | PROVEN cs+0x18b2a0 | PROVEN |
| 0x339 | `detach_reply` | S->C | >= 0xa | +6 u32 ok | PROVEN cs+0x34b70 | PROVEN, LIVE |
| 0x33a | `tuning_sync` | C->S | 0x16 | +6 u32 weapon unique id; +0xa / +0xe / +0x12 u32 unique ids of its option kinds 0x11 / 0x12 / 0x13 (-1 none). No reply. | PROVEN cs+0x18d2d0 (callers cs+0x1569b0, cs+0x159420) | PROVEN |
| 0x33c | `period_finish` | C->S | 0xa | +6 u32 (no caller in 2009) | PROVEN cs+0x18d3c0 | PROVEN (dead) |
| 0x33e | `expiry_check` | C->S | 0x19a | +6 i32 count; +0xa u32[100] unique ids (no caller in 2009) | PROVEN cs+0x18d480 | PROVEN (dead) |
| 0x33f | `expiry_push` | S->C | >= 0x1a6 | +6 u32 ok; +0xe u32 count; +0x12 u32[100] unique ids; +0x1a2 u32 refresh flag (-> 0x322). One box `res 0x1786` per item; the item is removed. | PROVEN cs+0x22cde0; LIVE | PROVEN, LIVE |
| 0x3011 | `expiry_notice_push` | S->C | 0xa + 4n | +6 u32 count; +0xa u32[count] item numbers -> one box `res 0x1786` per item (no inventory change) | PROVEN cs+0x39280; LIVE | PROVEN, LIVE |
| 0x35e / 0x360 | `function_item_a/b` | C->S | 0x12 | built by the caller, sent as its own frame: free-pass weapon equip (0x35e) / unequip (0x360): +6 u32 weapon item number (the record's +8); +0xa u32 the pass's item number (cs+0x287eb0); +0xe u32 slot (own slot, or backpack row + 10). Only for records whose flag +0x5d is set: the client's own records of the weapons of an in-use Weapon Free Pass (SubType 16), made by cs+0x28b550 / cs+0x28b050 (correction: an earlier note said 2009 code never sets the flag). | PROVEN cs+0x18ee60 (callers cs+0x147e40, cs+0x14baa0, cs+0x145ed0) | PROVEN (dormant) |
| 0x362 / 0x364 | `function_item_c/d` | C->S | 0x12 | +6 u32 item number; +0xa u32 flag; +0xe u32 parent (same condition) | PROVEN cs+0x18ee60 (callers cs+0x154e90 / cs+0x152c20) | PROVEN (dormant) |
| 0x368 / 0x36a | `function_equip` / `function_unequip` | C->S | 0xe | +6 u32 item number (the item's object +8); +0xa u32 unique id; **frame id 0x324**. Correction of the earlier "+6 unique id, +0xa item number": the caller cs+0x1413c0 passes the unique id as the FIRST argument (it looks the item up by it) and the item number as the second, and the sender writes the second argument at +6 and the first at +0xa. Sent only for items in use whose ROM row allows unequip | PROVEN cs+0x18f7b0 / cs+0x18f890 (caller cs+0x1413c0) | PROVEN |
| 0x35f / 0x361 / 0x363 / 0x365 | replies | S->C | >= 0xa | +6 u32 ok (0 -> `res 0x506` / `res 0x4dd`) | PROVEN cs+0x339d0 | PROVEN |
| 0x369 / 0x36b | `function_equip_reply` / `function_unequip_reply` | S->C | >= 0x12 | +6 u32 ok; +0xe u32 unique id | PROVEN cs+0x36710 / cs+0x36740 | PROVEN |

**0x329 details** (PROVEN cs+0x336a0 -> cs+0x150c40; shop addendum section 3, applied 2026-10-24). `+0x1e` is read only by the 0x16 shop
screen: when it is not -1 and the bought cell is a case (type 0x14), the client adds the item number `+0x1e` (with uid `+0xe`, end `+0x12`)
instead of the case (cs+0x150c40). A returned uid already in the client inventory shows "usage time extended" (`res 0x438`), else
`res 0x439`. Credit products (SaleType 1): the client deducts `SalePrice` from credit (+0x108) when the dialog mode is 1, else from prepaid
(+0x11c), and shows `res 0x17b6`; it adds nothing. The 0x328 `+0xa` option is the dialog mode - 1 (cs+0x145b60): 0 = credit balance.

### 4.3 Inbox, gifts, item use

| id | our name | dir | size | layout | evidence | status |
|---|---|---|---|---|---|---|
| 0x32c | `inbox_page` | C->S | 0xa | +6 u32 page | PROVEN cs+0x18df10 | PROVEN, LIVE |
| 0x32d | `inbox_page_reply` | S->C | 0x16 + 0xe8n | +6 u32 ok; +0xe u32 total; +0x12 u32 count; +0x16 rows of 0xe8: +0 u32 inbox entry id, +4 u32 product id, +8 u32 gift kind (0 own purchase, else gift), +0xc u32 opened flag, +0x10 wstr[17] sender, +0x32 FILETIME received, +0x3a wstr[83] message, +0xe0 FILETIME end | LIVE (cs+0x33730 -> cs+0x14bf80 -> cs+0x267a40) | LIVE |
| 0x32e | `inbox_open` | C->S | 0xe | +6 u32 inbox entry id; +0xa u32 product id | PROVEN cs+0x18dfe0 | PROVEN, LIVE |
| 0x32f | `inbox_open_reply` | S->C | >= 0x5e | +6 u32 ok; +0xe u32 item count; +0x22 u32[5] unique ids; +0x36 FILETIME[5] ends. The client adds the items and its GP bonus, then asks "use it now?" (-> 0x330). When the opened row is a supply case (type 0x14) the layout differs: see "0x32f supply-case branch" below the table. | PROVEN cs+0x33ca0 -> cs+0x14e010 | PROVEN, LIVE |
| 0x32a | `gift_send` | C->S | 0xd2 | +6 u32 product id; +0xa wstr[17] receiver; +0x2c wstr[83] message (credit products only, client-enforced) | PROVEN cs+0x18e0c0 | PROVEN, LIVE |
| 0x32b | `gift_send_reply` | S->C | >= 0xa | +6 u32 ok | LIVE (cs+0x1d6cb0) | LIVE |
| 0x3014 | `inbox_stale_push` | S->C | header | marks the inbox stale (shell +0x1bc +0x1a = 1); on the clan screen it calls that screen's refresh | PROVEN cs+0x33c20 | PROVEN |
| 0x330 | `item_use` | C->S | 0x48 | +6 u32 unique id; +0xa wstr[17] new name (wcsncpy 16); +0x2c u32 emblem; +0x30 u32 background; +0x34 u32 face; +0x38 u32 colour; +0x3c u32 used unique id; +0x40 u32 change item number; +0x44 u32 nation | PROVEN cs+0x18e1d0; emblem / background LIVE (cs+0x2ae630) | PROVEN, LIVE |
| 0x331 | `item_use_reply` | S->C | 0x22 | +6 u32 ok (0 -> box chosen by the +0xa reason through the reason table of cs+0x393f0); +0xa u32 reason; +0xe u32 unique id of the used item; +0x12 u32 unique id of a second item that the client deletes when it is non-zero and different (0 = none; kind 12: the replaced default weapon); +0x16 FILETIME end (lo / hi); +0x1e u32 remaining count. Window-specific reads and effects: see the table below this one | PROVEN cs+0x2452f0 (routing: dispatcher case 0x331 -> cs+0x33790 -> item-use manager cs+0x1db910 -> the open window's vfunc +0x40); readers of kinds 6, 7, 8, 9, 11: DISASM | PROVEN (the two DISASM readers: DISASM) |
| 0x346 | `recipient_check` | C->S | 0x28 | +6 wstr[17] name | PROVEN cs+0x18e410 | PROVEN, LIVE |
| 0x347 | `recipient_check_reply` | S->C | >= 0xe | +6 u32 ok; +0xa u32 reason. Answer {ok 0, reason 0x20} = the name exists: the gift window (cs+0x1d55e0) shows "recipient exists" (`res 0x1808`) for it and "no such character" (`res 0x1805`) for anything else; the rename window (vfunc +0x48 = cs+0x2426d0, routed the same way when no gift window is open) reads ok != 0 as "name free". So answer {0, 0x20} for an existing character and {1, 0} for a free name | LIVE; window semantics PROVEN cs+0x1d55e0, cs+0x2426d0 | LIVE |
| 0x36e | `megaphone_queue` | C->S | 6 | header only (megaphone window OK) | PROVEN cs+0x18f5d0 | PROVEN, LIVE |
| 0x36f | `megaphone_queue_reply` | S->C | >= 0x22 | +6 u32 ok. ok = 1: +0xe u32 seconds per message, +0x12 u32 order on air, +0x16 u32 last queued order, +0x1e u32 players waiting (the client shows a wait of (+0x16 - +0x12) * +0xe s). ok = 0: +0xa u32 reason (0x8f pending, 0x90 unavailable). | DECOMPILER cs+0x367b0 -> cs+0x2a8990; LIVE (modern build) | PROVEN, LIVE |
| 0x36c | `megaphone_send` | C->S | 0x26a | +6 u32 megaphone unique id; +0xa u32 1; +0xe wstr[101] message (<= 65 chars); +0xd8 wstr[201] colour runs (u16 pairs {colour index 1..16, run length}) | PROVEN cs+0x18f470 (built by cs+0x2a5be0) | PROVEN, LIVE |
| 0x36d | `megaphone_send_reply` | S->C | >= 0x16 | +6 u32 ok; +0xa u32 reason; +0xe u32 unique id; +0x12 u32 uses left (0 -> item removed); the window closes | DECOMPILER cs+0x36770 -> cs+0x2a5c90 | PROVEN, LIVE |
| 0x3013 | `case_announce_push` | S->C | 0x38 | +6 u32 `status` (must be 0, else nothing happens); +0xa..+0x11 not read (send zero); +0x12 wstr[17] `player_name`; +0x34 u32 `award_item_number` -> one line `res 0x18bc` ("<player> opened a HiSec case and obtained <item>") in the lobby chat or the in-match log; nothing is shown when the item number resolves to no ROM item | PROVEN cs+0x41470; HISEC_2009 section 2 | PROVEN, LIVE |

**Inbox paging** (PROVEN cs+0x267070, cs+0x2672f0, cs+0x148980; shop addendum section 7, applied 2026-10-24). The client asks for another 0x32c page only
while it holds fewer rows than the last 0x32d `+0xe` total; the page number it sends is `(held - 1) / 4 + 1`, 1 when empty; the inbox shows 4 rows
per page. Sending every row with total = rows sent avoids a second fetch.

**0x32f supply-case branch** (PROVEN cs+0x14e010; shop addendum section 2 refined by HISEC_2009 4.3, applied 2026-10-24). When the opened row's
product is a supply case (row +0x22c type 0x14) the reply carries: `+6 u32 ok`; on failure `+0xa u16 reason`, where the reasons 0x8b / 0x8c delete the
row client-side (`res 0x1891`) and other values go to the reason table (4.5); on success `+0x12 u32` award item number (-> cs+0x25c010); `+0x16 u32` GP
total after a GP award (non-zero -> GP pop-up cs+0x287110, no item); `+0x1a u32` award days (pop-up cs+0x287150, item +0x24; 0 or > 998 = permanent);
`+0x1e u32` **unique id of the key that was used** (a counted function item, SubType 0x13; 0 = none): when non-zero the client finds that inventory item
(cs+0x326590), decrements its count if it is above 0 and selects the HiSec award pop-up title `res 0x18c4`; `+0x22 u32` award unique id; `+0x36 FILETIME`
award end. The general layout above must carry exactly the product's item count at `+0xe` or the client ignores the reply; the client adds the
product's `BonusGP` to its GP itself. The only client-visible difference between a HiSec opening and a plain case opening is `+0x1e` naming an owned
counted item. That the 2009 server consumed the key itself is PLAUSIBLE (HISEC_2009 4.3).

**0x331 reply windows** (the item-use manager builds one window per function-item kind, cs+0x1db2c0, and opens the
window of the item's ROM `SubType`, cs+0x1db770; each window class has its own 0x331 reader). All kinds read `+6 ok`;
the fields beyond that are:

| kinds (SubType) | window ctor / vtable | reader | fields read, reaction | grade |
|---|---|---|---|---|
| 10, 12..16, 18..21, 22 (first window) | cs+0x1db140 / VA 0x374ede4c | cs+0x2452f0 | +0xa reason (box via cs+0x393f0 when ok = 0); +0xe unique id; +0x12 unique id of a second item to delete (non-zero and different); +0x16 FILETIME end; +0x1e remaining count; the item is marked in use (cs+0x22c990). Kind 10 also sets the room-badge flag (shell +0x424). Kind 12 converts the item to the window's change item number (cs+0x22c880) and shows `res 0x187b`. Kind 22 reopens the megaphone composer (cs+0x1db770 with flag 1) | PROVEN |
| 8, 9, 11 | cs+0x246ba0 / VA 0x374f2cf4 | cs+0x24a3a0 | +0xe, +0x12, +0x16 as above; the chosen colour becomes the item's function word (cs+0x22c990 argument 6); kind 8 -> shell +0x434, 9 -> +0x438, 11 -> +0x43c | DISASM (no decompiler function) |
| 2 | cs+0x2aacc0 / VA 0x374f8ecc | cs+0x2ab220 | +0xa reason; +0xe, +0x12, +0x16 as above; box `res 0x1798`; own emblem / background set locally | PROVEN |
| 1, 4 | cs+0x243520 / VA 0x374f288c | cs+0x249d80 | +0xa reason; the client renames the clan (1) or the player (4) and deletes the item | PROVEN |
| 3, 5 | cs+0x2445b0 / VA 0x374f2a3c | cs+0x244cf0 | +0xa reason; kind 5 clears kills (shell +0xfc), deaths (+0x100), headshots (+0xd4); the item is deleted | PROVEN |
| 6, 7 | cs+0x246960 / VA 0x374f2ca4 | cs+0x24a1d0 | +0xa reason; the chosen face is applied; the item is deleted | DISASM (no decompiler function) |
| 17 | cs+0x2a9350 / VA 0x374f8c5c | cs+0x2a94b0 | +0xa reason; nation -> shell +0x444; the item is deleted | PROVEN |

A server must not put the item number at +0x12: the client treats a non-zero +0x12 that differs from +0xe as the unique id of
an item to delete (grade PROVEN, cs+0x2452f0; source: lobby-shop worker, 2026-10-24). The harness reading of +6, +0xe,
+0x16 agrees.

### 4.4 Coupons

| id | our name | dir | size | layout | evidence | status |
|---|---|---|---|---|---|---|
| 0x340 | `coupon_redeem` | C->S | 0x88 | +6 wstr[65] code (the input box joins six 5-character parts; a shorter code is refused locally with reason 0x71) | PROVEN cs+0x18db00 (box cs+0x26a2a0) | PROVEN, LIVE |
| 0x341 | `coupon_redeem_reply` | S->C | >= 0x12 | +6 u32 ok; +0xa u32 reason (0x71 invalid, 0x72 already used, 0x73 try later); +0xe u32 product id. ok -> boxes `res 0x182b` / `res 0x182c` (check inbox). | DECOMPILER cs+0x26a3e0; LIVE (modern build) | PROVEN, LIVE |
| 0x34f | `coupon_redeem_reply_b` | S->C | as 0x341 | same handler as 0x341; no 2009 sender for its request | PROVEN cs+0x42df0 | PROVEN |

### 4.5 Item number ranges and the reason table

**Item number ranges** (PROVEN cs+0x25c010, cs+0x257bc0; shop addendum section 8, applied 2026-10-24). < 1000 weapons (type from the item table),
1000s options (types 0x11..0x13), 2000 -> 0xe, 3000 -> 0xf, 4000 -> 0xb, 5000 -> 0xc, 6000 -> 0xd, 7000 -> 0x10 backpack, 8000 -> 0x14 supply case,
9000 -> 0x15 function item, 10000s -> table or 0x1e, 11000 -> 0x17, 12000 -> 0x18, 13000 -> 0x19 (GP grant). The ROM shop list `Itemid` is the full
item number (the client keeps `% 1000` with the type).

**Reason table** (PROVEN cs+0x393f0; shop addendum section 9, applied 2026-10-24). Reasons the client maps to its own boxes (ids only): 0x3c `res 0x503`,
0x4a `0x17f2`, 0x4b `0x17f3`, 0x4c `0x17f4`, 0x4e `0x17f5`, 0x4f `0x17f6`, 0x53 `0x17f7`, 0x56 / 0x6b `0x1805`, 0x57 `0x17fc`, 0x58 `0x17f8`, 0x5f `0x1800`,
0x60 / 0x61 `0x17f9`, 0x62 `0x17fa`, 0x65 `0x18bf`, 0x67 `0x1873`, 0x68 `0x17fb`, 0x71 `0x1828`, 0x72 `0x182a`, 0x73 `0x182e`, 0x8b `0x1891`, 0x8f `0x1903`,
0x90 `0x1905`; other values print a generic server-error line with the number. With high word 4 a second table applies (0x33, 0x34 / 0x36 / 0x41,
0x3d / 0x3f / 0x42 / 0x50, 0x3e, 0x51 / 0x52 / 0x95); its target box ids were not recorded (Appendix B).

---

## 5. Clans and the clan web pages

### 5.1 Clan messages

| id | our name | dir | size | layout | evidence | status |
|---|---|---|---|---|---|---|
| 0x3c0 | `clan_login` | C->S | 6 | header only (after 0x321 when the clan id != 0) | PROVEN cs+0x18d570, cs+0x42340 | PROVEN, LIVE |
| 0x3c1 | `clan_login_reply` | S->C | >= 0xa | +6 u32 result | PROVEN cs+0x356f0 | PROVEN, LIVE |
| 0x3c2 | `clan_info` | C->S | 6 | header only (clan screen entry; 0x3061) | PROVEN cs+0x18d630 | PROVEN, LIVE |
| 0x3c3 | `clan_info_reply` | S->C | 0x82 | +6 u32 result (0 -> logged as failed, nothing else); +0xe u32 clan number; +0x12 u32 member limit; +0x16 u32 member count; +0x1a u32 previous rank; +0x1e u32 rank; +0x22 u32 level (1..6); +0x26 u32 mark word; +0x2a u32 wins; +0x2e u32 losses; +0x32 u32 draws; +0x36 u32 clan exp; +0x3a u32 clan points; +0x3e wstr[17] clan name; +0x60 wstr[17] leader name. Details below the table | PROVEN cs+0x35750 -> cs+0x106030 (twelve consecutive u32 from +0xe, then the name); LIVE (clan screen shows name, leader, 1/120 -> 2/120, level) | PROVEN, LIVE |
| 0x3c4 | `clan_members` | C->S | 6 | header only | PROVEN cs+0x18d6f0, cs+0x35980 | PROVEN, LIVE |
| 0x3c5 | `clan_members_reply` | S->C | 0x12 + 0x32n | +6 u32 result (0 -> logged as failed, list unchanged); +0xe i32 count; rows of exactly 0x32 from +0x12: +0 u32 character number, +4 u32 role (5 = leader, 0xf = a second privileged value, any other = ordinary member), +8 u32 exp, +0xc wstr[17] name, +0x2e u32 online. Details below the table | PROVEN cs+0x35820 (only on the clan screen) -> cs+0x111990 (steps 0x32 per row); LIVE (member list) | PROVEN, LIVE |
| 0x3c6 | `clan_notice` | C->S | 6 | header only | PROVEN cs+0x18d7b0, cs+0x35940 | PROVEN, LIVE |
| 0x3c7 | `clan_notice_reply` | S->C | 0x100 | +6 u32 result; +0xe wstr[121] notice (non-zero result: the wide string at +0xe is assigned to a wide string without a bound; wstr[121] fills the 0x100 bytes exactly, 0xe + 0xf2) | PROVEN cs+0x35880 (only on the clan screen) -> cs+0x105210 | PROVEN, LIVE |
| 0x3c8 | `clan_notice_set` | C->S | 0xf8 | +6 wstr[121] notice | PROVEN cs+0x18d870 | PROVEN, LIVE |
| 0x3c9 | `clan_notice_set_reply` | S->C | >= 0xa | +6 u32 result | PROVEN cs+0x358e0 | PROVEN, LIVE |
| 0x3061 | `clan_info_stale_push` | S->C | header | client sends 0x3c2 | PROVEN cs+0x42df0 (inline -> cs+0x18d630) | PROVEN, LIVE |
| 0x3062 | `clan_members_stale_push` | S->C | header | client sends 0x3c4 | PROVEN cs+0x35980 | PROVEN, LIVE |
| 0x3063 | `clan_notice_stale_push` | S->C | header | client sends 0x3c6 | PROVEN cs+0x35940 | PROVEN, LIVE |
| 0x3064 | `clan_removed_push` | S->C | header | if my role is 5 (leader) on the clan screen: box `res 0x502` (no clan any more) or `res 0x4c4` + clan reset; otherwise the waiting room flag +0x58d4 is set | PROVEN cs+0x42df0 (inline); LIVE | PROVEN, LIVE |
| 0x3012 | `clan_change_push` | S->C | var | +6 u32 sub; +0xa u32 clan id (ignored unless it equals mine, shell +0x124). Sub 1: +0xe wstr clan name. Sub 2: +0xe u32 emblem, +0x12 u32 background. Sub 9: +0xe u32 colour index. Then the clan screen's refresh. | PROVEN cs+0x33ac0 | PROVEN, LIVE (sub 2) |
| 0x348 | `clan_leader_check` | C->S | 0x28 | +6 wstr[17] | PROVEN cs+0x18e4f0 | PROVEN |
| 0x349 | `clan_leader_check_reply` | S->C | >= 0xe | +6 u32 ok; +0xa u32 reason | LIVE | LIVE |
| 0x34a | `clan_name_check` | C->S | 0x28 | +6 wstr[17] clan name | PROVEN cs+0x18e5d0 | PROVEN, LIVE |
| 0x34b | `clan_name_check_reply` | S->C | >= 0xe | +6 u32 ok; +0xa u32 reason (0x4e = taken) | LIVE | LIVE |
| 0x34c | `clan_mark_check` | C->S | 0xe | +6 u32 emblem; +0xa u32 background (one caller, the mark item window) | PROVEN cs+0x18e6b0 | PROVEN, LIVE |
| 0x34d | `clan_mark_check_reply` | S->C | >= 0xe | +6 u32 ok; +0xa u32 reason (0x4a no clan, 0x4b not leader, 0x4c mark in use) | DECOMPILER cs+0x33940 -> cs+0x1db9b0; LIVE | PROVEN (wrapper), LIVE |

**Clan reply details** (PROVEN, clean decompiler tree, clan addendum 5.1a, applied 2026-10-24; field names are ours).

- 0x3c3: when the result is non-zero, the clan screen (screen id 0x1c), if up, runs its record reader cs+0x106030; in every case `+0x36` and `+0x3a`
  are copied into the shell state (+0x130 / +0x134); then, when an item-use window is open (shell global +0x44c), the reply is forwarded to it
  (cs+0x2428e0): this is the crash path below (CD-31 / CDP-19). The clan screen (cs+0x105280) shows member count, rank, level, wins / losses / draws
  (with a win ratio), exp, points and the name. The mark word `+0x26` is not read by the screen code seen (its use is UNPROVEN). The clan name is
  copied with an unbounded wide copy, so send wstr[17]. The leader name `+0x60` is not read by cs+0x106030 (the screen takes the leader's name from
  the member list, role 5 -> cs+0x2035b0); keep sending it (LIVE layout, size 0x82); its use is UNPROVEN.
- 0x3c5: for the own row (character number equal to shell +8) the role is stored as the own clan role (shell +0x128), and roles 5 and 0xf enable the
  leader controls of the screen; a row with role 5 also names the clan leader. Name copied with an unbounded wide copy (17 units fit before +0x2e).

**Small replies and pushes** (PROVEN; clan addendum 5.1b, applied 2026-10-24):

| id | handler | behaviour |
|---|---|---|
| 0x3c1 | cs+0x356f0 | `+6` non-zero -> the screen refresh, then the client sends **0x3c2** itself (cs+0x18d630) |
| 0x3c9 | cs+0x358e0 | `+6` non-zero -> refresh, then the client sends **0x3c2** (not 0x3c6) |
| 0x3061 | dispatcher inline | refresh, then **always** sends 0x3c2 (whatever the screen): a server that pushes 0x3061 to a user with an item-use window open triggers the 0x3c3 crash path (reason for CD-31) |
| 0x3062 | cs+0x35980 | on the clan screen: sends 0x3c4 (cs+0x18d6f0); elsewhere sets a stale flag (+0x16e5a) that 0x3c5 clears |
| 0x3063 | cs+0x35940 | on the clan screen: sends 0x3c6 (cs+0x18d7b0); elsewhere sets a stale flag (+0x16e59) that 0x3c7 clears |
| 0x3064 | dispatcher inline | if the shell's own clan role (state +0xac) is 5: calls a clan-screen routine (cs+0x10c5c0) when that screen is up, then shows `res 0x502` on a normal server (shell +0x12c == 0) or `res 0x4c4` plus a local clan reset (cs+0x359d0) on the clan server; any other role: only sets the waiting-room flag +0x58d4 of screen 9 when it is up. So a plain member's leave is shown by the **0x303** that follows |
| 0x349 | cs+0x338e0 | forwarded to the gift window (+0x304) when it is open; otherwise dropped |
| 0x34b | cs+0x33910 -> cs+0x1db980 | forwarded to the open item-use window of kind 1 (clan name), its vfunc +0x48 |
| 0x34d | cs+0x33940 -> cs+0x1db9b0 | forwarded to the open item-use window of kind 2 (clan mark), its vfunc +0x48 |

The `{+6 ok, +0xa reason}` reading of 0x349 / 0x34b inside those windows was not followed (vfunc +0x48 of the window classes); the LIVE layout above
stands.

**Mark and flag fields.** Clan emblem / background and name colour appear in 0x303 (+0xd4 / +0xd8 / +0xe0), 0x371
(+0x32 / +0x36 / +0x3a), 0x38b (+0xa4 / +0xa8 / +0xac) and the profile 0x379. Emblem / background numbers index
the ROM clan-mark table. The nation index is a row of the ROM country table (0 = none). No 2009 request sets the
nation. LIVE.

**Client crash quirk (OBSERVED 3 times, 2026-10-20).** A 0x3c3 arriving while an item-use window is open crashes the
client (access violation at cs+0x2ebc72; the 0x3c3 handler cs+0x35750 forwards to the open use window). The server
rule that avoids this is CDP-19 in the proposed defaults.

### 5.2 Clan web pages (client behaviour)

Create, join, leave, kick, search and manage are **web pages** shown in an embedded browser inside the Clan screen.
Three URL pointers live in the client's data at cs+0x4475d8 / cs+0x4475dc / cs+0x4475e0, and the client appends the
login token. A page that ends with "leave clan" must make the lobby push 0x3064. Joining shows the member again via
0x303 / 0x3061 (a leave or kick pushes **0x3064 then 0x303** to the removed user and 0x3061 + 0x3062 to the remaining members; a create or join pushes
0x303 + 0x3061 + 0x3062 to the user and 0x3061 + 0x3062 to the other members; DeCombat design, clan addendum 5.2a). LIVE, with the pointers redirected by our launcher to a local page server. The page content and
URLs are ours. No URL from the ROM is reproduced here.

**Pages served by the DeCombat lobby** (our design, not 2009 data; the launcher redirects the three URL pointers to them; the 2009 client only needs a
page it can show): `GET /clan?url=guild/list|manager/guildmember|guild/create|guild/leave&p=<token>` (pages); `GET /clan/create?p=&name=&emblem=&bg=`,
`/clan/join?p=&c=`, `/clan/leave?p=`, `/clan/kick?p=&m=`, `/clan/notice?p=&text=` (actions; LG-420: the join is an application, the leader's
`/clan/accept?p=&a=`, `/clan/decline?p=&a=`, `/clan/invite?p=&n=` answer it or invite). `p` may also be `strPassport` / `passport`, with or without
`LOCAL:`.

**Clan chat and online flags** go through the client's messenger component (section 6.3), not the lobby protocol.

---

## 6. Messenger, notices, profile, operator (GM) commands

### 6.1 Profile

| id | our name | dir | size | layout | evidence | status |
|---|---|---|---|---|---|---|
| 0x378 | `profile_fetch` | C->S | 0xa | +6 u32 in-channel id (double-click a user) | PROVEN cs+0x18dd70 | PROVEN, LIVE |
| 0x379 | `profile_reply` | S->C | 0xe2 | account number, in-channel id, wstr[17] name, exp, headshots, kills, deaths, wins, losses, rank (+0x50), grade, clan id, wstr[17] clan name, u32[8], emblem, background, name colour, clan colour, nation (+0xae), u32[12] | LIVE (profile shows flag, nation, rank, clan, combined rank) | LIVE |

### 6.2 Notices and pushes

0x3052 (ticker), 0x3065 (broadcast), 0x367 (notice box), 0x3013 (case announcement) and 0x3004 (progress) are in
sections 3 and 4.

### 6.3 Messenger

Friends, whisper and the friend-menu invite in the 2009 client go through an external messenger component
(cs+0x45a508 object), not the lobby protocol. Our local emulation (LIVE, two clients) carries it through:
- client -> server: lobby chat 0x3a0 with a text prefix (our convention, see the harness);
- server -> client: a custom push id **0x3f01** {+6 wstr text}. The 2009 dispatcher ignores this id. Our launcher
  hook reads it.
This is our own extension, not a 2009 message. A 2009-only server has no messenger.

**0x3f01 line format of the DeCombat relay** (our extension, implemented in `crates/dc-lobby-server/src/messenger.rs`; GM addendum 2, applied 2026-10-24).
Lines are NUL-terminated wstr, fields separated by TAB; field 1 is the kind: `status` (text), `request` (from-name), `friends` (own name, friend
names...), `online` (name, 1/0), `whisper` (from, to, text), `invite` (from, room number). Client commands are chat lines `!nm <verb>` plus
TAB-separated arguments; verbs `add`, `accept`, `decline`, `remove`, `list`, `w` / `whisper`, `invite`. The launcher hook that shows it must follow this
table. LG-420 (DC_DEFAULTS): kind `claninvite` (clan name, clan number), a clan leader's invitation, re-sent at login while open; answers
`!nm clanyes` / `!nm clanno` with the clan number. HARNESS: test_persist_restart asserts the `status` / `friends` lines.
**CORRECTION 2026-10-26 (client-matchexit):** the `!nm sync` line that the reference stack sends after a match is **not a 2009 client message**: no `!nm` line appears in any real-client capture and CShell.dll holds no such text. A DeCombat lobby may tolerate it as a no-op (CLI-274) but must not expect it.

### 6.4 Operator commands (sent from chat with a leading `/`)

The chat box (cs+0x42920) treats a line that starts with `/` as an operator command when the shell's operator flag
is set. The command words are client data. They map to three request packets. The client pushes every request onto
a pending list (append cs+0x3e520 / cs+0x3e5b0; entries of 0xb2 bytes). The matching reply consumes the oldest
entry and uses its fields to word the confirmation.

| id | our name | dir | size | layout | evidence | status |
|---|---|---|---|---|---|---|
| 0x3d0 | `op_restrict` | C->S | 0xb2 | +6 wstr[17] target name; +0x28 u32 action (1 or 2 for the two restrict commands, 0 for lifting them); +0x2c i32 period (from the `-p` argument; 0 if absent); +0x30 wstr[65] reason (from the `-r` argument) | PROVEN cs+0x42920 -> cs+0x18e790 | PROVEN, LIVE |
| 0x3d1 | `op_restrict_reply` | S->C | >= 0xc | +6 u32 result: 1 -> confirmation by the pending entry's action: 0 -> `res 0x180a`(name, reason), 1 -> `res 0x180b`(name, reason), 2 -> `res 0x180c`(name, period, reason); != 1 with `+0xa u16 reason == 0x5d` -> `res 0x1809`(name) (target unknown); other failures are silent | PROVEN cs+0x3e0f0 | PROVEN, LIVE |
| 0x3d2 | `op_lookup` | C->S | 0x2c | +6 i32 selector: 0 = by account number (u32 at +0xa), 1 = by login id, 2 = by character name (wstr[17] at +0xa); -1 = nothing sent | PROVEN cs+0x40980 -> cs+0x18e850 | PROVEN, LIVE |
| 0x3d3 | `op_lookup_reply` | S->C | >= 0x90 | +6 u32 result (1 ok); +0xa u16 reason (0x6e invalid query, 0x6f no such user); +0xe u32 account number; +0x12 wstr[17] login id; +0x34 wstr[17] character name; +0x56 wstr[17] client address text (empty = none); +0x78 u32 online; +0x7c u32 server-kind index (0..8, a 9-entry table in the client); +0x80 i32 server index into the client's server list (-1 none); +0x84 i32 channel (0-based, -1 none); +0x88 i32 room (0-based, -1 none); +0x8c u32 in a match. Printed as a multi-line report. | PROVEN cs+0x40dd0 | PROVEN, LIVE |
| 0x3d4 | `op_mute` | C->S | 0xb2 | +6 wstr[17] target name; +0x28 u32 on (1 mute, 0 unmute); +0x2c i32 minutes (from `-t`; 0 if absent); +0x30 82 zero bytes | PROVEN cs+0x40790 -> cs+0x18e910 | PROVEN, LIVE |
| 0x3d5 | `op_mute_reply` | S->C | >= 0xc | +6 u32 result: != 0 -> confirmation by the pending entry's on flag (0: name; 1: name and minutes; client format strings); 0 with `+0xa u16 reason == 0x87` -> `res 0x1809`(name) | PROVEN cs+0x3e300 | PROVEN, LIVE |
| 0x3053 | `mute_until_push` | S->C | 0xe | +6 FILETIME chat-muted-until (8 bytes, local time) -> shell +0x448 (the same field as 0x303 +0xec). While the remaining time is >= 1 minute, the chat senders refuse to send and print `res 0x185d`(minutes) or `res 0x185e` (cs+0x40ca0 via cs+0x1b1640). | PROVEN cs+0x361c0 -> cs+0x1b1620 | PROVEN, LIVE |

The chat-mute check runs on the client. The server must still drop chat from a muted player (proposed default
CDP-21), because a modified client can skip the check.

### 6.5 GM accounts and GM names (lobby side; DeCombat rules LG-170..175, branch gm-lobby 2026-10-29)

The 2009 client has two independent GM signals:
- **Operator flag** = 0x201 +0x26 `== 2` (cs+0x42450): the `/` commands of 6.4, `/k NAME` in the waiting room, the
  member menu's "Kick User" for every member, entry to full servers / channels through the operator slots, clan-only
  servers without a clan. The lobby sends 2 for an account with `accounts.gm != 0` and 0 otherwise (LG-170; the
  flag reaches the client at the next login).
- **Name prefix** "GM" (`wcsncmp(name, L"GM", 2)`, case sensitive; e.g. cs+0x1604b0): the GM rank icon, yellow chat,
  local kick immunity, no vote kick. The lobby reserves the prefix for GM accounts at 0x204 / 0x202 (0x205 free 0,
  0x203 code 0xe), at the name-change item (reason 0x53) and on the registration page (LG-172).

Lobby rules on the wire:
- `/k NAME` and "Kick User" both send 0x38e {+6 in-channel id, +0xa character number} (cs+0x15fef0 / cs+0x15d740 ->
  cs+0x18f280): the client resolves the name in its own member list and refuses locally with `res 0x57c` (no such
  member) or `res 0x57b` (member state 3). The server needs no name lookup.
- 0x38e from a GM account is accepted in any room the GM is in; from anybody else only from the host (CD-27).
- A 0x38e whose target is a GM account or a GM-named character is answered 0x38f {0, reason 0x82} (`res 0x1845`).
- A kick by a GM account removes the target with 0x3034 reason 2 (`res 0x565`); a host kick keeps reason 1
  (`res 0x1851`).
- Operator slots: server_entry +0x98 / +0x9c and channel_entry +0x52 / +0x56 carry capacity 10 and the GMs online
  there; the lobby admits a GM to a full channel while fewer than 10 GMs are in it (LG-175).
- Vote kick: not run by the lobby or dc-dserver; a future one must never target a GM (LG-174).

Client side (DeCombat CLI-1300..1306, branch gm-client): dc-client reads +0x26, sends 0x3d0 / 0x3d2 / 0x3d4 from an
operator's `/` lines and words their replies from its pending lists (6.4), sends `/k NAME` and Kick User as the 0x38e
above (Kick User after the local refusals `res 0x1844` (member state 2 or 3) / `res 0x1845` (GM-named target),
cs+0x1604b0), and uses the operator slots only when the entry carries a non-zero operator capacity.

---

## 7. Game start handshake (lobby side only)

The UDP connection to the game server is specified separately. The lobby part:

1. The host presses Start: `room_user_flag` 0x392 (0, 2) (cs+0x15d680).
2. The lobby answers 0x393 {ok} and pushes to the room: 0x3035 {type 7, 1 starting}, 0x3042 {id, 2} per ready
   player, then **0x3044** once per second (LIVE: 5 ticks), then 0x3035 {7, 2} and 0x3042 {id, 3}, then **0x3043
   per player**. The pacing and conditions are server policy (CDP-11).
3. The client stores the address and runs its connect sequence (cs+0x1630a0). On a failure it returns to the waiting
   room ("Timeout", `res 0xbd0`) and sends 0x394 (LIVE: twice, +6 = 0 then 1). The lobby connection stays up.
4. A player who enters during the countdown gets the remaining 0x3044 ticks but no 0x3043. Its Ready button
   becomes Start (room option mid-match join), and its 0x392 (0, 1) must be answered with 0x393 ok + 0x3043 (LIVE).

| id | our name | dir | size | layout | evidence | status |
|---|---|---|---|---|---|---|
| 0x3044 | `start_countdown_push` | S->C | 0xa | +6 u32 count 0..4 -> chat line "starting in (5 - count) s" (`res 0x57a`) | PROVEN cs+0x33530 -> cs+0x15f2b0 | PROVEN, LIVE |
| 0x3043 | `match_address_push` | S->C | 0x30 + 0x50n | +6 wstr[17] game server IPv4 text; +0x28 u32 game server UDP port; +0x2c u32 record count N; +0x30 N x 0x50-byte records copied into the client's ranking panel (layout unproven: send N = 0). Accepted on screens 9 / 16 / 24..27. | PROVEN cs+0x165650 | PROVEN, LIVE |
| 0x394 | `match_exit` (formerly `match_join_failed`) | C->S | 0xa | see 3.7 and 7.1 | PROVEN cs+0x18b010 | PROVEN, LIVE, PROVEN-CAPTURE |

There is **no session key, ticket or world name** in 0x3043 (PROVEN cs+0x165650). The game server identifies the
player by what the client sends in its first game messages (separate spec), together with the lobby session index
from 0x301 +0x1e. PROVEN-CAPTURE (tdm1, tdm2): the client's first game message `UP_PLAYER_INFO` carries `room_slot` = 0x383 +0x12, `char_no` = 0x303 +0xe and `session_idx` = 0x301 +0x1e of the same login, and the hello blob
carries the login name (UTF-16), so the game server can match the player without any key in 0x3043.

**7.1 Observed lobby traffic around a match (real client in the captures; the lobby pushes are the reference server's, REFERENCE).**

| moment | lobby traffic | grade |
|---|---|---|
| room start (tdm2, tdm4) | 0x3035 {7, 2} and 0x3042 {id, 3} per player, then 0x3043 to **both** accounts in the same millisecond (the second player is a bot; the real client is the slower loader: its file-list check took 5.7 s before the world loaded) | REFERENCE |
| join of a running match (tdm2, 15 s after leaving) | client 0x392 (0, 1); the lobby answered **in the same millisecond** with 0x393 ok, 0x3042 {id, 3} to both accounts, and 0x3043 (step 4 above confirmed again; the game server saw the hello 7.3 s after the 0x3043) | PROVEN-CAPTURE (the client's ask), REFERENCE (the answer) |
| leaving a running match | client `UP_LEAVE_MATCH` 0xbd on the game link and 0x394 phase 0 on the lobby link in the same millisecond; the lobby pushes 0x3042 {id, 0, 0} to the room at once; later, when the game screen is closed, the client sends 0x306 {1}, 0x388 (answered by 0x389) and 0x38a (answered by 0x38b), and, on the game link, the plain transport `CTL_BYE` four times (7-9 s after the game server's bye code, `UDP_TRANSPORT_2009.md` 9.1) | PROVEN-CAPTURE |
| after a goal or forfeit end (bot as winner; tdm2, tdm4, botduel) | to every account still in the match, in the tick of the game server's 0x65: 0x3004 (exp total, GP total, popup flag 1; for example exp 43 and GP 10,034 after a 34-GP gain from 10,000) followed by 0x345 (balance); **none to the leaver**; 0x3042 {id, 0, 0} to the room when the players have left, 0x3035 type 7 (room state 0) and, if the host left, 0x3035 type 2 (host hand-over) to the remaining member; during the match 0x3014 pushes when a player walked over a dropped HiSec case | REFERENCE |
| the stored kill and death numbers | not visible: no 0x201, 0x303 or 0x379 was sent after a match and the statistics words of the 0x38b room records stayed 0 for both accounts | UNDECIDED |

**CORRECTION 2026-10-26 (report client-matchexit; three real-client matches on our stack: rust6, rust7, realclient; disassembly cshell+0x137dd0, cshell+0x1343b0, cshell+0x132400):** the 2009 client's match end, lobby side. The game server's 0x46 draws the scoreboard board 3000 ms later; 0x3f then ends the in-game frame and opens the result screen (`GAME_MESSAGES_2009_S2C.md` 2.1 and 2.1.2). Nothing in-game is sent after 0x3f. The player leaves with **0x306 {0xb}** (location "result") when the result screen takes focus, in the same moment as the game link closes, **7.7-9.3 s after 0x3f** (7657 / 8172 / 9266 ms; plain `c9 03` x 4, the realclient capture also a reliable `C2S_BYE` 16 ms later; the delay is not traced). The result screen (page "result" by name; window 0 two lists with team scores, rank / name / kills / deaths / mission / EXP columns; window 2 "MY SCORE" with level, ID, EXP, GP; windows 1 / 3 / 4 co-op board, clan summary, level-up; exit button command 6; cshell+0x132350, cshell+0x137dd0, cshell+0x132b80) closes by its exit command (command 6 -> the waiting-room screen 9; 0x3b -> room leave 0x386, cshell+0x132400) or **by itself 60000 ms after its rows were filled** (cshell+0x1343b0; observed 61547 / 62000 ms after 0xb, 16406 ms in rust6 where the player clicked). Room re-entry is **0x306 {1}, 0x388 (-> 0x389), 0x38a (-> 0x38b) in the same millisecond**, then the player's own actions (realclient: 0x392 ready 1.9 s later). The reference stack's per-match lobby pushes are in the rows above and stay REFERENCE.

The amounts are the reference stack's reward rule and are not copied (CD-54). The Rust lobby treats 0x394 {0} as "member left the match" (closed 2026-10-25, see 7.2).

**7.2 Fold record (spec-fold-5, 2026-10-25).** Sources: `reports/lobby-loadout.md`, `dserver-slice4.md`, the K-ids of `CAPTURE_tdm4_KILL_VALIDATION.md`. Grades as in this
spec plus **D** = DeCombat design (a DC_DEFAULTS row, proposed) and TESTED / RUN (a crate test / a loopback run of our own stack); none of these is 2009 evidence.

| item | finding | grade |
|---|---|---|
| K16 (tdm4) | 0x394 phase 0 is sent on a normal leave too (hence `match_exit`, 3.7, 7.1); 0x3004 went only to the account still in the match; the stored kills / deaths are not visible on the wire | PROVEN-CAPTURE (0x394), REFERENCE (0x3004) |
| 0x394 {0} in `dc-lobby-server` | no reply; the member goes back to waiting, 0x3042 to the room; the last one out ends the match (0x3035 type 7 value 0); a repeated {0} or a following {1} is a no-op; outside a room or in a waiting room it is ignored (CD-45) | D, TESTED (flow tests) |
| 0x3004 recipients | DeCombat pushes it to every account with an `@player` line of the result (the reference stack only to accounts whose totals changed and that were still in the match) | D (OK-DIFF), TESTED |
| RM-1 | a successful 0x38c on a waiting room (any field except host hand-over and password) clears every other member's ready state: 0x3042 state 0 per cleared member, after the 0x3035 update and before the 0x38d reply (the order is ours; the captures show only "after each 0x38c"); a start then needs a new 0x392 ready | D, TESTED; RUN not exercised |
| AUTH-1 | the first account of a never-used accounts table gets number 1001 (numbering parity with the capture reference stack for `captap shadow`); a table that had accounts continues its own numbering | D, TESTED |
| game server pipe | the lobby's `user` line now carries `loadout=<slot>:<item>,...` and `face=<n>` (LDO-1): `DSERVER_PIPE.md` 11, 12.A | D, TESTED |

Not decided: whether the 2009 lobby cleared ready states on a settings change (RM-1 rests on capture tdm1's "after each 0x38c" only); the 0x3004 recipient rule of the 2009 lobby. The crates still name 0x394
`MATCH_JOIN_FAILED` (`staging/spec-fold-5_CRATE_FOLLOWUPS.md`).

---

## Appendix A. Id table

Dir: C->S request, S->C reply or push. Status: PROVEN (2009 CShell C read at the rva), LIVE (real 2009 client
observed), both, or "dead" (2009 code exists but no caller). Inner-id overrides are marked.

| id | our name | dir | status | evidence |
|---|---|---|---|---|
| 0x200 | login_request | C->S | PROVEN, LIVE | cs+0x191e30 |
| 0x201 | login_reply | S->C | PROVEN, LIVE | cs+0x42450 |
| 0x202 | character_create | C->S | PROVEN, LIVE | cs+0x1922d0 |
| 0x203 | character_create_reply | S->C | PROVEN, LIVE | cs+0x34e60 |
| 0x204 | name_check | C->S | PROVEN, LIVE | cs+0x1921f0 |
| 0x205 | name_check_reply | S->C | PROVEN, LIVE | cs+0x34de0 |
| 0x206 | file_check | C->S | PROVEN, LIVE | cs+0x191fb0 |
| 0x207 | file_check_reply | S->C | PROVEN, LIVE | cs+0x38ce0 |
| 0x20a | server_pick | C->S | PROVEN, LIVE | cs+0x1924a0 |
| 0x20b | server_pick_reply | S->C | PROVEN, LIVE | cs+0x34f60, cs+0x13dff0 |
| 0x20e | referrer_check | C->S | PROVEN | cs+0x1925b0 |
| 0x20f | referrer_check_reply | S->C | PROVEN | cs+0x35b80, cs+0x12cf70 |
| 0x2000 | server_list_push | S->C | PROVEN, LIVE | cs+0x38a20, cs+0x13eda0 |
| 0x2001 | auth_alive_push | S->C | PROVEN | cs+0x42df0 |
| 0x2002 | bonus_event_push | S->C | PROVEN | cs+0x356b0 |
| 0x300 | lobby_login | C->S | PROVEN, LIVE | cs+0x18ab20 |
| 0x301 | lobby_login_reply | S->C | PROVEN, LIVE | cs+0x420d0 |
| 0x302 | character_info | C->S | PROVEN, LIVE | cs+0x18acf0 |
| 0x303 | character_info_reply | S->C | PROVEN, LIVE | cs+0x3f2c0 |
| 0x304 | logout | C->S | PROVEN | cs+0x18d210 |
| 0x305 | logout_reply | S->C | PROVEN | cs+0x35160 |
| 0x306 | location | C->S | PROVEN, LIVE | cs+0x18ac20 |
| 0x308 | option_set | C->S | PROVEN | cs+0x18da20 |
| 0x309 | option_set_reply | S->C | PROVEN | cs+0x35a20 |
| 0x30a | private_state | C->S | PROVEN | cs+0x18b6b0 |
| 0x30b | private_state_reply | S->C | PROVEN | cs+0x35c90 |
| 0x30c | private_state_end | C->S | PROVEN | cs+0x18b780 |
| 0x30d | private_state_end_reply | S->C | PROVEN | cs+0x35cc0 |
| 0x30e | observer_mode | C->S | PROVEN, LIVE | cs+0x18de40 |
| 0x30f | observer_mode_reply | S->C | PROVEN (case) | cs+0x42df0 |
| 0x310 | room_settings_fetch | C->S | PROVEN, LIVE | cs+0x18ec30 |
| 0x311 | room_settings_reply | S->C | PROVEN, LIVE | cs+0x3dbf0, cs+0x3db40 |
| 0x312 | player_report | C->S | PROVEN | cs+0x273790, cs+0x18e9d0 |
| 0x313 | (ignored) | S->C | PROVEN | cs+0x42df0 |
| 0x320 | inventory | C->S | PROVEN, LIVE | cs+0x18ae70 |
| 0x321 | inventory_reply | S->C | PROVEN, LIVE | cs+0x42340, cs+0x229130 |
| 0x322 | equipment | C->S | PROVEN, LIVE | cs+0x18adb0 |
| 0x323 | equipment_reply | S->C | PROVEN, LIVE | cs+0x229ca0 |
| 0x324 | equip (also frame of 0x368 / 0x36a) | C->S | PROVEN, LIVE | cs+0x18b0e0 |
| 0x325 | equip_reply | S->C | PROVEN, LIVE | cs+0x34ab0 |
| 0x326 | unequip | C->S | PROVEN | cs+0x18af30 |
| 0x327 | unequip_reply | S->C | PROVEN | cs+0x34a40 |
| 0x328 | shop_buy | C->S | PROVEN, LIVE | cs+0x18cf50 |
| 0x329 | shop_buy_reply | S->C | PROVEN, LIVE | cs+0x336a0 |
| 0x32a | gift_send | C->S | PROVEN, LIVE | cs+0x18e0c0 |
| 0x32b | gift_send_reply | S->C | LIVE | cs+0x1d6cb0 |
| 0x32c | inbox_page | C->S | PROVEN, LIVE | cs+0x18df10 |
| 0x32d | inbox_page_reply | S->C | LIVE | cs+0x33730 |
| 0x32e | inbox_open | C->S | PROVEN, LIVE | cs+0x18dfe0 |
| 0x32f | inbox_open_reply | S->C | PROVEN, LIVE | cs+0x33ca0 |
| 0x330 | item_use | C->S | PROVEN, LIVE | cs+0x18e1d0 |
| 0x331 | item_use_reply | S->C | PROVEN (kinds 6 to 9, 11: DISASM) | cs+0x2452f0 |
| 0x334 | item_discard | C->S | PROVEN, LIVE | cs+0x18a480 |
| 0x335 | item_discard_reply | S->C | PROVEN, LIVE | cs+0x32ee0 |
| 0x336 | attach | C->S | PROVEN, LIVE | cs+0x18b1c0 |
| 0x337 | attach_reply | S->C | PROVEN, LIVE | cs+0x34b10 |
| 0x338 | detach | C->S | PROVEN | cs+0x18b2a0 |
| 0x339 | detach_reply | S->C | PROVEN, LIVE | cs+0x34b70 |
| 0x33a | tuning_sync | C->S | PROVEN | cs+0x18d2d0 |
| 0x33c | period_finish | C->S | PROVEN (dead) | cs+0x18d3c0 |
| 0x33e | expiry_check | C->S | PROVEN (dead) | cs+0x18d480 |
| 0x33f | expiry_push | S->C | PROVEN, LIVE | cs+0x22cde0 |
| 0x340 | coupon_redeem | C->S | PROVEN, LIVE | cs+0x18db00 |
| 0x341 | coupon_redeem_reply | S->C | PROVEN, LIVE | cs+0x26a3e0 |
| 0x342 | item_sync | C->S | PROVEN, LIVE | cs+0x18dbf0 |
| 0x343 | item_sync_reply | S->C | PROVEN, LIVE | cs+0x35c40 |
| 0x344 | balance | C->S | PROVEN, LIVE | cs+0x18e310 |
| 0x345 | balance_reply | S->C | PROVEN, LIVE | cs+0x337b0 |
| 0x346 | recipient_check | C->S | PROVEN, LIVE | cs+0x18e410 |
| 0x347 | recipient_check_reply | S->C | LIVE | dispatcher cs+0x42df0 |
| 0x348 | clan_leader_check | C->S | PROVEN | cs+0x18e4f0 |
| 0x349 | clan_leader_check_reply | S->C | LIVE | dispatcher cs+0x42df0 |
| 0x34a | clan_name_check | C->S | PROVEN, LIVE | cs+0x18e5d0 |
| 0x34b | clan_name_check_reply | S->C | LIVE | dispatcher cs+0x42df0 |
| 0x34c | clan_mark_check | C->S | PROVEN, LIVE | cs+0x18e6b0 |
| 0x34d | clan_mark_check_reply | S->C | PROVEN, LIVE | cs+0x33940 |
| 0x34f | coupon_redeem_reply_b | S->C | PROVEN | cs+0x42df0 -> cs+0x26a3e0 |
| 0x350 | shop_view_fetch | C->S | PROVEN, LIVE | cs+0x18ea90 |
| 0x351 | shop_view_reply | S->C | PROVEN, LIVE | cs+0x42ca0 |
| 0x352 | list_fetch_0 | C->S | PROVEN, LIVE | cs+0x18ed10 |
| 0x353 | list_reply_0 | S->C | PROVEN, LIVE | cs+0x2581c0 |
| 0x354 | list_fetch_1 | C->S | PROVEN, LIVE | cs+0x18ed10 |
| 0x355 | list_reply_1 | S->C | PROVEN, LIVE | cs+0x258240 |
| 0x356 | list_fetch_2 (also frame of 0x35a / 0x35c) | C->S | PROVEN, LIVE | cs+0x18ed10 |
| 0x357 | list_reply_2 | S->C | PROVEN, LIVE | cs+0x2582c0 |
| 0x358 | price_fetch | C->S | PROVEN, LIVE | cs+0x18eb60 |
| 0x359 | price_reply | S->C | PROVEN, LIVE | cs+0x41350 |
| 0x35a | list_fetch_3 (frame 0x356) | C->S | PROVEN | cs+0x18ed10 |
| 0x35b | list_reply_3 | S->C | PROVEN | cs+0x42df0 |
| 0x35c | list_fetch_4 (frame 0x356) | C->S | PROVEN | cs+0x18ed10 |
| 0x35d | list_reply_4 | S->C | PROVEN | cs+0x42df0 |
| 0x35e / 0x360 / 0x362 / 0x364 | function_item_a..d | C->S | PROVEN (dormant) | cs+0x18ee60 |
| 0x35f / 0x361 / 0x363 / 0x365 | function_item replies | S->C | PROVEN | cs+0x339d0 |
| 0x366 | notice_fetch | C->S | PROVEN, LIVE | cs+0x18dcb0 |
| 0x367 | notice_reply | S->C | LIVE | dispatcher cs+0x42df0 |
| 0x368 | function_equip (frame 0x324) | C->S | PROVEN | cs+0x18f7b0 |
| 0x369 | function_equip_reply | S->C | PROVEN | cs+0x36710 |
| 0x36a | function_unequip (frame 0x324) | C->S | PROVEN | cs+0x18f890 |
| 0x36b | function_unequip_reply | S->C | PROVEN | cs+0x36740 |
| 0x36c | megaphone_send | C->S | PROVEN, LIVE | cs+0x18f470 |
| 0x36d | megaphone_send_reply | S->C | PROVEN, LIVE | cs+0x36770 |
| 0x36e | megaphone_queue | C->S | PROVEN, LIVE | cs+0x18f5d0 |
| 0x36f | megaphone_queue_reply | S->C | PROVEN, LIVE | cs+0x367b0 |
| 0x370 | channel_users | C->S | PROVEN, LIVE | cs+0x189f20 |
| 0x371 | channel_users_reply | S->C | PROVEN, LIVE | cs+0x1220e0 |
| 0x372 | channel_enter | C->S | PROVEN, LIVE | cs+0x189db0 |
| 0x373 | channel_enter_reply | S->C | PROVEN, LIVE | cs+0x32e30, cs+0xfacd0 |
| 0x374 | channel_leave | C->S | PROVEN, LIVE | cs+0x18d050 |
| 0x375 | channel_leave_reply | S->C | PROVEN, LIVE | cs+0x35010 |
| 0x376 | channel_list | C->S | PROVEN, LIVE | cs+0x18d110 |
| 0x377 | channel_list_reply | S->C | PROVEN, LIVE | cs+0x350f0, cs+0xfb880 |
| 0x378 | profile_fetch | C->S | PROVEN, LIVE | cs+0x18dd70 |
| 0x379 | profile_reply | S->C | LIVE | dispatcher cs+0x42df0 |
| 0x380 | room_list | C->S | PROVEN, LIVE | cs+0x18ca00 |
| 0x381 | room_list_reply | S->C | PROVEN, LIVE | cs+0x332a0, cs+0x12a530 |
| 0x382 | room_create | C->S | PROVEN, LIVE | cs+0x18a560 |
| 0x383 | room_create_reply | S->C | PROVEN, LIVE | cs+0x33320, cs+0x138ad0 |
| 0x384 | room_enter | C->S | PROVEN, LIVE | cs+0x18a710 |
| 0x385 | room_enter_reply | S->C | PROVEN (wrapper), LIVE | cs+0x33370 |
| 0x386 | room_leave | C->S | PROVEN, LIVE | cs+0x18b380 |
| 0x387 | room_leave_reply | S->C | PROVEN, LIVE | cs+0x33400 |
| 0x388 | room_info | C->S | PROVEN, LIVE | cs+0x18a820 |
| 0x389 | room_info_reply | S->C | PROVEN, LIVE | cs+0x1684c0 |
| 0x38a | room_users | C->S | PROVEN, LIVE | cs+0x18b440 |
| 0x38b | room_users_reply | S->C | PROVEN, LIVE | cs+0x166660 |
| 0x38c | room_setting_change | C->S | PROVEN, LIVE | cs+0x18a8e0, cs+0x18a9c0 |
| 0x38d | room_setting_change_reply | S->C | PROVEN, LIVE | cs+0x162370 |
| 0x38e | room_kick | C->S | PROVEN | cs+0x18f280, cs+0x18f1a0 |
| 0x38f | room_kick_reply | S->C | PROVEN | cs+0x160340 |
| 0x390 | team_change | C->S | PROVEN, LIVE | cs+0x18b500 |
| 0x391 | team_change_reply | S->C | PROVEN | cs+0x15f070 |
| 0x392 | room_user_flag | C->S | PROVEN, LIVE | cs+0x18b5d0 |
| 0x393 | room_user_flag_reply | S->C | PROVEN, LIVE | cs+0x15f0d0 |
| 0x394 | match_exit (formerly match_join_failed) | C->S | PROVEN, LIVE, PROVEN-CAPTURE | cs+0x18b010 |
| 0x396 | room_invite_all | C->S | PROVEN, LIVE | cs+0x18d960 |
| 0x397 | room_invite_all_reply | S->C | PROVEN, LIVE | cs+0x39060 |
| 0x3a0 | chat_send | C->S | PROVEN, LIVE | cs+0x18a0d0 |
| 0x3b0 | server_list | C->S | PROVEN, LIVE | cs+0x18f0e0 |
| 0x3b1 | server_list_reply | S->C | PROVEN, LIVE | cs+0x13f0b0 |
| 0x3b2 | server_move | C->S | PROVEN, LIVE | cs+0x18f010 |
| 0x3b3 | server_move_reply | S->C | PROVEN, LIVE | cs+0x13e200 |
| 0x3c0 | clan_login | C->S | PROVEN, LIVE | cs+0x18d570 |
| 0x3c1 | clan_login_reply | S->C | PROVEN, LIVE | cs+0x356f0 |
| 0x3c2 | clan_info | C->S | PROVEN, LIVE | cs+0x18d630 |
| 0x3c3 | clan_info_reply | S->C | PROVEN, LIVE | cs+0x35750 |
| 0x3c4 | clan_members | C->S | PROVEN, LIVE | cs+0x18d6f0 |
| 0x3c5 | clan_members_reply | S->C | PROVEN, LIVE | cs+0x35820 |
| 0x3c6 | clan_notice | C->S | PROVEN, LIVE | cs+0x18d7b0 |
| 0x3c7 | clan_notice_reply | S->C | PROVEN, LIVE | cs+0x35880 |
| 0x3c8 | clan_notice_set | C->S | PROVEN, LIVE | cs+0x18d870 |
| 0x3c9 | clan_notice_set_reply | S->C | PROVEN, LIVE | cs+0x358e0 |
| 0x3d0 | op_restrict | C->S | PROVEN, LIVE | cs+0x42920, cs+0x18e790 |
| 0x3d1 | op_restrict_reply | S->C | PROVEN, LIVE | cs+0x3e0f0 |
| 0x3d2 | op_lookup | C->S | PROVEN, LIVE | cs+0x40980, cs+0x18e850 |
| 0x3d3 | op_lookup_reply | S->C | PROVEN, LIVE | cs+0x40dd0 |
| 0x3d4 | op_mute | C->S | PROVEN, LIVE | cs+0x40790, cs+0x18e910 |
| 0x3d5 | op_mute_reply | S->C | PROVEN, LIVE | cs+0x3e300 |
| 0x3003 | grade_push | S->C | PROVEN | cs+0x36690 |
| 0x3004 | progress_push | S->C | PROVEN, LIVE | cs+0x40580 |
| 0x3011 | expiry_notice_push | S->C | PROVEN, LIVE | cs+0x39280 |
| 0x3012 | clan_change_push | S->C | PROVEN, LIVE | cs+0x33ac0 |
| 0x3013 | case_announce_push | S->C | PROVEN, LIVE | cs+0x41470 |
| 0x3014 | inbox_stale_push | S->C | PROVEN | cs+0x33c20 |
| 0x3021 | lobby_user_add_push | S->C | PROVEN, LIVE | cs+0x122230 |
| 0x3022 | lobby_user_remove_push | S->C | PROVEN, LIVE | cs+0x1222b0 |
| 0x3023 | room_invite_push | S->C | PROVEN, LIVE | cs+0x39110 |
| 0x3031 | lobby_room_add_push | S->C | PROVEN, LIVE | cs+0x12a8b0 |
| 0x3032 | lobby_room_remove_push | S->C | PROVEN, LIVE | cs+0x126130 |
| 0x3033 | room_user_add_push | S->C | PROVEN, LIVE | cs+0x166310 |
| 0x3034 | room_user_remove_push | S->C | PROVEN, LIVE | cs+0x165400 |
| 0x3035 | room_update_push | S->C | PROVEN, LIVE | cs+0x168680, cs+0x12ae60 |
| 0x3036 | room_info_stale_push | S->C | PROVEN | cs+0x42df0 |
| 0x3037 | voice_room_push | S->C | PROVEN | cs+0x348e0 |
| 0x3038 | room_option_push | S->C | PROVEN | cs+0x38890 |
| 0x3041 | room_team_push | S->C | PROVEN, LIVE | cs+0x166c90 |
| 0x3042 | room_user_state_push | S->C | PROVEN, LIVE | cs+0x1673a0 |
| 0x3043 | match_address_push | S->C | PROVEN, LIVE | cs+0x165650 |
| 0x3044 | start_countdown_push | S->C | PROVEN, LIVE | cs+0x15f2b0 |
| 0x3051 | chat_push | S->C | PROVEN, LIVE | cs+0x192f0 |
| 0x3052 | ticker_notice_push | S->C | PROVEN, LIVE | cs+0x34270 |
| 0x3053 | mute_until_push | S->C | PROVEN, LIVE | cs+0x361c0, cs+0x1b1620 |
| 0x3054 | shop_view_version_push | S->C | PROVEN, LIVE | cs+0x36420 |
| 0x3055 | room_settings_push | S->C | PROVEN, LIVE | cs+0x3c890 |
| 0x3056 | list_version_push_0 | S->C | PROVEN, LIVE | cs+0x42df0 |
| 0x3057 | list_version_push_1 | S->C | PROVEN, LIVE | cs+0x42df0 |
| 0x3058 | list_version_push_2 | S->C | PROVEN, LIVE | cs+0x42df0 |
| 0x3059 | price_counter_push | S->C | PROVEN | cs+0x36450 |
| 0x305a | list_version_push_3 | S->C | PROVEN, LIVE | cs+0x42df0 |
| 0x305b | list_version_push_4 | S->C | PROVEN, LIVE | cs+0x42df0 |
| 0x3061 | clan_info_stale_push | S->C | PROVEN, LIVE | cs+0x42df0 |
| 0x3062 | clan_members_stale_push | S->C | PROVEN, LIVE | cs+0x35980 |
| 0x3063 | clan_notice_stale_push | S->C | PROVEN, LIVE | cs+0x35940 |
| 0x3064 | clan_removed_push | S->C | PROVEN, LIVE | cs+0x42df0 |
| 0x3065 | broadcast_push | S->C | PROVEN, LIVE | cs+0x38190 |
| 0x3f01 | (our messenger push; ignored by 2009) | S->C | LIVE | our extension |
| 0x3f02 | (our modded-servers push before 0x2000, LG-272; ignored by 2009) | S->C | ours | our extension |

## Appendix B. Unproven ids and fields

Ids whose layout (or part of it) has no 2009 evidence. Do not rely on them without a new decompiler read or capture.

- (0x331 `item_use_reply` was listed here; its layout is now in section 4.3, PROVEN. Open only: the exact window kind a 2009 server chose per item.)
- 0x3001, 0x3002 (anti-cheat pair; never send 0x3001)
- 0x3c3 `+0x26` mark word and `+0x60` leader name: layout PROVEN, use by the 2009 client not found (send as in 5.1)
- reason table with high word 4 (4.5): the source ids are listed, the target box ids were not recorded
- whether the 2009 server consumed the HiSec key itself (0x32f `+0x1e`, 4.3): PLAUSIBLE only
- 0x33b, 0x33d (no dispatcher case: the requests 0x33a / 0x33c have no reply)
- 0x3043 ranking records (+0x30, 0x50 bytes each: field layout)
- 0x381 / 0x3031 `room_record` +0x46 (not read)
- 0x206 meaning of the five checksum words (layout PROVEN, values ignorable)
- 0x377 / server_entry display-only fields (+0x90..+0xb8): offsets LIVE, meaning not read in C
