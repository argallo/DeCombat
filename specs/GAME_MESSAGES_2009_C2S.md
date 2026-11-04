# GAME_MESSAGES_2009_C2S: client -> server game messages of the 2009 client (clean-room)

- **Title:** GAME_MESSAGES_2009_C2S, the catalogue of the game messages that the 2009 `CShell.dll` writes and sends
  to the game server inside the engine's client -> server shell wrapper (`C2S_SHELL_MSG`, engine id 11), plus a
  Team Deathmatch round walkthrough that pairs them with the server -> client catalogue.
- **Scope:** every send site in CShell that reaches the engine's "send to server" method (re-derived for this spec
  from the clean decompiler tree and a read-only disassembly scan), the field sequence of each message decoded from the
  writer method slots of `GAME_MESSAGES_2009.md` section 1.4, its trigger, its reliability, its sender rva, and what
  the server is expected to answer. NOT in scope: the engine wrapper and engine-level client messages
  (`UDP_SESSION_2009_B.md` sections 6, 10, 11), server-side rules (damage, scoring, respawn delay: DeCombat defaults
  where a value is needed), the server -> client payloads (sibling spec `GAME_MESSAGES_2009_S2C.md`).
- **Evidence sources:** clean decompiler output of the 2009 EU-test `CShell.dll`
  (`CShell.dll/<rva>.c`, image base 0x37100000, cited `cshell+0xRVA`) and `Engine.exe`
  (image base 0x00460000, cited `engine+0xRVA`); read-only capstone disassembly of the user's own `Game/CShell.dll`
  from a scratch script (cited `DISASM: cshell+0xRVA`) for the 17 send sites that lie in code decompiler never made into
  functions and wherever the C lost a pushed argument; PE-DATA (addresses and integers read from the ROM, never
  strings); the clean specs `GAME_MESSAGES_2009.md` sections 1-2, `UDP_SESSION_2009_B.md`, `MOVEMENT_2009.md`,
  `UDP_TRANSPORT_2009.md`, and the parallel `GAME_MESSAGES_2009_S2C.md` for server -> client names;
  `DC_DEFAULTS.md` (cited `CD-nn`). One OBSERVED fact comes from the user's ROM room table, read read-only
  at run time (section 3.4). Since 2026-10-25 the client-to-server layouts are checked against three captures of the real
  2009 client (tdm1, tdm2, tdm4: `OBSERVED: captures/<id>/capture.jsonl`, Appendix C) and the respawn arming
  is re-derived from a further disassembly pass.
- **Status:** DRAFT 1, complete for review (written section by section; not implemented). The 14 client message ids of a TDM
  session and their layouts are PROVEN-CAPTURE (bit-exact parses); the others stay as graded in their sections.
- **Date:** 2026-10-24

Evidence grades (as in `UDP_TRANSPORT_2009.md`): **PROVEN** = literally visible in the decompiler output or the disassembly of
the cited function; **PLAUSIBLE** = one step reconstructed (a lost `this`, a branch merged by the decompiler) but the
only reading consistent with the code; **UNPROVEN** = not visible, needs a capture or a further read. A field's
**layout** (order, width) and its **meaning** carry separate grades: layouts are mostly PROVEN, meanings mostly
PLAUSIBLE. ROM strings are never quoted: where a trigger is recognised by a string the client looks up, the key is
given as `sha256(key)[:16]/length`. Capture grades (fourth fold): **PROVEN-CAPTURE** = the real client's own bytes, or a client behaviour
visible in them (2009 evidence, never downgraded); **REFERENCE** = what the reference server did (never a rule); **REFERENCE-CONFIRMED** = a server
form the real client accepted. Fifth fold (spec-fold-5, 2026-10-25): **D** = DeCombat design (a `DC_DEFAULTS.md` row, PROPOSED unless approved),
with its test state TESTED (a crate test) or RUN (a loopback run of our own stack); never 2009 evidence. Its fold record is Appendix F.

## 0. Contents

1. Send pipeline, conventions and the re-derived send-site inventory
2. Player update stream and movement notices (0x64, 0x76 sub 1)
3. Spawn, team, respawn and observer camera
4. Weapons: fire, hit report, reload, weapon change, weapon events
5. Match flow, player info and connection-level messages
6. Chat and radio commands
7. Use actions, mode objectives, votes and the remaining in-match messages
8. Generic sender, developer console commands and unreachable senders
9. Server answers: cross-reference to the server -> client catalogue
10. Appendix A: Team Deathmatch round walkthrough
11. Appendix B: names registered for `GLOSSARY_UDP.md`
12. UNPROVEN items, capture list and DeCombat default proposals

## 1. Send pipeline, conventions and the re-derived send-site inventory

### 1.1 How CShell sends a game message (PROVEN)

Every client -> server game message is built and sent the same way (example: cshell+0x1a1680, the smallest one):

1. **New writer.** CShell asks the engine common interface (CShell global VA 0x375964f0, method +0x24, with a
   pointer to a local slot as the only argument) for an empty message writer; a non-zero result means failure and
   the slot is cleared. Some sites use the inline helper cshell+0x29e50, which does the same.
2. **Fields.** The writer's methods are called in field order. The slots are those of `GAME_MESSAGES_2009.md`
   section 1.4: +0x24 `write(value, n)`, +0x28 64-bit write, +0x2c raw block of n bits from memory, +0x38 `zstr8`,
   +0x3c `zstr16`, +0x40 `lstr16h`, +0x48 `pos48`, +0x4c `quat128`, +0x5c `objref16`. The **first** field is always
   `shell_id` as `write(id, 8)`.
3. **Finish.** cshell+0x299c0 calls writer slot +0x1c and returns the finished packet (reference counted).
4. **Send.** The packet and a flag word are passed to the engine client interface: CShell global VA 0x37560ea8
   holds a pointer to that object, and the send is its **virtual** method +0x158 (`mov ecx,[0x37560ea8]; mov
   edx,[ecx]; push flags; push packet; call [edx+0x158]`), which is engine+0x13f40 (`UDP_SESSION_2009_B.md` 11.1:
   it prefixes `msg_id = 11` and queues the entry). Flag bit 0 = reliable.

Pitfall for scanners (DISASM): the same object also carries plain function-pointer **members** at object offsets,
and object offset +0x158 is one of them (called as `mov eax,[0x37560ea8]; call [eax+0x158]`, without the extra
dereference). It returns screen dimensions (for example cshell+0x1a3f30) and is **not** a send. Eleven such call
sites exist (cshell+0x48663, 0x1a3f52, 0x1a75d7, 0x1b5ecb, 0x1b6378, 0x1bbb6f, 0x1bbb9e, 0x1bbeaf, 0x1bbee4,
0x2a492e, 0x2d2029) and are excluded below.

### 1.2 Wire form and conventions

```
client -> server: [msg_id:u8 = 11][shell_id:u8][fields ...]        C2S_SHELL_MSG (UDP_SESSION_2009_B.md 11.1)
```

Layouts below start **after** `shell_id`, in bit order, exactly as written (no padding, no length). Types: `uN` =
`write(value, N)` with N bits (the low N bits of the value; signed source values are written as their low N bits,
marked `iN`); `bool1` = one bit, 1 when the source is non-zero; `f32` = the 32 raw bits of an IEEE float written
with `write(bits, 32)`; `vec96` / `raw128` = a raw block of 96 / 128 bits copied from memory with writer slot +0x2c
(three / four IEEE floats, x first; `vec96` is a position or a direction); `zstr8`, `zstr16`, `lstr16h`,
`objref16`, `pos48`, `quat128` as in `UDP_TRANSPORT_2009.md` sections 2-3 and 15 and `GAME_MESSAGES_2009.md` 1.4.
A field written only under a condition is shown as `if <cond>: [...]`; unless stated otherwise every conditional
field is announced by an earlier bit or by a sub-id, so the server can always parse the message.

Reliability column: **R** = flag 1 (reliable, ordered), **U** = flag 0 (unreliable). Our message names use the
prefix `UP_` (uplink); a sub-message carries its sub-id in the name.

Client objects use the names of the sibling server -> client spec (`GAME_MESSAGES_2009_S2C.md` section 1.2):
`local_player` (stage C object, global VA 0x3759bf60, derived method table VA 0x374d327c, base VA 0x374eefc4),
`roster` (stage B, global VA 0x375603a8), `conn_mgr` (stage D, global VA 0x3759e22c), `mode_rules` (stage E,
global VA 0x3759e120), `hud_mgr` (stage F), `session_info` (static VA 0x37598058, accessor cshell+0x1b1a10: the
lobby-side profile of the own character; fields are cited by offset and, where `PROTOCOL_2009.md` fixes them, by
the lobby message that fills them), `match_stats` (VA 0x3759e110, ammo and per-match counters), `client_shell` (VA
0x37597e50, `mode_class` at +0x16938, the mode number of `GAME_MESSAGES_2009.md` 1.2), `engine_client` (VA
0x37560ea8). Two more objects are named here:

| our name | where | method table (PE-DATA) | role |
|---|---|---|---|
| `weapon` | one per carried weapon, in the weapon list at `local_player`+0x18 (list +0x08 current index, +0x0c current weapon) | derived VA 0x374cec2c (98 slots), base VA 0x374f1914 | firing, reloading, weapon animation keys |
| `client_profile` | global VA 0x3759b9d8 | (not needed) | the client's option store (sound, video and game options of the user profile) |

