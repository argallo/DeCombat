# HISEC_2009: the 2009 "HiSec" feature (clean-room spec, DRAFT 1)

Status: DRAFT 1, written 2026-10-24 by the hisec-spec worker from the clean decompiler tree
(`CShell.dll/<rva>.c`, cited `cs+0xRVA`, CShell image base 0x37100000), read-only
capstone DISASM of the user's CShell.dll and CRes.dll (scratch scripts, nothing copied), the clean specs in
`specs/` and our harness `lobby_harness`. Not capture-validated. No ROM text is quoted:
client strings are named by CRes resource id (`res 0x....`) and their meaning is paraphrased in our words.

Grades: **PROVEN** (read in decompiler output), **DISASM** (read in capstone output, decompiler output missing or garbled),
**PLAUSIBLE** (consistent reading, not fully traced), **UNPROVEN** (guess or open question).

## 0. Finding in one paragraph

The task brief assumed that "HiSec" is a lobby security handshake (a secondary password). **It is not.** In the
2009 client "HiSec" is an **in-match item drop**: a "HiSec case" appears in the world during a match, is shown with
a HUD indicator and countdown, can be picked up, lands in the player's inbox as a supply case, and can be opened
only with a matching **HiSec key** (a counted function item). Opening it awards an item, and the lobby server
announces that award to every lobby user with push **0x3013**. The harness command `!hisec <item>` is our own test
trigger for exactly that announcement. There is **no** secondary-password prompt, request, answer or hash anywhere
in the 2009 client (section 5). DC_DEFAULTS proposals for the real feature are staged in
`staging/hisec-spec_DC_DEFAULTS.md` (HS-1..HS-12).

| piece | where | wire | section |
|---|---|---|---|
| case dropped / reserved / freed / picked up | in match | UDP shell message 0xd6 (S2C) | 3.2 |
| the case object in the world | in match | UDP object create type 0x17, pickup kind 5 (S2C) | 3.1 |
| the case countdown | in match | message to the existing pickup object, sub 2 (S2C) | 3.1 |
| owning a case and a key | lobby | inventory / inbox rows (types 0x14 and 0x15) | 4 |
| opening a case | lobby | 0x32e -> 0x32f (supply-case branch) | 4.2 |
| announcement | lobby | 0x3013 push | 2 |
| dated id substitution | host config | `server.cfg` sections `EventHisec<n>` | 3.4 |

## 1. What the harness drives

`test_lb5cash` (HARNESS: lobby_harness::test_lb5cash) sends, from lobby session `b1` (character "Alice"),
a lobby chat line 0x3a0 whose text is `!hisec 9002`, and then expects lobby session `b2` to receive **0x3013** with
total length 0x38, the sender's character name at +0x12 and the number 9002 at +0x34.

- `!hisec <item number>` is **our** harness convention (like the `/notice`, `/expire` lines of `test_lobby5` and the
  messenger prefix of PROTOCOL_2009 §6.3), not a 2009 client command: the 2009 chat box only treats a leading `/`
  as an operator command (PROTOCOL_2009 §6.4). It stands in for "this player has just opened a HiSec case and
  received item N" so that the push can be tested without the case, key and match flow. HARNESS, PROVEN (test).
- It is **not** a handshake: nothing is answered to `b1`, no password or token is involved. HARNESS.
- The test does not check +6; the client shows the line only when +6 is 0 (section 2). HARNESS / PROVEN cs+0x41470.
- DeCombat: the trigger should be accepted only in test or operator mode (HS-11, proposed), because in a real
  server the announcement follows a real case opening (section 6).

## 2. Lobby wire: 0x3013 case announcement

Routing: lobby dispatcher cs+0x42df0, `case 0x3013` -> cs+0x41470. PROVEN.

