# GAME_MESSAGES_2009: shell-level game messages of the 2009 client (clean-room)

- **Title:** GAME_MESSAGES_2009, the catalogue of the game messages that travel inside the engine's shell-message
  wrapper (`S2C_SHELL_MSG` id 13 server -> client, `C2S_SHELL_MSG` id 11 client -> server) and are produced or
  consumed by the 2009 `CShell.dll`, with payload layouts and the client reaction.
- **Scope:** every server -> client shell id that the CShell dispatch chain handles (ids from
  `UDP_SESSION_2009_B.md` section 7.1, payloads here), every client -> server shell id that CShell writes, grouped
  by subsystem, plus a Team Deathmatch round walkthrough. NOT in scope: the engine wrapper itself, object create /
  update records, engine sounds and time sync (`UDP_SESSION_2009_A.md` / `_B.md`), the bit transport
  (`UDP_TRANSPORT_2009.md`), the effect ids of shell object messages (a later FX spec), and every server-side rule
  (damage values, respawn delay, score rules): those appear only as "what the client displays or expects" and are
  DeCombat defaults where a value is needed.
- **Evidence sources:** decompiler output of the 2009 EU-test `CShell.dll` (`CShell.dll/<rva>.c`, image
  base 0x37100000, cited `cshell+0xRVA`) and `Engine.exe` (image base 0x00460000, cited `engine+0xRVA`);
  `CShell.dll.funcs.orig.tsv`; read-only capstone disassembly of the user's own `Game/CShell.dll` (cited
  `cshell+0xRVA (DIS)`) wherever the decompiler dropped a pushed read width or a store target; method-table
  addresses read from the user's ROM binary (PE-DATA, addresses only); the sibling clean specs
  `UDP_TRANSPORT_2009.md`, `UDP_SESSION_2009_B.md`, `GLOSSARY_UDP.md`. The TDM message flow is checked against three captures of the real client (tdm1, tdm2, tdm4, 2026-10-25), listed in Appendix C of the two catalogues; the conventions and dispatch chain below are unchanged by them.
- **Status:** DRAFT; reduced to the index plus shared sections 1 and 2 on 2026-10-24 (spec-fold), with the corrections of `GAME_MESSAGES_2009_S2C.md` 11.1 C1-C3 applied. The fifth fold
  (2026-10-25) added the DeCombat server forms (grade D: a DC_DEFAULTS row, never 2009 evidence) and the corrected respawn model (the client arms its own wait; the server respawns on
  0x89) to the two catalogues; their records are the "Appendix F" sections there.
- **Date:** 2026-10-24

Evidence grades as in `UDP_TRANSPORT_2009.md`: **PROVEN** (literally visible in the decompiler output or in the disassembly),
**PLAUSIBLE** (reconstructed where decompiler lost an argument, a return target or a `this`; the reading is the only one
consistent with the code but one step is not literally visible), **UNPROVEN** (not visible; confirm by capture or by a
decompiler re-run before relying on it). Meanings ("kill notice", "round timer") are interpretations of what the client
does with a field and carry their own grade.

## 0. Index: where the game messages are specified

This file is the short index and the shared base of the game-message specs. It keeps what both catalogues rely on
(conventions and reader / writer method tables in section 1, the dispatch chain in section 2); the messages
themselves live in the two catalogues (promoted 2026-10-24, DRAFT 1; client-to-server layouts of a TDM session PROVEN-CAPTURE, server-to-client layouts REFERENCE-CONFIRMED, fourth fold 2026-10-25):