Fields of `weapon` used below: +0x98 `weapon_id:u16` (the weapon's item number, the key of the ROM weapon table),
+0x9c `ammo_id:u16`, +0xa8 a `vec96` (the weapon's position, PLAUSIBLE), +0x210 the inventory slot byte (the index
`match_stats` uses for ammo, cshell+0x2399e0).

Two predicates recur:

- `is_network_game()` = cshell+0x1a0470: true when `conn_mgr` exists and its request kind (`conn_mgr`+4,
  `UDP_SESSION_2009_A.md` 2.3) is neither 4 nor 5 (4 = local offline world). Several senders test it.
- `observer_camera()` = cshell+0x1f0840 on `local_player`: true when the camera object (`local_player`+0x130) is in
  camera mode 5 (field +0x54). Weapon event notices are suppressed while it is true (PLAUSIBLE: free / observer
  camera).

### 1.3 Re-derived send-site inventory

Method (DISASM, scratch tools in the session scratchpad, never committed): every `call [reg+0x158]` in the CShell
`.text` section whose register holds the dereferenced pointer of VA 0x37560ea8 was collected. Inside the function
bounds of the clean `CShell.dll.funcs.tsv` this gives **68 sites in 50 functions**; the remaining **17 sites lie in
14 code regions decompiler never made into functions** (entry points recovered from call and method-table references,
given as `region` below). Total **85 send sites**: 82 with flag 1, 2 with flag 0 (both the player update 0x64), 1
generic helper whose flag is its caller's argument (1 at all three callers). The `UDP_SESSION_2009_B.md` 11.1 scan
reported 84 sites; it did not use the region recovery, which explains a difference of one or more. Every site
writes its `shell_id` as a constant except the generic helper.

| shell id | send sites (rva) | sender entry (function or region) | flag | section |
|---|---|---|---|---|
| 0x33 | 0x48e37 | cshell+0x48cd0 (no caller found) | R | 5.3 |
| 0x4d | through 0x1a16fb | helper cshell+0x1a1680 called at cshell+0x2ec56 (region of `weapon` derived slot +0x08) | R | 8.1 |
| 0x59 | 0x629b5, 0x62a90 | cshell+0x61e40 (`local_player` derived slot +0x1c, per-frame update) | R | 7.1 |
| 0x60 | 0x601cd, 0x60314, 0x60583, 0x60905, 0x62563 | cshell+0x60010, 0x60270, 0x60490, 0x60680, 0x61e40 | R | 3.5 |
| 0x61 | 0x60455 | cshell+0x603e0 | R | 3.5 |
| 0x64 | 0x5f93b (R), 0x5ffa5 (U) | region cshell+0x5f7f0 (`local_player` derived slot +0x100, the live builder) | R / U | 2.1 |
| 0x64 | 0x1fa1a8 (R), 0x1fa5ef (U) | region cshell+0x1fa060 (base slot +0x100, replaced in the live object) | R / U | 2.1 |
| 0x68 | 0x73159 | cshell+0x72df0 (chat input) | R | 6.1 |
| 0x68 | 0xba0e9 | cshell+0xb9d80 (radio command, text part) | R | 6.2 |
| 0x69 | 0x29ceb2 | cshell+0x29ce20 (local world only) | R | 8.2 |
| 0x6e | 0x5f5dc | cshell+0x5f500 (no caller found) | R | 8.3 |
| 0x71 | through 0x1a16fb | helper cshell+0x1a1680 called at cshell+0x1f3794 (region) | R | 8.1 |
| 0x72 | 0x23d20a | cshell+0x23ccb0 (`conn_mgr` slot +0x08) | R | 5.1 |
| 0x75 | 0x1fa779 | region cshell+0x1fa660 (`local_player` slot +0x38) | R | 3.6 |
| 0x76 | 0x1ff18a, 0x1fff99, 0x200178 | cshell+0x1ff0b0, 0x1ffec0 (movement) | R | 2.2 |
| 0x76 | 0x236c4f, 0x23806c, 0x239da7, 0x23a520, 0x23a8aa, 0x23a025, 0x23a1aa | `weapon` methods cshell+0x2360c0, 0x237eb0, 0x239c90, 0x23a210, regions 0x239e40, 0x23a0d0 | R | 4.5 |
| 0x7f | 0x1f9050 | cshell+0x1f8fa0 | R | 4.4 |
| 0x7f | 0x1f8eef | cshell+0x1f8e50 (no caller found) | R | 4.4 |
| 0x80, 0x81 | 0x2366b5; 0x236812, 0x2369f2 | `weapon` base slot +0x08 cshell+0x2360c0 (animation keys) | R | 4.3 |
| 0x82 | 0x238e64 | `weapon` slot +0x174 cshell+0x238b10 | R | 4.1 |
| 0x83 | 0x239bdc | region cshell+0x2399e0 (`weapon` slot +0x2c) | R | 4.2 |
| 0x84 | 0xb78fe | cshell+0xb77e0 | R | 7.3 |
| 0x85 | 0x1ed0fb | cshell+0x1ed040 (`client_profile`) | R | 5.2 |
| 0x87 | 0x1a9e38 | cshell+0x1a9dc0 | R | 5.4 |
| 0x89 | 0x62111 | cshell+0x61e40 | R | 3.4 |
| 0x8b | 0x1f6b2f, 0x1f6c8f, 0x1f6def, 0x1f6f4f, 0x1f6782, 0x1f68b3, 0x1f69a3 | console handlers cshell+0x1f6a10, 0x1f6b70, 0x1f6cd0, 0x1f6e30, regions 0x1f6670, 0x1f6800, 0x1f68f0 | R | 8.4 |
| 0x8c | 0x29f1b | cshell+0x29e80 (`roster`) | R | 5.5 |
| 0x96 | 0x5b244 | cshell+0x5b1a0 (`mode_rules` derived slot +0x50) | R | 7.4 |
| 0x96 | 0x22fe94 | cshell+0x22fe10 (`mode_rules` base slot +0x50, replaced in the live object) | R | 7.4 |
| 0x98 | 0x22fdc5 | region cshell+0x22fcd0 (`mode_rules` slot +0x34) | R | 7.4 |
| 0x99 | through 0x1a16fb | helper cshell+0x1a1680 called at cshell+0x22f70c (in cshell+0x22f6e0) | R | 7.4 |
| 0xa7, 0xa8, 0xa9, 0xb0 | 0x1a922f, 0x1a937b; 0x1a90e2; 0x1a8d25, 0x1a8dfe; 0x1a8f10 | console handlers cshell+0x1a9150, 0x1a9290, 0x1a8f50, region 0x1a8c40, 0x1a8e60 | R | 8.5 |
| 0xac | 0x23c9f0, 0x23cbf1 | cshell+0x23c900 (`conn_mgr` slot +0x34) | R | 5.6 |
| 0xad | 0x275cc6 | cshell+0x275ad0 | R | 7.2 |
| 0xae | 0x27bb68 | cshell+0x27b9e2 (no caller found) | R | 8.3 |
| 0xb8 | 0xcf2be | cshell+0xcf230 | R | 7.5 |
| 0xba, 0xd1 | 0x4563e; 0x455ab | cshell+0x45050 (`hud_mgr` derived slot +0x38) | R | 7.5 |
| 0xbd | 0x484d2 | cshell+0x48450 | R | 5.7 |
| 0xbe | 0xb9fdf | cshell+0xb9d80 (radio command) | R | 6.2 |
| 0xc3 | 0x2afdf, 0x2b121 | cshell+0x2ae70 (`roster`, periodic) | R | 5.8 |
| 0xc3 | 0x3a619 | region cshell+0x3a4f0 (console handler) | R | 5.8 |
| 0xc5 | 0x62262 | cshell+0x61e40 | R | 3.4 |
| 0xca | 0x1ff8d2 | cshell+0x1ff7f0 (movement) | R | 2.3 |
| 0xcc | 0x2393da | region cshell+0x239310 (`weapon` slot +0xf4) | R | 4.6 |
| 0xcd | 0xb5d32, 0xb7ad7, 0xb7b92, 0xb7cad | cshell+0xb5c40, 0xb7a20 | R | 7.3 |
| 0xce | 0xd04a9 | cshell+0xd03b0 | R | 7.5 |
| 0xd2 | 0x623d8 | cshell+0x61e40 | R | 3.4 |
| 0xd5 | 0x23952a | cshell+0x239420 (`weapon`) | R | 4.6 |
| 0xd8 | 0x1f7a13, 0x1f7bad, 0x1f7cc4 | cshell+0x1f7890 (`local_player` base slot +0x1c, run by the derived update) | R | 7.1 |
| 0xd8 | 0xb03a0 | region cshell+0xb01c0 (slot +0x10 of method table VA 0x374d88b0) | R | 7.1 |
| 0xdb | 0xbad07 | cshell+0xbac00 (reply to a server 0xdb) | R | 7.6 |

49 distinct shell ids. The two directions are separate id spaces (`UDP_SESSION_2009_B.md` 7.1); several ids are
nevertheless the same number in both directions because the message is a request and its answer, or a notice and
its relay, and those are cross-referenced in section 9.

Messages a DeCombat game server must handle for a playable TDM round: 0x64, 0x72, 0x76, 0x7f, 0x80, 0x81, 0x82,
0x83, 0x85, 0x87, 0x89 (or 0xc5 / 0xd2 by game type), 0x68, 0xbd, 0x8c, 0x60 / 0x61 (observer camera), 0xc3 (only
when the server enabled it), **0x4d** (throw hold, 4.6; the release is the next 0x82) and **0xca** (sprint state,
2.3; the server must answer it with S2C 0xca). The rest may be logged and dropped; console and developer messages
(section 8) are ignored by default (proposal DC-C2S-1, section 12). PROVEN-CAPTURE (tdm1): a solo TDM session of
157 s sent exactly 0x4d, 0x64, 0x68, 0x72, 0x76, 0x7f, 0x80, 0x82, 0x83, 0x85, 0x87, 0x8c, 0xbd and 0xca; tdm2 and
tdm4 added no other id. Never sent in the three captures (negative evidence only, each consistent with its trigger):
0x33, 0x59, 0x60, 0x61, 0x69, 0x6e, 0x71, 0x75, 0x81, 0x84, 0x89, 0x8b, 0x96, 0x98, 0x99, 0xa7..0xa9, 0xac..0xae,
0xb0, 0xb8, 0xba, 0xbe, 0xc3, 0xc5, 0xcc, 0xcd, 0xce, 0xd1, 0xd2, 0xd5, 0xd8, 0xdb. Every reliable / unreliable
choice of 1.3 matches: everything reliable except the unreliable form of 0x64.

## 2. Player update stream and movement notices

### 2.1 `UP_PLAYER_UPDATE` (0x64): the body, view and input stream

Sender: `local_player` derived slot +0x100 = region cshell+0x5f7f0 (DISASM, no decompiler function; the method-table
entry is PE-DATA VA 0x374d337c). It runs once per client frame. The base-class slot +0x100 is region cshell+0x1fa060,
a shorter variant (see the end of this section); the shell's player object is the derived class
(`GAME_MESSAGES_2009.md` section 2), so the live message is the one below (PROVEN-CAPTURE that the base variant never
runs: all 5,285 unreliable updates of tdm1, tdm2 and tdm4 parse with this layout to their last bit, 2.1.1). This section re-derives and extends `UDP_SESSION_2009_B.md` 10.3 and `MOVEMENT_2009.md` 12.2
from the disassembly; where they differ, this section is the newer reading.

**Gate** (DISASM cshell+0x5f80b..0x5f9e2): nothing is sent unless the own player state (`local_player`+0x30) is 1.
The flag word `pu_flags` (`local_player`+0x148, u16) accumulates bits during the frame and is cleared after each
unreliable send.

**Reliable state form** (cshell+0x5f8b5..0x5f96f), sent first in the same frame when the controller state byte
`local_player`+0x80 differs from its last-sent copy +0x84 (a second controller state byte, not the `life_state`
+0x30 that the server's 0x67 sets; meaning UNPROVEN):

```
[pu_flags:u16 = 0x0010][state:u8]                                    R  (cshell+0x5f93b)
```

Bit 0x0010 is then cleared from `pu_flags`.

**Unreliable form** (U, cshell+0x5ffa5). Send rule: the player's mover has a body (`local_player`+0x10, field +8)
and, only when `conn_mgr` request kind is 3 (a remote server), at least `1 / R` seconds have passed since the last
unreliable send (time from `engine_client` slot +0x6c, last time at `local_player`+0x14c). `R` is the value of a
client variable (object VA 0x3759bf8c, key VA 0x3759bf88, method +0x5c) and **15.0** when it is missing
(PE-DATA VA 0x374ce220). For other request kinds (local worlds) every frame sends.

Flag sources (DISASM cshell+0x5f83b..0x5fc03; bits of `pu_flags`):

| bit | set when | field it announces |
|---|---|---|
| 0x0001 | `local_player`+0x19d is set | `eye_offset` |
| 0x0004 | the camera object's mode (`local_player`+0x130, +0x54) changed between "1 or 4" and "anything else" since the last frame | none |
| 0x0008 | together with 0x0004 when the new camera mode is 1 or 4 | none |
| 0x0010 | reliable state form above (never in the unreliable form) | `state` (reliable form only) |
| 0x0020 | a client variable (object VA 0x3759bf98, key VA 0x3759bf94) is greater than 0.0 and `local_player`+0x19c is clear | `yaw8c` + `body_angles` |
| 0x0040 | the same variable is greater than 0.0 and `local_player`+0x19c is set (0x0020 cleared) | `yaw16c` |
| 0x0080 | no writer found that sets it in this builder (the view block below still tests it) | `view_q` |
| 0x0100 | `local_player`+0x455 is set and the rotation built from `local_player`+0x48..+0x50 differs from the last one sent (kept at VA 0x37560e2c) | `aim_q` |
| 0x0200 | the current weapon's slot +0x44 float differs from the last one sent (VA 0x37560e1c), while the state is 1 | `weapon_f` |
| 0x0400 | `local_player`+0x455 is set and the camera object position (camera +0x18) differs from the last one sent (VA 0x37560e20) | `cam_point` |
| 0x0800 | `local_player`+0xf0 > 0 or `local_player`+0xf4 != 0 | none |
| 0x1000 | `local_player`+0x1c4 is set | none |
| 0x2000 | the current weapon's slot +0x134 returns true | none |

When the variable of 0x0020 / 0x0040 is missing, the default 0.0 (PE-DATA VA 0x374cda90) is not greater than 0.0, so
no view block is written (PROVEN from the comparison; PLAUSIBLE that the variable normally exists and is positive).

Layout (field order PROVEN, DISASM cshell+0x5fc0a..0x5ff80):

```
[pu_flags:u16]
view block, the first that applies:
  pu_flags & 0x0020 : [yaw8c:u8][body_angles:vec96]       angles = local_player+0x68, +0x6c, +0x70
  pu_flags & 0x0040 : [yaw16c:u16]
  pu_flags & 0x0080 : [view_q:quat128]                    rotation built from local_player+0x68..+0x70
if pu_flags & 0x0060 : [aux: 6 x u32]                     local_player+0x40c .. +0x420 (two 3-float vectors)
if pu_flags & 0x0001 : [eye_offset:vec96]                 point A minus the body position (below)
if pu_flags & 0x0100 : [aim_q:quat128]                    rotation built from local_player+0x48, +0x4c, +0x50
if pu_flags & 0x0200 : [weapon_f:f32]
if pu_flags & 0x0400 : [cam_point:vec96]
[control:u32]                                             move_mgr+0x24, the frame's control word
[movement trailer]                                        cshell+0x1fed20, MOVEMENT_2009.md 12.2
```

- `yaw8c = trunc(wrap(yaw(q)) * 40.425354)`, toward zero, with the yaw wrapped into (-pi, pi] first, and `yaw16c` with the
  scale 5215.03 (not sent in any capture; presumably the same truncation, UNPROVEN), of the rotation `q` built from the
  body angles (cshell+0x1b700, then cshell+0x19f390 / cshell+0x19f3e0), as in `UDP_SESSION_2009_B.md` 10.3. The earlier
  reading `round(...)` was wrong: PROVEN-CAPTURE, 2,338 of 2,338 (tdm1), 1,361 + 483 (tdm2) and 1,103 of 1,103 (tdm4) updates
  match truncation; rounding matches only 957 of 2,338, 1,347 of 1,361, 2 of 483 and 527 of 1,103.
  `quat128` is writer slot +0x4c (`GAME_MESSAGES_2009.md` 1.4), which closes the "encoding UNPROVEN" note of
  `UDP_SESSION_2009_B.md` 10.3 for both rotations.
- The angle triples follow `MOVEMENT_2009.md` 12.4 / 12.5: +0x48..+0x50 are the controller's pitch, yaw, roll (the
  values the server's 0x6a sets), +0x68..+0x70 the body angles copied from them (PLAUSIBLE naming).
- `eye_offset`: point A is the position of the object at `local_player`+0x12c when cshell+0x1f0820 is true and
  `local_player`+0x8c is 0 (engine client interface slot +0xa0, object position), otherwise a model attachment point
  found through the model interface (global VA 0x375964e4, two lookups by name, keys at VA 0x374ce520 and VA
  0x374ce514); the body position is that of the object `move_mgr`+0x08. **PROVEN-CAPTURE: the first-person eye point
  relative to the body position, (0, 33, 0) standing and (0, 20, 0) crouched (about 0.2 s transition through 28.95 and
  23.92), with the box half extents (15, 50, 15) in force; `cam_point` is the absolute camera position (body + eye offset
  in first person).**
- `control` is the control word that `MOVEMENT_2009.md` 5.1 documents at mover+0x24 (`move_mgr` VA 0x3759c220):
  FWD 0x1, BACK 0x2, JUMP 0x10, CROUCH 0x20, RUN 0x200, **FIRE 0x400 (the primary-fire command held; not an aim bit,
  see 2.1.1)**, SPRINT 0x400000 and the rest of that table. This identifies the `state:u32` of `UDP_SESSION_2009_B.md` 10.3 (PROVEN: same global and offset).
- Movement trailer (`MOVEMENT_2009.md` 12.2, PROVEN + DISASM there): `[move_seq:u8][pos:vec96][vel:vec96]
  [time:f32][on_ground:u8][surface:u8][standing:u64]`, written on `local_player`+0x10 (the mover).

After the send: `local_player`+0x14c := the time, `pu_flags` := 0.

**Base variant** (region cshell+0x1fa060, base slot +0x100): same gate, same reliable form; the unreliable form
writes the view block 0x0020 as `[yaw8c:u8]` only, then `[u8]` (an angle compressed by cshell+0x19ec60) when
`pu_flags & 0x0060`, then for 0x0001 three u16 (a vector compressed with scale 100, cshell+0x19f180), then
`[control:u32]` masked to bit 0x20 unless a caller flag is set, then the same trailer (as in `UDP_SESSION_2009_B.md`
10.3). A server should accept both shapes only if it can tell them apart; it cannot from the bits alone (the flag
word has the same meaning), so DeCombat parses the derived (live) shape only (proposal DC-C2S-2).

**Server answer:** none per message. The server uses the stream to move the player's server object, validates it
(DeCombat policy, `MOVEMENT_2009.md` 12.4) and corrects only with the mover update 0xa0 (sub-flag 0x0020 teleport,
new `move_seq`). Updates whose `move_seq` is not the latest are ignored. Other clients see the player through the
engine object stream (`UDP_SESSION_2009_B.md` sections 3-4), not through a game message.

#### 2.1.1 Capture results for 0x64 (PROVEN-CAPTURE; tdm1 2,338 updates, tdm2 1,361 + 483, tdm4 1,103)

- **Gate, state form, rate.** The first 0x64 of any form follows the server's 0x67 `[1]`; the reliable state form `0b 64 10 00 01` (state byte 1) is sent once, right before 0x87 (tdm2 re-join: `[0x0010][0]`); nothing is
  sent after 0xbd. The unreliable form goes out at 15 per second (14.86-14.87 per second; gaps of 65-70 ms on the trailer clock, 2,335 of 2,337) **whether the player moves or stands** (mean gap 67.1 ms moving, 67.5 standing); the
  longest gap was one client stall of 438 ms. Rule: "0x64 at 15 per second (1/15 s, quantised to frames), moving or standing" (closes V-C6 for the rate).
- **Layout (closes U11).** Every unreliable update parses with the layout above to **exactly** its last bit (sizes 768, 800, 864, 896, 928, 992, 1,024 bits); DC-C2S-2 (derived layout only) is right. Example head
  `0b 64 21 15 db 00 00 00 00 00 c0 67 ab 40 ...` (`pu_flags` 0x1521, `yaw8c` 0xdb = -37).
- **Flags (counts of the 2,338 of tdm1).** 0x0001, 0x0020 and 0x1000 in every update; 0x0100 in 1,472 (when the view rotation changed); 0x0400 in 1,302 (when the camera point moved); 0x0200 in 491 (weapon float changed); 0x2000 in
  95 (during repeat fire: 18 runs inside the automatic bursts of one weapon and on the grenade frame; in tdm4 also during the grenade hold); **never** 0x0004, 0x0008, 0x0040, 0x0080, 0x0800 (also the 0x82 `moving` bit stays 0
  in all 83 + 5 + 11 shots, so those two sources are zero while walking, running, sprinting and firing). 0x0010 only in the reliable form. Closes U10 except 0x0004 / 0x0008 (need a camera-mode change).
- **View block.** `body_angles` = `(0, body_yaw, 0)` in every update (pitch and roll always 0.0); the body yaw is **not wrapped** (range -2.85 .. 10.45 rad; tdm4: 43.96 rad of turning). `aux[0]` = view pitch in radians
  (positive = looking down; -1.277 .. 1.080), `aux[1]` = view yaw minus body yaw (exactly within +-pi/4: the body follows the view with a dead zone of pi/4), `aux[2..5]` = 0. In all 1,472 updates of tdm1 that carry `aim_q` (in tdm4 the aim yaw equals the body yaw within 0.005 rad in 935 of 1,103 updates and the body lags by up to pi/4 otherwise):
  **`aim_q = Ry(body_yaw + aux[1]) * Rx(aux[0])`** with `Ry(a) = (0, sin(a/2), 0, cos(a/2))`, `Rx(a) = (sin(a/2), 0, 0, cos(a/2))` in the `quat128` order x, y, z, w (largest squared error 4.2e-10; the other composition order fails);
  the forward axis is the rotated +Z. A server that needs the exact view direction reads `aim_q` (flag 0x0100, sent whenever the view changed).
- **Control word** (`ANY_MOVE` equals `control & 0x19f != 0` in 2,338 of 2,338): observed values 0x200, 0xa01, 0x400a01, 0xb01, 0x600, 0x0, 0xa81, 0x801, 0xa80, 0x400a81, 0x821, 0xb00, 0x400b01, 0x20, 0xa02, 0xe01, 0xa82,
  0x881, 0x80200, 0xb02, 0xf01. SPRINT appears only with FWD and RUN, CROUCH only without RUN. **Bit 0x400 is set exactly while the fire key is held**: 39 runs (tdm1) coincide with the 0x82 bursts, and in tdm4 it appears
  only in the five updates around four rifle shots and in the 2.2 s grenade hold, never together with SPRINT (the fire bit is suppressed while sprinting); `pu_flags` 0x2000 is set in the same frames. There is no fire bit in
  `pu_flags` and no extra 0x64 per shot: shots travel only as 0x82. **JUMP 0x10 never appears** in any sent word (the bit lives on the take-off frame only; 27 + 26 jumps were made): a server takes jumps from 0x76 [1][1], not
  from the control word. CROUCH 0x20 appeared with the eye drop to +20 in 52 of 52 crouched updates; no crouch was attempted in tdm4.
- **Weapon float** (0x0200): the weapon's current spread; `frac / 255` of the next 0x82 follows it (0.047, 0.094, 0.145 ... rising during a burst, decaying to 0 afterwards).
  The law that produces it is not read (4.1.1, spread note). A second reader (`dc-client`, client-weapons; D, CLI-20' / CLI-33 (proposed); TESTED
  `net::update_while_firing_has_the_2009_flag_and_control_words`: the 928-bit firing form, head equal to the observed one up to `aux[0]`) sends `weapon_f` only when it changed
  since the last update that carried it, and control 0x400 with `pu_flags` 0x2000 while the fire button is held and for 67 ms after a shot (one update interval), never while
  sprinting.
- **Trailer.** `move_seq` is the sequence byte of the latest 0xa0 teleport (1 in the first two updates, 2 afterwards: tdm1; echoed 31-63 ms after the second teleport on both links of tdm2); the first update's position is the
  teleport position raised by 0.12; positions agree with the mean velocity times dt (median 0.10 units); `time` is the client's **own clock** (138.197 s at capture time 139.953 s while the server's stream time was about 0:
  a server must use differences only; the 0x82 `time_ms` uses the same clock); `on_ground` 1 / 0; `surface` 71, 10, 20, 23 on the ground, 0 in the air, 200 on a ladder (69 ladder updates climb at vertical speed 201-208);
  `standing` = -1 / -1 while airborne, otherwise a constant high word (0x5d in tdm1, a world-model index PLAUSIBLY) and a low word that varies with the floor polygon.
- **Runaway yaw (tdm2, one session).** A single absurd relative mouse delta of about 2^31 units (99.6 % of 2^31, at the first mouse movement after the spawn, full-monitor windowed mode at 2560x1440 with the high-DPI layer)
  drove the unwrapped body yaw to 2,426,654 rad: the f32 spacing there is 0.25 rad, so a per-frame delta below 0.125 rad rounds to no change and the user could only look up and down (34 `yaw8c` steps of 10-11 units in 90 s).
  A re-join in the same process and tdm1 / tdm4 at the default resolution were normal. The wire shows the effect, not the cause (client input path, UNPROVEN). A server may repair the client with S2C 0x6a kind 0 carrying the
  yaw folded into (-pi, pi] (proposed CV2-5: when |yaw| exceeds 1000 rad, at most once per 2 s per player) and must fold the yaw (in f64) before it stores it or derives a `yaw8` from it.

### 2.2 `UP_MOVE_EVENT` (0x76 sub 1): jump, fall and landing notices

Senders: cshell+0x1ff0b0 and cshell+0x1ffec0 (mover methods). Layout, all R:

```
[category:u8 = 1][event:u8]
```

| event | sender | when (PROVEN control flow; meaning PLAUSIBLE) |
|---|---|---|
| 1 or 3 | cshell+0x1ff0b0 (called from cshell+0x1ffec0 and cshell+0x2001c0) | `event = 1` when the mover's jump-airborne flag (+0x80) is set, else 3 (cshell+0x1ff10b: `3 - 2 * flag`); the jump case also plays one of two jump sounds unless the surface type is 0xca (`MOVEMENT_2009.md` 12.3) |
| 3 | cshell+0x1ffec0 (DISASM 0x1fff69..0x1fff76) | landed |
| 2 | cshell+0x1ffec0 (DISASM 0x200148..0x200155) | started falling without a jump |

The same id 0x76 is used by the weapon notices of section 4.5 (category 3) and the deflect notice (category 5); the
first byte after the id selects the category. **Observed (PROVEN-CAPTURE; tdm1 618 notices, tdm4 61):** 1 = jump
take-off (27 + 26: every one is a take-off, last update before it grounded, next airborne with vertical speed 244..320); 2 = the body
lost the ground without a jump (282 + 3; frequent on stairs and edges, where the 2 / 3 pair repeats more than 100 times within one
stair run); 3 = ground contact regained (309 + 29). **After a jump the 3 normally comes one frame after the 1** (26 of 27 in tdm1: next
reliable block, same 16 ms tick, before the first airborne 0x64) because the ground test still finds the floor; the real landing then
sends nothing. So event 3 is no landing marker; a server that wants landing events (fall damage, landing sounds for others) takes them
from the 0x64 trailer (`on_ground` 0 -> 1 and the vertical speed of the last airborne update). **Server answer:** none required. The
reference server relays each jump 0x76 [1][1] as S2C 8.2 type 0x19 `k` 0x0f to everyone (no relay of 2 and 3); the 2009 server's own
form is unknown.

### 2.3 `UP_SPRINT_STATE` (0xca)

Sender: cshell+0x1ff7f0 (mover), called from the input sampler cshell+0x1ff980 (six sites) and cshell+0x2013f0.
Sent only when the pair changes (kept as `2 * a + b` at mover+0x46c). R.

```
[sprinting:bool1][stopped:bool1]
```

- `(1, 0)`: sprint started (command 0x77 on, RUN set, stamina above 0, not on a ladder; cshell+0x1ff980 line
  with `match_stats` slot +0xe0, see `MOVEMENT_2009.md` 5.5).
- `(0, 1)`: sprint cancelled (forward command off, backward command on, crouch camera offset non-zero, or a
  non-zero value from cshell+0x6f860), and when the sprint flag is cleared at frame start (cshell+0x2013f0).
- `(0, x)`: sprint ended by the stamina path (cshell+0x1ffcc1 area; `x` from the stamina test).

Meaning of `stopped`: PLAUSIBLE. **Server answer: S2C 0xca `[allowed:bool1]`, the sprint permission, to the sender, reliable, for
every message whose `sprinting` bit changed** (PROVEN-CAPTURE by the client's reaction, tdm1 and tdm2: the reference stack answered 15-79 ms after
each change (tdm1: 56 messages, 48 echoes; tdm2: 34 messages, 32 echoes); without the echo `[1]` the client's sprint test of `MOVEMENT_2009.md` 7 fails and it runs at run speed
although it reports SPRINT; the first update after the echo is 60-100 units faster and the plateau is about 1.39 x run). Observed
transitions (tdm1): (0, 1) -> (1, 0) x 24, (1, 0) -> (0, 1) x 17, (1, 0) -> (0, 0) x 7 (stamina end), (0, 0) -> (0, 1) x 7; the first (0, 1)
comes right after the spawn; the `sprinting` bit of the last 0xca equals the SPRINT bit of every following 0x64 (2,337 of 2,337). When to
refuse the permission (stamina, rules) is DeCombat policy (proposal CV2-4: always echo; hook `movement.allow_sprint`).

## 3. Spawn, team, respawn and observer camera

### 3.1 No spawn request at match or round start (PROVEN in the negative)

The 2009 client sends **no** message that asks for a spawn, a team or a class when a match or round starts: none of
the 85 send sites is reached from the loading, team or round code except the ones listed in this section and in
section 5. The team comes from the lobby (`session_info`+0xf8, set by the lobby room messages and by the server's
team swap 0x5e, `GAME_MESSAGES_2009_S2C.md` 2.3); the client reports it once in `UP_PLAYER_INFO` (0x72, section
5.1). Placement and life state are imposed by the server: player state through server 0x67 (`local_player`+0x30;
1 = playing, 2 / 3 / 5 / 6 = the not-playing states, `MOVEMENT_2009.md` 12.5), position and box through the mover
update 0xa0 sub-flag 0x0020, facing through 0x6a (`MOVEMENT_2009.md` 12.4 / 12.5). The client only reports that it
finished loading (`UP_WORLD_READY` 0x87, section 5.4).

### 3.2 Own player state as the client tests it

`local_player`+0x30 (our name `life_state`) drives every sender of this section:

| value | client behaviour that depends on it (PROVEN) | meaning (PLAUSIBLE) |
|---|---|---|
| 1 | the player update 0x64 is sent only in this state (2.1); the respawn and observer logic below is off | alive, playing |
| 2, 3, 5, 6 | jump and crouch blocked (`MOVEMENT_2009.md` 5); the respawn wait (3.3) can run; the observer camera target logic (3.5) runs; 3 also zeroes movement | dead / waiting / observing (6 enters the observer camera, cshell+0x61e40) |

### 3.3 The client-side respawn wait

The client keeps its own respawn wait of **7000 ms** (constant in cshell+0x5df50 and cshell+0x5dfe0, PROVEN):
`local_player`+0x44c := `GetTickCount()`, +0x450 := 7000, +0x454 := 1 (armed), and `hud_mode_notice` (global
VA 0x37560ce8) starts a 7.0 s countdown (+0x30 := 7.0, +0x2c := now, +0x38 := 1, +0x3c := 1). Arming happens only
while `life_state` is 2, 3, 5 or 6, from **three** places; the first is the normal path and was missed by the first draft of this
section (DISASM pass of 2026-10-25, tdm4; the routine lies in a gap of the decompiler function table):

- **By the client itself after a death** (cshell+0x60ac0, `local_player` derived slot +0x70, reference only in PE-DATA VA 0x374d32ec; the third
  arming site of a scan of the whole `.text` for stores to `local_player`+0x450 / +0x454: cshell+0x60b94..0x60baa for 7000 ms and
  cshell+0x60bd6..0x60bec for 1500 ms). The per-frame update cshell+0x61e40 (DISASM cshell+0x62467..0x62494) calls it every frame while
  `life_state` is 2, 3, 5 or 6 **and the death time `local_player`+0x440 is non-zero**; that time is written by server 0x67 state 2
  (derived handler, jump table VA 0x37161b38, DISASM cshell+0x619d5..0x619de), cleared by state 1 (cshell+0x618e0) and not touched by
  state 3 (cshell+0x61975). The routine switches on the observer camera mode `local_player`+0x428 (jump table VA 0x37160d70: mode 0 ->
  cshell+0x60adc, 2 -> cshell+0x60ce0, 3 -> cshell+0x60cf6, 4 -> cshell+0x60c0f, 1 -> no action). In **mode 0** (the own view, which the
  player has when he dies; 0x67 state 1 sets it through cshell+0x60680(0)) with `life_state` 6 it only enters the free camera; otherwise it
  binds the own roster record, selects the killer camera (mode 4) when `roster`+0x10 (the last killer id, written by 0x6d) is not negative,
  else the follow camera (mode 2), and arms by `mode_class` (`client_shell`+0x16938; DISASM cshell+0x60b2e..0x60c0e):

  | `mode_class` | effect |
  |---|---|
  | 1, 4, 6, 7, 8 | arm 7000 ms |
  | 5 | arm 7000 ms only when the own roster record +0x1f8 is above 0 (the value that 0xd7 sub 2 stores) |
  | 9 | arm 7000 ms only when `mode_rules`+0xb4 is 0 |
  | 10 | arm 1500 ms |
  | 2, 3, 11, 12, 13 and others | nothing (no respawn request is ever sent) |

  Arming also starts the mode-notice countdown (cshell+0x5c860 on `hud_mode_notice` with 7). After the first call the camera mode is 4 or
  2, so the wait is armed **once per death**. Grade: DISASM for the client rule (a gap of the decompiler table), consistent with the bot of
  botduel3 / botduel4, which armed on 0x67 state 2 and asked 7001-7016 ms later in 37 of 37 deaths; a capture with deaths of the
  real client now exists: tdm6 (five deaths, one `0b 89` each 7.015-7.031 s after the datagram with 0x67 state 2, no arming message from the server; S2C 4.1.1), so the
  timing is PROVEN-CAPTURE (folded by spec-fold-8 from capval-tdm6 4.2; the earlier wording "no capture contains a death of the real client (tdm1, tdm2, tdm4 survived), so
  the timing is not yet PROVEN-CAPTURE" is superseded).
- The two older sites, each of which only **re-arms** (7000 ms):

- cshell+0x5df50, called by the roster handler for server message **0xd7 sub-id 2** (`GS_COOP`, S2C §3.5, a
  co-op mode message; cshell+0x2d3a9..0x2d3e0,
  DISASM: `[2][a:u8][b:u8]`; the client stores `b` in its own roster record +0x1f8 and arms the wait when `a`
  equals that stored value, i.e. `a == b`). Layout PROVEN; meaning of `a` / `b` UNPROVEN.
- cshell+0x5dfe0, called from cshell+0xbac00 (the reply path of server message 0xdb, section 7.6), unconditionally.

A server therefore needs **no** arming message in the respawn classes: about 7000 ms (class 10: 1500 ms) after the client
processed `GS_PLAYER_STATE` 2 it sends its respawn request (3.4) on its own, and in classes 2, 3, 11, 12 and 13 no request is ever sent.
(The first draft said the wait was armed only by 0xd7 sub 2 and 0xdb sub 6; that was wrong.) The server's reaction to the request is
the respawn set (Appendix A, A11); the reference stack respawns only on request (S2C 4.1.1). Not read: whether a round-end message (0x46,
0x48 or 0x3f) disarms a running wait; a client whose wait expires after 0x46 may still ask (our client did, `client-reference stack.md` L11; the reference stack
ignored it): UNPROVEN, so a server must tolerate a request after the round end (DSV-32 ignores it).

What our test client does instead (D, BOT-18 proposed; `dc-botclient` `lifecycle.rs` `Respawn::on_round_over`, merged 96983bf): a bot that is dead when 0x46 arrives disarms its armed
wait, sends **at most one** more request after 0x46 (its `--respawn-after` fallback) and none afterwards until its next 0x67 state 1 (spawn or next round); a bot that is alive at
0x46 sends nothing. This is a rule for our bot, chosen to keep the server log quiet, **not** a reading of the 2009 client, whose behaviour after 0x46 stays UNPROVEN (a death of the
real client near a round end is part of capture item V-KILL-1). RUN: before the rule a bot dead at a goal end re-sent 0x89 about every 16 ms on the result screen (stack-verify round 2:
7,568 requests, each logged by `dc-dserver` as ignored, "the round is over"); with it, exactly one request about 7 s after 0x46, then none (bot-roundend round 1). TESTED: three
`lifecycle.rs` unit tests.

### 3.4 `UP_RESPAWN_REQUEST` (0x89), `UP_RESPAWN_REQUEST_B` (0xc5), `UP_RESPAWN_REQUEST_C` (0xd2)

Sender: the per-frame update of `local_player`, cshell+0x61e40 (derived slot +0x1c; DISASM 0x620ce..0x623d8).
Condition (all PROVEN): `life_state` is 2, 3, 5 or 6, the wait is armed (+0x454), and more than +0x450 ms (7000)
have passed since +0x44c. Then the client clears the countdown (`hud_mode_notice` +0x3c := 0, +0x10 := 0), shows a
7000 ms notice (cshell+0xbb670(0, -1, 0, 7000)), sends **one** of the messages below by `mode_class`, disarms the
wait (+0x454 := 0) and, if an observer camera mode is active (`local_player`+0x428 != 0), resets the followed record to the
own roster record (cshell+0x2aca0) and the engine camera to the own view, and sets the mode to 0.

| mode_class | message | layout | flag |
|---|---|---|---|
| 1, 4, 5, 6, 9 | `UP_RESPAWN_REQUEST` 0x89 | (no fields) | R |
| 7, 8 | `UP_RESPAWN_REQUEST_B` 0xc5 | (no fields) | R |
| 10 | `UP_RESPAWN_REQUEST_C` 0xd2 | `[zero:u8 = 0]`; sent only when cshell+0x293d0 (a count over the roster for the own team, PLAUSIBLE "team members alive") is above 0 | R |

For the TDM row of the EU-test ROM the client takes the 0x89 branch (OBSERVED by reading the user's ROM read-only:
the row's game-type value is in the first group; the value itself is read from the ROM at run time and not
reproduced here). Meaning PLAUSIBLE: "my respawn wait is over, place me". **Server answer:** a new life: player
state 1 (server 0x67), placement (0xa0 with sub-flag 0x0020 and a new `move_seq`), facing (0x6a), the loadout and
ammo state (`GAME_MESSAGES_2009_S2C.md` section 4), in that order on the reliable channel. The request is the **trigger** of the
respawn: the reference stack answered 37 of 37 requests 30-80 ms later with the spawn set (S2C 4.1.1) and never respawned a player unasked
(a dead bot that did not ask stayed dead for 104 s); a request after the round end is ignored. A request that arrives before the
server's respawn delay has passed (the client waits strictly more than 7000 ms after it processed 0x67 state 2, so a request is due at
about `respawn_ms - 500` after the death on the server's clock) is ignored silently, and a request from a living player is ignored
(proposals DC-KILL-1 and DC-C2S-3: answer a due request at once, respawn on a fallback timer only for clients that never ask; both are now
refined by DSV-32 below, and DC-C2S-3's "before the server's respawn delay" is superseded because there is no server delay in these classes).
The bot timing 7001-7016 ms after arming and 7047-7112 ms from death to state 1 is the bot's, not the real client's. The real client's own timing (tdm6,
PROVEN-CAPTURE; spec-fold-8): one request 7.015-7.031 s after the datagram with 0x67 state 2 in five of five deaths, answered by the reference stack 16-63 ms later; a server timer of
exactly 7000 ms would race it.

DeCombat server (D, DSV-32 proposed; `dc-dserver` slice 4, TESTED in `game_sim`, RUN 7 of 7 deaths): in classes 1, 4, 6, 7, 8 (wait 7000 ms) and 10 (1500 ms)
the server keeps **no respawn timer**; a request from a dead player that arrives no earlier than the class wait minus 250 ms after the death (server clock) is
answered at once with the spawn set; an earlier request, a request from a living player and a request after the round end are logged and ignored (no reply).
Classes 5 and 9, where the client asks only under a client state, also respawn 10 s after the death on the server's timer; round classes 2, 3, 11, 12, 13 have no
respawn inside a round. RUN: answered 7048-7059 ms after the death on the bot's self-armed 0x89.

Bot side (D; `dc-botclient` on master 96983bf): **one request per death.** Once a bot's self-armed request (3.3) went out for a death, its `--respawn-after` fallback no longer fires
for that death (bot-respawn, B-1); the fallback still serves deaths without an armed request (an unknown class, class 5 with the 0xd7 value 0). RUN (bot-respawn round 1): 6 deaths,
6 requests answered 7047-7087 ms after the death, 0 ignored (before: two extra requests per death, logged as "not dead"); stack-verify (master 692de66) had answered every respawn
7049-7059 ms after the death. After 0x46 the bot follows BOT-18 (3.3).

### 3.5 Observer camera: `UP_OBSERVE_TARGET` (0x60) and `UP_OBSERVE_FREE` (0x61)

The not-playing states use an observer camera with modes (`local_player`+0x428, set by cshell+0x60680): 0 own view,
1 free camera, 2 follow a player, 3 a player's first-person view (weapons drawn), 4 a further mode (UNPROVEN). The
followed player is a roster record kept at `local_player`+0x434; its byte +0x04 is the player's id (the engine
client id, `GAME_MESSAGES_2009_S2C.md` 1.1).

`UP_OBSERVE_TARGET` (0x60), R:

```
[target_id:u8]               roster record +0x04 of the followed player
```

Senders and triggers (PROVEN control flow):

| sender | trigger |
|---|---|
| cshell+0x60010 | pick the next player to follow (cshell+0x29090 searches the roster with a team filter; with the "enemy dead camera" option, profile key at VA 0x374d3540, 94396d73e3a2a1fd/12 read through cshell+0x19fde0, the opposite team is allowed too); sent only when the camera is in mode 5 (`observer_camera()`) |
| cshell+0x60270 | re-send the current target after a camera switch (`observer_camera()` true) |
| cshell+0x60490 | the observer pressed a player-select key (key codes 0x31..0x38 select team 1, 0x70..0x77 team 2, through cshell+0xa8bf0; called from the key handler cshell+0x49670) |
| cshell+0x60680 mode 3 | first-person view of a target when the previous mode was not 3 |
| cshell+0x61e40 | per frame in a not-playing state when `local_player`+0x440 is set; when `local_player`+0x460 is set and `roster`+0x10 is not negative it sends `roster`+0x10, the id of the player who last killed the local player (S2C §3.1; PLAUSIBLY a killer camera), else the followed record's id |

`UP_OBSERVE_FREE` (0x61), R, no fields: sent by cshell+0x603e0 when the observer camera enters free mode (cshell+0x60a20
with mode argument 1 or 3, and on entering `life_state` 6 with no camera mode yet, cshell+0x61e40).

Meanings PLAUSIBLE. **Server answer:** the server may start sending the followed player's view to this client (the
spectator HUD elements `hud_spectator_ammo` / `hud_spectator_health` exist, `GAME_MESSAGES_2009_S2C.md` 1.2; which
server message feeds them is in that catalogue) and stops on 0x61. A minimal server may ignore both.