| offset | width | field (our name) | client use | grade |
|---|---|---|---|---|
| +0 | 6 | inner header (InnerHeader6, PROTOCOL_2009 §1.2; the 8-byte frame header precedes it and is not counted in these offsets) | inner id 0x3013 | PROVEN |
| +6 | u32 | `status` | the line is shown only when this is **0**; any other value: nothing happens | PROVEN cs+0x41470 |
| +0xa | 8 bytes | not read | send zero | PROVEN (no read) |
| +0x12 | wstr[17] (0x22 bytes, UTF-16LE, NUL-terminated) | `player_name` | first argument of the line | PROVEN cs+0x41470 |
| +0x34 | u32 | `award_item_number` | full item number (PROTOCOL_2009_addendum_shop §8 ranges); the client classifies it (cs+0x25c010), finds the ROM item record (cs+0x25c480) and uses the record's display name (record +0x248, converted to UTF-16 by cs+0x20e30) as the second argument | PROVEN cs+0x41470 |
| total | 0x38 | | the harness checks exactly 0x38 | HARNESS |

Client reaction (PROVEN cs+0x41470):

1. If the item number does not resolve to a ROM item record, nothing is shown (the packet is ignored).
2. A one-line text item is built from client format `res 0x18bc` (meaning: "<player> opened a HiSec case and got
   <item>"), in a fixed colour (ARGB 0xffff9a52 passed to the text item, slot +0xb0) with client font 0xe.
3. If the client is in a match (interface object slot +0xac returns an object whose +0xac is 1), the line goes to
   the in-match message log (cs+0x75fa0 on the HUD chat-log object, global VA 0x37560ca8, only when that object
   exists); otherwise it goes to the lobby chat window (slot +0xd8 of the object from cs+0x361f0).

So the push is accepted in the lobby **and** during a match; a server sends it to every connected lobby session,
including the opener and players currently in a match (PLAUSIBLE: the client gives the in-match branch no other
purpose). Whether the 2009 server also sent it to the opener is UNPROVEN (the harness checks only `b2`); HS-9
proposes yes.

No client -> server request carries 0x3013 or any id near it; the push is server-initiated only. DISASM (byte scan of
the CShell code section: the value 0x3013 appears as a push / dword / word immediate nowhere except the lobby
dispatcher's case table; the two word-pattern hits at cs+0xa4b20 and cs+0x229469 are instruction bytes, not ids).

## 3. In-match HiSec case (client side)

The case is a server-created world object plus one game message (0xd6) that tells every client what happens to
it. The client never sends anything for a HiSec case: picking up is decided by the server (PLAUSIBLE: no C2S
message in GAME_MESSAGES_2009_C2S.md refers to a pickup, and no HiSec code path writes a message).

### 3.1 The case object: object create type 0x17, pickup kind 5

The case is an `OBJ_PICKUP` object (GAME_MESSAGES_2009_S2C.md §8.2, type 0x17). The create record (constructor
cs+0x20f1c0, read inline by the create router cs+0x217760 at DISASM cs+0x2181c8..0x218317) is, in wire order:

| # | wire field | width | record offset | meaning | grade |
|---|---|---|---|---|---|
| 1 | `flag_a` | bool1 | +0x08 | not used by the HiSec code | DISASM |
| 2 | `flag_b` | bool1 | +0x0c | not used by the HiSec code | DISASM |
| 3 | `flag_c` | bool1 | +0x10 | not used by the HiSec code | DISASM |
| 4 | `fx_name` | zstr8 (buffer 0x100) | +0x14 (string) | name of a client FX attached to the object when not empty (cs+0x282490 checks the string length, creates the FX through cs+0x26e3b0) | DISASM |
| 5 | `pickup_kind` | u32 | +0x30 | **5 = HiSec case**; 6 = item pickup with the u16 pair below; 2 and 3 other pickups | DISASM |
| 6a | kind 2: `[state:u32][team:u32][at_base:bool1]` | | +0x34, +0x38, +0x3c | not HiSec: the flag of the flag modes (corrected 2026-10-24, GAME_MODES_2009 C-GM-8, DISASM cshell+0x2824d2..0x2824f0, cshell+0x5dad0) | DISASM |
| 6b | kind 3: `[bool1]` | | +0x3c | not HiSec | DISASM |
| 6c | kind 5: `case_key` | u32 | +0x48 | **the HiSec case key**: the number that 0xd6 uses to address this case | DISASM |
| 6d | kind 6: `[u16 item][u16 b]` | | +0x44, +0x46 | not HiSec (cs+0x282490 looks the u16 up in the weapon / item table and attaches its model) | DISASM |

The object (method table VA 0x374f086c; init cs+0x282490 copies record +0x30 to object +0x80 and record +0x48 to
object +0x94) answers the HiSec HUD through two accessors: slot +0x44 returns the pickup kind (cs+0x21a480, object
+0x80) and slot +0x48 returns the case key (cs+0x21a490, object +0x94). DISASM.

**Messages to the existing object** (shell id 0xa6 addressing, S2C §8.3; the object's server-message method is
slot +0x30 = cs+0x2828e0, PROVEN):

| sub (u8) | then | effect | grade |
|---|---|---|---|
| 1 | `[fx_name:zstr8(256)]` | replaces the attached client FX (the old one is removed first; none when empty) | PROVEN cs+0x2828e0 |
| 2 | `[remaining:u32]` | object +0x90 := `remaining`; the HUD shows it as a countdown (a negative value reads as 0, cs+0x2823b0) | PROVEN cs+0x2828e0, cs+0x2823b0 |

PLAUSIBLE: `remaining` is in seconds (the HUD prints it with `%d` and turns it red at <= 5, section 3.3), and the
server re-sends sub 2 while the timer runs: no client code decrements object +0x90 (DISASM of the HUD update
cs+0x9ac40 and of cs+0x2823b0; the object's own update method, slot +0x14 = cs+0x282670, was not fully traced:
UNPROVEN that it never touches +0x90).

### 3.2 Shell message 0xd6: case events

Routing: stage B roster handler cs+0x2c430, `case 0xd6` (GAME_MESSAGES_2009_S2C.md row 0xd6, there named
`GS_SECURE_MODE`; we propose `GS_HISEC_CASE`, section 7). DISASM cs+0x2e6a4..0x2e834 (the decompiler output argument
lists are garbled). Every field is read with reader slot +0x3c (`read(n)`).

```
[sub:u8]
  sub 0: (nothing)
  sub 1: [case_key:u32][player:u32]
  sub 2: [case_key:u32][owner:u32]          owner 0xFFFFFFFF (-1) = nobody
  other: nothing read
```

| sub | client reaction | grade |
|---|---|---|
| 0 | chat-log line `res 0x18c1` (meaning: "a HiSec case has dropped"), colour 0xffffff00, size 0xe, 5000 ms (0x1388) | DISASM cs+0x2e800..0x2e834 |
| 1 | looks `player` up in the roster (record +4 = engine client id). **Not found: nothing at all happens** (the case entry is not removed). Found: chat-log line `res 0x18c2` with the player's name (roster record +0xc; meaning "<player> has obtained a HiSec case"), same colour, size and 5000 ms; a kill-feed style note with the HiSec pickup image (cs+0xa75a0 on global VA 0x37560cec; the image at +0x4a0 comes from HUD layout key `MKPickupHisecImg`, cs+0xa36a0; own-team or enemy colour by the player's team); the case's entry is removed from the HUD case map (cs+0x9a3e0) and, only if an entry existed, the "picked up" sound plays; if `player` is the own client id, also the "picked up by me" sound and the pickup success banner (cs+0x993e0 starts the HUD element configured from the `HUDHiSec` keys `PickupHisecSuccessMsg*`, cs+0x98ab0) | DISASM cs+0x2e6e7..0x2e7fb; PROVEN cs+0x9a3e0, cs+0x993e0, cs+0xa75a0 |
| 2 | HUD case map: insert `{case_key -> owner}`, or update the owner of an existing entry (cs+0x9a620); sound: `owner == -1` gives the "timer running out" sound, any other owner the "drop" sound | DISASM cs+0x2e6c1..0x2e6e2; PROVEN cs+0x9a620 |

The four sounds are ROM sound-table entries looked up through cs+0x2b2c00 (OBSERVED: ROM table key
`Hisec_DropSnd`, `Hisec_TimerRunningOutSnd`, `Hisec_PickedUpByMeSnd`, `Hisec_PickedUpSnd`; cs+0x98d50 maps the
kinds 0..3 to them). Engine client ids are compared as full u32 values (keep ids below 256, S2C §3).

### 3.3 The HiSec HUD (global VA 0x37560d30, method table VA 0x374d7374)

Per frame (slot +0x14 = cs+0x9ac40, DISASM, no decompiler function) the HUD walks the client's special-object list
(client shell slot +0xd4, list at +0x174) and, for every object that has an engine object and pickup kind 5:

1. `owner` := the case map entry for the object's case key, or -1 when there is no entry.
2. It reads the object's engine visibility flag (engine object interface, VA 0x375964f0, slot +0x44, flag word 0,
   bit 0):
   - hidden and (`owner` is the own client id or -1): the client makes it visible (slot +0x48, set = 1);
   - visible and `owner` is another player: the client hides it (slot +0x48, set = 0) and draws nothing;
   - visible and (`owner` is the own client id or -1): an indicator (HUD image and size from the `HUDHiSec`
     keys read by cs+0x9a860) is drawn at the object's screen position, raised by the console value `FlagOffset`
     (default 70 world units, cs+0x19fde0), and not drawn when the point is behind the camera. When the object's
     `remaining` (3.1, sub 2) is > 0 a number is drawn with it, white while > 5 and red (0xffff0000) at <= 5.