| need | spec | what it holds |
|---|---|---|
| every server -> client game message (shell ids, layouts, client reaction, match flow, a TDM round in server order) | `GAME_MESSAGES_2009_S2C.md` | stages A-F handlers, shell object messages (effects, 0xa6), subsystem index, corrections 11.1 |
| every client -> server game message (send pipeline, 49 shell ids, triggers, reliability, expected server answers) | `GAME_MESSAGES_2009_C2S.md` | senders, developer-only ids, TDM walkthrough with DeCombat decisions, UNPROVEN / capture lists, DC-C2S proposals |
| the engine wrapper messages 11 / 13 and the shell dispatcher | `UDP_SESSION_2009_B.md` sections 6, 7, 10, 11 | wire form, dispatcher stages (corrected by section 2 below) |
| movement messages 0xa0 / 0x64 | `MOVEMENT_2009.md` | mover semantics, update trailer |

The old plan of this file (sections 3-11: per-subsystem catalogues, a walkthrough, glossary names, UNPROVEN list)
was carried out in the two catalogues; sections 3-14 of the original skeleton do not exist here any more. Section
numbers 1.x and 2 below are cited by both catalogues and are kept stable.

## 1. Conventions, reader and writer method tables

**1.1 Wire form.** Every game message in this spec is the body of an engine shell message
(`UDP_SESSION_2009_B.md` section 6.1):

```
server -> client: [msg_id:u8 = 13][shell_id:u8][fields ...]      S2C_SHELL_MSG
client -> server: [msg_id:u8 = 11][shell_id:u8][fields ...]      C2S_SHELL_MSG
```

There is no length, no version and no padding inside a game message; it ends where the transport entry ends.
Field layouts below start **after** `shell_id`. Bit order, `uN`, `f32`, `zstr8`, `zstr16`, `objref16`, `pos48`,
`vec72`, `yaw8`, `quat128` are exactly as in `UDP_TRANSPORT_2009.md` sections 2, 3 and 15. A field that is
missing at the end of a message reads as 0 (transport 2.3); the client never checks that a message was fully
consumed, so trailing bits are ignored. A layout that depends on a condition is written as `if <cond>: [...]`;
conditions on **client** state are flagged, because the server must know that state to build a message the client
will parse correctly.

**1.2 Field types used only here.**

| name | layout | evidence |
|---|---|---|
| `vec96` | three raw `f32` (x, y, z), read as one 96-bit raw block | cshell+0x26230 (DIS: reader method +0x44 with 0x60 bits, copied to 3 floats) |
| `zstrN` | `zstr8` read into an N-byte buffer; the client keeps at most N-1 characters and drops the rest (the whole string is still consumed) | reader method +0x4c (engine+0x41640 -> engine+0x80450) |
| `wstrN` | `zstr16` read into an N-unit buffer, same truncation rule | reader method +0x50 (engine+0x41650 -> engine+0x80490) |
| `bool1` | one bit, any non-zero value means true | e.g. cshell+0x49e8b (DIS) |
| `team` | integer team number: 1 = first team, 2 = second team (the round-win banner plays the first team's sound for 1 and the second team's for 2, cshell+0xc1a50); other values: no team / spectator (PLAUSIBLE) | cshell+0xc1a50 |
| `mode_class` | the client's current game-type number (client shell field +0x16938) | set by cshell+0x1630a0 from cshell+0x264050 |

`mode_class` is set from two sources (correction of the first draft, which said it is never on the wire;
`GAME_MESSAGES_2009_S2C.md` 11.1 C2, DISASM cshell+0x2f86e..0x2f87c): shell id **0x9e** (`[mode_class:u8]` first,
`GAME_MESSAGES_2009_S2C.md` 5.1) stores its first byte into `client_shell`+0x16938, so the mode number **is** on the
wire; and the client also derives it from the room's mode index through the game-room attribute table of the user's
ROM (cshell+0x264050 returns field +0x104 of the mode row; table loaded by cshell+0x266550; set by cshell+0x1630a0).
A server must send 0x9e before any message whose layout depends on `mode_class`, and must compute the value the
same way at run time (ROM model; no table is reproduced here). Grade: the 0x9e store PROVEN by disassembly; which
of the two sources the 2009 server relied on is UNPROVEN. Several game messages change layout with `mode_class`; the
code constants that matter are 2, 3, 5, 6, 9, 10, 11 (each cited where used). The Team Deathmatch row of the EU-test
ROM has a `mode_class` that is **none** of these special values (OBSERVED by reading the user's ROM read-only,
2026-10-24); Appendix A of the C2S catalogue uses that fact and nothing else from the table.