### 3.6 Client-side inactivity kick (no message)

For reference: the per-frame update (cshell+0x61e40) compares `GetTickCount()` with the last input time
(`client_shell` slot +0xbc). Once more than 288,999 ms have passed without input it computes the remaining
`(300000 - elapsed) / 1000` seconds, shows them in a notice (CRes string id 0x140e) while fewer than 11 remain, and
at zero leaves the match by itself (the same local path as `UP_LEAVE_MATCH`, section 5.7, but **without** sending
0xbd; the engine then disconnects with `C2S_BYE`). A server sees only the disconnect. PROVEN (constants 0x468e7,
300000 and 0xb in cshell+0x61e40). DeCombat needs no server-side idle rule for the 2009 client.

## 4. Weapons: fire, hit report, reload, weapon change, weapon events

Hit detection is **client-side** in the 2009 client: the fire message carries, for every pellet or bullet, the
trace result the client computed (hit object, hit point, ray). There is no separate hit-report message. The server
decides damage from these reports (DeCombat policy) and answers with its own damage and kill messages.

### 4.1 `UP_FIRE` (0x82): one message per pellet

Sender: `weapon` slot +0x174 = cshell+0x238b10 (PROVEN; method-table entries PE-DATA VA 0x374ceda0 derived and
VA 0x374f1a88 base), `ret 0x20` = eight arguments. Caller: the weapon's fire routine, slot +0x160 = cshell+0x23bca0,
loops over the pellets of one shot and calls slot +0x174 once per pellet (DISASM cshell+0x23c0a5..0x23c119; the
last argument is 1 for the last pellet of the shot, else 0). The fire routine runs from the weapon's update
(slot +0x1c = cshell+0x237180, DISASM call at cshell+0x23752e) after the fire key of the fire animation has passed:
the animation key handler (cshell+0x2360c0, section 4.3) calls slot +0x180 = cshell+0x237130, which only sets the
weapon's fire-pending byte +0xe0 when the playing animation is the weapon's own and the weapon state (slot +0x98) is
3 (firing). R.

Arguments (our names): `a2` a float in 0..1, `a3` the ray origin, `a4` a 128-bit value, `a5` the ray direction,
`a6` the hit point, `a7` the hit object (or none), `a8` last-pellet flag; `a1` is not read.

**Cadence = the fire clip** (spec-fold-7, from the client-weapons report; PLAUSIBLE for the 2009 client, RUN-consistent for ours): since the shot is released by the fire key
of the weapon's own fire animation (above and 4.3), the time between shots of an automatic weapon is the length of the first-person model's fire clip (the composed clip list:
the model's own clips, then its child model's). For weapon 145 that clip is 105 ms, the 104-110 ms that tdm1 measured (ROM-gated test
`arms::tests::rom_clips_match_the_observed_timing`); the row key `fireDelay` is 0 in 268 of 297 rows and absent in the rest, so no bute key holds the rate. `dc-client` loops the
fire clip while the button is held (each pass from where the last ended) and fires one shot per press when the row's `RapidFireCount` is 1 or more (CLI-30 (proposed); the
2009 rule is not read). The pistol's prefire clip (234 ms) is not played per shot (the observed 160-200 ms spacing rules that out); when the 2009 client plays its prefire /
postfire clips is not read (UNPROVEN).

Layout (DISASM cshell+0x238b7c..0x238e41, order and widths PROVEN):

```
[wkind:u8]                 weapon slot +0x178; returns 0 in both weapon method tables (cshell+0x2f5b0)
[weapon_id:u16]            weapon+0x98
[ammo_id:u16]              weapon+0x9c
[weapon_pos:vec96]         weapon+0xa8
[origin:vec96]             *a3
[aim:raw128]               *a4 (four floats; PLAUSIBLY the shot rotation as a quaternion)
[yaw:f32]                  local_player+0x4c (0.0 when there is no player object)
[dir:vec96]                *a5
[seed:u8]                  rand() in 2..255 (cshell+0x19eb40(2, 255))
[moving:bool1]             local_player+0xf0 > 0 or local_player+0xf4 != 0 (the same test as pu_flags 0x0800)
[wflag:bool1]              current weapon (weapon list local_player+0x18, cshell+0x1f06c0) slot +0xf8; 0 if none
[mode1:bool1]              weapon+0xd4 == 1
[frac:u8]                  truncate(a2 * 255.0); a2 = the weapon's current spread (the float of 0x64 `weapon_f`)
[shot_scale:f32]           weapon+0x20c (formerly `shot_no:u32`: a float, 1.0 or 1.2, constant per weapon; 4.1.1)
[time_ms:u32]              truncate(client time * 1000.0) (engine_client slot +0x6c)
[has_extra:bool1]          weapon+0x1cd
if has_extra:
   [extra_obj:objref16]    weapon+0x1c8
   [extra_kind:u8]         weapon+0x1d0
   [extra_a:vec96]         weapon+0x1d8
   [extra_b:vec96]         weapon+0x1e4
[flag_c:bool1]             weapon+0x1cc
[last:bool1]               a8
[has_hit:bool1]            a7 != none
if has_hit:
   [hit_dist:f32]          dot(a6 - a3, a5): the hit point's distance along the ray
   [hit_obj:objref16]      a7
[stat:u32]                 local_player+0x1fc
```

Meanings after the captures (4.1.1; PROVEN-CAPTURE): `weapon_id`, `ammo_id` the ids of the fired weapon and its ammunition row;
`origin` = the shooter's 0x64 position + `eye_offset` (median distance 0.0); `weapon_pos` about 10.9 units from `origin`; `aim` a unit
quaternion = the view rotation (equal to the 0x64 `aim_q` of the same frame; `dir` adds the spread, within 0.009 median / 0.042 max of its
forward axis); `yaw` = the controller yaw, unwrapped (it equals the yaw of `aim`, not the body yaw, which lags up to pi/4);
`frac / 255` = the weapon's spread; `time_ms` = the 0x64 trailer clock x 1000 (the client's own clock); `last` 1 on every message of a
one-vector weapon; `shot_scale` a float per weapon, **equal to the weapon row's `DynamicPerturbFactor`** (1.0 when the row has none) for all three weapons the
captures fired (145 -> 1.0, 7 -> 1.0, 164 -> 1.2; tdm1 bits 0x3f800000 / 0x3f99999a): PLAUSIBLE, 3 of 3 (client-weapons, folded by spec-fold-7; the clean decompiler record reader
cshell+0x176f10 reads that key into a weapon-record float at record +0x260; that weapon+0x20c is a copy of it is not traced); `stat` +2 per 0x82 (the grenade included; meaning UNPROVEN); `wkind` 0; `moving`,
`wflag`, `mode1`, `flag_c` always 0; `seed` 2..255. `extra_kind` = **the index of the skeleton node the client's ray
hit** (PROVEN-CAPTURE tdm6: 53 for a hit in the head region, 6 for a hit in the chest region; with 24, 24, 11, 11 of tdm4 all six real-client hits name nodes whose ROM
`[Skeleton<s>] Node<k>Location` of the victim's body row is the location the reference stack answered, S2C 4.3.1; one node number can be another location on another skeleton, for
example node 53 is a leg in `[Skeleton1]`, so the victim's body row matters; capval-tdm6 2.1, folded by spec-fold-8; the earlier "Still UNPROVEN: `extra_kind` (24 and 11
observed, PLAUSIBLY a model part)" is superseded). The multi-pellet case and melee: 4.1.1 (tdm6). The float constants 255.0 and 1000.0 are PE-DATA VA 0x374d08a8 and VA
0x374ce72c. A miss is a message with `has_hit = 0`; the hit point itself is not sent, only its distance along the
ray, so the server reconstructs it as `origin + hit_dist * dir`.

**Server answer:** validation and damage are DeCombat policy (the 2009 rules are not in the client): the server
checks that `hit_obj` is a player, that the reconstructed point lies on or near that player's server-side box,
applies damage by weapon (ROM weapon table, at run time) and sends: to the victim the damage message (server 0x74,
whose first 96 bits are the hit direction, `MOVEMENT_2009.md` 12.5), to everyone the health / kill / kill-feed
messages of the server -> client catalogue (`GAME_MESSAGES_2009_S2C.md`, roster and match-flow sections), and to
the other clients the shot effect so they see muzzle flash and tracer (shell object messages, effect ids
`UDP_SESSION_2009_B.md` 7.2; which effect id the 2009 server used is UNPROVEN).

#### 4.1.1 Observed forms (PROVEN-CAPTURE; tdm1 83 shots, tdm2 11, tdm4 5, bit-exact: 0 bits left in every message)

Two shapes: a **miss / world hit / throw release** is 623 bits (`has_extra` 0, `has_hit` 0); a **hit on an object** is 887 bits (`has_extra` 1 and `has_hit` 1). Heads: rifle hit
`0b 82 00 91 00 91 00 f7 44 b6 c4 63 2d 2f 44 23 11 8a 44 38 85 b7 c4 b0`, grenade release `0b 82 00 07 00 09 00 75 01 1a c5 dc 21 15 44 e2`. Bit offsets from the engine message start (ids at bits 0-15, fields from
16): `wkind` 16, `weapon_id` 24, `ammo_id` 40, `weapon_pos` 56, `origin` 152, `aim` 248, `yaw` 376, `dir` 408, `seed` 504, `moving` 512, `wflag` 513, `mode1` 514, `frac` 515, `shot_scale` 523, `time_ms` 555, `has_extra` 587;
with an extra block: `extra_obj` 588, `extra_kind` 604, `extra_a` 612, `extra_b` 708, `flag_c` 804, `last` 805, `has_hit` 806, `hit_dist` 807, `hit_obj` 839, `stat` 855 (887 bits); without: `flag_c` 588, `last` 589, `has_hit` 590, `stat` 591 (623 bits).