3. Indicators are pooled (cs+0x9a860 reuses free ones) and all are hidden at the start of each frame.

Meaning (PLAUSIBLE): a dropped case can first be **reserved** for one player (`owner` = that player's engine client
id): only that player sees the case and its indicator. When the reservation ends the server sends sub 2 with
`owner` = -1: everyone sees the case and everyone hears the "timer running out" sound. A case without a map entry
(never announced with sub 2, or removed by sub 1) is visible to all. The pickup is a server decision reported by
sub 1; the server then removes the object (UDP object removal, UDP_SESSION_2009_B §5.1). UNPROVEN: which event
made the 2009 server drop a case (kill, timer, random) and how long a reservation lasted.

### 3.4 `server.cfg` sections `EventHisec<n>` (host-side configuration in CShell)

CShell contains a host configuration loader (cs+0x16a010, method table VA 0x374e6220, object from cs+0x169fb0)
for the file `..\server.cfg` (opened by cs+0x119000). After the `Event<n>` sections (dated exp / GP bonus rates)
it reads sections `EventHisec<n>`, n = 0, 1, ... while they exist, each with the keys `FromDate`, `ToDate`
(strings of up to 10 characters), `FromTime`, `ToTime` (up to 8), `SpecificWeek`, `OriginID` (int, default -1)
and `ChangeTo` (int, default -1), into 0x40-byte records (+0x38 `OriginID`, +0x3c `ChangeTo`). PROVEN cs+0x16a010.