**1.3 Reader methods** (the reader object that CShell receives; engine method table VA 0x679eb0, PE-DATA; each slot
matched to its engine code by disassembly). Only the slots CShell uses for game messages are listed.

| slot | use | engine code | evidence |
|---|---|---|---|
| +0x20 | sub-reader from a bit position to the end | engine+0x42b30 | `UDP_SESSION_2009_B.md` 6.2 |
| +0x24 | length in bits | engine+0x415d0 (DIS) | PROVEN |
| +0x2c | set the read position (`seek(0)` = rewind to the first bit after `shell_id`) | engine+0x41db0 (DIS) | PROVEN |
| +0x30 | current read position | engine+0x415e0 (DIS) | PROVEN |
| +0x38 | 1 when the position is at or past the end | engine+0x41600 (DIS) | PROVEN |
| +0x3c | `read(n)`: next n <= 32 bits, zero-extended | engine+0x41610 -> engine+0x80220 (DIS) | PROVEN |
| +0x40 | read up to 64 bits | engine+0x41620 -> engine+0x802c0 (DIS) | PROVEN |
| +0x44 | read a raw block of n bits into memory | engine+0x41630 -> engine+0x80300 (DIS) | PROVEN |
| +0x48 | `nest16` | engine+0x5b9d0 | PROVEN |
| +0x4c | `zstr8` into a buffer of given size | engine+0x41640 -> engine+0x80450 (DIS) | PROVEN |
| +0x50 | `zstr16` into a buffer of given size (units) | engine+0x41650 -> engine+0x80490 (DIS) | PROVEN |
| +0x54 | `lstr16h` | engine+0x5b560 | PROVEN |
| +0x58 | `vec72` | engine+0x41660 -> engine+0x5b5d0 (DIS) | PROVEN |
| +0x5c | `pos48` | engine+0x5c5d0 -> engine+0x5c320 | PROVEN |
| +0x60 | `quat128` | engine+0x41680 -> engine+0x5b650 (DIS) | PROVEN |
| +0x64 | `lstr16` | engine+0x5b900 | PROVEN |
| +0x68 | `objref16`, returned as the client's object (none for 0xFFFF; an unknown id also gives none, PLAUSIBLE) | engine+0x5c5a0 (DIS: `read(16)`, 0xFFFF -> none, else lookup engine+0x25370 in the client id table) | PROVEN |
| +0x6c | `yaw8` | engine+0x416a0 -> engine+0x5b990 (DIS) | PROVEN |

**1.4 Writer methods** (the message writer that CShell obtains from the engine common interface, CShell global VA
0x375964f0 method +0x24, wrapped by cshell+0x29e50; concrete engine method table VA 0x679e48, PE-DATA, base table VA
0x677f68 with pure slots). CShell then hands the finished packet (writer slot +0x1c, engine+0x5c8d0; wrapper
cshell+0x299c0) to the engine client interface slot +0x158 (engine+0x13f40, which prefixes `msg_id = 11`).