- **The extra block of a hit** (tdm4, four rifle hits on the bot): `extra_obj` = `hit_obj` (the victim's character object), **`extra_a` = the hit point = `origin + hit_dist * dir` exactly** (error 0.000), `extra_b` = a unit vector
  against the shot, (-1, 0, 0) in all four (this is the entered face's normal of the doubled object box, not a constant: correction bullet of 2026-10-26 below; the earlier PLAUSIBLY-the-surface-normal reading is superseded), `extra_kind` 24, 24, 11, 11. A miss carries no extra block. The hit points lay 30 units in front of the victim's centre on the shooter's side and 24-27
  units above it, so the client's hit volume is the model, not the reference stack's 15 / 50 / 15 box (a server tolerance must allow for it: DSV-11's 24 / 53 / 24 grown by 48 accepts all four and classes them as body hits).
- **Hits on world geometry carry `has_hit` = 0** (tdm1, tdm2): only object hits fill `hit_obj`; a miss and a wall hit look the same on the wire (PLAUSIBLE; **PROVEN 2026-10-26 for the level and for world models, correction bullet below**).
- **Corrections 2026-10-26 (reports `client-brushhits` and `client-hitnodes`; disassembly PROVEN, captures PROVEN-CAPTURE).** They replace the readings marked above.
  1. **A hit on a world model carries no object.** The impact step (cshell+0x238eb0) fills the 0x82 hit object (`hit_obj`) and the whole extra block only when the engine's hit has no
     polygon (result words +0x20 / +0x24 both 0xFFFFFFFF), the hit object is non-null and its user flags carry 0x01000000 (the characters). The engine traces world models (object kinds
     2 and 9) through their own BSP and returns a polygon (engine+0x40650 -> engine+0x40190, polygon variant engine+0x407a0); any other object is traced by its box without a polygon.
     So a crate, a drum or any other world model stops the pellet like the level does and the message is the 623-bit form; the fire message's object form exists only for characters.
     PROVEN-CAPTURE: tdm1, about 33 rifle shots into the crates (229-270 s), all 623 bits; over 112 captures all 8,209 0x82 decode as either no extra block and no hit (5,263) or both
     (2,946), and every hit object is a character (kind 1). A server therefore cannot learn from the message that a breakable was hit and must trace each pellet's `origin` / `dir`
     against its breakables itself (DSV-122; the query layout and flags: `WEAPON_FIRE_2009.md` 5.3.1).
  2. **`extra_b` is the entered face's normal of the doubled object box, not a constant.** The box test (cshell+0x235330) takes centre = (min + max) / 2 and half-extent = (max - min) / 2
     times a float in the module's initialised data at rva 0x456bb8 (value 2.0; that test is its only reader), checks the three axis faces (strict bounds on the other two axes, early
     exit when both ends lie beyond a plane) and returns the outward normal of the face the ray enters (+1 or -1 on that axis); a start strictly inside the box returns the start point
     (the normal then is PLAUSIBLE). Hence every hit point lies 29.9-30.0 units in front of the victim's centre (half-extent 15 x 2). The "constant (-1, 0, 0)" reading is **retired**: the real
     client's shooter stood at lower x in tdm4 / tdm6, so the entered face was the -x face; shots along -z in the arena runs carry (0, 0, 1).
  3. **`extra_kind` is the node pick** (cshell+0x235c00, called by the impact step with the same end points): for each skeleton node whose sphere radius is above 0, with `v` = node centre
     minus the ray start, `t` = `v` . direction, the node is crossed when -r <= `t` <= length + r and the squared distance of the ray from the centre is <= r squared; among the
     crossed nodes the one with the smallest value of |`v`|^2 - r^2 is kept (the first in table order on a tie), and the result is **0xff when none is crossed**. The node centre is the node's
     world transform from the posed model (or its named socket).
  4. **Node-sphere pre-filter.** For a character, the engine's per-object filter (cshell+0x235220 -> cshell+0x234fb0) returns true only when the whole segment crosses at least one node sphere
     of the character's skeleton row (nodes with radius above 0); a body row without a skeleton passes on its box alone. Only then does the box test of item 2 run, and it supplies the hit
     point and normal. **If no sphere is crossed there is no hit:** the pellet goes on to the world or the next object and the message is the 623-bit form. (The engine's own object
     bounding-sphere pre-test, engine+0x40080, is not modelled; node spheres lie inside it.) The skeleton row and the node radii are ROM data, read at run time.
- One message per shot for one-vector weapons (four semi-automatic shots 295 / 329 / 344 ms apart by `time_ms`); automatic bursts of 30 round clips; a multi-pellet weapon was never fired in tdm1 / tdm2 / tdm4 (settled by tdm6, below).
- Weapons seen: 145 (ammo 145), 164 (ammo 164), 7 (the grenade, ammo 9), 0 (the knife was selected and never used).
- **Multi-pellet weapon (PROVEN-CAPTURE, tdm6 seq 22624, t 347.000; capval-tdm6 3, folded by spec-fold-8):** one shotgun shot is **eight 0x82 in one reliable block of one
  datagram**, 623 bits each (no hit), the count = the ROM `[Weapon125] VectorsPerRound` (read at run time); the pellets share every field except `dir`, `seed`, `last` and
  `stat`; `last` 1 only on the eighth; `stat` +2 per message (it counts messages, not shots); `frac` 0 (the weapon's first shot); `shot_scale` 0x3F99999A = 1.2; the pellet
  directions lie 0.30-3.00 deg from the aim's forward axis and up to 4.72 deg apart. 0x80 `[4][125][own_id][pos]` came at the select and at the pump, not per pellet.
- **Melee swing (PROVEN-CAPTURE, tdm6 seq 5540 / 5572; capval-tdm6 2.3):** a knife swing that hits nothing is one 623-bit 0x82 with weapon and ammunition 0, **`weapon_pos` =
  `origin` exactly** (the eye point; a rifle's lies about 10.9 units away), `dir` = the view forward, `frac` 0, `shot_scale` 1.0, `last` 1, no extra block, no hit, and no 0x80
  or 0x76 around it; two swings 248 ms apart. The hit form of a swing is not captured.
- **Head and torso hits (PROVEN-CAPTURE, tdm6 seq 6588 / 10125):** the 887-bit form with `extra_kind` 6 (hit point 15.1 above the victim's centre) and 53 (33.3 above); both
  hit points lie on the plane 29.94 units in front of the victim's centre on the shooter's side, `extra_b` (-1, 0, 0) (the face normal, correction bullet below), `extra_a` = `origin + hit_dist * dir` (error below
  1e-4). The node reading is in 4.1.
- **Also in tdm6:** weapon 125 (ammo 125) from weapon slot 4, the backpack shotgun (S2C 4.4.1); slot 1 (164) was never selected; no 0x83 (no clip emptied). The first 0x83 of
  the real client in any capture is in rust3 (158.813, ammunition 144; not analysed).
- The server answered each 0x82 within one reference stack tick (0-47 ms); the reference stack's reaction (0x74, 0x66, relays) is in `GAME_MESSAGES_2009_S2C.md` 4.3.1, 8.2.
- **Spread (`frac` and 0x64 `weapon_f`), what is known and what is a stand-in** (spec-fold-7, from the client-weapons report): tdm1 shows `frac` 0 on the first shot of a
  burst, +9 .. 13 / 255 per shot for weapon 145, saturating at **73 / 255 (0.29)**, and back to 0 within 0.2-0.4 s after the burst; the pistol 164 jumps to **45 / 255** after one
  shot; the shot direction lies within 0.009 (median) / 0.042 (max) of the aim's forward axis (PROVEN-CAPTURE). The 2009 spread law itself is **not read**: the weapon record keys
  `PerturbIncSpeed`, `PerturbDecSpeed`, `MinPerturb`, `MaxPerturb`, `DynamicPerturbFactor` are loaded by the cshell+0x176f10 family, but the code that grows and decays the
  spread was not traced (UNPROVEN). `dc-client` uses a stand-in, CLI-31 (proposed): grow by `PerturbIncSpeed x 0.036` per shot, fall by `PerturbDecSpeed x 1.5` per second, cap
  1.0, pellets uniform in the cone of half-angle `(MinPerturb + f (MaxPerturb - MinPerturb)) / 1000` rad (the milliradian reading PLAUSIBLE). It does **not** match tdm1: it
  grows past 0.29 to 1.0, and after one pistol shot it rises by 0.036 in our run (`PerturbIncSpeed x 0.036`), not to 0.18. A server must therefore not judge `frac` against CLI-31.
- **Second reader of the fire forms** (`dc-client`, client-weapons; D, TESTED, RUN against `dc-dserver` (115 shots, 99 hits, 3 kills in `net_weapons`); not against the real
  client): one 0x82 per pellet (`VectorsPerRound` of the row), 887 bits with the extra block when its ray hits a remote player's box before the world (`extra_kind` 24, normal
  (-1, 0, 0), CLI-43; superseded 2026-10-26 by `client-hitnodes`: the node pick and the face normal of items 2 and 3 above), 623 bits otherwise; `stat` +2 per message, `seed` 2..255, `time_ms` on the clock of the 0x64 trailer, `frac` = the spread before the shot. The first 23 of
  the 24 printed head bytes of the rifle hit and of the grenade release above are reproduced bit for bit (byte 24 needs `origin.y`'s exact bits, which this text gives rounded);
  the field offsets 523 / 555 / 587 / 588 / 604 / 805 / 807 / 839 / 855 agree. The hit volume is the box, while the real client traced its model (hit points 24-27 units above
  the box centre, above): a known difference (CLI-43).

### 4.2 `UP_RELOAD` (0x83)

Sender: `weapon` slot +0x2c = region cshell+0x2399e0 (DISASM; PE-DATA VA 0x374cec58). Arguments: a requested count
(negative = take the inventory count) and a "notify" flag. The weapon takes ammunition from `match_stats` (slot
+0x80 with the inventory slot byte weapon+0x210 returns the count; slots +0x38 and +0x14 book the transfer),
sets the clip (weapon+0xe8) and, when the notify flag is set, sends (R; PROVEN-CAPTURE, tdm1 four reloads `0b 83 91 00`, `0b 83 a4 00`: the two automatic reloads came in the same
reliable block as the 30th 0x82 of a clip, **after** it, so a clip mirror sees the shot before the reload; each is followed 100-250 ms later by `0x76 [3][1][v]` and three 0x80 sounds 1, 2, 3 over about 1.8 s):

```
[ammo_id:u16]              weapon+0x9c
```

Not sent: while `observer_camera()`; when the weapon state (slot +0x98) is 0xc (unless forced) or 0xb with the
weapon record flag +0x250; when the clip is already full or equals the stock. Afterwards the weapon state becomes 9
(PLAUSIBLE "reloading"). The weapon change path calls this with "notify" when the new weapon is empty (cshell+0x22e930).
**Server answer:** none required; the server updates its own ammunition count for the player (DeCombat: the server
mirrors clip and stock per weapon and refuses `UP_FIRE` reports beyond them, proposal DC-C2S-4).

### 4.3 `UP_WEAPON_SOUND` (0x80) and `UP_WEAPON_LOOP` (0x81): animation-key sounds for the other players

Sender: `weapon` base slot +0x08 = cshell+0x2360c0, the handler of the weapon model's animation keys (a key is a
name plus arguments placed on an animation frame; the names are ROM strings compared case-insensitively by
cshell+0x2eb60). The relevant branches (DISASM cshell+0x2364e7..0x2369f2):

| key (key object VA; the key strings are ROM data) | client action | message |
|---|---|---|
| fire key and alternate-fire key (VA 0x3759e214 / 0x3759e20c) | weapon slot +0x180 (sets the fire-pending byte, 4.1) | none here (0x82 per pellet follows) |
| sound key (second one, VA 0x3759e204) with argument `n` (1..14) | plays the weapon record's sound `n` locally | `UP_WEAPON_SOUND` 0x80, below |
| bute-sound key (VA 0x3759e1fc) | plays a sound named by the argument | none |
| loop-sound key (VA 0x3759e1f4) with argument `n` or the stop word (key 04dedf8afeb8a417/4) | starts loop sound `n` (or stops it) | `UP_WEAPON_LOOP` 0x81, below |
| deflect key (VA 0x3759e1cc) with a float argument | none locally | `UP_DEFLECT` (0x76 category 5), section 4.5 |

The two messages are sent only when cshell+0x21810 on `local_player` returns 0 (PLAUSIBLE: the player is not in a
third-person or replay view). R.

The keys live on the clips of the **first-person model** of the weapon row (spec-fold-7, from the client-weapons report): the composed clip list carries clips for select,
fire, reload, prefire, postfire and idle (found by name key), and their markers are the fire key, the throw key (the key of 4.6, `a40505c761a7c907/9`) and the sound key with
its argument `n`, matched by the SHA-256 key of the upper-cased marker word (the words are ROM data and are not quoted). A client that plays those clips and acts at the
markers (`dc-client`; D, CLI-29 for the no-ROM fallbacks (proposed); ROM-gated TESTED) reproduces the observed timing: sound 3 at the select clip's sound key, 0.68 s for
weapon 145 and 0.26 s for weapon 164 (observed 0.67-0.73 s and 0.25 s), the reload sounds at the reload clip's keys, sound 10 at the wind-up clip's key. This supports the
table's reading (the 0x80 is an animation-key sound) with timing; the 2009 client's choice of clip per weapon state is not traced (PLAUSIBLE).

```
0x80: [sound_no:u8][weapon_id:u16][own_id:u8][weapon_pos:vec96]
0x81: [sound_no:u8][weapon_id:u16]                      sound_no = 0 means stop
```

`own_id` is filled by `engine_client` member +0x84 (the own engine client id, `GAME_MESSAGES_2009_S2C.md` 1.1;
PROVEN call; **PROVEN-CAPTURE meaning**: 0 in 22 of 22 messages of tdm1, where `S2C_CLIENT_ID` gave id 0, and 1 in tdm4 / tdm2 where it gave 1). Observed (tdm1: 22 messages, all 144 bits, exact parse; head
`0b 80 03 91 00 00 cb db 75 c4 65 c3 55 44 74 7f cd c4`): `sound_no` **3** 0.67-0.73 s after every draw of weapon 145 (7 of 7) and 0.25 s after a draw of weapon 164, **1, 2, 3** (or 2, 1, 3) during each reload, **10** once
0.19 s before the first 0x4d of a grenade; none after the draw of weapon 0; **none per shot**: these are animation-key sounds of the select, reload and hold animations. No 0x81 was ever sent. **Server answer:** relay to
the other clients so they hear the weapon: the reference server relays as S2C 8.2 type 0x19 `[k = sound_no][weapon_id:u16][player:u32][pos:vec96]` to everyone including the sender, unreliable, 32-46 ms later (U5 decided for the
reference stack form; the 2009 server's own form is unknown).

### 4.4 `UP_WEAPON_CHANGE` (0x7f)

Sender: cshell+0x1f8fa0, called by the weapon list's change routine cshell+0x22e930 (and cshell+0x22e0c0) after a
weapon switch (number key, scroll, pickup, auto switch when empty). R. Layout (DISASM cshell+0x1f8ffa..0x1f902d):

```
[slot:u8][weapon_id:u16][aux:u16]
```

- `slot`: the weapon list index 0..8 (an out-of-range index is logged and replaced by 0, cshell+0x22e930).
- `weapon_id`: the item number of the new weapon.
- `aux`: **the new weapon's ammunition id** (weapon+0x9c, the `ammo_id` of 0x82 / 0x83; PROVEN-CAPTURE: 20 of 20 in tdm1, 14 of 14 in tdm4, 7 of 7 in tdm2, always equal to the `ammo_kind` of the server's 0xb2 for that
  weapon: (145, 145), (0, 0), (164, 164), (7, 9)). Every 0x7f is immediately preceded in the same block by `UP_WEAPON_EVENT [3][2]` (draw): 34 of 34. The client sends four 0x7f for slot 0 at the spawn, one per grant of the
  server's doubled spawn set, and switches back to slot 0 on its own after its last grenade (`0x76 [3][2]` + `0x7f [0][145][145]` in the block of the throw's 0x82).

After the send the client updates its HUD and, when the camera mode is 1 or 4, the first-person model. A second
writer cshell+0x1f8e50 builds `[u16][u16]` without the slot byte and has no caller (dead in this build).
**Server answer:** none required; the server records the current weapon (for damage and third-person display) and
relays the change to other clients. REFERENCE form, accepted by the real client: a `CF_FILES` change record on the player's weapon child
object (`UDP_SESSION_2009_B.md` 3.5), 3 of 3 changes, within 47 ms, to everyone including the sender. S2C 0x7f is the forced switch and
is not the relay.

### 4.5 `UP_WEAPON_EVENT` (0x76 category 3) and `UP_DEFLECT` (0x76 category 5)

Weapon animation-state notices, all R, all suppressed while `observer_camera()`:

```
[category:u8 = 3][event:u8]                  events 2, 3, 5
[category:u8 = 3][event:u8 = 1][value:u8]
[category:u8 = 5][deflect:f32]               the deflect key's float argument (atof)
```

| event | sender | when (PROVEN control flow) | meaning |
|---|---|---|---|
| 1 + `value` | cshell+0x23a210 (two sites, called from slots +0x14c, +0x150, +0x154) | after the weapon plays an animation chosen from its record (cshell+0x237c10) | UNPROVEN |
| 2 | slot +0x58 = cshell+0x239c90, slot +0x54 = region cshell+0x239e40 | after the select animation (weapon+0xf0) starts; slot +0x54 also scales its speed by the attachments (sum of +0x2b4 plus 1.0) | draw / select (PLAUSIBLE) |
| 3 | slot +0x64 = region cshell+0x23a0d0 | the weapon enters state 0xc and cshell+0x237da0 succeeds | UNPROVEN |
| 5 | cshell+0x237eb0 (from slot +0x154) | an idle animation is played on a random 5..15 s timer (cshell+0x19eb80(5.0, 15.0)) | idle fidget (PLAUSIBLE) |
| category 5 | cshell+0x2360c0, deflect key | animation key with a float argument | melee deflect window (PLAUSIBLE) |

**Observed (PROVEN-CAPTURE, tdm1 24 messages, tdm4 14):** event 2 (draw) once with every 0x7f; event 1 + `value` four times, 100-250 ms after each 0x83, with `value` 30 for weapon 145 and 15 for weapon 164 (the
reload animation, `value` PLAUSIBLY the clip size); events 3 and 5 and category 5 never occurred (no 20 s idle with a weapon drawn, no melee). `dc-client` sends `76 [3][1][clip size]` 150 ms after each 0x83 (CLI-32
(proposed), inside the observed 100-250 ms). There is **no fire event and no reload event beyond this**: firing produced no
0x76. **Server answer:** none required; the reference server sent no relay for category 3.

### 4.6 Weapon accessory messages: `UP_WEAPON_TOGGLE` (0xcc), `UP_WEAPON_PIECE` (0xd5), `UP_THROW_HOLD` (0x4d, formerly `UP_THROW_RELEASE`)

- `UP_WEAPON_TOGGLE` 0xcc, `[on:bool1]`, R. Sender: `weapon` slot +0xf4 = region cshell+0x239310. Only for weapons
  whose record (weapon slot +0x70) has flag +0x214: the weapon's toggle byte +0x1f8 is inverted, +0x201 := 1 and
  the time is kept at +0x208, then the new state is sent. Meaning PLAUSIBLE: an on/off accessory or alternate mode
  of the weapon (the select path cshell+0x239e40 resets it).
- `UP_WEAPON_PIECE` 0xd5, `[body:objref16][visible:bool1][piece:zstr8]`, R. Sender: cshell+0x239420 (from the
  reload path, the select path and two more weapon methods). The client hides or shows the named piece of the
  weapon model locally (engine model interface slots +0x24 find and +0x2c set hidden, hidden = not `visible`) and,
  when its third argument is set and the player's body exists (`move_mgr`+0x304), sends the body object
  (`move_mgr`+0x304, field +0x10) with the same piece name (a ROM model piece name, never fixed in code by the
  server). **Server answer:** relay to the other clients (the stage B list has a server -> client 0xd5,
  PLAUSIBLY the relay, section 9).
- `UP_THROW_HOLD` (formerly `UP_THROW_RELEASE`) 0x4d, no fields, R. Sender: the derived weapon's animation key handler (derived slot +0x08,
  region cshell+0x2ebb0, DISASM cshell+0x2ec40..0x2ec56) when the throw key (key a40505c761a7c907/9) passes, then
  the base key handler runs. **PROVEN-CAPTURE: it is sent repeatedly while a throwable is held** (18 times in 1.5 s, 62-78 ms apart, with a 406 ms gap while the player sprinted: tdm4; 8 times in tdm2; once in tdm1 where
  the hold was short), starting about 0.19-0.4 s after the 0x80 with sound 10 and after the fire key (0x400, `pu_flags` 0x2000) went down; **the release itself is one `UP_FIRE` 0x82** with the throwable's weapon and ammunition ids
  (7 / 9), `origin` = the eye point, `dir` = the throw direction (for example (-0.001, -0.911, -0.412) at the own feet), no extra block, no hit, `last` 1, sent 0.2 s after the last 0x4d, in the block that also carries
  `0x76 [3][2]` and `0x7f [0][145][145]` (the automatic switch back to slot 0 when it was the last grenade). **Server answer:** none for 0x4d; the server creates the thrown object from the 0x82 (trajectory, fuse and blast are
  DeCombat policy). The reference server created a projectile object (type 0x13 blob, `UDP_SESSION_2009_B.md` 3.5), relayed the release as S2C 8.2 type 0x19 `k` 0x12, sent the thrower 0x66 kind 1 for its grenade count, and
  2.5-2.9 s later removed the object, relayed a type 0x06 blast effect and sent 0x74 + 0x66 to the players in range. Every step of that answer (the 0x66 kind 1 count in the datagram of
  the create, the create form, the flight's change records and stream entries, the blast fields with `from` = this 0x82's `origin`, the order of removal and blast) is answered from
  tdm2 / tdm4 in `UDP_SESSION_2009_B.md` 3.5.4 and `GAME_MESSAGES_2009_S2C.md` 8.2 (spec-fold-6; the blast came 2.52 s after the 0x82 in both captures). DeCombat (D, DSV-39
  proposed; `dc-dserver` on master 96983bf): the 0x82 within the DSV-29 window since the last 0x4d is the release (the hold is consumed), its origin must lie within the DC-C2S-11
  shot origin tolerance, it costs one round of the clip mirror, and it starts the server-side flight whose object and blast forms and open differences are in S2C 8.2 (DeCombat
  paragraph); speed, gravity, fuse and the area damage are DSV-39 rule values (ROM `ProjectileFX` / ammunition rows at run time, else ours).
  **Client side, second reader** (`dc-client`, client-weapons; D, TESTED, RUN against `dc-dserver`; spec-fold-7): the throwable's clips drive the whole sequence, as the
  captures' timing suggests: the press plays the wind-up clip (0x80 sound 10 at its sound key), each pass of the hold clip while the button is held sends one 0x4d (the
  observed 62-78 ms spacing), and the release clip sends the release 0x82 (623 bits, the throwable's ids, the view direction, `last` 1) at its fire key (observed 0.2 s after
  the last 0x4d); after the last grenade `76 03 02` + `7f [0][...]` go out in the same moment. Without clips (no ROM) the CLI-29 fallbacks (proposed) apply.
  **More throws by the real client** (PROVEN-CAPTURE; folded by spec-fold-8): tdm6 has two throws of weapon 7 (0x80 `[10][7]`, then 19 / 7 x 0x4d 62-78 ms apart, then the release
  0x82 at 240.890 / 326.593, each release with 0x76 `[3][2]` + 0x7f `[0][145][145]` in the same block: the switch back after the last grenade; capval-tdm6 8). Against our stack
  with the 2-grenade pack (weapon 143, ammunition 143): in rust3 the client held three times (44 x 0x4d) after the wind-up and sent no release before the user died (cause
  UNPROVEN, V-NADE-1; capval-rust3 9.4); in rust4 each life's first throw had the wind-up 0x80 `[10]` and a release with an **0x83 `[143]` in the release block** (the client
  reloads its 1-round clip from its stock) and a reload event 0x76 `[3][1][1]` 0.23 s later, but after our 0x66 kind 1 said 0.0 it played only the hold (15 / 12 x 0x4d) without
  the wind-up and without a release (aa-triage-grenades item 6; the corrected count is in S2C 4.3.1).

## 5. Match flow, player info and connection-level messages

### 5.1 `UP_PLAYER_INFO` (0x72): the answer to the server's info request

Sender: `conn_mgr` slot +0x08 = cshell+0x23ccb0 (PE-DATA VA 0x374cedcc derived, VA 0x374f1b54 base). It runs when
the server sends shell message **0x72** (the stage D handler cshell+0x23c570 maps server 0x72 to slot +0x3c =
cshell+0x23c620, which calls slot +0x08; DISASM, PE-DATA jump table at VA 0x3733c5bc). It does nothing unless
`is_network_game()`. R. Immediately afterwards it sends `UP_CLIENT_OPTIONS` (0x85, section 5.2) with
`client_profile` (cshell+0x23d224).

Layout (DISASM cshell+0x23cd2a..0x23d1ea, order and widths PROVEN; sources are `session_info` fields, with the lobby
message that fills them from `PROTOCOL_2009.md` where it fixes them):

```
[room_slot:u32]            session_info+0xa4   own id in the room (0x383 +0x12 / room entry reply)
[char_no:u32]              session_info+0x08   character number (0x303 +0xe)
[session_idx:u16]          session_info+0x14   lobby session index (0x301 +0x1e, CD-20)
[exp:u32]                  session_info+0x194  experience total (0x303 +0x5e, 0x3004)
[look]                     cshell+0x1c2bc0, below
[r1:i8]                    first byte returned by method cshell+0x2251d0(5) of session_info+0x1b4
[r2:i8]                    profile record (session_info+0x1b8) +0x184
[r3:i8]                    profile record +0x17c
[r4:i8]                    profile record +0x18c
[r5:u32]                   profile record +0x180
[team:i8]                  session_info+0xf8   own team (1 or 2)
[r6:i8]                    profile record +0x160
[r7:u32]                   profile record +0x1a0
[gp:u32]                   session_info+0x104  GP total (0x303 +0x5a, 0x3004)
[n:u8]                     number of loadout entries (sizes of the two lists below added)
n x [item_no:u16][e1:u8][e2:u8][e3:u8]   loadout entries, first list session_info+0x1b4 -> +0x18..+0x1c,
                                         then list +0x28..+0x2c; entry +0x04, +0x30, +0x34, +0x38
[in_clan:bool1]            session_info+0x124 (clan id) != 0
if in_clan:
   [band_name:zstr16]      from cshell+0x321a0 on session_info
   [clan_id:u64]           session_info+0x124 (low 32 bits; high 32 bits written as 0)
   [clan_mark_a:u32]       session_info+0x428
   [clan_mark_b:u32]       session_info+0x42c
   [clan_word_c:u32]       session_info+0x438
[word_d:u32]               session_info+0x434
[nation:u32]               session_info+0x444  nation index (0x303 +0x100)
[flag_e:bool1]             session_info+0x19b
[flag_f:bool1]             session_info+0x19c
```

`look` (cshell+0x1c2bc0 on the compact look record that cshell+0x1b1b0 rebuilds from the profile record at
`session_info`+0x1b8, fields +0x148 .. +0x2ac; the compact record starts at profile record +4):

```
15 x [i8]                  record bytes +0x04, +0x00, +0x08, +0x0c, +0x10, +0x14, (argument or +0x2c),
                           +0x18, +0x1c, +0x20, +0x24, +0x28, +0x20 (again), +0x30, +0x13c
[text_a:zstr8]             record +0x31
[text_b:zstr8]             record +0xb1
```

The seventh byte is the argument (profile record +0x148) unless that is -1, then record byte +0x2c. Each "byte" is
the low 8 bits of a 32-bit field (`movsx`), so values outside -128..127 are truncated (PROVEN). The block has the
same shape (15 signed bytes, two 8-bit strings) as the `look` block that the server sends in `GS_PLAYER_INFO` 0x6b
(S2C §3.2.1 field 6), whose reader cshell+0x1c2a90 is the neighbouring method of this writer on the same object:
`look` is PLAUSIBLY the character's appearance record, which the server forwards to the other players in their 0x6b.