No reader of these records was found (the record list at loader object +0x119f8 has no other reference in the
clean tree). PLAUSIBLE: during the dated window the host replaces one HiSec-related id (`OriginID`) with another
(`ChangeTo`). UNPROVEN which id (case item, key item or award). This is host behaviour; a DeCombat server takes
the idea as its own configuration (HS-10, proposed), not this file format.

### 3.5 What the reference stack did in tdm6 (REFERENCE, observed only; capval-tdm6 section 6, folded by spec-fold-8)

No value here is a DeCombat rule (HS-1 is the user's decision of about one case per match; HS-4's despawn differs on purpose). Six cases dropped in the seven kills of one TDM
round of the real client against one bot (`captures/20261025_065016_tdm6`):

1. **Drop:** every kill dropped a case unless a case was still reserved (the kill at 254.250 dropped none); free cases lying in the world did not stop a new drop. The case
   object lay at the victim's position (x and z within 10 units of its last 0x64 position; 30 units below its centre in four of six, PLAUSIBLY the floor point); the case keys
   counted from 1; 0xd6 sub 0 and sub 2 `[key][owner]` came in the kill block before 0x6d on both links.
2. **Reservation:** owner = the killer's pid for 15 s, with the countdown (0xa6 type 0x17 sub 2) sent each second (16 at the first drop, 15 later), then sub 2 `[key][-1]` with
   the countdown at 0; a reserved case was released at once when its owner died (case 3, freed at 254.250).
3. **No despawn:** no case object was removed except by a pickup; five lay in the world at the end. The one pickup (case 2, by the real client, 31.9 s after its drop): 0xd6
   sub 1 `[2][1]`, the object removed, and the lobby's 0x3014 to the user 31 ms earlier (seq 10685).