| slot | use | engine code | evidence |
|---|---|---|---|
| +0x1c | finish: return the written packet | engine+0x5c8d0 | PROVEN |
| +0x20 | bits written so far | engine+0x40cf0 (DIS) | PROVEN |
| +0x24 | `write(value, n)`: low n <= 32 bits | engine+0x41460 -> engine+0x80a30 (DIS) | PROVEN |
| +0x28 | write up to 64 bits | engine+0x41470 -> engine+0x80ac0 (DIS) | PROVEN |
| +0x2c | write a raw block of n bits | engine+0x41490 -> engine+0x80b00 (DIS) | PROVEN |
| +0x30 | `nest16`: `[bits:u16]` then the nested packet | engine+0x5b2b0 (DIS) | PROVEN (closes the writer gap of transport section 3) |
| +0x34 | append a packet's bits with no length | engine+0x5b2e0 -> engine+0x80bf0 (DIS) | PROVEN |
| +0x38 | `zstr8` | engine+0x414a0 -> engine+0x80e30 (DIS) | PROVEN |
| +0x3c | `zstr16` | engine+0x414b0 -> engine+0x80ed0 (DIS; target not decompiled) | PLAUSIBLE |
| +0x40 | `lstr16h` (string handle) | engine+0x5b300 (DIS) | PROVEN |
| +0x44 | `vec72` | engine+0x414c0 -> engine+0x5b360 (DIS) | PROVEN |
| +0x48 | `pos48` | engine+0x5c290 -> engine+0x5c040 (DIS) | PROVEN |
| +0x4c | `quat128` | engine+0x414e0 -> engine+0x5b3d0 (DIS) | PROVEN |
| +0x58 | `lstr16` of a C string: count = string length **plus one**, the terminating 0 byte is sent | engine+0x5b4d0 (DIS) | PROVEN |
| +0x5c | `objref16` | engine+0x41500 -> engine+0x5b530 (DIS) | PROVEN |
| +0x60 | `yaw8` | engine+0x41520 -> engine+0x5b8a0 (DIS) | PROVEN |

**1.5 Reliability.** CShell send sites pass flag 1 (reliable) to engine slot +0x158 unless section 11 says
otherwise. Server -> client game messages are expected reliable and in order: several handlers depend on state set
by an earlier message (for example the match-start values of 3.2 are used by 3.3). DeCombat default proposal: send every
game message in this spec reliable.

## 2. Dispatch chain and gating

The dispatcher (cshell+0x1a86b0, `UDP_SESSION_2009_B.md` 6.2) reads `shell_id:u8`, makes a sub-reader positioned after
it and offers `(shell_id, reader)` to the stages below in order; the first stage that returns non-zero consumes the
message. The getter slots were disassembled (DIS), so the stage-to-object mapping is PROVEN here (it was PLAUSIBLE in
`UDP_SESSION_2009_B.md` 7.1). Stage names are those of `UDP_SESSION_2009_B.md` 7.1; the role column is ours.