Meanings: the four head fields, `team`, `gp`, `exp`, `nation`, `clan_id`, `band_name` follow from the
`PROTOCOL_2009.md` sources (PLAUSIBLE for the wire meaning, PROVEN for the source). `clan_mark_a` / `_b` are
written by the lobby clan-change push 0x3012 sub 2 (emblem, background) and by 0x303 (cshell+0x33ac0 and
cshell+0x3f2c0 both call the setters cshell+0x1b14e0 / cshell+0x1b1550), so they are PLAUSIBLY the clan emblem and
background. The loadout entries are PLAUSIBLY the equipped items (item number plus three per-item bytes). All
`look` bytes, `r1`..`r7`, `word_d`, `clan_word_c`, `flag_e`, `flag_f`: UNPROVEN.

Parsing note: the count `n` is the sum of the two list sizes, but entries whose list pointer is null are skipped
without being written (cshell+0x23cf90 / cshell+0x23d016). If a list ever held a null slot the message would carry
fewer than `n` entries and the rest would be misread; PLAUSIBLE that the lists never hold null slots. A server
parses `n` entries as written.

**Server use:** identify the player (DeCombat: match `char_no` and `room_slot` against the roster the lobby handed
over, CD-60, and refuse a mismatch), take the team and loadout from the lobby's own record rather than from this
message (the lobby is authoritative; proposal DC-C2S-5), and then continue the join: roster records of all players
to the new client and the new player's record to the others (server -> client roster messages), followed by the
spawn sequence (Appendix A). The `look` block is forwarded unchanged in the player's 0x6b (PLAUSIBLE pairing).

#### 5.1.1 Observed 0x72 (PROVEN-CAPTURE; tdm1 seq 408, tdm2 conns 5/6/7, tdm4)

643 bits, exact parse (0 bits left), identical on a re-join of the same client (tdm2 conn 7 byte-identical to conn 6). Head `0b 72 00 00 00 00 ea 03 00 00 04 00 00 00 00 00 00 00 00 00 00 00 01 00`. Values of tdm1:
`room_slot` 0, `char_no` 1002, `session_idx` 4, `exp` 0, `look` bytes [0,0,0,0,0,1,0,0,0,1,1,-1,1,0,-1] with both strings empty, `r1..r4` = [-1,0,0,-1], `r5` 0, `team` 1, `r6` 0, **`r7` = 0x442f0000 (the f32 bit pattern of
700.0; 0x442e0000 = 696.0 in tdm2)**, `gp` 10000, `n` 4 with entries (145, 0xff, 0xff, 0xff), (164, ...), (0, ...), (7, ...), `in_clan` 0, `word_d` 0, `nation` 0, `flag_e` 0, `flag_f` 0. Cross-checks with the lobby on the same login:
`room_slot` = 0x383 +0x12 (0 for the host), `char_no` = 0x303 +0xe, `session_idx` = 0x301 +0x1e, `exp` = 0x303 +0x5e, `gp` = 0x303 +0x5a, `nation` = 0x303 +0x100, the clan bit = 0x86 of the clan record (0). V-C2 passes for a non-member
and for a second login and a re-join; the clan-member branch is untested. The loadout entry `item_no` is the weapon number of the weapon table (the `weapon` of the server's `GS_WEAPON_GIVE`), `e1..e3` = 0xff when no attachment (U12: with
this loadout no null slot occurred). The `look` bytes are forwarded unchanged by the reference server into the player's 0x6b (REFERENCE-CONFIRMED pairing). `r7` is PLAUSIBLY an f32; U2 meanings stay UNPROVEN (one character per
capture). The client answers the server's 0x72 request in the same 16 ms tick; DC-C2S-5 / DSV-19 (identify by `char_no` / `room_slot`) accept this client.

### 5.2 `UP_CLIENT_OPTIONS` (0x85)

Sender: cshell+0x1ed040 on `client_profile`, R:

```
[opt_a:u8]                 client_profile+0x1d0
[opt_b:bool1]              client_profile+0x1de > 0
```

Triggers (PROVEN): right after `UP_PLAYER_INFO` (cshell+0x23d224); when the server sends shell message **0x73**
(`conn_mgr` slot +0x40 = cshell+0x23c610 -> slot +0x0c = cshell+0x23c8e0, which first calls a `weapon_db` method with
a ROM string, purpose UNPROVEN, and then sends); and when the option values are re-applied while connected (cshell+0x1ed700, guarded by
`engine_client` member +0x8c "is connected"). Meanings of the two options: UNPROVEN. PROVEN-CAPTURE (tdm1, tdm2, tdm4): `0b 85 02 01` (`opt_a` 2, `opt_b` 1, 25 bits), in the same datagram as 0x72, right after it; the other trigger (server 0x73) was not exercised.
**Server answer:** none.

### 5.3 `UP_PLAYER_INFO_OLD` (0x33): unused variant

cshell+0x48cd0 writes `[char_no:u32][session_idx:u16][exp:u32][look][r2:i8][r3:i8][team:i8][r6:i8]`
(the same sources as 0x72 in a shorter order). No call, jump or table reference to it exists in the binary
(DISASM xref scan), so it is dead in this build. A server ignores 0x33.

### 5.4 `UP_WORLD_READY` (0x87)

Sender: cshell+0x1a9dc0, no fields, R. Trigger (PROVEN): the loading-screen tick cshell+0x1bb910 sees the in-world
test cshell+0x1f3950 pass (`UDP_SESSION_2009_A.md` 8.3: view object, switching-worlds state 2 or 3, entered-world
flag, and `life_state` non-zero); it closes the loading screen, sends 0x87 **once** and switches the game shell to
its playing state 8.

Meaning PLAUSIBLE: "loading finished, I am in the world". Note the order this implies: the server must already have
sent the view object, shell 0x88 and a `life_state` (the player-state carrier of `UDP_SESSION_2009_A.md` 8.3 d)
**before** the client can send 0x87. **Server answer:** mark the client ready; start sending match state (round
timer, scores, roster) and spawn it (Appendix A step A6). PROVEN-CAPTURE (V-C5 passes on every client link: tdm1 1, tdm2 2, tdm4 1): exactly one 0x87 per link, 47-109 ms after the server's 0x67 `[1]` and right after
the character's create record, in the same block as the reliable 0x64 state form; before it the client had received 0x88 `[2]`, the view object, 0x67 `[1]` and the object group that creates its character. The view
object id may be named before its create record arrives; the in-world test waits for the record.

### 5.5 `UP_ROSTER_QUERY` (0x8c)

Sender: cshell+0x29e80 on `roster`, R, only when `is_network_game()`:

```
[pid:u32]            0xFFFFFFFF = "not found by name or object" (see below)
```

Triggers (PROVEN): the roster lookups by player id (cshell+0x29f70), by object (cshell+0x29fb0, cshell+0x2a070)
and by name (cshell+0x29ff0) send it when the lookup fails and their "ask" argument is set; they pass
0xFFFFFFFF because they do not know the id. cshell+0x2b170 also sends one for every id queued in the roster's
pending list (`roster`+0x48) and clears the list. The client stores the send time at `roster`+0x0c.
Meaning PLAUSIBLE: "I miss the record of this player (or of somebody)". **Server answer:** the roster record(s)
(server -> client roster messages of stage B; for 0xFFFFFFFF the full roster). **PROVEN-CAPTURE (tdm1, tdm2):** the trigger is a server 0x6b whose `character` the client has not created yet: the client queues that pid
(S2C 3.2.1: cshell+0x2bd40(pid) when `character` is none) and sends `0b 8c <pid:u32>` about 1 s later, with the **real pid** (tdm1: `0b 8c 00 00 00 00` for the own id; tdm2: pids 0 and 1), not 0xFFFFFFFF, which the name and
object lookups send. Exactly one query per unresolved pid; none on a link where every 0x6b named an existing object. The reference server answered with the 0x6b of that pid (kind 2) 30-60 ms later. Rule: send the create record
before the 0x6b (S2C 3.2.2).

### 5.6 `UP_INTEGRITY_REPLY` (0xac): answer to the server's 0xac challenge

Sender: `conn_mgr` slot +0x34 = cshell+0x23c900, reached from server shell message 0xac (stage D maps 0xac to slot
+0x34). The server message starts with `[kind:u8]`:

| server kind | client action | reply |
|---|---|---|
| 0, followed by `[check:u16]` | if `check` = 0x100: reply `[0][version:u16][word:u32]` (`version` = the u16 at PE-DATA VA 0x374d3b6c, `word` a 32-bit value of the client, source UNPROVEN); any other `check`: the client sets `conn_mgr`+0x11e0 and calls its slot +0x30 (disconnect, PLAUSIBLE) | `[sub:u8 = 0][version:u16][word:u32]` |
| 1, followed by `[seed:u32][u32]` | computes four words from the seed, a string of `client_profile` (+0x5c), the own module file name and a module check value (cshell+0x297720) | `[sub:u8 = 2][w1:u32][w2:u32][w3:u32][w4:u32]` |
| 3 | nothing | none |
| 4 and any other value | sets `conn_mgr`+0x11e0 and calls slot +0x30 (disconnect) | none |

This is an integrity challenge of the 2009 service. Its hash is not reproduced here and a DeCombat server does not
need it: **DeCombat never sends server 0xac** (proposal DC-C2S-6), so the client never sends 0xac. Layout PROVEN
(DISASM cshell+0x23c99e..0x23c9ca and 0x23cb8a..0x23cbcb); the four words of sub 2 are opaque.

### 5.7 `UP_LEAVE_MATCH` (0xbd)

Sender: cshell+0x48450, the "yes" callback of the leave-the-match message box (CRes string id 0xbbf, built at
cshell+0x48fe5..0x4900a), no fields, R. After the send the client sets the two leaving latches (`match_ui`+0x6a6d
and `conn_mgr`+0x11e0, only when connected), stops the match locally, returns the shell to the lobby (PLAUSIBLE) and
clears two `session_info` flags (+0x198, +0x199). Control flow PROVEN. **Server answer:** none; remove the player at once
(do not wait for the link timeout) and tell the lobby (`DSERVER_PIPE.md`). **PROVEN-CAPTURE (tdm1, tdm2, tdm4):** `0b bd` (16 bits) leaves in the same millisecond as the lobby's TCP 0x394 with `+6` = 0
(`PROTOCOL_2009.md`, the 0x394 row and section 7); the engine disconnect follows either at once (`C2S_BYE`, 15-16 ms later: first leave in tdm2, tdm1, tdm4; also tdm6, 356.500 -> 356.515, and rust3, 234.531 -> 234.547, dserver-round2 G6) or only when the client is back in the waiting room (the plain transport `CTL_BYE` x 4 with TCP
0x306 `{1}`, 7-9 s later: tdm2 second leave, where no `C2S_BYE` was sent at all). After `C2S_BYE` the client sends nothing more on the link until then (`UDP_TRANSPORT_2009.md` 9.1). The reference server removed the player's
objects only at the end of the link, never on 0xbd; DeCombat's "remove at once on 0xbd" is the safer choice and stays.
DSV-51 (dserver-round2, merged; PENDING in `DC_DEFAULTS.md`; folded by spec-fold-8): the player leaves the world at the 0xbd, the link stays open until the client's
`C2S_BYE` (then closed with `CTL_BYE_CODE(5)` x4) or 1000 ms later (ours); never the plain bye at the 0xbd. In rust3 our server sent its plain bye x4 at the 0xbd, before the
client's `C2S_BYE`, and the client accepted it (it returned to the room; PROVEN-CAPTURE, `UDP_TRANSPORT_2009.md` 9.1): DSV-51 is fidelity, not a fix.

### 5.8 `UP_PERF_REPORT` (0xc3): ping and frame-rate statistics

Sender: cshell+0x2ae70 on `roster` (periodic, from cshell+0x35e90) and the console handler region cshell+0x3a4f0.
R. **Off by default:** the periodic part runs only after the server enabled it with its own **0xc3 sub-id 1**
(`[1][pid:u32][enable:u8][ping_ms:u16][stats_ms:u16]`, handler cshell+0x2acd0 -> cshell+0x29300: `enable`
= 1 turns it on, the two periods are stored as seconds at `roster`+0x28 / +0x2c, and the client's own statistics
overlay is shown only when `pid` is its own engine client id). Layouts (DISASM):

```
sub 2 (every stats_ms):  [sub:u8 = 2][own_id:u32][fps:u16][ping:u16][s2:u16][s3:u16]
sub 3 (every ping_ms):   [sub:u8 = 3][own_id:u32][t:f32][0:u32][0:u32][0:u32]
sub 1 (console only):    [sub:u8 = 1][own_id:u32][enable:u8][ping_ms:u16][stats_ms:u16]   (section 8.7)
```

- `own_id`: the own roster record +0x04 (`roster`+0x08 holds the own id).
- `fps`: `truncate(value)` of the client variable whose key is the ROM string dc20e29f4a2f96d4/10 (read through
  `engine_client` member +0x104 and slot +0x5c), also stored in the own record +0x1ec.
- `ping`, `s2`, `s3`: the own record's words +0x1e0, +0x1e4, +0x1e8 (low 16 bits). +0x1e0 is the round-trip time
  the client measured from the server's echo (below).
- `t`: the client time (`engine_client` slot +0x6c) as raw float bits.

Server answers (PROVEN from the client handler cshell+0x2acd0):

- echo every sub 3 back as server **0xc3 sub 3** `[3][pid:u32][t:f32][x:u32][x:u32][x:u32]` with the same
  `t`; the client sets its own record +0x1e0 := `truncate((now - t) * 1000)` (milliseconds) when `pid` is its
  own id;
- distribute each player's sub 2 as server **0xc3 sub 2** `[2][pid:u32][u16][u16][u16][u16]` (the client
  stores them in that player's record through cshell+0x2a180; PLAUSIBLY for the statistics overlay). The
  scoreboard ping column itself is fed by `GS_PING_TABLE` 0xa2 (S2C §3.2).

DeCombat default (proposal DC-C2S-7): enable with sub 1 after `UP_WORLD_READY` (ping period 2000 ms, statistics
period 5000 ms), echo sub 3, relay sub 2, and fill the scoreboard with `GS_PING_TABLE` 0xa2 from the reported pings.
PROVEN-CAPTURE in the negative (tdm1 157 s, tdm2, tdm4): without an enable the real client sends no 0xc3. The enable with `pid` 0xFFFFFFFF is untested (a `pid` equal to its own id also shows its statistics overlay).

## 6. Chat and radio commands

### 6.1 `UP_CHAT` (0x68)

Sender: the in-match chat input element `hud_chat_input` (constructor cshell+0x72cc0), its send method cshell+0x72df0.
R. Layout (DISASM cshell+0x730d2..0x7312b):

```
[text:zstr16]              the typed line, after the client's word filter (object VA 0x375bbdf4, method +0x0c)
[tag:zstr16]               empty in this sender (the writer is called with a null string)
if mode == 0 : [scope:u8 = 0]          all players
if mode == 1 : [scope:u8 = 1]          own team only
```

`mode` is the chat input's mode `hud_chat_input`+0x1e10; for any other mode value no `scope` byte is written (the
server then reads 0 at the end of the message, `GAME_MESSAGES_2009.md` 1.1). Lines that the client treats as local
commands (three tests, cshell+0x42920, cshell+0x40ca0, cshell+0x3c5d0) are not sent here.

Client-side repeat guard (PROVEN, cshell+0x72df0): the input keeps the last line sent (+0x1cf2) and a counter
(+0x1dec, 9 after a new line). Every repeat of the same line lowers the counter; at 0 the time is stored (+0x1df0)
and for 60,000 ms every further send is refused with a notice (CRes string id 0x1028), after which the counter is
reset to 10. A server does not need its own repeat filter for this client (DeCombat may still rate-limit, CD-30
covers the audience).

**Server answer:** server **0x68** (`GS_CHAT`, `GAME_MESSAGES_2009_S2C.md` 2.4) to the audience: everyone for
scope 0, the sender's team for scope 1 (with `team_only` set), **including the sender** (the client prints its own
line only from the server's echo: the S2C handler compares `sender_id` with the own id to mark it, and the chat input
does not print locally; PLAUSIBLE, consistent with the lobby rule CD-30). The server fills `sender_name` and
`sender_id` from its own roster, never from the message. PROVEN-CAPTURE (tdm1): two lines with scope 0 (`UP_CHAT` 104 and 88 bits: `text` of 3 and 2 units, empty `tag`, `scope` 0; exact parse); the reference server echoed each 47 ms
later as S2C 0x68 to the sender. Team chat (scope 1) and the radio pair 0xbe + 0x68 were not exercised (V-C4 open for them).

### 6.2 `UP_RADIO` (0xbe) and its text line (0x68)

Sender: cshell+0xb9d80, PLAUSIBLY a method of the radio-command element `hud_radio_call` (constructor cshell+0xb9930). Trigger: while the
radio menu is open (+0x15b4 set), one of nine command keys (key codes 0x54..0x59 and 0x7c..0x7e select entries 0..8)
picks an entry of the current radio page; the index must be below the page's entry count (radio table object VA
0x375790d4, cshell+0xf6db0). The menu closes (+0x15b4 := 0) and two reliable messages are sent:

```
0xbe: [pos:pos48][radio_set:u8][entry:u8][variant:u8][to_all:bool1][voice:u8]
0x68: [text:zstr16][tag:zstr16][scope:u8]
```

- `pos`: the own view object's position (`engine_client` slot +0x20c, then slot +0xa0), `pos48` writer slot +0x48.
- `radio_set` (+0x15b8) and `entry` (page offset +0x15c4 plus the key index) select the radio line in the ROM radio
  table; `voice` (+0x1610) is the speaker voice set taken from the own character (cshell+0xf6700); `variant` =
  `rand() % n` with `n` the number of sound variants of that line and voice (cshell+0xf6e30), 0 when there are none.
- `to_all`: the radio table's flag for the line (cshell+0xf6c00). The text line uses `scope = 1` (team) when
  `to_all` is 0 and `scope = 0` when it is 1.
- `text` is the radio line's text and `tag` the radio set's caption, both read from the ROM radio table (UTF-16).

Meanings PLAUSIBLE. **Server answer:** relay 0xbe to the audience (the stage B list contains a server -> client
0xbe, PLAUSIBLY this relay with the sender's id added; the client plays the radio sound and marks the position on
the radar, `hud_radar`), and send the text line as server 0x68 exactly as in 6.1.

## 7. Use actions, mode objectives, votes and the remaining in-match messages

None of these is needed for a plain TDM round; they are listed so a server can parse and log them.

### 7.1 Use actions: `UP_USE` (0x59) and `UP_USE_HOLD` (0xd8)

Both are driven by engine command 2 (PLAUSIBLY "use / activate"; `MOVEMENT_2009.md` 5.1 notes that the aim bit
requires command 2 to be off), tested with `engine_client` member +0x204.

`UP_USE` 0x59, R, sender cshell+0x61e40 (DISASM 0x62959..0x62a90):

```
[target:objref16][phase:u8]          phase 0 = pressed, 1 = released
```

The target is found by cshell+0x5e050: the first entry of the shell's object list (`client_shell` slot +0xd4,
list at +0x374, count +0x380) with kind 8 (+0x9c), active (+0x2c4) and a team byte (+0x94) equal to the own team
(`local_player`+0x1e4) or 0xfe (any team); `target` is that entry's object id (+0x10). Phase 0 is sent once when
command 2 is on, such a target exists and neither latch is set (then latch `local_player`+0x488 := 1). When command
2 is off or no target is found, +0x488 is cleared and, if the second latch +0x487 is set, phase 1 is sent and +0x487
cleared (the object written in that branch is not visible in the C, PLAUSIBLY the same target). +0x487 is set by
server `GS_PLAYER_NOTE` 0x5a kind 0 and cleared by its kind 3 (S2C §4.4), so phase 1 answers a server note.
Meaning PLAUSIBLE: use a team object (supply or objective object of a mode). **Server answer:** mode rule; 0x5a
kind 0 when the use was accepted (PLAUSIBLE).

`UP_USE_HOLD` 0xd8, R:

```
[phase:u8][target:objref16]
```

| phase | sender | when (PROVEN) |
|---|---|---|
| 1 | cshell+0x1f7890 (`local_player` base update, run by the derived update) | command 2 is on, `local_player`+0x1c9 or +0x1c8 is set, no hold is active (+0x200) and the player has a use target (`local_player`+0x1e8; its object id +0x10 is sent); the hold starts (+0x200 := 1, target kept at +0x208) |
| 4 | cshell+0x1f7890 (two sites) | command 2 went off while a hold was active, or the hold ended on the client side; the kept target is sent and cleared |
| 2 | region cshell+0xb01c0 (slot +0x10 of the HUD element with method table VA 0x374d88b0) | a HUD state change of that element while a target exists; trigger UNPROVEN |

Meaning PLAUSIBLE: hold-to-use (plant, defuse, capture). The stage C list has a server -> client 0xd8 (PLAUSIBLY
the server's progress or refusal). **Server answer:** mode rule.

### 7.2 `UP_USE_PROGRESS_END` (0xad)

Sender: cshell+0x275ad0 (a use-progress object: target at +0x08, remaining time at +0x04 counted down by the frame
time `engine_client` slot +0x70). R:

```
[target:objref16][remaining:f32]
```

Sent when the hold stops: command 2 released, the target or the weapon state changed (cshell+0x2b1d30 /
weapon slot +0x98 = 3), or the remaining time reached 0. Meaning PLAUSIBLE: "my hold on this target ended with
this much time left" (0 = completed). The stage C list has a server -> client 0xad. **Server answer:** mode rule.

### 7.3 Scanner item: `UP_SCAN` (0xcd) and `UP_SPOTTED` (0x84)

Senders: methods of the HUD radar element (`hud_radar`, global VA 0x37560c98): cshell+0xb7a20 (called from the
`local_player` update cshell+0x61e40 with "on" when `local_player`+0x478 is set and the player does not sprint),
cshell+0xb5c40 and cshell+0xb77e0. R.

```
0xcd sub 1: [sub:u8 = 1][team:u16][on:u8]               on = 1 switched on, 0 off (a third site sends another byte)
0xcd sub 2: [sub:u8 = 2][team:u16][name:zstr8]           name from cshell+0x2ac60 for the own roster id
0x84:       [obj:objref16][flag:bool1][team:u8]           one per tracked object of another team
```

`team` is `hud_radar`+0x1a0 (own team). 0xcd sub 2 is sent when a charge is spent (+0x2ac reduced by +0x2b0).
0x84 is sent while the scan is active (+0x3b0 = 1) for every tracked object whose team (+0x114) is neither the own
team nor 0xff, with the object's flag +0x12d. Meaning PLAUSIBLE: a scanner / radar accessory that reveals enemies
and reports them. Server 0x84 is handled by the stage F handler (cshell+0x45050 -> cshell+0xb8610 / 0xb8650),
PLAUSIBLY the relay to the team. **Server answer:** mode rule; DeCombat relays 0x84 to the sender's team.

### 7.4 Mode-rules messages: `UP_LEVEL_LOADED` (0x96), `UP_LEVEL_REQUEST` (0x98), `UP_LEVEL_EXIT_ACK` (0x99)

Senders on `mode_rules` (stage E object; derived method table VA 0x374d3194):

- `UP_LEVEL_EXIT_ACK` 0x99, no fields, R: cshell+0x22f6e0 (slot +0x28) through the generic sender cshell+0x1a1680.
  It is the acknowledgement of the server's **0x99** ("exit level", handled by slot +0x3c = cshell+0x22f950:
  `[next_world:zstr8(260)][flag_a:bool1][want_ack:bool1]`): when `want_ack` was set, the client sends 0x99 once
  the pending level change (+0x8c) is processed, then starts the next level (slot +0x44 with the stored name).