4. Other type 0x17 objects created at every death (the victim's dropped items, mostly parked at (-9999.9, -9999.8, -9999.9)) are not cases; the six cases were identified by
   their 0xd6 keys and countdown notes (the pickup kind of the blobs was not decoded: PLAUSIBLE identification).

DeCombat state (damage-calibrate, Wave Z): `dc-dserver` has no HiSec case path yet (HS-2..HS-5 not implemented), so HS-1 was not applied (`DC_DEFAULTS.md` Wave Z part 1,
section 3).

## 4. HiSec cases and keys in the shop / inventory / inbox UI

### 4.1 Which items are HiSec cases and keys (ROM tables, read at run time)

- **HiSec case** = an item of type 0x14 (supply case, item numbers 8000.., PROTOCOL_2009_addendum_shop §8) whose
  ROM `[Supply<n>]` row has a `FunctionValue<k>` list (parsed by cs+0xe5280 into the item record: count at
  +0x418, values from +0x420) whose **first value is not -1**. PROVEN: loader cs+0x25dd60 (DISASM cs+0x261605..
  0x261619 passes record +0x418 and the key `FunctionValue`), tests cs+0x14cc00, cs+0xd7ec0.
  (OBSERVED: ROM table key `FunctionValue`, `Supply`.)
- **HiSec key** = an item of type 0x15 (function item, 9000..) whose ROM `SubType` (record +0x14) is **0x13 (19)**
  and whose first `FunctionValue` equals the case's first `FunctionValue`. The client finds the key's item number
  with cs+0x25c760(type 0x15, index 0, value, subtype 0x13). PROVEN cs+0x14cc00, cs+0x25c760.
- The key is a **counted** inventory item: the client shows and decrements a count on it (4.2, 4.3). PROVEN.
- The tooltip of such a case shows an extra mark image (cs+0xd7ec0 shows image +0x138, loaded by cs+0xd46d0).
  PROVEN cs+0xd7ec0. A second panel (cs+0x692b0) loads the same mark image at +0x218; its show condition was not
  traced (UNPROVEN).
- The inventory "my items" panel has a group of 4 slots labelled with `res 0x18be` (meaning: "HiSec key"), and an
  empty function-item slot gets the same label (cs+0x1425d0, cs+0x149a20). PLAUSIBLE: the 2009 inventory keeps
  keys in their own slot group.

### 4.2 Opening a HiSec case from the inbox (lobby, before any request)

The inbox screen's open action (DISASM cs+0x15025d..0x150333, inside the inbox screen's command handler; the
selected row object is screen +0x5554 with +0x224 product id, +0x228 item number, +0x22c item type, +0x234 inbox
entry id):