| stage | getter (client shell slot) | object | handler | role (ours) | evidence |
|---|---|---|---|---|---|
| A `ShellPartA` | +0xac = cshell+0x41c00: `this + 0xc52c` | class table VA 0x374d07ac | slot +0x24 = cshell+0x49a40, default -> cshell+0x1b90e0 | match UI: match and round flow, timers, banners, mode objectives | DIS cshell+0x41c00; PE-DATA table |
| B `ShellPartB` | +0xa8 = cshell+0x41c40: `this + 0x168e8` | class table VA 0x374ce808 (also global VA 0x375603a8) | slot +0x0c = cshell+0x2c430, default -> cshell+0x207120 (returns 0) | player roster: player records, hits, deaths, summaries | DIS cshell+0x41c40, cshell+0x207120 |
| C `ShellPartC` | +0xb0 = cshell+0x41c10: `this + 0x16054` | **derived** class table VA 0x374d327c (base VA 0x374eefc4; also global VA 0x3759bf60) | slot +0x24 = **cshell+0x60d90**, then always the base cshell+0x1f0470 | local player: weapons, inventory, spawn state | DIS cshell+0x41c10; PE-DATA tables |
| D `ShellPartD` = `conn_mgr` | global VA 0x3759e22c | derived class table VA 0x374cedc4 (base VA 0x374f1b4c) | slot +0x24 = cshell+0x23c570 | connection | PE-DATA tables |
| E `ShellPartE` | global VA 0x3759e120 | derived class table VA 0x374d3194 (base VA 0x374f120c) | slot +0x20 = cshell+0x5ac40 | mode rules (co-op scripting, mission text) | PE-DATA tables |
| F `ShellPartF` = `hud_mgr` | global VA 0x375acdb8 | derived class table VA 0x374d06dc (base table VA 0x374f4c9c) | slot +0x38 = **cshell+0x45050** (the base table's slot +0x38 is `return 0` cshell+0x26b220, replaced in the live object) | HUD manager: vote panels, radar marks, respawn countdown; handles 0x7e, 0x84, 0xb6, 0xb7, 0xb9, 0xbb, 0xbc, 0xcd, 0xcf, 0xd0, 0xd4 (`GAME_MESSAGES_2009_S2C.md` 7.1) | PE-DATA VA 0x374d06dc +0x38; DISASM cshell+0x46ed1..0x46ed9, cshell+0x45083..0x45676 |
| fallback | the shell itself | | switch in cshell+0x1a86b0 | console, misc | `UDP_SESSION_2009_B.md` 7.1 |

Corrections and additions to `UDP_SESSION_2009_B.md` 7.1 (for its author and the glossary owner):

1. Stage C is the shell member at +0x16054 whose **derived** slot +0x24 (cshell+0x60d90) handles shell ids 0x5a,
   0x66, 0xb2, 0xb3, 0xb4, 0xd8, 0xd9, 0xda itself and then calls the base handler cshell+0x1f0470 (ids 0x67, 0x6a,
   0x74, 0x7f, 0xa0, 0xa3, 0xa4, 0xad, 0xaf, forwarded to derived slots +0xb4 .. +0xd4). The two results are OR-ed
   (cshell+0x60d90 tail). The global VA 0x3759bf60 is this same member (set by its slot +0x04, cshell+0x1f6fb0).
2. Stage A's default path calls the base handler cshell+0x1b90e0, which handles **0x77** and **0x79** (missing from
   the 7.1 id list).
3. Stage F is not empty (correction C1): its slot +0x38 is the derived HUD manager handler cshell+0x45050. It returns 1 for its ids except 0xd4, which it handles and then leaves unconsumed (returns 0). Ids 0x7e, 0x84, 0xb6, 0xb7, 0xb9, 0xbb, 0xbc, 0xcd, 0xcf, 0xd0, 0xd4 reach it only if stages A-E decline them first.

Gating and ordering rules a server must respect:

- Stage A handles nothing until the match-UI object's initialised flag (byte +0x6aa4) is set (cshell+0x49a40 and
  cshell+0x1b90e0 return 0 otherwise). The flag is set at the end of the interface init cshell+0x1b7ff0 (DISASM
  cshell+0x1b8864), i.e. at client start, and cleared only by the interface teardown (cshell+0x1b4e00 at 0x1b4ff3,
  cshell+0x1ba923): stage A is open during the whole game session (`GAME_MESSAGES_2009_S2C.md` 11.1 C3; the first
  draft's "PLAUSIBLE that it is set once the in-game interface exists" is replaced, grade DISASM). The same flag gates
  most methods of that object (for example cshell+0x1b50b0, cshell+0x1bb0b0). An id declined by every stage is dropped
  silently.
- Shell id 0x66 is shared: stage A reads `[kind:u8][flag:u8][x:u8][value:u32]`, handles only `kind = 0x0c`, and
  otherwise **rewinds** the reader (`seek(0)`) and declines, so stage C sees the same message from its first bit
  (section 5.6). No other id is handled by two stages.
- Aliases: 0x3a = 0x44, 0x3c = 0x46, 0x3d = 0x48 (the first id only adds a debug print and falls into the second
  id's body: cshell+0x49b81, cshell+0x4a608, cshell+0x4aac1). 0x3b is consumed by nobody (debug print, then
  declined).