- `UP_LEVEL_LOADED` 0x96, R: derived slot +0x50 = cshell+0x5b1a0: `[one:u8 = 1]` followed, only when
  `is_network_game()`, by `[name:zstr8]` (the string at +0x04 of the object returned by cshell+0x169fb0, PLAUSIBLY
  the current mission or world name). The base slot +0x50 (cshell+0x22fe10) writes `[one:u8 = 1]` only and is
  replaced in the live object.
- `UP_LEVEL_REQUEST` 0x98, `[name:zstr8]` (`mode_rules`+0x88), R: region cshell+0x22fcd0 (slot +0x34). The method sets
  +0x94 := 3, asks `conn_mgr` slot +0x2c and then its own slot +0x50 (so 0x96 is sent first); only if both succeed
  it sends 0x98.

Meanings PLAUSIBLE (mission / level-chain flow of the mode-rules object, `UDP_SESSION_2009_A.md` 1.1 "mission
methods"). A TDM round on a DeCombat server never sends server 0x99, so the client sends none of these in TDM.
**Server answer:** for 0x99, the next `S2C_LOAD_WORLD` (`UDP_SESSION_2009_A.md` 4.6).

### 7.5 Votes and acknowledgements: `UP_VOTE_ANSWER` (0xb8), `UP_VOTE_START` (0xce), `UP_ACK_7E` (0xd1), `UP_ACK_B6` (0xba)

- `UP_VOTE_ANSWER` 0xb8, `[yes:bool1]`, R: cshell+0xcf230, called by the key handler cshell+0xcf300 of the vote
  request element (`hud_vote_request`, global VA 0x37560cf8) while a request is shown (+0xa0 set, +0xa4 clear): key
  code 0x74 sends `yes = 1`, key code 0x75 sends `yes = 0`.
- `UP_VOTE_START` 0xce, R: cshell+0xd03b0 on the vote element (`hud_vote`, global VA 0x37560cfc), from its key
  handler cshell+0xd1c80 (number keys select an entry, key code 0x74 confirms in page 3):

  ```
  [kind:u8][target_id:u8][reason:u8]
  ```

  `kind` = 1 when `hud_vote`+0x150 is 0, else 0; `target_id` = byte +0x04 of the selected entry (a roster record,
  so the player id); `reason` = `hud_vote`+0x10c. Only sent when the selected player is still in the roster
  (cshell+0x29f70). Meaning PLAUSIBLE: start a vote (for example a kick) against a player for a reason.
- `UP_ACK_7E` 0xd1, no fields, R: sent by the stage F handler cshell+0x45050 right after it processed server
  **0x7e** (`[text:zstr8(64)][value:u32]`, stored for `hud_vote`).
- `UP_ACK_B6` 0xba, no fields, R: sent by cshell+0x45050 right after it processed server **0xb6** (cshell+0x1d0120).

**Server answer:** vote rules are DeCombat policy (CD-07 covers moderator kicks; a player vote is a new rule,
proposal DC-C2S-8: votes are off by default). The two acknowledgements need no answer.

### 7.6 `UP_RESPAWN_TOKEN` (0xdb)

Sender: cshell+0xbac00 on the respawn-token element (`hud_respawn_token`, global VA 0x37560d3c), R:

```
[zero:u8 = 0][sub:u8 = 5][token:u32]          token = hud_respawn_token+0x38
```

Trigger (PROVEN): key code 0x76 while the element is in state 1 or 6 and holds a token (+0x38 != 0)
(cshell+0xbad80), sent once until the server answers (+0x3c latch, time kept at +0x28). The element's state and
token come from server **0xdb** (`[0][sub:u8]`, stage B, cshell+0x2e839): sub 1 `[count:u32]`, 2 / 3 / 4 set the
element state, 6 `[token:u32]` stores the token and **arms the respawn wait** (cshell+0x5dfe0, section 3.3), 7
resets it. Meaning PLAUSIBLE: "use a respawn token (a shop item) to respawn now". **Server answer:** server 0xdb
sub 6 (accept) or 7 (refuse); DeCombat has no respawn-token item rule yet (proposal DC-C2S-9: refuse).

## 8. Generic sender, developer console commands and unreachable senders

### 8.1 The generic sender cshell+0x1a1680 and its callers

cshell+0x1a1680 takes `(shell_id, flags)` (cdecl), builds `[shell_id:u8]` and sends it with the given flags (PROVEN).
Callers (DISASM direct-call scan; a tail jump at cshell+0x1a3990 also enters it):

| id | caller | when | our name |
|---|---|---|---|
| 0x4d | region cshell+0x2ebb0 (`weapon` derived slot +0x08) | throw animation key (section 4.6) | `UP_THROW_HOLD` (formerly `UP_THROW_RELEASE`) |
| 0x71 | cshell+0x1f3770 | `life_state` is 2, 3, 5 or 6 and `is_network_game()`; no call or table reference to cshell+0x1f3770 was found (PLAUSIBLY unreachable in this build) | `UP_STATE_71` (meaning UNPROVEN) |
| 0x99 | cshell+0x22f6e0 (`mode_rules` slot +0x28) | acknowledgement of server 0x99 (section 7.4) | `UP_LEVEL_EXIT_ACK` |

All three pass flag 1 (R).

### 8.2 `UP_LOCAL_CHEAT` (0x69): local worlds only

cshell+0x29ce20 writes `[cheat:u8][value:u32]` (R). It is called only by the 27 small wrappers cshell+0x29cef0 ..
cshell+0x29d570, all dispatched by cshell+0x29d5d0 for cheat numbers 0 .. 0x23, and that dispatcher sends
**only when `is_network_game()` is false** (a local world; number 0x17 has a separate local action in network
games). Each wrapper keeps its own toggle in a global and sends the new value. A remote server never receives 0x69
from an unmodified client; DeCombat drops it (proposal DC-C2S-1).

### 8.3 Senders with no caller (dead in this build)

DISASM xref scan (direct calls, jumps, absolute references in code and data): no reference reaches these senders.

| id | sender | layout written |
|---|---|---|
| 0x33 | cshell+0x48cd0 | see 5.3 |
| 0x6e | cshell+0x5f500 (console-handler signature: argument count and list) | `[a:u8][b:u16]` from `atoi` of the first two arguments; with no argument it only prints a usage line |
| 0x7f (short) | cshell+0x1f8e50 | `[u16][u16]` (see 4.4) |
| 0xae | cshell+0x27b9e2 | `[0xae]` followed by the fields of cshell+0x279e00 (not decoded) |

### 8.4 Weapon-attachment developer commands: `UP_DEV_ATTACH` (0x8b)

Eight console commands registered by the controller set-up cshell+0x1f6fb0 (engine client interface member +0xfc
registers a command name and a handler; the names are ROM strings, given as sha256[:16]/length). R.

| command (key) | handler | layout after `shell_id` |
|---|---|---|
| a0c40af6b7946702/10 | region cshell+0x1f6670 | `[sub:u8 = 0][result:u8]` after the current weapon's attachment slot +0x104 accepted the attachment named by the argument (`weapon_db` lookup) |
| 8e8c0030d4ca784c/12 | region cshell+0x1f6800 | `[sub:u8 = 1][part:u8 = 0]` |
| 6b4ac1504a94568c/11 | region cshell+0x1f68f0 | `[sub:u8 = 1][part:u8 = 1]` |
| 636787f41a04854e/9 | cshell+0x1f69e0 | runs the two previous handlers |
| ac41c2b40f27909e/14 | cshell+0x1f6a10 | `[sub:u8 = 2][part:u8 = 0][pos:bool1 = 1][argc:u8]` and, if `argc > 2`, `[x:f32][y:f32][z:f32]` (`atof` of the arguments) |
| 299ae9e170eab2ee/14 | cshell+0x1f6b70 | `[sub:u8 = 3][part:u8 = 0][pos:bool1 = 0][argc:u8]` + the same optional floats |
| d7c13fb2432f26d5/13 | cshell+0x1f6cd0 | `[sub:u8 = 2][part:u8 = 1][pos:bool1 = 1][argc:u8]` + the same optional floats |
| f46be4d895859237/13 | cshell+0x1f6e30 | `[sub:u8 = 3][part:u8 = 1][pos:bool1 = 0][argc:u8]` + the same optional floats |

Note the parse rule: the three floats are present only when `argc > 2` (PROVEN, cshell+0x1f6aa1); a server must read
`argc` first. Meaning PLAUSIBLE: attach / detach the muzzle or scope part and tune its offset or rotation
(developer tools). DeCombat drops 0x8b (proposal DC-C2S-1).

### 8.5 Developer console commands: 0xa7, 0xa8, 0xa9, 0xb0

Registered by the shell set-up (engine client interface member +0xfc at cshell+0x1a9822..0x1a9939). R.

| command (key) | handler | id | layout |
|---|---|---|---|
| 1695b48bccfe5126/3 | cshell+0x1a8f50 | 0xa8 | `[command:lstr16h]` (a string handle built from the arguments, call at cshell+0x1a9070) |
| 8b9c643731c94119/7 | cshell+0x1a9150 | 0xa7 | `[object:lstr16h][message:lstr16h]` |
| 6f202f54a7b2d8fe/4 | cshell+0x1a9290 | 0xa7 | `[object:lstr16h = null handle][message:lstr16h]` |
| b1a96dd646bccaa2/5 | cshell+0x1a8e60 | 0xb0 | `[name:zstr8][value:u32]` |
| 85736ee4731ef67b/8 | region cshell+0x1a8c40 | 0xa9 | sub 0: `[sub:u8 = 0][name:zstr8][pos:vec96]`; sub 1: `[sub:u8 = 1][name:zstr8][name2:zstr8]` |

Meanings PLAUSIBLE (send a server console command, send a trigger message to a named object, effect test
commands). These are server-control tools. DeCombat ignores them unless the account has the game-moderator flag
and an operator option enables them (the same policy as CD-64 for `C2S_CONSOLE`; proposal DC-C2S-1).

### 8.6 `UP_DEV_TRACE` (0x75)

Sender: `local_player` slot +0x38 = region cshell+0x1fa660. It first reads a client string variable (object VA
0x3759c0dc, key VA 0x3759c0d8): when the variable is missing it runs an empty console command and returns; when
its first character is not a space it runs the variable's text as a console command (`engine_client` member
+0x208) and returns. Only when the text starts with a space does it take a trace record from the object at
`local_player`+0x1c (slot +0x1c fills it) and, unless the record's byte +0x1c is 3 and that object's slot +0x24 is
false, sends (R, helper cshell+0x1f46e0):

```
[a:vec96][q:quat128][b:u8][obj:objref16][c:vec96][d:u8]       record +0x00, +0x0c, +0x1c, +0x2c, +0x30, +0x3c
```

Meaning PLAUSIBLE: a developer report of the current aim trace (position, rotation, hit kind, hit object, hit
point). DeCombat drops it (proposal DC-C2S-1).

### 8.7 `UP_PERF_REPORT` sub 1 (0xc3) console command

Region cshell+0x3a4f0 (console command key 94e68ab24016ae21/21) sends
`[sub:u8 = 1][own_id:u32][enable:u8][ping_ms:u16][stats_ms:u16]` (R; DISASM cshell+0x3a59d..0x3a5f6), the same shape
as the server's enabling message of 5.8, PLAUSIBLY a request to switch the statistics on. DeCombat drops it.

## 9. Server answers: cross-reference to the server -> client catalogue

Server -> client names are those of `GAME_MESSAGES_2009_S2C.md` (written in parallel; section numbers there are
cited as `S2C §n`; ids whose section was not yet written are cited by id and stage). "Same id" pairs are noted
because a server implementer will meet them in both directions.

| client message | server must / may answer with | same id in both directions? | grade of the pairing |
|---|---|---|---|
| (none, server first) | server 0x72 (stage D, body ignored) asks for `UP_PLAYER_INFO` | yes: 0x72 request / 0x72 answer | PROVEN (cshell+0x23c620 -> slot +0x08) |
| `UP_PLAYER_INFO` 0x72 | roster to the new client: `GS_PLAYER_INFO` 0x6b for every player (S2C §3.2.1), `GS_MATCH_SETTINGS` 0x7d (S2C §3.2); the new player's 0x6b to the others | | PLAUSIBLE |
| (none, server first) | server 0x73 (stage D, body ignored) asks for `UP_CLIENT_OPTIONS` | no (0x73 -> 0x85) | PROVEN (cshell+0x23c610 -> cshell+0x23c8e0) |
| `UP_CLIENT_OPTIONS` 0x85 | nothing | | |
| `UP_WORLD_READY` 0x87 | match state (`GS_MATCH_BEGIN` 0x45 or `GS_ROUND_BEGIN` 0x44, `GS_TEAM_SCORE` 0x4a, S2C §2.1) and the spawn set (row "respawn") | | PLAUSIBLE |
| `UP_PLAYER_UPDATE` 0x64 | nothing per message; corrections only by `GS_MOVER_UPDATE` 0xa0 sub-flag 0x0020 (S2C §4.2); other clients see the player through the engine object stream | | PROVEN (client side) |
| `UP_MOVE_EVENT` 0x76 sub 1 | nothing; optional fall damage through `GS_DAMAGE_TAKEN` 0x74 (from the 0x64 trailer, not from event 3); the reference server relays a jump as S2C 8.2 type 0x19 `k` 0x0f | | REFERENCE (relay) |
| `UP_SPRINT_STATE` 0xca | **S2C 0xca `[allowed]` to the sender for every change of the `sprinting` bit** (`GS_SPRINT_ALLOWED`, S2C 3.2) | yes, a different layout and not a relay | PROVEN-CAPTURE (the client sprints only after `[1]`) |
| `UP_FIRE` 0x82 | victim: `GS_DAMAGE_TAKEN` 0x74 and `GS_PLAYER_STATUS` 0x66 (health kind, S2C §4.3); on a kill: `GS_KILL` 0x6d to all (S2C §3.3.1), victim `GS_PLAYER_STATE` 0x67 state 2 (S2C §4.1), (0xc8 is not sent, S2C 3.3), team totals `GS_TEAM_SCORE` 0x4a; shot effect to the others (shell object message, effect id UNPROVEN) | | PLAUSIBLE |
| `UP_RELOAD` 0x83 | nothing (server books ammunition); a refill uses `GS_PLAYER_USE` 0xd8 sub 7 | | PLAUSIBLE |
| `UP_WEAPON_CHANGE` 0x7f | nothing to the sender | 0x7f server -> client is `GS_WEAPON_SWITCH` (forced switch, different layout, S2C §4.4) | PROVEN (layouts differ) |
| `UP_WEAPON_SOUND` 0x80, `UP_WEAPON_LOOP` 0x81 | relay to the other players (the reference server: S2C 8.2 type 0x19 `[k = sound_no][weapon:u16][player:u32][pos]`, unreliable, to everyone incl. the sender) | | REFERENCE (accepted); 2009 form UNPROVEN |
| `UP_WEAPON_EVENT` / `UP_DEFLECT` 0x76 sub 3 / 5 | nothing (the reference server sent no relay for category 3) | | REFERENCE |
| `UP_WEAPON_PIECE` 0xd5 | relay as server 0xd5 to the others | yes: S2C `GS_OBJECT_LABEL` 0xd5 `[obj:objref16][flag:bool1][label:zstr8(65)]` has the same shape; note for the S2C author: the client side sends a weapon model piece name, so the server message is PLAUSIBLY the piece visibility relay rather than a name tag | PLAUSIBLE |
| `UP_WEAPON_TOGGLE` 0xcc | toggle relay UNPROVEN (never sent in a capture) | | UNPROVEN |
| `UP_THROW_HOLD` 0x4d (then the release `UP_FIRE` 0x82 of the throwable) | nothing for 0x4d; the thrown object is created from the 0x82 as an engine object (kind-1 object, type 0x13 blob), relay type 0x19 `k` 0x12, blast type 0x06 | | PROVEN-CAPTURE (client side), REFERENCE (server) |
| `UP_RESPAWN_REQUEST` 0x89 / 0xc5 / 0xd2 | the respawn set: `GS_MOVER_UPDATE` 0xa0 (0x0020 teleport, new `move_seq`; 0x0001 body when the character is new), `GS_VIEW_ANGLES` 0x6a kind 0, weapons `GS_WEAPON_GIVE` 0xb2 / `GS_WEAPON_EQUIP` 0xb3, ammunition `GS_PLAYER_STATUS` 0x66 kind 1, then `GS_PLAYER_STATE` 0x67 state 1. The request is the respawn **trigger** (no server timer in the self-arming classes, DSV-32, D); the reference stack's answer came 30-80 ms after the request (REFERENCE) | | PLAUSIBLE (order: DeCombat default) |
| `UP_OBSERVE_TARGET` 0x60, `UP_OBSERVE_FREE` 0x61 | optional spectator data (S2C, spectator HUD) | | UNPROVEN |
| `UP_CHAT` 0x68 (both senders) | `GS_CHAT` 0x68 to the audience including the sender (S2C §2.4) | yes, different layouts (the server adds name and id) | PROVEN layouts, PLAUSIBLE pairing |
| `UP_RADIO` 0xbe | `GS_POINT_SOUND` 0xbe `[pos:pos48][set:u8][kind:u8][c:u8][d:u8]` to the audience (S2C §3.6): the client message without its `to_all` bit (`set` = `radio_set`, `kind` = `entry`, `c` = `variant`, `d` = `voice`; both sides use the radio table object VA 0x375790d4) | yes | PLAUSIBLE (shape and table match) |
| `UP_ROSTER_QUERY` 0x8c | `GS_PLAYER_INFO` 0x6b (kind 2) for the id (all players for 0xFFFFFFFF) | | PROVEN-CAPTURE (trigger and pid), REFERENCE (answer) |
| `UP_PERF_REPORT` 0xc3 sub 2 / 3 | `GS_PERF_STATS` 0xc3 sub 2 (relay to all) / sub 3 (echo to the sender); sub 1 from the server enables the reports | yes | PROVEN (client handler arithmetic) |
| `UP_INTEGRITY_REPLY` 0xac | only after a server 0xac challenge; DeCombat sends none | yes: 0xac challenge / 0xac reply | PROVEN |
| `UP_LEAVE_MATCH` 0xbd | nothing; drop the player, `GS_PLAYER_LEFT` 0x6c to the others (S2C §3.2) | | PLAUSIBLE |
| `UP_USE` 0x59 | mode rule; server `GS_PLAYER_NOTE` 0x5a kind 0 / 3 sets / clears the latch that lets the client send phase 1 (`local_player`+0x487, S2C §4.4) | | PROVEN (shared field +0x487) |
| `UP_USE_HOLD` 0xd8 | `GS_PLAYER_USE` 0xd8 sub 5 (use state of the object), subs 6 / 8 (end) | yes, different sub-ids | PLAUSIBLE |
| `UP_USE_PROGRESS_END` 0xad | mode rule; server 0xad is `GS_TRACK_OBJECT` (different layout) | yes, different layouts | layouts differ |
| `UP_SCAN` 0xcd, `UP_SPOTTED` 0x84 | server 0x84 (stage F) to the team; server 0xcd (stage F, `[kind:u8][x:u8]` then kind 2 / kind 1 fields) | yes | UNPROVEN |
| `UP_VOTE_ANSWER` 0xb8, `UP_VOTE_START` 0xce | vote messages of stage F (0xb7 request, 0xb9 / 0xbb / 0xcf / 0xd0 results; S2C stage F section) | no | PLAUSIBLE |
| `UP_ACK_7E` 0xd1, `UP_ACK_B6` 0xba | nothing (they acknowledge server 0x7e / 0xb6) | no | PROVEN |
| `UP_RESPAWN_TOKEN` 0xdb | `GS_RESPAWN_TOKEN` 0xdb sub 6 (accept: token stored, respawn wait armed) or sub 7 (reset) | yes | PROVEN client side |
| `UP_LEVEL_EXIT_ACK` 0x99 | the next `S2C_LOAD_WORLD` | yes: 0x99 exit request / 0x99 ack | PROVEN (client side) |
| `UP_LEVEL_LOADED` 0x96, `UP_LEVEL_REQUEST` 0x98 | mission flow, not in TDM | no | UNPROVEN |
| developer messages (section 8) | none; dropped | | |

## 10. Appendix A: Team Deathmatch round walkthrough

The ordered exchange for one TDM match of one round, from the client's join to the result screen, as the 2009 client
expects it. **C>** = client -> server game message of this spec, **S>** = server -> client game message
(`GAME_MESSAGES_2009_S2C.md`), **E>** = server -> client and **E<** = client -> server engine-level messages
(`UDP_SESSION_2009_A.md` / `_B.md`). Every
game message travels on the reliable channel except `UP_PLAYER_UPDATE` (unreliable) and the engine object stream.
**[DC]** marks a value or decision that the 2009 client does not fix and a DeCombat default decides (`CD-nn` =
`DC_DEFAULTS.md`; `DC-C2S-n` = a proposal of section 12). Steps without a client-side proof of their order are
marked (order PLAUSIBLE).

```
client                                   server
  |  E< HELLO ......................... >|  A1 engine join, world load
  |< S2C_VERSION, CLIENT_ID, FILE_LIST . |
  |< EVARs, S2C_LOAD_WORLD ............. |
  |  E< LOAD_STATE(0) / preload / (1) .>|
  |< object creates, S2C_VIEW_OBJECT ... |
  |< S> 0x72 ........................... |  A2 info request
  |  C> 0x72 UP_PLAYER_INFO, 0x85 ...... >|
  |< S> 0x6b x n, 0x7d, shell 0x88 ..... |  A3 roster, settings
  |< S> 0x67 state 3 ................... |  A4 life state (loading ends)
  |  C> 0x87 UP_WORLD_READY ............ >|
  |< S> 0x45 (or 0x44), 0x4a, 0xc3/1 ... |  A5 match / round start
  |< S> 0xa0, 0x6a, 0xb2/0xb3, 0x66, 0x67=1 | A6 spawn
  |  C> 0x64 stream (U), 0x76, 0x7f .... >|  A7 play
  |  C> 0x82 per pellet, 0x83 .......... >|  A8 fire / hit
  |< S> 0x74, 0x66 (victim) ............ |
  |< S> 0x6d, 0xc8, 0x4a, 0x67=2 ....... |  A9 kill, kill feed, death
  |  C> 0x60 / 0x61 (observer) ......... >|  A10 death camera
  |  C> 0x89 (client-armed, ~7 s) ...... >|  A11 respawn request
  |< S> A6 set again, 0x67=1 ........... |  A11 respawn [DC]
  |< S> 0x41 / 0x48, 0x46 .............. |  A12 round end
  |< S> 0x65 x n, 0xb5, 0x3f ........... |  A13 match end, result screen
  |  E< BYE ............................ >|  client returns to the lobby
```