1. cs+0x14cc00(item number, item type) runs the key check. For a HiSec case (4.1):
   - key item number not found in the ROM, or the player owns no such key, or the owned key fails the
     inventory check cs+0x326800: the **key-required box** cs+0x148e30(case, key, 1) opens: the case and key
     images, text `res 0x18b9` with the key's and the case's names (meaning: "a <key> is needed to open a
     <case>"), confirm button only. Nothing is sent. PROVEN cs+0x14cc00, cs+0x148e30.
   - key owned: the check passes.
2. For a HiSec case with a key the **open confirmation** cs+0x148e30(case, key, 0) opens: text `res 0x18ba`
   (meaning: "a HiSec case is being opened; one <key> will be used") and a client-formatted line with the number of
   keys left (count field +0x14 of the owned key, read through cs+0x326640). PROVEN cs+0x148e30; DISASM
   cs+0x1502b9..0x1502f6.
3. Other rows (no case, or a case without a key requirement) take the generic open dialog (cs+0x148ac0). DISASM.
4. Confirming sends **0x32e `inbox_open`** {+6 inbox entry id, +0xa product id} (cs+0x18dfe0, called at DISASM
   cs+0x150333). There is **no** separate HiSec request and the key is not named in the request: the server must
   find the key itself. PROVEN (sender), DISASM (call site).

The inventory screen runs the same key check before its own action on a selected case (DISASM cs+0x14f925;
selected item +0x555c with +0x234 item number, +0x23c type). PLAUSIBLE: opening an owned case from the inventory.

### 4.3 The answer: 0x32f supply-case branch

Handler cs+0x33ca0 -> cs+0x14e010. When the opened row's type is 0x14 the client reads the case layout
(PROTOCOL_2009_addendum_shop §2, CD-76 proposed). The HiSec-specific part (PROVEN cs+0x14e010):

| offset | width | field | HiSec meaning |
|---|---|---|---|
| +6 | u32 | ok | 0 = refused (below) |
| +0xa | u16 | reason (when ok = 0) | 0x8b / 0x8c delete the row client-side (`res 0x1891`); other values go to the reason table cs+0x393f0 |
| +0x12 | u32 | award item number | the item shown in the award pop-up |
| +0x16 | u32 | GP total after a GP award | 0 = item award |
| +0x1a | u32 | award days | shown in the pop-up (0 or > 998 = permanent) |
| +0x1e | u32 | **unique id of the key used** | non-zero: the client finds that inventory item (cs+0x326590) and, if its count +0x14 is > 0, decrements it **and selects the HiSec variant of the award pop-up** |
| +0x22 | u32 | award unique id | added to the inventory with +0x36 when +0x16 is 0 (cs+0x32c790) |
| +0x36 | FILETIME | award end | |

The award pop-up (cs+0x2412c0 -> cs+0x287150 / cs+0x287110 -> cs+0x286d40) has three title variants chosen by two
flags: HiSec (`res 0x18c4`, the pop-up flag +0x3d := "a key count was decremented"), supply crate (`res 0x18f7`,
flag +0x3e := the product's ROM row field +0x24 is 6) and plain supply case (`res 0x393`). PROVEN cs+0x2412c0,
cs+0x286d40, cs+0x14e010.

So the **only** client-visible difference between a HiSec opening and a plain case opening is +0x1e naming an
owned counted item. PROVEN. The 2009 server therefore consumed the key itself and told the client which key row to
decrement (PLAUSIBLE; the same field is what the addendum calls "unique id of a counted case item").

After the reply the server sends 0x3013 (section 2) to every lobby user. PLAUSIBLE (the announcement names the
award item, which is only known after the draw; `res 0x18bc` says "opened a HiSec case").

CRes `res 0x18c5` (meaning: "the item from the HiSec case was put into your storage") has no direct code
reference in CShell (DISASM byte scan for a push of 0x18c5: none). UNPROVEN where 2009 showed it.

### 4.4 Shop

The shop shows HiSec cases with the tooltip mark (4.1). Buying a case is the ordinary 0x328 / 0x329 flow; the
0x329 handler (cs+0x150c40) calls the award pop-up opener cs+0x2411b0, which clears both variant flags, so a case
opened on purchase never shows the HiSec title (PROVEN cs+0x2411b0). Keys are ordinary function-item products.
No HiSec-specific shop message exists. PROVEN (no other reference to the HiSec tests than 4.1 / 4.2).

## 5. No secondary-password handshake in the 2009 client

The task brief asked for the "HiSec request / answer / status" messages of a secondary-password handshake, the
client's prompt, its hash or encoding of the secondary password, and the server's success / failure / lockout
answers. The evidence says that the 2009 client has **none of these**:

| check | result | grade |
|---|---|---|
| every CShell function that mentions "HiSec" in any spelling (11 files in the clean tree: cs+0x16a010, 0x286d40, 0x692b0, 0x98ab0, 0x98d50, 0x993e0, 0x9a3e0, 0x9a620, 0x9a860, 0xa36a0, 0xd46d0) | all belong to the in-match case, its HUD, the case pop-up, the tooltip mark or the host config (sections 3 and 4) | PROVEN |
| CRes strings with "HiSec" (ids 0x18ba, 0x18bc, 0x18be, 0x18c1, 0x18c2, 0x18c4, 0x18c5) | all about cases, keys and awards | OBSERVED (ROM read, ids only) |
| CRes strings about a second / secondary password, PIN or security code | none | OBSERVED |
| CRes strings with "password" (0x1c0, 0x2fa, 0x341, 0x3f2, 0x527, 0x52c, 0x550, 0x556, 0xbcf, 0x1035, 0x1036, 0x15c4) | login password boxes and their failure texts, and the 4-character room password | OBSERVED |
| lobby login 0x200 | one password box text (+0x2006 wstr[17], PROTOCOL_2009 §2.2); no second secret field | PROVEN cs+0x191e30 (spec) |
| lobby ids next to the HiSec pushes (0x3013 announcement, 0x3014 inbox stale) | no request partner; 0x3013 is push-only (section 2) | PROVEN / DISASM |
| hash constants (MD5 / SHA-1 initial words 0x67452301, 0xefcdab89; CRC tables) referenced from any HiSec code path | none of the 11 functions above calls a hash routine; their only inputs are ids, counts and names | PROVEN (read) |

Consequently there is nothing to specify for: a HiSec prompt, inputs, a hash or encoding of a secondary password,
a server answer for success / failure / lockout, or an interaction with login or channel entry. **UNPROVEN** is the
wrong grade for those: the client simply has no such feature. Login lockout texts that do exist (0x52c, 0x556,
meaning: "too many wrong passwords, logging in is limited for a while") belong to the **login** answers of the
auth / lobby servers, not to HiSec; their trigger reasons are a matter for PROTOCOL_2009 §2 (open question Q-6).

A DeCombat secondary password would need a new client screen and new wire ids, which the 2009 ROM cannot show.
The staging file therefore proposes **no** secondary-password rule for the 2009 path; HS-12 only reserves the
rule for a future DeCombat client (proposed, out of scope).

## 6. What a DeCombat server must do

Wire facts first (from sections 2-4), then the rules a server has to choose (DeCombat defaults, staged as HS-n).

### 6.1 Wire obligations

| when | server sends | layout | grade |
|---|---|---|---|
| a case appears in a match | object create type 0x17 with `pickup_kind` 5 and a fresh `case_key` (unique per match) | 3.1 | DISASM |
| same moment | 0xd6 sub 0 to all clients (the "a case has dropped" line) | 3.2 | DISASM |
| the case is reserved for player P | 0xd6 sub 2 {`case_key`, P's engine client id} to all clients (others then hide it) | 3.2, 3.3 | DISASM / PLAUSIBLE |
| the reservation ends | 0xd6 sub 2 {`case_key`, 0xFFFFFFFF} to all clients | 3.2 | DISASM |
| every timer step | 0xa6 to the case object, sub 2 {`remaining`} | 3.1 | PROVEN (reader) / PLAUSIBLE (unit) |
| player P picks it up | 0xd6 sub 1 {`case_key`, P} to all clients (P must be in every client's roster, or nothing happens), then remove the object | 3.2 | DISASM |
| P's case is delivered | an inbox row for the case product; 0x3014 when P is in the lobby | PROTOCOL_2009 §4.3 | PLAUSIBLE |
| P opens the case (0x32e) and owns a matching key | consume one key; 0x32f case layout with +0x1e = the key's unique id and the award | 4.3 | PROVEN (reader) |
| P opens it without a key (only a modified client can) | refuse: 0x32f ok 0 with a reason (HS-7) | 4.3 | PROVEN (reader) |
| after a successful opening | 0x3013 {+6 = 0, +0x12 P's character name, +0x34 award item number} to every lobby session | 2 | PROVEN |

The client never asks the server whether a key exists: it decides locally (4.2). The server must still check,
because the request carries no key id (4.2 step 4).

### 6.2 Rules the server must choose (no 2009 evidence)

Not visible in the client and therefore DeCombat defaults (`staging/hisec-spec_DC_DEFAULTS.md`):
which modes drop cases and how often (HS-1), how a case is placed (HS-2), the reservation (HS-3), the despawn timer
(HS-4), pickup rules (HS-5), delivery (HS-6), key match and consumption (HS-7), the award draw and its duration
(HS-8), the announcement scope (HS-9), dated substitutions (HS-10), the harness trigger (HS-11), and the reserved
secondary-password rule for a future client (HS-12).

### 6.3 Interaction with login, channel entry and other screens

None. No login, server-select, channel or room message carries HiSec data, and no screen asks for HiSec input.
The only lobby screens involved are the inbox (open with key check), the inventory (key slots, key check) and the
shop (tooltip mark). PROVEN for the screens listed in section 4; the negative is PROVEN for the 11 HiSec functions
and DISASM (byte scan) for 0x3013.

## 7. Corrections proposed to other specs

Status 2026-10-24 (spec-fold-2): items 1 and 2 applied to `GAME_MESSAGES_2009_S2C.md` (row 0xd6, hud role table, 8.3); items 3 and 4
applied to `PROTOCOL_2009.md` (row 0x3013, row 0x32f and the supply-case paragraph of 4.3); item 5 staged as
`staging/spec-fold-2_dc-bute.md` (a crate patch, not applied here); item 6 is a DC_DEFAULTS matter (not applied).
The text below is kept as the record of the proposals.
spec-fold-3 (2026-10-24): the type 0x17 kind 2 row of 3.1 was corrected (GAME_MODES_2009 C-GM-8); items 1-4 above are unchanged.

1. `GAME_MESSAGES_2009_S2C.md` row 0xd6: rename `GS_SECURE_MODE` to `GS_HISEC_CASE` and the HUD role
   `hud_secure` (global VA 0x37560d30, S2C §2 role table, slot +0xc0) to `hud_hisec`; field names
   `sub 1 [case_key:u32][player:u32]`, `sub 2 [case_key:u32][owner:u32]` (-1 = nobody); add "sub 1 with a player
   not in the roster does nothing". Evidence 3.2.
2. `GAME_MESSAGES_2009_S2C.md` §8.2 type 0x17: kind 5 `[case_key:u32]` = HiSec case (record +0x48); the
   object's server message (via 0xa6) is sub 1 `[fx_name:zstr8(256)]`, sub 2 `[remaining:u32]` (cs+0x2828e0).
3. `PROTOCOL_2009.md` §4.3 row 0x3013: name the fields `status` (must be 0), `player_name`, `award_item_number`;
   note "+0xa..+0x11 not read"; the line text means "opened a HiSec case and obtained <item>".
4. `PROTOCOL_2009_addendum_shop.md` §2 and CD-76: 0x32f case branch +0x1e is the unique id of the **HiSec key**
   that was used (a counted function item, SubType 0x13); non-zero selects the HiSec pop-up title `res 0x18c4`.
5. `dc-bute` doc comment (`crates/dc-bute/src/lib.rs`, room-line option `hisec_prefix`) says "HiSec is a DeCombat
   feature (not a 2009 client concept)". Section 3 shows it is a 2009 client concept (in-match case). The room-line
   prefix itself is not 2009 evidence; whoever owns dc-bute should re-check what that option is meant to carry.
   (Reported only; this worker does not edit crates.)
6. `DC_DEFAULTS.md` CD-01 already excludes HiSec cases from the general duration table; HS-8 fills that gap.

## 8. Open questions

| id | question | how to settle |
|---|---|---|
| Q-1 | Unit and cadence of the pickup object's `remaining` (sub 2 of the object message): seconds re-sent each second, or a value the object counts down in its update (cs+0x282670 not traced)? | trace cs+0x282670 (DISASM); live test with our server |
| Q-2 | Which event dropped a case in 2009 (kill, timer, mode) and in which modes; how long a reservation lasted | no 2009 evidence expected (server rule): DeCombat default HS-1..HS-3 |
| Q-3 | Where the client showed `res 0x18c5` ("item from the HiSec case placed in storage") | search for a table or `mov` of 0x18c5 (DISASM); live test |
| Q-4 | What `EventHisec<n>` `OriginID` / `ChangeTo` substitute | find the reader of loader +0x119f8 (it may index from another base) |
| Q-5 | Whether 0x3013 reached players inside a match in 2009, or only lobby sessions | live test: the client accepts it in both (section 2) |
| Q-6 | The login lockout answers (`res 0x52c`, `0x556`): which auth / lobby reply reasons select them | PROTOCOL_2009 §2 owner; unrelated to HiSec |
| Q-7 | The inventory "4 key slots" reading (cs+0x1425d0 group (2, 6)) | screenshot of the inventory with a key owned |
| Q-8 | Visibility flag bit (engine object flag word 0, bit 0) semantics for a reserved case on a 2009 client | live test with two clients |

## Appendix I: fold record (spec-fold-8, 2026-10-25)

Source: `CAPTURE_tdm6_VALIDATION.md` section 6 (findings 1-3 and the case table) and section 11 ("HISEC section 3, reference stack note, not a rule"). New section 3.5; nothing renumbered.
Grade: REFERENCE (observed only).