**A0 Preconditions (lobby).** The room is in the TDM mode; teams were assigned in the lobby (**[DC] CD-51** team on
entry, **CD-56** room limits); the host started the match (**[DC] CD-23** start pacing, **CD-24** who may start,
**CD-49** joining a running match). The lobby pushed the game server address (`PROTOCOL_2009.md` §7, 0x3043) and
handed the roster to the game server (`DSERVER_PIPE.md`; **[DC] CD-10** port range). The client derives `mode_class`
from the room's mode row at run time; for the EU-test TDM row it selects the 0x89 respawn branch (section 3.4,
OBSERVED).

**A1 Engine join and world load** (`UDP_SESSION_2009_A.md` §5 and §9). E< `C2S_HELLO` (5); the server identifies
the player by the UTF-16 name of the hello blob (**[DC] CD-60**). E> `S2C_VERSION` with the host rate (**[DC]
CD-58**), `S2C_CLIENT_ID` (the engine client id is the `pid` of every roster message and must stay below 256,
S2C §3.1), `S2C_FILE_LIST` (**[DC] CD-59**), `S2C_EVAR`s, `S2C_LOAD_WORLD` (the world from the room's map, read
from the ROM at run time). E< `C2S_LOAD_STATE(0)`, preload, E< `C2S_LOAD_STATE(1)`. The server creates the
player's character object (**[DC] CD-66** object ids) and the other players' objects, then E> `S2C_VIEW_OBJECT`.
Start-up deadline **[DC] CD-61** (120 s). Tick rate **[DC] CD-63** (10 Hz), send order **[DC] CD-65**, sounds
**[DC] CD-62**, own view object **[DC] CD-67**.

**A2 Player info.** S> server 0x72 (stage D, no body). C> `UP_PLAYER_INFO` 0x72 (section 5.1), immediately followed
by C> `UP_CLIENT_OPTIONS` 0x85 (5.2). The server checks `char_no` / `room_slot` against the lobby roster and keeps
the lobby's team and loadout (**[DC] DC-C2S-5**; mismatch: drop with a bye code as in CD-60).

**A3 Roster and settings.** S> `GS_PLAYER_INFO` 0x6b `kind` 1 for every player including the new one (S2C
§3.2.1; the own record sets the client's team; `character` must name an object created in A1); to the other
clients the new player's 0x6b with `kind` 0 (join notice). S> `GS_MATCH_SETTINGS` 0x7d (goal and team counters,
S2C §3.2; the goal is the room's goal setting from the lobby). S> shell 0x88 with state 2 or 3
(`UDP_SESSION_2009_A.md` §4.11; which value: **[DC]** 2). Order within A3: PLAUSIBLE.

**A4 Life state, end of loading.** S> `GS_PLAYER_STATE` 0x67 with a non-zero state (S2C §4.1): the client's in-world
test needs it (`UDP_SESSION_2009_A.md` §8.3 d). **[DC] DC-C2S-10'** (rewritten 2026-10-25, Appendix F.2): the spawn set of A6 with state 1
may be sent here, before 0x87, which is the path the real client was observed to accept (PROVEN-CAPTURE, S2C 3.2.2; DSV-36 does so). *Superseded:*
"DC-C2S-10: send state 3 (waiting) here, not 1, so the player is not playable before the match start" (still accepted by the in-world test,
but untested with the real client). When the test passes, the client closes its loading screen and sends C> `UP_WORLD_READY` 0x87 once (5.4).

**A5 Match and round start.** S> `GS_MATCH_BEGIN` 0x45 for the first round a player sees (S2C §2.1.1:
`round_secs` = the room's time limit, `score_goal` = the room's goal; `countdown_secs` **[DC]** 0 for TDM (no
pre-round countdown), `show_intro` 1 with the intro block from the map's ROM data) or `GS_ROUND_BEGIN` 0x44 for a
later round (S2C §2.1); S> `GS_TEAM_SCORE` 0x4a with the current team kills; S> `GS_PERF_STATS` 0xc3 sub 1 to enable
ping reports (**[DC] DC-C2S-7**). A player who joins a running match (CD-49) gets the same messages with the
remaining time.

**A6 Spawn placement.** The server picks a spawn point of the player's team (**[DC] DC-C2S-12**) and sends, in this
order (**[DC] DC-C2S-10**; the client accepts each independently, order PLAUSIBLE):

1. S> `GS_MOVER_UPDATE` 0xa0 with sub-flag 0x0001 (body) when the character is new and 0x0020 (teleport: new
   `move_seq`, position, box) (`MOVEMENT_2009.md` §12.4);
2. S> `GS_VIEW_ANGLES` 0x6a kind 0 (facing of the spawn point);
3. S> `GS_WEAPON_GIVE` 0xb2 for each loadout weapon and `GS_WEAPON_EQUIP` 0xb3 for the drawn one (S2C §4.4; the
   loadout is the lobby's, **[DC] CD-18 / CD-48 / CD-52**);
4. S> `GS_PLAYER_STATUS` 0x66 for health, armour and ammunition (S2C §4.3; the start values come from the ROM
   character and gear tables at run time, **[DC] DC-C2S-11** names the hook);
5. S> `GS_PLAYER_STATE` 0x67 state 1.

The client enables movement and starts C> `UP_PLAYER_UPDATE` 0x64: one reliable state form when its controller
state byte changed, then the unreliable form at up to 15 per second (2.1).

**A7 Play.** C> 0x64 stream: the server moves the player's server object, validates (DeCombat policy,
`MOVEMENT_2009.md` §12.4) and corrects only with S> 0xa0 / 0x0020; updates with an old `move_seq` are ignored.
C> `UP_MOVE_EVENT` 0x76 sub 1 and `UP_SPRINT_STATE` 0xca (no answer). C> `UP_WEAPON_CHANGE` 0x7f (no answer; the
server records the weapon). C> `UP_WEAPON_SOUND` 0x80 / `UP_WEAPON_LOOP` 0x81 / `UP_WEAPON_EVENT` 0x76 sub 3 (relay,
format UNPROVEN). C> `UP_CHAT` 0x68 -> S> `GS_CHAT` 0x68 to the audience including the sender (**[DC] CD-30**).
C> `UP_RADIO` 0xbe + 0x68 -> S> `GS_POINT_SOUND` 0xbe + `GS_CHAT` 0x68. C> `UP_PERF_REPORT` 0xc3 sub 3 -> S> echo
sub 3; sub 2 -> S> sub 2 to all.

**A8 Fire and hit.** C> `UP_FIRE` 0x82, one message per pellet with `last` set on the last (4.1); C> `UP_RELOAD`
0x83 (4.2). The server checks ammunition (**[DC] DC-C2S-4**), reconstructs each hit point as `origin + hit_dist *
dir`, validates it against the victim's server-side box and computes damage (**[DC] DC-C2S-11**: weapon values from
the ROM weapon table at run time, hit-zone factors and tolerance are DeCombat policy). On damage: S> to the victim
`GS_DAMAGE_TAKEN` 0x74 (direction, zone, amount; S2C §4.3) and `GS_PLAYER_STATUS` 0x66 with the new health; the
shot effect for the others (shell object message, effect id UNPROVEN).

**A9 Kill, kill feed, death.** (Observed with the reference server, accepted by the real client: after each 0x6d the team kill totals travel in S> 0xb5 `[kills team 1][kills team 2]`, and 0x4a / 0xc8 were never sent; the kill messages precede the
victim's own 0x74 / 0x66 / 0x67 of the same hit. S2C 3.3.2.) When health reaches 0: S> `GS_KILL` 0x6d to every client (S2C §3.3.1: victim,
killer, weapon, `hit_zone` 1 for a head shot; the clients keep their own kill / death counters from this stream);
S> *(corrected by spec-fold-9)* 0xc8 is a combo banner the reference stack never sent and DeCombat does not send; the multi-kill level (the streak rule of DC-C2S-13) rides the `kill_kind` of the 0x6d (S2C 3.3.1);
S> `GS_ROUND_WINS` 0xb5 with the new team kill counts (DeCombat: never 0x4a, DSV-33 / DC-KILL-3, D; the older text named 0x4a here);
S> `GS_PLAYER_STATE` 0x67 state 3 then state 2 to the victim in one block (death camera; DSV-32, D). Reward counting for later: **[DC] CD-03 / CD-04** (per kill), computed by the lobby (**CD-54**).

**A10 Death camera.** In states 2 / 3 the client may send C> `UP_OBSERVE_TARGET` 0x60 and C> `UP_OBSERVE_FREE` 0x61
(3.5). A DeCombat server may ignore both (no spectator feed in DRAFT 1).

**A11 Respawn.** The 2009 client requests the respawn itself in TDM: about 7000 ms after it processed S> 0x67 state 2 it sends C> `UP_RESPAWN_REQUEST` 0x89 once (3.3, 3.4; DISASM cshell+0x60ac0, class 10: 1500 ms and 0xd2, classes 2, 3, 11,
12, 13: never). **[DC] DSV-32** (refines DC-KILL-1): the server answers a due request at once with the A6 spawn set and keeps no respawn timer in the classes whose client asks
(a fallback timer only in classes 5 and 9). **[DC] DC-C2S-10'**: at the death S> 0x67 state 3 and state 2 in one block (the client's death time is set by state 2 alone, so both
orders arm the wait); the server sends no arming message (0xd7 sub 2, 0xdb sub 6) in TDM. A 0x89 from a living player, one earlier than the wait minus 250 ms, or one after the
round end is logged and ignored (DSV-32; supersedes DC-C2S-3). *Superseded:* "DC-C2S-10: after 2000 ms in state 2 the server may send state 3; respawn 7000 ms after the death".

**A12 Round end.** (Observed with the reference server: goal end 0x46 with the deciding kill, 3.0 s later 0x47 / 0x6b refresh / 0x65 / 0x48, 3.0 s later 0x3f; forfeit end 0x6c + 0x46 0.3 s after the leaver's 0xbd; no 0x99, no server bye:
S2C 2.1.3.) **[DC] DC-C2S-13**: the round ends when a team reaches the goal or the round timer runs out
(equal scores at time-out: draw), or when a team has been empty for 10 s (DSV-31, D). DeCombat sends the observed set (DSV-35, D): S>
`GS_ROUND_RESULT` 0x46 at once, 0x47 / 0x6b refresh / 0x65 / `GS_ROUND_CLOSE` 0x48 3 s later, 0x3f 3 s after that; never `GS_ROUND_TIMER_STOP` 0x41
(the older text sent "0x41 (or 0x48), then 0x46"). Field use of S>
`GS_ROUND_RESULT` 0x46 (`win_team` 1 or 2 plays that team's win banner and sound, 0 shows no team, S2C §2.1.2;
`wins_team1` / `wins_team2` the round wins). A match of several rounds continues with S> `GS_ROUND_BEGIN` 0x44 and
the A6 set for every player (**[DC]** rounds per match = the room setting, default 1).

**A13 Match end and result screen.** S> `GS_PLAYER_SUMMARY` 0x65 for every player (S2C §3.4: experience and GP
before / gained / bonus; the values are the lobby's rule **[DC] CD-03 / CD-04 / CD-54**, so the game server either
asks the lobby through the pipe before sending 0x65 or applies the same hook; which order the 2009 servers used is
UNPROVEN); S> `GS_ROUND_WINS` 0xb5; S> `GS_MATCH_RESULT_SCREEN` 0x3f: the client opens its result screen (screen 7),
sets its leaving latches and leaves the game server; the engine disconnect follows (E< `C2S_BYE`). The game server
reports the result to the lobby (`DSERVER_PIPE.md`), and the lobby pushes the new totals (0x3004, rank-up **CD-41**). DeCombat: the lobby's gains reach
the 0x65 only through the `gain` pipe line (`DSERVER_PIPE.md` 12.B, not yet sent by `dc-lobby-server`: 0x65 shows 0 gains today); the server sends no bye
and no 0x99 after 0x3f (DSV-35, D): leaving is the client's (PLAUSIBLE from the 0x3f latches, S2C 2.1).

**A14 Leaving early.** C> `UP_LEAVE_MATCH` 0xbd (5.7): the server removes the player at once and sends S>
`GS_PLAYER_LEFT` 0x6c to the others. A client that times out (no input for 300 s, 3.6) only disconnects.

## 11. Appendix B: names registered for `GLOSSARY_UDP.md`

Proposed rows for a section S.C (client -> server game messages). Each name is ours; the id is the wire fact.

| name | id (sub) | meaning (grade) | section |
|---|---|---|---|
| `UP_` | | prefix of every client -> server game message in this spec | 1.2 |
| `UP_PLAYER_UPDATE` | 0x64 | body, view and control stream; reliable 4-byte state form (PROVEN layout) | 2.1 |
| `pu_flags` | 0x64 field | flag word announcing the optional blocks of 0x64 | 2.1 |
| `UP_MOVE_EVENT` | 0x76 sub 1 | jump / fall / landing notice (PLAUSIBLE) | 2.2 |
| `UP_SPRINT_STATE` | 0xca | sprint started / stopped (PROVEN-CAPTURE; the server answers with S2C 0xca `GS_SPRINT_ALLOWED`) | 2.3 |
| `life_state` | client field | `local_player`+0x30, set by server 0x67 | 3.2 |
| `UP_RESPAWN_REQUEST`, `_B`, `_C` | 0x89, 0xc5, 0xd2 | respawn wait over, by game type (PLAUSIBLE) | 3.4 |
| `UP_OBSERVE_TARGET`, `UP_OBSERVE_FREE` | 0x60, 0x61 | observer camera target / free camera (PLAUSIBLE) | 3.5 |
| `UP_FIRE` | 0x82 | one shot pellet with the client's trace result (PROVEN layout) | 4.1 |
| `UP_RELOAD` | 0x83 | reload with the ammunition id (PLAUSIBLE) | 4.2 |
| `UP_WEAPON_SOUND`, `UP_WEAPON_LOOP` | 0x80, 0x81 | animation-key weapon sounds for others (PLAUSIBLE) | 4.3 |
| `UP_WEAPON_CHANGE` | 0x7f | weapon switch: slot, weapon, aux (PROVEN layout) | 4.4 |
| `UP_WEAPON_EVENT`, `UP_DEFLECT` | 0x76 sub 3, sub 5 | weapon animation notices (UNPROVEN / PLAUSIBLE) | 4.5 |
| `UP_WEAPON_TOGGLE`, `UP_WEAPON_PIECE`, `UP_THROW_HOLD` (formerly `UP_THROW_RELEASE`) | 0xcc, 0xd5, 0x4d | weapon accessory state, model piece visibility, throw hold (the release is the next `UP_FIRE`) | 4.6 |
| `UP_PLAYER_INFO` | 0x72 | answer to the server's 0x72: profile, room rules, loadout, clan (PROVEN layout) | 5.1 |
| `look` | 0x72 block | 15 bytes and two strings of the character look record (the same shape as the 0x6b look block) | 5.1 |
| `UP_CLIENT_OPTIONS` | 0x85 | two client options (UNPROVEN meanings) | 5.2 |
| `UP_PLAYER_INFO_OLD` | 0x33 | dead variant of 0x72 | 5.3 |
| `UP_WORLD_READY` | 0x87 | loading screen closed (PLAUSIBLE) | 5.4 |
| `UP_ROSTER_QUERY` | 0x8c | missing roster record (PLAUSIBLE) | 5.5 |
| `UP_INTEGRITY_REPLY` | 0xac | answer to a server integrity challenge (PROVEN layout) | 5.6 |
| `UP_LEAVE_MATCH` | 0xbd | the player confirmed leaving (PROVEN trigger) | 5.7 |
| `UP_PERF_REPORT` | 0xc3 subs 1-3 | ping echo request and frame-rate statistics (PROVEN) | 5.8 |
| `UP_CHAT` | 0x68 | chat line with scope (PROVEN layout) | 6.1 |
| `UP_RADIO` | 0xbe | radio command with position (PROVEN layout) | 6.2 |
| `UP_USE`, `UP_USE_HOLD`, `UP_USE_PROGRESS_END` | 0x59, 0xd8, 0xad | use and hold-to-use actions (PLAUSIBLE) | 7.1, 7.2 |
| `UP_SCAN`, `UP_SPOTTED` | 0xcd, 0x84 | scanner item and spotted enemies (PLAUSIBLE) | 7.3 |
| `UP_LEVEL_LOADED`, `UP_LEVEL_REQUEST`, `UP_LEVEL_EXIT_ACK` | 0x96, 0x98, 0x99 | mission flow of `mode_rules` | 7.4 |
| `UP_VOTE_ANSWER`, `UP_VOTE_START` | 0xb8, 0xce | vote answer and vote start (PLAUSIBLE) | 7.5 |
| `UP_ACK_7E`, `UP_ACK_B6` | 0xd1, 0xba | acknowledgements of server 0x7e / 0xb6 (PROVEN) | 7.5 |
| `UP_RESPAWN_TOKEN` | 0xdb sub 5 | use a respawn token (PLAUSIBLE) | 7.6 |
| `UP_STATE_71` | 0x71 | sender without caller (UNPROVEN) | 8.1 |
| `UP_LOCAL_CHEAT` | 0x69 | local-world cheat toggles | 8.2 |
| `UP_DEV_ATTACH` | 0x8b | weapon attachment developer commands | 8.4 |
| `UP_DEV_TRACE` | 0x75 | developer aim-trace report | 8.6 |
| `weapon`, `client_profile` | client objects | method table VA 0x374cec2c / global VA 0x3759b9d8 | 1.2 |
| `is_network_game()`, `observer_camera()` | client predicates | cshell+0x1a0470, cshell+0x1f0840 | 1.2 |

## 12. UNPROVEN items, capture list and DeCombat default proposals

### 12.1 UNPROVEN and open items

| # | item | why open | how to close |
|---|---|---|---|
| U1 | meanings of most `UP_FIRE` fields (`wkind`, `frac`, `shot_no`, `extra_*`, `flag_c`, `stat`, `aim`) | **partly settled by tdm1 / tdm2 / tdm4 (4.1, 4.1.1):** `wkind` 0, `frac` = spread x 255, `shot_no` is an f32 per weapon, `aim` = the view rotation, `extra_obj` / `extra_a` / `extra_b` of a hit = the victim, the hit point, the entered face's normal (4.1.1 correction of 2026-10-26); `extra_kind` = the node pick (4.1, 4.1.1 correction); `stat`, multi-pellet weapons still open | capture V-C1 with a shotgun |
| U2 | `look` bytes, `r1`..`r7`, `word_d`, `clan_word_c`, `flag_e`, `flag_f` of `UP_PLAYER_INFO` | values recorded for one character (5.1.1; `r7` is an f32 bit pattern); meanings need a second character with another appearance | compare a capture (V-C2) with the character appearance and the lobby profile |
| U3 | the two options of `UP_CLIENT_OPTIONS` | no writer of `client_profile`+0x1d0 / +0x1de found in the code read | capture after toggling each option screen setting |
| U4 | events 1 and 3 of `UP_WEAPON_EVENT` and the meaning of `value` | event 1 + `value` follows every reload (30 / 15 per weapon, PLAUSIBLY the clip size); events 3 and 5 never occurred | capture V-C3 with a 20 s idle |
| U5 | which server message relays 0x80 / 0x81 / 0x76 / 0x7f / 0xcc to other clients | **decided for the reference stack form that the real client accepts** (S2C 8.2): 0x80 -> type 0x19, jump -> type 0x19 `k` 0x0f, 0x82 -> type 0x06, 0x7f -> `CF_FILES` on the weapon child, 0x76 category 3 -> none; 0x81 and 0xcc never sent. The 2009 server's own form stays unknown | none possible without a 2009 server |
| U6 | who calls `UP_STATE_71` (cshell+0x1f3770), `UP_DEV_TRACE` (`local_player` slot +0x38), the 0x96 / 0x98 paths | no static reference found | decompiler re-run with indirect-call analysis, or a debugger breakpoint during a session |
| U7 | other writers of `local_player`+0x487 besides server 0x5a (S2C §4.4) | needed for `UP_USE` phase 1 | data-reference scan for +0x487 stores |
| U8 | `UP_USE_HOLD` sub 2 trigger (HUD element VA 0x374d88b0) | state machine of that element not traced | capture in a mode with use objects |
| U9 | the integrity words of 0xac sub 2 | deliberately not reproduced (not needed) | none (DeCombat never challenges) |
| U10 | `pu_flags` bits 0x0004 / 0x0008 / 0x0800 / 0x1000 / 0x2000 meanings | 0x0800 never set, 0x1000 always set, 0x2000 with repeat fire and a held throwable (2.1.1); 0x0004 / 0x0008 need a camera-mode change | capture with a camera-mode change |
| U11 | base-variant 0x64 (cshell+0x1fa060) never running | **closed (PROVEN-CAPTURE):** all 5,285 unreliable updates of three captures parse with the derived layout to the last bit | none |
| U12 | whether the null-slot case of the 0x72 loadout lists can happen | settled for the observed loadout (4 entries written for `n` 4); the general case stays open | capture V-C2 with another loadout |

### 12.2 Captures to take (prepared, not run in this wave)

| id | scenario | expected (this spec) |
|---|---|---|
| V-C1 | one shot with a pistol, one with a shotgun at a wall, then at a player | one 0x82 per bullet / pellet, `last` = 1 only on the final one; `has_hit` = 1 for the player shot, `hit_dist` equal to the distance; `seed` in 2..255 |
| V-C2 | join a TDM room, clan member and non-member | server 0x72 -> client 0x72 then 0x85; `n` loadout entries; `in_clan` bit as expected; fields 1-4 equal to the lobby values |
| V-C3 | reload, switch weapons, idle 20 s | 0x83 with the ammunition id; 0x7f with slot and weapon; 0x76 sub 3 events 2 and 5 |
| V-C4 | chat to all, to team, one radio command | 0x68 with scope 0 / 1; radio 0xbe followed by 0x68 |
| V-C5 | wait for loading to end | exactly one 0x87, after the server's 0x67 |
| V-C6 | stand still, walk, sprint, jump | 0x64 rate about 15 per second when moving; 0xca (1, 0) then (0, 1); 0x76 sub 1 events 1 and 3 |

Captures use `evidence/captap.py` (tap mode) against a DeCombat server; the client is driven by a human
(`captures/README.md`).

**Results (tdm1, tdm2, tdm4).** V-C1 partly: one 0x82 per shot for one-vector weapons with `last` = 1, `seed` 4..255, `has_hit` 1 against the bot with `hit_dist` and the extra block (4.1.1); no shotgun, so pellets are open.
V-C2 passes for a non-member, a second login and a re-join (5.1.1); clan member open. V-C3 partly: 0x83 with the ammunition id, 0x7f layout, 0x76 [3][2] with every change, event 1 + value after reloads; event 5 not seen (no idle).
V-C4 scope 0 passes; scope 1 and the radio are open. V-C5 passes on every link (5.4). V-C6 passes (rate 15 per second moving or standing; 0xca (1, 0) / (0, 1) / (0, 0); 0x76 sub 1 events 1, 2, 3 with the timing of 2.2).

### 12.3 DeCombat default proposals (for `DC_DEFAULTS.md`, ids to be assigned by the coordinator)

| proposal | rule | default | 2009 client evidence it must satisfy |
|---|---|---|---|
| DC-C2S-1 | Developer, console and local-world messages (0x33, 0x69, 0x6e, 0x75, 0x8b, 0xa7, 0xa8, 0xa9, 0xae, 0xb0, client 0xc3 sub 1) are dropped and logged; an operator option may enable 0xa7 / 0xa8 for game-moderator accounts only (extends CD-64) | drop | none of them is needed by the client to play (sections 5.3, 8) |
| DC-C2S-2 | `UP_PLAYER_UPDATE` is parsed with the derived (live) layout only | derived layout | section 2.1 |
| DC-C2S-3 (SUPERSEDED 2026-10-25 by DSV-32) | *old:* A respawn request (0x89 / 0xc5 / 0xd2) before the server's respawn delay is ignored; after it, the player is respawned at once. *Now:* there is no server delay in the self-arming classes; a request no earlier than the class wait minus 250 ms after the death is answered at once, earlier / living / post-round requests are ignored (DSV-32) | 250 ms guard | the client sends the request once per armed wait (section 3.4) |
| DC-C2S-4 | The server mirrors clip and stock per weapon from `GS_WEAPON_GIVE` / `GS_PLAYER_STATUS` and `UP_RELOAD`; `UP_FIRE` reports beyond the clip do no damage | on | 0x83 carries the ammunition id, 0x82 the weapon and ammunition ids (sections 4.1, 4.2) |
| DC-C2S-5 | Identity, team and loadout come from the lobby roster; `UP_PLAYER_INFO` is used to identify (`char_no`, `room_slot`) and is otherwise logged | as stated (extends CD-60) | section 5.1 |
| DC-C2S-6 | The server never sends the 0xac integrity challenge | never | the client answers only when asked (section 5.6) |
| DC-C2S-7 | Statistics reports are enabled after `UP_WORLD_READY` with server 0xc3 sub 1; ping period 2000 ms, statistics period 5000 ms; sub 3 echoed, sub 2 relayed; the reported pings feed `GS_PING_TABLE` 0xa2 | on, 2000 / 5000 ms | section 5.8 |
| DC-C2S-8 | Player votes (0xce / 0xb8) are off: a vote start is refused | off | section 7.5 |
| DC-C2S-9 | Respawn-token use (0xdb sub 5) is refused until a token item rule exists | refuse (server 0xdb sub 7) | section 7.6 |
| DC-C2S-10 (SUPERSEDED 2026-10-25) | *old:* Spawn and respawn sequence: before `UP_WORLD_READY` the player gets `GS_PLAYER_STATE` 3 and shell 0x88 state 2; the spawn set is 0xa0 (body, teleport) -> 0x6a -> 0xb2 / 0xb3 -> 0x66 -> 0x67 state 1; after a death: 0x67 state 2 at once, state 3 after 2000 ms, respawn 7000 ms after the death | as stated | the in-world test needs a non-zero state before 0x87 (5.4); 7000 ms is the client's own respawn countdown (3.3) |
| DC-C2S-10' (rewritten 2026-10-25, staged) | Spawn and respawn sequence: shell 0x88 state 2 at the link and the spawn set (0xa0 body / teleport, 0x6a, 0xb2 / 0xb3, 0x66, 0x67 state 1) before `UP_WORLD_READY` (state 3 first stays allowed, untested with the real client); after a death 0x67 state 3 and state 2 in one block; **the client arms its own wait on state 2 and asks with 0x89 / 0xc5 / 0xd2; the server respawns on that request** (DSV-32), never on its own timer in classes 1, 4, 6, 7, 8, 10 | as stated | in-world test (5.4) passed with state 1 before 0x87 (PROVEN-CAPTURE, S2C 3.2.2); client arming DISASM cshell+0x60ac0 (3.3); reference stack respawn on request (REFERENCE, S2C 4.1.1) |
| DC-C2S-11 | Hit validation and damage: the hit point `origin + hit_dist * dir` must lie within a tolerance of the victim's server-side box at the shot time; damage per weapon from the ROM weapon table at run time; hit-zone factors and tolerance are config values with a Lua hook (`combat.on_hit`) | tolerance and factors to be fixed in a GAME_RULES spec | sections 4.1, A8 |
| DC-C2S-12 | Spawn point: a random spawn point of the player's team that no living enemy can see, else any team spawn point (world spawn objects read from the world file at run time) | as stated | none (server-side only) |
| DC-C2S-13 | TDM round end: first team to the room's goal in team kills, or the round timer; equal scores at time-out = draw (`win_team` 0); multi-kill banner (`GS_MULTI_KILL`) from 2 kills within 4 s. Note 2026-10-25: the end set is DSV-35 and the team-empty end DSV-31; `dc-dserver` slice 4 still sends 0xc8 by this streak rule although the reference stack never sent one (D, the client's banner path is PROVEN, S2C 3.3) | as stated | `GS_ROUND_RESULT` shows a team banner only for 1 / 2 (S2C §2.1.2) |

### 12.4 Corrections and notes for sibling specs

- `UDP_SESSION_2009_B.md` 10.3: the two `pu_flags` rotation fields are `quat128` (writer slot +0x4c); the
  `state:u32` is the mover control word (`move_mgr`+0x24, `MOVEMENT_2009.md` 5.1); the live builder is the derived
  slot +0x100 (cshell+0x5f7f0), the base one (cshell+0x1fa060) is replaced; the 0x20 / 0x40 view flags require a
  client variable above 0.0.
- `MOVEMENT_2009.md` 12.2 names cshell+0x5f710 / cshell+0x1f9e30 as the 0x64 builders; the builders are the regions
  cshell+0x5f7f0 / cshell+0x1fa060 (cshell+0x5f710 is the neighbouring method that sends the local mover message
  0x180).
- `UDP_SESSION_2009_B.md` 11.1 id list: add 0x4d, 0x59, 0x61, 0x69, 0x6e, 0x71, 0x75, 0x80, 0x81, 0x83, 0x84, 0x85,
  0x87, 0x8c, 0x99, 0xa7, 0xa9, 0xb0, 0xb8, 0xdb (20 ids; with its 29 this gives the 49 of section 1.3).
- `GAME_MESSAGES_2009_S2C.md` 3.2 `GS_OBJECT_LABEL` 0xd5: the client sends the same shape with a weapon model piece
  name (4.6), so the server message is PLAUSIBLY the piece-visibility relay; 3.6 `GS_POINT_SOUND` 0xbe is PLAUSIBLY
  the radio relay (6.2: same table object VA 0x375790d4).
- `GAME_MESSAGES_2009_S2C.md` 1.1 proposes "CD-48" for reliable server messages; CD-48 is already the starter
  backpack row of `DC_DEFAULTS.md`, so that proposal needs a new id.

## Appendix C: capture notes (spec-fold-4, 2026-10-25)

Sources (evidence records, kept in full): `CAPTURE_tdm1_GAME_VALIDATION.md` (section 2), `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md` (sections 2-5), `CAPTURE_tdm4_KILL_VALIDATION.md` (sections 2 and 4),
`CAPTURE_tdm4_WEAPON_FORMS.md`, `REFERENCE_FINDINGS_botduel.md` (the bot's 37 respawn requests). Grades as in the header. Client ids of a TDM session and their results:

| id | our name | count (tdm1) | result | grade |
|---|---|---|---|---|
| 0x72 | `UP_PLAYER_INFO` | 1 | exact parse; values equal the lobby's (5.1.1) | PROVEN-CAPTURE |
| 0x85 | `UP_CLIENT_OPTIONS` | 1 | `02 01` after 0x72 | PROVEN-CAPTURE |
| 0x76 | `UP_MOVE_EVENT` / `UP_WEAPON_EVENT` | 642 | layout and codes confirmed; event 3 is not a landing marker (2.2); category 3 events 1 / 2 (4.5) | PROVEN-CAPTURE |
| 0x7f | `UP_WEAPON_CHANGE` | 20 | `aux` = ammunition id | PROVEN-CAPTURE |
| 0x64 | `UP_PLAYER_UPDATE` | 1 R + 2,338 U | exact layout, 15 Hz, `yaw8c` truncation, FIRE bit, eye offsets, aim law (2.1.1) | PROVEN-CAPTURE |
| 0x87 | `UP_WORLD_READY` | 1 | once, 47-109 ms after 0x67 `[1]` | PROVEN-CAPTURE |
| 0xca | `UP_SPRINT_STATE` | 56 | needs the server echo (2.3) | PROVEN-CAPTURE |
| 0x80 | `UP_WEAPON_SOUND` | 22 | animation-key sounds, `own_id` = client id | PROVEN-CAPTURE |
| 0x8c | `UP_ROSTER_QUERY` | 1 | real pid after an unresolved 0x6b (5.5) | PROVEN-CAPTURE |
| 0x4d | `UP_THROW_HOLD` | 1 (tdm4: 18) | repeated while held; the release is a 0x82 (4.6) | PROVEN-CAPTURE |
| 0x82 | `UP_FIRE` | 83 (tdm4: 5, tdm2: 11) | 623 / 887 bits exact; extra block and hit point (4.1.1) | PROVEN-CAPTURE |
| 0x83 | `UP_RELOAD` | 4 | automatic reload after the last shot of a clip | PROVEN-CAPTURE |
| 0x68 | `UP_CHAT` | 2 | scope 0 | PROVEN-CAPTURE |
| 0xbd | `UP_LEAVE_MATCH` | 1 | with TCP 0x394 {0}; bye variants (5.7) | PROVEN-CAPTURE |
| 0x89 | `UP_RESPAWN_REQUEST` | 0 (the real client never died) | rule from DISASM (3.3); 37 of 37 bot requests answered by the reference stack | DISASM + REFERENCE |

Undecided: a death and respawn of the real client; multi-pellet fire; melee; crouch with a held fire key; team chat and radio; the clan-member 0x72; `UP_USE`, votes, observer camera, level messages and every developer message (never sent).

Proposed DC_DEFAULTS rows this fold supports (approval is the user's; ids are the staging ids): DC-KILL-1 (respawn on request), DC-KILL-2 (the 0x74 form), DC-KILL-3 (0xb5 team kills), DC-KILL-4 (sound relays), DC-KILL-5 (0x6b refresh at the match end), CV2-1 .. CV2-8 (character
form, remote stream, relays, sprint echo, view repair, forfeit, roster kinds, removal), GMD-3 correction, CLI-3 (eye offset 33 / 20), BOT-3 / BOT-9 (the aux words and aim law), DC-C2S-10 note (state 3 first is not required).

Count: PROVEN-CAPTURE 14 ids, DISASM + REFERENCE 1 (0x89).

## Appendix F: fold record (spec-fold-5, 2026-10-25)

Sources: `reports/dserver-slice4.md` (DSV-31 .. DSV-38 as implemented), `client-reference stack.md` section 4 (the Rust client's bytes against the real client's PROVEN-CAPTURE
forms), the K-ids of `CAPTURE_tdm4_KILL_VALIDATION.md` section 0 that no base spec cited by id, and `DC_DEFAULTS.md` "Capture notes 2026-10-25" (the corrected respawn model). No new
2009 evidence: the grades below were given in the evidence records and are only placed here.

### F.1 Respawn rows rewritten (Appendix A A4 / A9 / A11 / A12, 3.4, 12.3)

| row | change | grade |
|---|---|---|
| DC-C2S-10 -> DC-C2S-10' | the state 3 before 0x87 and the "state 3 after 2000 ms, respawn 7000 ms after the death" parts are superseded; the spawn set with state 1 before 0x87 and death 3 + 2 in one block; respawn on the client's request | D (rewritten, staged); the client facts PROVEN-CAPTURE (5.4 accepted state 1) and DISASM (3.3 arming) |
| DC-C2S-3 | superseded by DSV-32 (no server delay; 250 ms guard; post-round and living requests ignored) | D |
| DC-C2S-13 | note: end set DSV-35, team-empty end DSV-31; 0x41 not sent | D |
| A9 / A12 | 0x4a replaced by 0xb5 (DSV-33), 0x41 dropped (DSV-35) | D; the observed forms REFERENCE-CONFIRMED (S2C Appendix C) |

### F.2 tdm4 kill items (K-ids) in the client -> server text

| id | content | now in | grade |
|---|---|---|---|
| K1 | 0x82 layout bit for bit (887 / 623 bits); `shot_no` is the float 1.0; the extra block carries the victim object and the hit point | 4.1, 4.1.1 | PROVEN-CAPTURE |
| K2 | one 0x82 per pellet, `last` 1 for a one-vector weapon | 4.1, 4.1.1 | PROVEN-CAPTURE |
| K3 | hit point = `origin + hit_dist * dir` exactly | 4.1.1 | PROVEN-CAPTURE |
| K4 | 0x64 at 15 Hz while firing; control bit 0x400 = fire key held | 2.1.1 | PROVEN-CAPTURE |
| K5 | 0x7f `aux` = the new weapon's ammunition id, always after 0x76 [3][2] | 4.4 | PROVEN-CAPTURE |
| K6 | 0x76 categories (1: events 1 / 2 / 3; 3: event 2); no fire or reload event on the wire | 2.2, 4.5 | PROVEN-CAPTURE |
| K7 | 0x4d repeats while the throw is held; the release is the single 0x82 | 4.6 (`UP_THROW_HOLD`) | PROVEN-CAPTURE |
| K13 | the real client did not die (no 0x89 / 0xc5 / 0xd2 in any capture) | 3.3, 3.4, Appendix C row 0x89 | PROVEN-CAPTURE (absence) |
| K14 | `yaw_c8` truncation over 43.96 rad of yaw (1,103 / 1,103) | 2.1.1 | PROVEN-CAPTURE |

K8-K12, K15, K17 are cited in `GAME_MESSAGES_2009_S2C.md` Appendix F.3, K16 in `PROTOCOL_2009.md` 7.2. The open capture item **V-KILL-1** (the real client killed, waiting without input, then
playing on) would make 3.3 / 3.4 PROVEN-CAPTURE.

### F.3 The Rust client against the real client (`client-reference stack.md` section 4)

The method of that report (both directions of tdm1 / tdm4 and of four Rust-client runs replayed into receiving links) reproduced this spec's PROVEN-CAPTURE numbers on the real captures
(2,338 / 2,338 `yaw_c8` truncations, `aim = Ry*Rx` 1,472 / 1,472, eye 33, keep-alives 7 bytes 203 ms apart): the 2.1.1 rows are independently re-measured (PROVEN-CAPTURE, second reader).
Of its remaining differences these touch this catalogue; each is a property of the real client that the spec now states, or an open item:

| item | real client (PROVEN-CAPTURE unless noted) | spec place | state |
|---|---|---|---|
| L1 | at every spawn 4 x (0x76 [3][2] + 0x7f) before the state form; 0x80 / 0x82 / 0x83 / 0x4d and the 0x0200 `weapon_f` / 0x2000 flags while firing | 4.1-4.6, 2.1.1 | already stated (CAPTURE_tdm4_WEAPON_FORMS); our client has no weapons yet |
| L2 | `surface` 71 / 10 / 20 / 23 on the ground and 200 on ladders; `standing` ids of the floor polygon on the ground, -1 / -1 only airborne | 2.1, `MOVEMENT_2009.md` 12.2 | stated there |
| L3 | body yaw unwrapped (-2.85 .. 10.45 in one session) | 2.1.1 (runaway yaw) | stated |
| L4 | `yaw_c8` at exactly yaw = -pi: a binary32 product gives 127, a double check 126 | 2.1.1 | how the 2009 client rounds at exactly +-pi is not read: UNPROVEN (harmless for a server) |
| L5 | `aux[1]` carries the free head look (+-pi/4); `aux[2..5]` about 1e-6 noise in 25 % of updates | 2.1.1 | noise: new observation, PROVEN-CAPTURE; meaning UNPROVEN |
| L6 | trailer `time` is the client's uptime clock (62-138 s at the join) | `MOVEMENT_2009.md` 12.2 | stated there; a server uses differences only |
| L7 | `JUMP` 0x10 never on the wire (0 in 2,338 updates with 27 jumps) | 2.1.1, MOVEMENT 5.1 | stated; a client that keeps the bit for one 60 Hz tick still parses (RUN) |
| L9 | own plain `CTL_BYE` x4 8.7-9.0 s after the host's bye, when the result / room screen returns | `UDP_TRANSPORT_2009.md` (C-T12) | stated there |
| L11 | (our client) a 0x89 sent after 0x46 | 3.3 | whether the 2009 client disarms on a round end: UNPROVEN |
| L13 | goal 5 is below the ROM goal minimum a 2009 host can choose | `GAME_MODES_2009.md` 4.1 | test choice; the lobby accepted it (REFERENCE) |

L8 and L10 (silence after `C2S_BYE`; no keep-alive in the same datagram as an update) are transport rows: `UDP_TRANSPORT_2009.md` Appendix F.

### F.4 Not folded

The crate change lists (dc-client weapons, dc-net-engine quiet-after-bye, dc-botclient fallback requests): in `staging/spec-fold-5_CRATE_FOLLOWUPS.md`.

Count: proposal rows rewritten 3 (DC-C2S-10, DC-C2S-3, DC-C2S-13 note), id cross references 9 K + 10 L; one new UNPROVEN item (L4 rounding at +-pi), one PROVEN-CAPTURE observation (L5 noise); no grade lowered.

## Appendix G: fold record (spec-fold-6, 2026-10-25)

Sources: the captures tdm2 (`20261025_001409_tdm2`) and tdm4 (`20261025_002236_tdm4`) decoded with our own readers (the `dc-botclient` capture reader `capdecode` and a scratch record / stream dump built on the same crate, outside the tree; identifiers and seq numbers only, no reference stack log was read); the reports `reports/{dserver-tidy, stack-verify, bot-respawn, bot-roundend, client-dserver-fix, render-styles}.md`; the addenda `GAME_MESSAGES_2009_S2C_addendum_blast_questions.md` and `RENDER_2009_addendum_styles.md` (both now stubs); `DC_DEFAULTS.md` rows DSV-39, DSV-40, GAIN-1, BOT-18, REN-1..11 (PENDING). Grades as in the header plus **D** (README). No existing grade was lowered; no REFERENCE value became a rule.

| place | change | grade |
|---|---|---|
| 3.3 (end) | BOT-18: what `dc-botclient` does after 0x46 (at most one request, then none until state 1) against the 2009 client's behaviour, which stays UNPROVEN; the 7,568-request RUN before and the one-request RUN after | D (proposed, bot rule), TESTED, RUN |
| 3.4 (DeCombat paragraph) | bot side: one request per death (bot-respawn B-1), RUN 6 / 6 | D, TESTED, RUN |
| 4.6 (`UP_THROW_HOLD` bullet) | cross references to the answered reference stack sequence (B 3.5.4, S2C 8.2) and to DSV-39's release rule | REFERENCE (pointer); D |

The client-side question that BOT-18 sidesteps (does the 2009 client disarm its wait at 0x46?) is part of V-KILL-1.

## Appendix H: fold record (spec-fold-7, 2026-10-25)

Sources: `reports/client-weapons.md` (sections 1.1, 4, 5) and its staged notes "Facts used that are not defaults"
(`staging/client-weapons_DC_DEFAULTS.md`, now in `DC_DEFAULTS.md` Wave R section, PENDING). No capture was re-read; no new RVA (cshell+0x176f10 is
the client-weapons worker's citation of the clean decompiler tree). Grades as in the header plus **D** (README). No existing grade was lowered; the CLI rows are DeCombat client
choices, not 2009 evidence.

| place | change | grade |
|---|---|---|
| 2.1.1 weapon float | the law is not read; `dc-client`'s `weapon_f` / fire-bit rule | PROVEN-CAPTURE (form); D (CLI-20', CLI-33), TESTED |
| 4.1 (new paragraph) | cadence = the first-person fire clip (105 ms = tdm1's 104-110 ms); `fireDelay` holds no rate; `RapidFireCount`; prefire not per shot | PLAUSIBLE (2009), OBSERVED: ROM (key census), RUN / TESTED (ours); CLI-30 (proposed) |
| 4.1 layout and meanings | `shot_scale` = `DynamicPerturbFactor`, 3 / 3 | PLAUSIBLE |
| 4.1.1 spread note | tdm1 saturation 73 / 255 and the pistol's 45 / 255; the 2009 law UNPROVEN; CLI-31 is a stand-in that does not match | PROVEN-CAPTURE (numbers), UNPROVEN (law), D (CLI-31) |
| 4.1.1 second reader | `dc-client`'s 0x82 forms reproduce the heads and offsets; box hit volume vs the real client's model trace | D, TESTED, RUN (CLI-43) |
| 4.3 (new paragraph) | the keys are the first-person clips' markers; observed 0x80 timing reproduced from the clip keys | PLAUSIBLE (2009), TESTED (ours) |
| 4.5 | event 1 + clip size 150 ms after 0x83 | D (CLI-32) |
| 4.6 | the throw sequence driven by the throwable's clips | D, TESTED, RUN; CLI-29 fallbacks |

The marker words of the first-person clips are ROM data; they are named here by role (fire key, throw key, sound key) and by the existing hash of the throw key, as 4.3
already does.

## Appendix I: fold record (spec-fold-8, 2026-10-25)

Sources: `CAPTURE_tdm6_VALIDATION.md` sections 2-4, 8, 9 and 11; `CAPTURE_rust3_RUSTSTACK_VALIDATION.md` 9.4, 10, 12; `UDP_SESSION_2009_B_addendum_round2.md` G6 (now a stub);
`reports/aa-triage-grenades.md` item 6; the DSV-51 row (`DC_DEFAULTS.md` Wave Z part 2, PENDING). No capture was decoded again.

| place | change | grade |
|---|---|---|
| 3.3, 3.4 | the real client's own respawn timing (tdm6, five deaths) | PROVEN-CAPTURE (upgrade from DISASM + bot) |
| 4.1 | `extra_kind` = the skeleton node index | PROVEN-CAPTURE + ROM + REFERENCE agreement |
| 4.1.1 | multi-pellet form, melee swing, head and torso hits, slot-4 weapon, the first real-client 0x83 (rust3, not analysed) | PROVEN-CAPTURE |
| 4.6 | the tdm6 throws; the 2-pack throws against our stack (rust3, rust4) | PROVEN-CAPTURE; cause UNPROVEN (V-NADE-1) |
| 5.7 | `C2S_BYE` 15-16 ms after 0xbd also in tdm6 / rust3; DSV-51 | PROVEN-CAPTURE; D |

No grade was lowered; the replaced sentences are quoted in place.
