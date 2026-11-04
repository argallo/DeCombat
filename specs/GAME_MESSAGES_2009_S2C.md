# GAME_MESSAGES_2009_S2C: server -> client game messages of the 2009 client (clean-room)

- **Title:** GAME_MESSAGES_2009_S2C, the catalogue of the server -> client game messages that the 2009 `CShell.dll`
  handles: every shell id of the dispatch chain (`GAME_MESSAGES_2009.md` section 2) and every effect id of the shell
  object message path, with the bit-level field reads in order, what the client does with them, the handler address
  and the reliability.
- **Continues:** `specs/GAME_MESSAGES_2009.md` (its sections 1-2: reader and writer method tables, field types,
  dispatch chain and gating). Section numbers of that file are cited as `GM §n`. The five corrections listed in the
  report `reports/game-messages-2009.md` to `UDP_SESSION_2009_B.md` §7.1 are applied throughout:
  (1) stage C is the shell member at +0x16054 with derived slot +0x24 = cshell+0x60d90 (ids 0x5a 0x66 0xb2 0xb3 0xb4
  0xd8 0xd9 0xda) followed by the base cshell+0x1f0470; (2) stage A's base cshell+0x1b90e0 handles 0x77 and 0x79;
  (3) stage F (cshell+0x26b220) consumes nothing; (4) shell slot +0xd4 = cshell+0x41c30 is a getter
  (`this + 0x164e0`) and the effect dispatcher cshell+0x217760 runs on the object it returns; (5) id 0x66 is shared
  between stage A (kind 0x0c only, otherwise rewind and decline) and stage C.
- **Scope:** server -> client only. Client -> server messages, the engine wrapper and the transport are elsewhere
  (`GAME_MESSAGES_2009.md` §11 when written, `UDP_SESSION_2009_A/_B.md`, `UDP_TRANSPORT_2009.md`). Server-side rule
  values (damage, respawn delay, score rules, rewards) are not taken from anywhere: where a value is needed it is a
  DeCombat default (`DC_DEFAULTS.md`, CD-nn) or left to the implementer.
- **Evidence sources:** clean decompiler tree `CShell.dll/<rva>.c` (preferred; CShell image base
  0x37100000, so `FUN_372f97d0` is cshell+0x1f97d0), the older tree `CShell.dll/*.c` where the
  clean tree has no file, `CShell.dll.funcs.orig.tsv` and the clean `CShell.dll.funcs.tsv` for function bounds,
  read-only capstone disassembly of the user's own `Game/CShell.dll` (tag **DISASM**: decompiler lost the pushed read
  width in most reader calls, so every width below was checked in the disassembly), PE-DATA reads (method tables,
  RTTI used only to understand an object's role; no class name of the ROM is written here), and the sibling clean
  specs. Captures of the real 2009 client against the reference server (tdm1, tdm2, tdm4, 2026-10-25) are cited `OBSERVED tdm<n>` and listed in
  Appendix C; server bytes in them are the reference stack's (REFERENCE), not 2009 server bytes.
- **Status:** DRAFT 1. Server-to-client layouts are checked against captures only in the sense REFERENCE-CONFIRMED (the reference server wrote a form that parses exactly under this text and the real client accepted it and went on); no message here is yet shown to be what the 2009 server sent. Where the real client's own reaction proves a rule it is graded PROVEN-CAPTURE (sections 3.2.2, 3.5, 4.1.1).
- **Date:** 2026-10-24.

Evidence grades as in `GAME_MESSAGES_2009.md`: **PROVEN** (visible in the decompiler output or in the disassembly),
**DISASM** (visible only in the disassembly, decompiler has no function there; ranks below decompiler output), **PLAUSIBLE**
(one step reconstructed, for example a lost `this`), **UNPROVEN** (not visible; capture or decompiler re-run needed).
Meanings such as "kill feed line" or "round timer" are interpretations of what the client does and carry their own
grade. ROM strings are never quoted: a string the code uses is given as `str(VA; sha256[:16]; length)`.
Capture grades (added by the fourth fold): **PROVEN-CAPTURE** = the real client's own bytes or a client behaviour visible in them (2009 evidence);
**REFERENCE-CONFIRMED** = a form written by the reference server that parses exactly under this text and was accepted by the real client (its next
messages could only follow a successful handling); **REFERENCE** = what the reference server does, recorded for comparison and never a rule value
(`REFERENCE_TESTING.md` section 1; server rule values such as damage amounts, delays and rewards are DeCombat defaults, not copied from it).
Grade added by the fifth fold (spec-fold-5, 2026-10-25): **D** = DeCombat design, a `DC_DEFAULTS.md` row (PROPOSED unless the row says approved) that a
DeCombat server follows where the 2009 evidence leaves a choice; its test state is given as TESTED (a crate test asserts it) or RUN (seen in a loopback run of our own
stack, no real client). D is never 2009 evidence. The fold record of spec-fold-5 is Appendix F.

## 0. Contents

1. Conventions and the client objects the handlers drive
2. Stage A: match UI object (cshell+0x49a40, base cshell+0x1b90e0)
3. Stage B: player roster object (cshell+0x2c430)
4. Stage C: local player object (cshell+0x60d90, base cshell+0x1f0470), incl. the player-state carrier
5. Stage D: connection object (cshell+0x23c570)
6. Stage E: mode-rules object (cshell+0x5ac40)
7. Stage F and the shell's fallback switch (cshell+0x1a86b0)
8. Shell object messages: cshell+0x1a3d00, getter cshell+0x41c30, effect dispatcher cshell+0x217760, cshell+0x26e070
9. Subsystem index (every id grouped by what it drives)
10. Messages needed for one TDM round (server -> client)
11. UNPROVEN items and the decompiler re-run list
12. Names registered here

## 1. Conventions and the client objects the handlers drive

### 1.1 Layout notation

Every message below is the body of `S2C_SHELL_MSG` (engine id 13): `[13][shell_id:u8][fields...]`, fields start after
`shell_id` (GM §1.1). Field types and the reader slots behind them are those of GM §1.2-§1.3:

| written here | reader slot (GM §1.3) | meaning |
|---|---|---|
| `uN` (`u8`, `u16`, `u32`) | +0x3c `read(N)` | N bits, unsigned, bit order of the transport |
| `iN` | +0x3c `read(N)`, then sign-extended by the handler (`movsx`) | signed |
| `f32` | +0x3c `read(32)`, the 32 bits then used as an IEEE float (`fld dword`) | raw float bits |
| `bool1` | +0x3c `read(1)` | 0 / non-zero |
| `u64` | +0x40 | 64 bits |
| `zstr8(N)` | +0x4c with buffer size N | `zstr8`, the client keeps at most N-1 bytes |
| `zstr16(N)` | +0x50 with buffer size N units | `zstr16`, at most N-1 units kept |
| `objref16` | +0x68 | object id, 0xFFFF = none |
| `vec96` | +0x44 with 96 bits (cshell+0x26230) | three raw f32 x, y, z |
| `raw128` | +0x44 with 128 bits (cshell+0x26260) | four raw f32 (the handler stores 16 bytes) |

`if <cond>: [...]` marks a conditional part. Conditions on client state are flagged **(client state)**: the server
must track the same state to build a message the client parses correctly. `mode_class` is the client shell's mode
number (client shell +0x16938; the mode number of GM §1.2). It is **also set on the wire** by shell id 0x9e (section 5.1; a correction
to GM §1.2, which says it is never on the wire), besides being derived from the room's mode row at room entry.

A handler that returns 0 leaves the message to the next stage (GM §2). "Not consumed" below means the client acted on
the message but returned 0, so later stages see the reader **at its current position** (the reader is not rewound
between stages). No later stage handles such an id, so the only consequence is that the message is dropped after its
effect took place.

Reliability: the 2009 client does not show how the server sent a game message (the server writer is not in the
client, `UDP_SESSION_2009_B.md` §12.1). Every row below is therefore **reliability UNPROVEN** unless stated; the
DeCombat default is PD-S2C-1 (proposed here; the coordinator assigns the CD number): send every server -> client game message on the reliable channel, in
order, as GM §1.5 proposes. Where a handler depends on state that an earlier message set, this is said.

Player ids: the roster (section 3) keys players by a small id (record +0x04; sent as `u8` in most roster messages,
as `u32` in 0x68, 0xc3, 0xd4, 0xd6 and 0xd7) that the client compares with its own engine client id (engine client
interface slot +0x84, the id of `S2C_CLIENT_ID`, `UDP_SESSION_2009_A.md` §4.3), for example in the chat handler
(2.4, DISASM cshell+0x4b452..0x4b4aa) and in 0x6b / 0x6c / 0x6d. A server must use each player's engine client id as
this `member_id` and keep it below 256 (PROVEN by the compares; the `u8` width is PROVEN by the reads).

### 1.2 Client objects (our names)

Stage objects of the dispatch chain (GM §2; classes are identified by their method-table VA only):

| our name | where | method table (PE-DATA) | role |
|---|---|---|---|
| `match_ui` | shell +0xc52c; the same object is the interface-root singleton VA 0x37598920 (set by the base constructor cshell+0x1ba580, which the derived constructor cshell+0x47e90 runs first) | derived VA 0x374d07ac, base VA 0x374ec3dc | stage A; owns `hud_mgr` (+0x6b74), `screen_mgr` (+0x6c4c, getter slot +0x68 = cshell+0x48020), `layout_mgr` (+0x6e18, also globals VA 0x37560d70 / 0x37560d5c) |
| `roster` | shell +0x168e8, global VA 0x375603a8 | VA 0x374ce808 | stage B; per-player records in a list from `roster`+0x04, next pointer at record +0x218 |
| `local_player` | shell +0x16054, globals VA 0x3759bf60 and VA 0x37560db8 | derived VA 0x374d327c, base VA 0x374eefc4 | stage C; own player state (+0x30), weapons, spawn |
| `conn_mgr` | global VA 0x3759e22c | derived VA 0x374cedc4, base VA 0x374f1b4c | stage D |
| `mode_rules` | global VA 0x3759e120 | derived VA 0x374d3194, base VA 0x374f120c | stage E |
| `hud_mgr` | `match_ui` +0x6b74; global VA 0x375acdb8 is set to this object by the base init cshell+0x26b380, which the derived init cshell+0x45c30 calls with the same `this` (DISASM cshell+0x46ed1..0x46ed9: `mov ecx, edi; ...; call 0x3736b380`) | derived VA 0x374d06dc, base VA 0x374f4c9c | stage F (section 7.1: its derived slot +0x38 is cshell+0x45050, **not** the base `return 0`) |

Other singletons the handlers call:

| our name | VA | evidence |
|---|---|---|
| `engine_client` (engine client interface; slot +0x18 debug print, +0x6c game time in seconds as float, +0x84 own client id, +0x8c is-connected, +0x158 send, +0x20c own view object) | 0x37560ea8 | `UDP_SESSION_2009_A.md` §1.2; cshell+0x6f860 (timer arithmetic in seconds) |
| `client_shell` (game shell; `mode_class` at +0x16938) | 0x37597e50 | GM §1.2 |
| `sound_mgr` | 0x375984ac | destructor cshell+0x1b1d60 clears the pointer; play request through cshell+0x1b1e20 (PLAUSIBLE) |
| `config_db` (game attribute reader; cshell+0x1b2c00 reads a string value by key) | 0x37598544 | destructor cshell+0x1b30a0 |
| `weapon_db` (ROM weapon table; lookup by name cshell+0x1723b0, by index cshell+0x172590, count at +0x15c8) | 0x3758cbfc | constructor cshell+0x180590 |
| `session_info` (own profile: +0xa4 own id, +0xf8 own team, +0x1b8 profile record) | static VA 0x37598058, accessor cshell+0x1b1a10 | cshell+0x1b1330 writes +0xf8 |
| `move_mgr` | 0x3759c220 | constructor cshell+0x1fdba0 |
| `match_stats` (status block and per-match weapon statistics; PLAUSIBLY the same instance as `local_player`+0x278, section 4.3) | 0x3759e110 | constructor cshell+0x63e60 at `local_player`+0x278, destructor cshell+0x64550 clears the global |

HUD elements (created by the `hud_mgr` init cshell+0x45c30, each stored at `hud_mgr`+off and copied to a global; the
name is ours and describes what the element's methods do):

| `hud_mgr` off | global VA | constructor | our name |
|---|---|---|---|
| +0x28 | 0x37560c94 | cshell+0x852b0 | `hud_crosshair` |
| +0x2c | 0x37560ca0 | cshell+0x6ea50 | `hud_ammo` |
| +0x30 | 0x37560c9c | cshell+0x97200 | `hud_health` |
| +0x34 | 0x37560c98 | cshell+0xb9140 | `hud_radar` |
| +0x38 | 0x37560ca4 | cshell+0xc9410 | `hud_overview_map` |
| +0x3c | 0x37560ca8 | cshell+0x757b0 | `hud_chat_log` (message lines with colour and lifetime) |
| +0x40 | 0x37560cac | cshell+0xbd710 | `hud_banner` (centre-screen notice) |
| +0x44 | 0x37560cb0 | cshell+0x9d3b0 | `hud_kill_feed` |
| +0x48 | 0x37560cb4 | cshell+0xbe4e0 | `hud_round_timer` (minutes:seconds, cshell+0xbe570) |
| +0x4c | 0x37560cb8 | cshell+0x6fd10 | `hud_bomb_timer` |
| +0x50 | 0x37560cbc | cshell+0xc4600 | `hud_team_score` (two team counters at +0x68 / +0x6c) |
| +0x54 | 0x37560cc0 | cshell+0x7e3b0 | `hud_coop_score` |
| +0x58 | 0x37560cc4 | cshell+0x79ad0 | `hud_coop_result` |
| +0x5c | 0x37560cc8 | cshell+0x72cc0 | `hud_chat_input` |
| +0x60 | 0x37560cd0 | cshell+0xca8e0 | `hud_target_name` |
| +0x64 | 0x37560cd4 | cshell+0x8d140 | `hud_game_info` |
| +0x68 | 0x37560cd8 | cshell+0xaf9a0 | `hud_progress_bar` |
| +0x6c | 0x37560cdc | cshell+0xa7a20 | `hud_objective` |
| +0x70 | 0x37560ce0 | cshell+0x9b530 | `hud_held_item` |
| +0x74 | 0x37560ce4 | cshell+0x8b030 | `hud_marker` (world markers) |
| +0x78 | 0x37560ce8 | cshell+0xcd0c0 | `hud_mode_notice` |
| +0x7c | 0x37560cec | cshell+0xa5d60 | `hud_multi_kill` |
| +0x80 | 0x37560cf0 | cshell+0xa11e0 | `hud_weapon_slots` |
| +0x84 | 0x37560cf4 | cshell+0x93460 | `hud_scoreboard` |
| +0x88 | 0x37560cf8 | cshell+0xcef90 | `hud_vote_request` |
| +0x8c | 0x37560cfc | cshell+0xcf0b0 | `hud_vote` |
| +0x90 | 0x37560d00 | cshell+0xb9930 | `hud_radio_call` |
| +0x94 | 0x37560d04 | cshell+0x9bde0 | `hud_kill_count` |
| +0x98 | 0x37560d08 | cshell+0xcd400 | `hud_side_switch` |
| +0x9c | 0x37560d0c | cshell+0xa8f80 | `hud_spectator` |
| +0xa0 / +0xa4 | 0x37560d10 / 0x37560d14 | cshell+0xaa4e0 / cshell+0xab460 | `hud_spectator_ammo` / `hud_spectator_health` |
| +0xa8 | 0x37560d18 | cshell+0xaf440 | `hud_spectator_map` |
| +0xac | 0x37560d1c | cshell+0xbb000 | `hud_revenge` |
| +0xb0 | 0x37560d20 | cshell+0x86b20 | `hud_flag` |
| +0xb4 | 0x37560d24 | cshell+0x822c0 | `hud_coop_item` |
| +0xb8 | 0x37560d28 | cshell+0x94b10 | `hud_gold` |
| +0xbc | 0x37560d2c | cshell+0x71af0 | `hud_bounty` |
| +0xc0 | 0x37560d30 | cshell+0x9ab90 | `hud_hisec` (the HiSec case HUD, HISEC_2009 3.3) |
| +0xc4 | 0x37560d34 | cshell+0xb12b0 | `hud_infection` (two two-digit counters, cshell+0xb0e30 / cshell+0xb0f70) |
| +0xc8 | 0x37560d40 | cshell+0x9f1f0 | `hud_strike` (three-part countdown, cshell+0x9ef80) |
| +0xcc | 0x37560d38 | cshell+0xcdde0 | `hud_voice` |
| +0xd0 | 0x37560d3c | cshell+0xba700 | `hud_respawn_token` |

Text the client shows from its own resources is cited as "CRes string id N" (loaded by cshell+0x1a0280,
cshell+0x1a0230 or cshell+0x1a00b0 through `LoadString` on the CRes module, `FMT_CRES_UI.md`). HUD layout values
(colours, positions, notice texts) come from the HUD layout section of the ROM attribute files through `layout_mgr`
getters (cshell+0x57220 and its neighbours) and are not reproduced (ROM data).

## 2. Stage A: match UI object (cshell+0x49a40, base cshell+0x1b90e0)

Handler: `match_ui` derived slot +0x24 = cshell+0x49a40 (decompiler output present, read widths lost; every width below is
from the disassembly of the same function, DISASM grade for the width, PROVEN for the control flow). Gate: nothing
is handled while `match_ui`+0x6aa4 is 0 (GM §2). Ids the switch does not list, and the cases that end in the default
label (marked "not consumed"), call the base handler cshell+0x1b90e0, which handles only 0x77 and 0x79 and returns 0
otherwise (switch default label cshell+0x4b6a9). The switch is `id - 0x36` through a byte table (cshell+0x49a8e,
table VA 0x3714b9ac, jump table VA 0x3714b944, PE-DATA).

Common side effects of the round-flow ids (0x3f, 0x40, 0x41, 0x44, 0x46, 0x47, 0x48): they clear
`match_ui`+0x9a50 (the "banner already shown this round" latch used by 0x45) and `hud_mode_notice`+0x74, and most of
them print a debug line through `engine_client` slot +0x18 (debug output only).

### 2.1 Match and round flow

| id | our name | layout (after `shell_id`) | client reaction | evidence |
|---|---|---|---|---|
| 0x44 (0x3a is an alias: debug print, then the 0x44 body) | `GS_ROUND_BEGIN` | `[round_secs:u32][round_no:u32][round_limit:u32][keep_team:bool1]` | `local_player` measure-frame flag := (`mode_class` == 5) (cshell+0x1f0c40, issues an engine console command); roster refresh (cshell+0x291e0); stops two sound groups (`sound_mgr` +0x28, +0x30); `hud_marker` reset with the own client id (cshell+0x89d30); `hud_round_timer` shown and started with `round_secs` seconds, running, colour from the HUD layout (cshell+0x51a50, cshell+0xbe0c0); `match_ui`+0x9a58 := `round_no`, +0x9a5c := `round_limit`; `hud_scoreboard`+0x11c := `round_no` (cshell+0x92120); `hud_infection` two-digit counters := `round_no` / `round_limit` (cshell+0xb0e30 / 0xb0f70); `hud_team_score` and `hud_coop_score` per-element reset (cshell+0xc3150, cshell+0x7d950); scoreboard refresh (cshell+0x93120); if `keep_team` = 0 and own team (`session_info`+0xf8) = 1 and `mode_class` is 2 or 3: an extra reset through `local_player`+0x18 (cshell+0x22e0c0) and `match_ui` slot +0xb0; `hud_target_name` shown; result banners cleared (cshell+0xc1a50(0,0), cshell+0x7caf0(0,0), cshell+0x76db0(0,0)); `hud_team_score`+0x7c := `round_limit`; `hud_bounty` reset; `hud_kill_feed` cleared (cshell+0x9dc20); `hud_game_info` shown; if the radar exists and `match_ui`+0x9a95 is set: radar refresh (cshell+0xb4f00). Consumed. Meaning of `round_no` / `round_limit`: PLAUSIBLE (the half-time check cshell+0xcd210 compares `round_limit / 2` with `match_ui`+0x9a58) | cshell+0x49b81..0x49dc4 (DISASM widths: 0x49c1a `read(32)`, 0x49c64, 0x49c6f `read(32)`, 0x49c7c `read(1)`) |
| 0x45 | `GS_MATCH_BEGIN` | `[round_secs:u32][show_intro:bool1][waiting_banner:bool1][countdown_secs:u32]` then `if show_intro and intro_ok (client state): [intro block]`, then `[round_no:u32]` (always last) | see 2.1.1 | cshell+0x49de1..0x4a5f0 |
| 0x46 (0x3c alias) | `GS_ROUND_RESULT` | `[round_secs:u32][win_team:i8][wins_team1:u8][wins_team2:u8]` then `if mode_class == 5: [co-op block]` | see 2.1.2. **CORRECTION 2026-10-26 (client-matchexit):** the announcer plays at once, but the scoreboard element (with its win / lose / draw header) draws **nothing until 3000 ms after the result** and then the whole board (window, header, rows); a respawn wait armed earlier may still ask between 0x46 and 0x3f (the sender has no round gate) | cshell+0x4a608..0x4aa42 |
| 0x48 (0x3d alias) | `GS_ROUND_CLOSE` | `[round_secs:u32]` | a render variable str(VA 0x374f4d88; 629370cd955fde39; 10) set to 0 (cshell+0x26c070, through the game-variable writer cshell+0x19ff60); `hud_round_timer` set to `round_secs` (running) and hidden; `hud_banner` timed state (cshell+0xbbd00(1, `round_secs`)); `hud_infection` reset (cshell+0xb2260(0)); co-op result cleared; `hud_marker`+0x59 := 0; global flag VA 0x37560d50 := 1 (read by the kill-feed and scoreboard code, PLAUSIBLE "round over"). Consumed | cshell+0x4aac1..0x4ab4e (0x4aaf7 `read(32)`) |
| 0x41 | `GS_ROUND_TIMER_STOP` | `[round_secs:u32]` | `hud_round_timer` set to `round_secs`, hidden; flag VA 0x37560d50 := 1. Consumed | cshell+0x4ac72..0x4acc0 (0x4ac9c `read(32)`) |
| 0x47 | `GS_ROUND_OBJECTS_END` | `[unused:u32]` | value read and dropped; radar, overview map and spectator map objective layers cleared (cshell+0xb77a0(0), cshell+0xc74e0(0), cshell+0xad900(0)); the render variable of 0x48 set to 0 (cshell+0x26c070(0)). Consumed | cshell+0x4aa47..0x4aaa4 (0x4aa60 `read(32)`) |
| 0x3f | `GS_MATCH_RESULT_SCREEN` | `[result_flag:bool1][result_value:u32]` | timers hidden, sound groups stopped, map layers cleared, team / co-op score and scoreboard reset (cshell+0xc3150, cshell+0x7d950, cshell+0x93120), bounty reset, banner reset (cshell+0xbbd00(0, 1000)); `match_ui`+0x6ac9 := `result_flag`, +0x9a80 := `result_value`; if `engine_client` slot +0x8c (is-connected) is true: two "leaving match" latches are set (`match_ui`+0x6a6d, `conn_mgr`+0x11e0); then `match_ui` opens screen 7 (cshell+0x1bb140(7): interface state 5, screen manager slot +0x54(7, 0)); `match_ui`+0x9a85 := 1. Consumed. Screen id 7 is the match **result screen** (PROVEN: the screen factory cshell+0x65160 case 7 -> cshell+0x64a90 -> constructor cshell+0x1375d0, method table VA 0x374e325c; its row builder cshell+0x137720 reads the roster summary of 0x65); the meaning of the two fields is UNPROVEN. **CORRECTION 2026-10-26 (report client-matchexit; disassembly, three real-client captures):** the switch from the playing state (1) to the screen state (5) (cshell+0x1bb0b0 / cshell+0x1baf00) **ends the in-game frame**: the per-frame draw runs the HUD in state 1 and the current screen in state 5 (cshell+0x1bc590), so the local player update, its 0x64 and 0x89 senders and the HUD no longer run. OBSERVED in rust6, rust7 and realclient: the last 0x64 at most 63 ms before 0x3f and **nothing in-game after 0x3f** (no 0x64 / 0x89 / 0x8c / 0xca / 0x76; keep-alive and ack datagrams only). The player leaves 7657 / 8172 / 9266 ms after 0x3f when the result screen takes focus (cshell+0x137dd0): lobby location 0x306 {0xb} (cshell+0x18ac20) and, in the same moment, the game link closes (plain `c9 03` x 4; the realclient capture adds a reliable `C2S_BYE` 16 ms later). Why the focus comes that late is not traced. The result screen runs its exit command itself 60000 ms after its rows were filled (cshell+0x1343b0; observed 0x306 {1} after 61547 and 62000 ms), then the room is re-entered with 0x306 {1}, 0x388 and 0x38a in the same millisecond (lobby side: `PROTOCOL_2009.md` 7.1) | cshell+0x4ab58..0x4ac68 (0x4ac0e `read(1)`, 0x4ac22 `read(32)`) |
| 0x40 | `GS_TRAINING_END` | (none) | timers hidden, sound group stopped, map layers cleared, banner reset; if `session_info`+0x194 = 0: a client-shell step through slot +0x9c(1) and cshell+0x18cbf0; cshell+0x1bebf0 and cshell+0x1ee4c0 with three saved values (VA 0x3757ae6c..0x3757ae78). **Not consumed** | cshell+0x4acca..0x4ad98 |
| 0x43 | `GS_SCORES_CLEAR` | (none) | `match_ui`+0x9a50 := 0; `hud_mode_notice`+0x74 := 0; `hud_team_score`+0x68 := 0, +0x6c := 0; `hud_coop_score`+0x64 := 0, +0x68 := 0. Consumed | cshell+0x49b2a..0x49b69 |
| 0x4a | `GS_TEAM_SCORE` | `[score_team1:u8][score_team2:u8]` | `hud_team_score`+0x68 := `score_team1`, +0x6c := `score_team2` (the two team counters of the HUD). Consumed | cshell+0x4ad9d..0x4adf0 (0x4adb6, 0x4adc1 `read(8)`) |
| 0x42 | `GS_NOP_42` | (none) | debug print only. Consumed | cshell+0x49b0d |
| 0x3b | `GS_NOP_3B` | (none) | debug print only. **Not consumed** | cshell+0x49dc9..0x49ddc |

#### 2.1.1 `GS_MATCH_BEGIN` (0x45) in detail

Reads in order (cshell+0x49de1..0x4a5f0; all widths DISASM):

1. `round_secs:u32` (0x49e41). Non-zero: `hud_round_timer` shown and started with `round_secs` seconds (colour from
   the layout, cshell+0x51a50); zero: timer hidden and set to 0.
2. `show_intro:bool1` (0x49e8b).
3. `waiting_banner:bool1` (0x49e9b; called `first_round` in earlier revisions of this file). Set: `hud_side_switch`+0x6a := 1
   (cshell+0x49eab, cshell+0xcd340) and `match_ui`+0x9a95 := 1; clear: +0x9a95 := 0. +0x6a is cleared at the top of every
   0x45 (cshell+0x49dff, cshell+0xcd350) and by 0x46; nothing else writes it. While +0x6a is set the element's per-frame draw
   (method table VA 0x374da5fc slot +0x0c = cshell+0xcd170, at cshell+0xcd1f4..0xcd205) draws its picture +0x54, loaded at
   init (slot +0x14 = cshell+0xcd4d0) from the HUD layout key `TrainingModInfoTex` (getter cshell+0x58ce0): the client's
   "waiting for players to connect" training banner, top centre. So a server that sets the flag in a running round leaves
   that banner up until the round result (PROVEN-CAPTURE: rust4, Wave AA item 7; DISASM). DeCombat sends 0 (DSV-20).
4. `countdown_secs:u32` (0x49ecc): `hud_strike` starts its three-part countdown with this many seconds
   (cshell+0x9ef80, ignored when 0); `local_player`+0x484 := (`countdown_secs` == 0) (slot +0xa0 = cshell+0x5cb50;
   meaning UNPROVEN). If the own player state (`local_player`+0x30, section 4.1) is 1, a start sound is played:
   config key str(VA 0x374d0dd4; fdabbb65adcd9cf9; 15) when `waiting_banner` is set, else str(VA 0x374d0dc4;
   ea9ef9bf4d197c53; 15) (cshell+0x1b2c00 then cshell+0x1b20c0).
5. `if show_intro`: the intro block below is read **only if** `match_ui`+0x9a2c >= 0 and +0x9a30 >= 0
   **(client state)**: two interface image handles loaded when the interface starts (cshell+0x1980 stores them; a
   failed load gives a negative value). On a normally installed client both are valid, so a server always sends the
   block when `show_intro` = 1 (PLAUSIBLE). When the handles are invalid the client shows CRes string id 0xbc5 instead
   and reads nothing more here.

   Intro block (cshell+0x49fc2..0x4a41f):

   | # | field | condition | use |
   |---|---|---|---|
   | a | `mode_caption:zstr8(256)` | only if `mode_class` is 6, 9 or 11 (then `hud_team_score` text cshell+0xbfc90) or 5 (then `hud_coop_score` text cshell+0x7b370); both copy it to `client_shell`+0x16e61 first (cshell+0x47120, 260 bytes) | mode title |
   | b | `notice_head:zstr8(256)` | always | `hud_banner` title |
   | c | `notice_body:zstr8(256)` | always | if non-empty and `match_ui`+0x9a50 = 0: `hud_banner` shows title and text for 4000 ms at scale 1.0 (cshell+0xbbfc0), latch +0x9a50 := 1 |
   | d | `map_image_a:zstr8(256)` | always | overview and spectator map picture (cshell+0xc6930, cshell+0xace50, with f) |
   | e | `map_min:vec96`, `map_max:vec96` | always | overview and spectator map world bounds (cshell+0xc63f0 / 0xc63d0, cshell+0xac8e0 / 0xac8c0) |
   | f | `radar_image:zstr8(256)`, `map_image_b:zstr8(256)` | always | radar picture (cshell+0xb4ee0), second map picture |
   | g | `radar_map_scale:vec96`, `radar_map_offset:vec96` | always | the radar map window: scale pair (x and z used; formerly named `radar_min`, cshell+0x47e30) and offset pair (x and z used; formerly `radar_max`, cshell+0x47e60). Not bounds: see the rule in row h |
   | h | `radar_zoom:f32`, `radar_half:f32`, `radar_w:u32`, `radar_h:u32` | always | `hud_radar`+0x388 := `radar_zoom` (as float; **never read by the 2009 client**), +0x394 := `radar_half` (as float; formerly `radar_rot`), +0x38c := `radar_w`, +0x390 := `radar_h` (formerly `radar_p3` / `radar_p4`). DISASM (client-radar, 2026-10-25; fold: spec-fold-9): the five fields are one map window. With the point P of the camera object, the radar picture's top-left uv is `u0 = P.x * scale.x / w + offset.x - half`, `v0 = offset.z - P.z * scale.z / h - half`, and the window is 2 x `half` wide and high; north-up, not turned (`RADAR_2009.md`). The names of the first version of this catalogue (`radar_min`, `radar_max`, `radar_rot`, `radar_p3`, `radar_p4`) are historical; `dc-dserver` still writes the fields under those names, `dc-client` reads them under the new ones |
   | i | `base_team1:vec96`, `base_team2:vec96` | only if `mode_class` != 6 | team base markers on both maps (cshell+0xc6b60(pos, team, mode_class), cshell+0xacfb0); `mode_class` 6: no read, map markers cleared (cshell+0xc6440) |
   | j | `x:u8`, `y:u8`, `z:bool1` | only if `mode_class` == 5 | `y` to `hud_health` (cshell+0x96540, an empty method in this build); `x`, `z` unused |

   After the block (whether read or not): `local_player`+0x424 := 1; if the own team (`local_player` slot +0xb0 =
   cshell+0x5ca90, field +0x1e4) is neither 0 nor 0xff: `move_mgr`+0xa8 := 1 and +0x84 := 1.

   Field sources (REFERENCE, tdm4 and tdm6; dserver-round2 G4, folded by spec-fold-8): the reference stack's intro block equals the
   room's mode-row data as our lobby's room line carries it: `notice_head` = `obj`, `notice_body` = `cmt` field 0,
   `map_image_a` / `map_image_b` = `large` fields 0 / 1, `map_min` / `map_max` = `large` fields 2 / 3, `radar_image` =
   `radar` field 0, `radar_map_scale` / `radar_map_offset` = fields 1 / 2, `radar_zoom` = field 3, `radar_half` = field 4, `radar_w` /
   `radar_h` = fields 5 / 6; `base_team1` / `base_team2` = the centre of the team's spawn points whose mode filter is the
   room's mode (tdm6 bit for bit; tdm4 within one f32 step). The tdm6 0x45 of the reference stack had 2,450 bits with `show_intro` 1;
   our rust3 0x45 had 114 bits without the intro (OURS; capval-rust3 9.1), so the HUD had no radar map and no mission intro
   (PLAUSIBLE; the user did not report it). DeCombat: DSV-50 (proposed, PENDING in `DC_DEFAULTS.md`; TESTED bit for bit
   against tdm6, against tdm4 except the team bases); the mode key -> mode filter text match is the capital-letter rule of
   DSV-50, ours.
6. `if not show_intro`: nothing more is read here (the client formats a local "waiting" text that is not shown,
   cshell+0x4a514).
7. `hud_marker`+0x59 := 1 if `waiting_banner` = 0 and `mode_class` is 2 or 3, else 0.
8. `round_no:u32` (0x4a576): `hud_team_score`+0x80 := `round_no` (the field that 0x7d also writes with its `round_no`;
   PLAUSIBLE: the current round number).
9. Result banners cleared (cshell+0xc1a50(0,0), cshell+0x7caf0(0,0), cshell+0x76db0(0,0)), `hud_game_info` shown,
   `hud_banner` reset (cshell+0xbbd00(0, 1000)), `hud_crosshair`+0x2cc := 0, `match_stats` slot +0x108
   (cshell+0x63d50: stores a profile value as float). Consumed.

#### 2.1.2 `GS_ROUND_RESULT` (0x46) in detail

Reads (cshell+0x4a608..0x4aa42): `round_secs:u32` (0x4a65e: `hud_round_timer` set, then hidden),
`win_team:i8` (0x4a685, used sign-extended), `wins_team1:u8` (0x4a692), `wins_team2:u8` (0x4a69f).

- Before the reads: the render variable of 0x48 set to 0 (cshell+0x26c070(0)); `match_ui`+0x9a86 := 0; if the own player has a
  model (cshell+0x1f0840) a player reset (cshell+0x609e0).
- If `match_ui`+0x9a95 (`waiting_banner` of 0x45, formerly `first_round`) is clear: half-time team-change picture check
  (cshell+0xcd210(`round_limit`, `wins_team1`, `wins_team2`); it shows the "teams change sides" sprite for
  `mode_class` 3, 4, 8 and 13 when half the rounds are played).
- `hud_team_score`+0x68 := `wins_team1`, +0x6c := `wins_team2`.
- `if mode_class == 5` (co-op): `[stage:u8][c1:u32][c2:u32][c3:u32][c4:u32][c5:u32][c6:u32]` (0x4a734..0x4a780)
  go to the co-op result panel (cshell+0x76ee0 stage, +0x76ab0 .. +0x76a70 the six values, +0x76a80 `c1 - c5 - c4`). **`stage` selects the result banner**
  (C-FT-6, folded by spec-fold-7 from `FIRETEAM_2009.md` 2.6; PROVEN cshell+0x76ee0, cshell+0x77400): 2 -> banner 0 (mission complete), 3 or 4 -> banner 1 (failed),
  5 -> banner 2 (timed out), by the load order of the three result pictures of the co-op layout section; any other value keeps the previous banner (0 / 1 UNPROVEN).
  The six values land in panel fields +0xf8 (cshell+0x76ab0), +0xe0 (0x76a50), +0xe8 (0x76a60), +0xf4 (0x76aa0), +0xec (0x76a90), +0xf0 (0x76a70) and +0xe4 (0x76a80,
  `c1 - c5 - c4`), and the panel counts them up (cshell+0x79350..0x796a3); which value is the kills, mission, survival, time or total label is UNPROVEN (V-FT-11);
  `if stage == 2`: `[n:u32]` then n x `[name:zstr8(260)]` (cshell+0x79f00(i, name)), `[title:zstr8(260)]`
  (cshell+0x76f30(1, title)), `[text:zstr8(260)]` (cshell+0x77030), `[panel_ms:u32]`; `match_ui`+0x9a9c := `c6`.
  Otherwise (`mode_class` != 5): `hud_scoreboard`+0x138 := `wins_team1`, +0x13c := `wins_team2` (each followed by
  cshell+0x90a60), `match_ui`+0x9a9c := 0.
- Bomb timer hidden; map objective layers off; `hud_marker` and `hud_crosshair` reset; `local_player`+0x424 := 0;
  `hud_side_switch`+0x6a := 0; `match_ui`+0x9a78 := `wins_team1`, +0x9a7c := `wins_team2`.
- `mode_class` != 5: the round-result banner: `hud_team_score` cshell+0xc1a50(1, `win_team`) and `hud_coop_score`
  cshell+0x7caf0(1, `win_team`). cshell+0xc1a50 plays the first team's win sound for `win_team` = 1 and the second
  team's for 2 (GM §1.2); other values (0 = draw, PLAUSIBLE) show no team. `mode_class` == 5: cshell+0xc1a20(0),
  cshell+0x7cac0(0), co-op panel for `panel_ms` (default 1000 when not sent).
- `hud_infection` reset; if `match_ui`+0x9a85 and `mode_class` is 2 or 3: marker cleanup; if the profile flag
  `session_info`+0x408 is set: `local_player` cshell+0x60a20(1). Consumed.
- **CORRECTION 2026-10-26 (report client-matchexit; EVIDENCE cshell+0xc1a50 header texture and result time, cshell+0xc02e0 `3000 < now - result time`, cshell+0xc2490 header = child frame of the board window; DISASM cshell+0x62038..0x620db for the respawn sender: mode class, life state, armed byte and elapsed wait only):** the scoreboard keeps the result time and the header; its draw tests 3000 ms against the result time, so the win / lose / draw header appears with the board, not with the announcement. In the real-client captures (rust6, rust7, realclient) the server sent 0x46, then 0x48 and 0x65 rows 3 s later (rust7: at once) and 0x3f 6 s after 0x46 (rust7: 3 s). A death at the result arms the 7000 ms respawn wait, 0x3f comes first and the in-game frame ends there (the 0x3f row of the table in 2.1), so no respawn request is sent after the round: it is gated by 0x3f, not by 0x46.

#### 2.1.3 Observed round flow (REFERENCE; accepted by the real client, tdm1, tdm2, tdm4, botduel)

Match start, in the order the reference server sent it to a player of the round's first spawn (tdm1 seq 410..448, REFERENCE-CONFIRMED: every
message parsed exactly with `mode_class` 1 and the client went on): 0x43 (no body), 0x44 `[round_secs 0][round_no 1][round_limit 1][keep_team 0]`, 0xb5
`[0][0]`, then 0x45 (2,490 bits): `round_secs` 0, `show_intro` 1, `waiting_banner` (formerly `first_round`) **0**, `countdown_secs` 0, an intro block of five strings (43, 41, 38, 43 and
39 characters; no mode caption for class 1), map bounds and radar scale / offset pairs as fractions, `radar_zoom` 0.0, `radar_half` 0.15, `radar_w` = `radar_h` = 1024, the two team base
points, then `round_no` 0 (head `0d 45 00 00 00 00 01 00 00`). A player who joins a **running** round gets 0x43, 0x44 and 0xb5 and **no 0x45** (tdm2 conn 7). Both are
accepted. The flag (`waiting_banner`, step 3) is 0 in every reference stack 0x45; a 1 shows the "waiting for players to connect" banner for the whole round
(rust4, Wave AA item 7), so section 10 step 11 now sends 0.
The TDM room of the captures has no time limit (`round_secs` 0 in 0x44 and 0x45), so a time-limit end was never observed.

Round end (REFERENCE; the receiver of these messages was always a bot, so the real client's display of them is not covered by any capture):

| ending | order and timing | notes |
|---|---|---|
| goal reached (botduel3, goal 30) | in the tick of the deciding kill: 0x6d, 0xb5 `[30][0]`, **0x46** `[round_secs 3][win_team][wins1][wins2]`; **+3.000 s** 0x47 `[0]`, 0x6b kind 2 of every player (absolute kills and deaths), one **0x65** per player, **0x48** `[3]`; **+3.000 s** **0x3f** `[result_flag 0][result_value]` | no 0x41, no 0x99, no 0xb5 after 0x46, no server bye: leaving is the client's job (the bots left 3 s after 0x3f) |
| forfeit (the last player of one team left; tdm2, tdm4, botduel4) | 0.31-0.34 s after the leaver's 0xbd: **0x6c** `[pid][0]` + **0x46** `[3][win_team = the remaining team][0][0]` + the removal record of the leaver's objects in one datagram; **+2.70-2.72 s** 0x47, own 0x6b, one 0x65 per remaining player, 0x48 `[3]`; **+3.03 s** 0x3f `[0][win team]` | the world keeps running and accepts a re-join; no 0x99 |
| a request 0x89 after the round end | not answered (2 requests ignored) | |

`result_value` of 0x3f equals the winning team in all three samples (2, 2, 1): PLAUSIBLE, UNPROVEN for the 2009 server. The 0x65 of the forfeit
end carried `[pid 0][char = 408 (the player's character object)][exp_before 0][exp_gain 43][exp_bonus 0][gp_gain 34][gp_bonus 0][flag 0]`: the second field
is **not unused** (3.4). Amounts and delays here are reference stack values, not rules (CD-54; DSV-13, which held 0x99 8 s after the result, is retired by DSV-35). The lobby pushed 0x3004 only to the account that was still in the
match, in the tick of its 0x65 (none to the leaver); the TCP side is in `PROTOCOL_2009.md` 7.

DeCombat end set (D, DSV-35 and DSV-31 proposed; implemented in `dc-dserver` slice 4, TESTED in `game_sim`, RUN in a loopback round with two bots, no real client):
the round ends by the goal, the round timer, or a team that has had no player for 10 s (DSV-31; the reference stack ended 0.31-0.34 s after the leave, the 10 s are ours so a
disconnect does not end the round at once). In the tick of the end: 0x46 `[round_secs 3][win_team][wins_team1][wins_team2]` (the round wins **before** this round) with the
pipe's `@player` / `@result`; **+3 s** 0x47 `[0]`, a 0x6b kind-2 refresh of every roster record, one 0x65 per player (its character object, the experience before the match,
the lobby's gains from a `gain` pipe line when one arrived, else 0: `DSERVER_PIPE.md` 12.B), 0x48 `[3]`; **+3 s** 0x3f `[0][win_team]`. Never 0x41, never 0x99, no server
bye: the session ends when every client has left, or 120 s after the result screen (`-linger`). RUN (team-empty end): 0x46 10.0 s after the leave, 0x47 +3.001 s,
0x3f +3.005 s; the goal end was first RUN by stack-verify (record below). The form copies only what the real client was observed to accept (REFERENCE-CONFIRMED rows of
Appendix C); the 3 s pauses are REFERENCE timings adopted as D, not 2009 evidence.

RUN record (stack-verify, 2026-10-25: release build of the Rust stack of master 692de66, lobby + `dc-dserver` + two `dc-botclient` bots on loopback, no real client; times in seconds since the
bot's join (its event clock); report `reports/stack-verify.md`):

| round | end | 0x46 | 0x47 / 0x6b / 0x65 / 0x48 | 0x3f | notes |
|---|---|---|---|---|---|
| 1 | team empty (DSV-31): the round ended 10.001 s after the second bot left | 160.043 s | +3.014 s (one 0x65: only the remaining player) | +2.986 s | score 2 : 3 below the goal 5 |
| 2 | goal 3 (the deciding kill and the round end in the same millisecond) | 99.458 s | +2.988 s (0x6b x 2, 0x65 x 2) | +3.007 s | identical on both bots |

In both rounds every 0x65 carried the lobby's non-zero gains of its `gain` line (GAIN-1, `DSERVER_PIPE.md` 12.B), every respawn was answered 7049-7059 ms after the death on the bot's
own 0x89 (DSV-32), and the session ended with `@end` after the clients had left (no server bye; DSV-35). Grade: RUN of our own stack; it shows the DeCombat timings, not the 2009
client's display of them.

Consequence of DSV-35 for clients (client-dserver-fix, 2026-10-25): because the server never closes the session, the clients leave one by one, so **each remaining client receives
0x6c `[pid][0]` (3.2) and the removal of the leaver's objects for every earlier leaver before its own link closes**; a client that reads the result screen from its roster must keep
the departed players' records. `dc-client` keeps a departed player's track in its match record and only takes it out of the drawn set (TESTED: `net_dserver` 2 / 2 after the fix).

### 2.2 Mode objectives (bomb, objective items)

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0x51 | `GS_BOMB_PLANTED` | `[bomb_secs:u32]` (the handler first rewinds the reader to the start of the body, `seek(0)`, then reads) | plays the sound of config key str(VA 0x374d0a84; 9d55b46ede8635c0; 20); round timer set to 0 and hidden; `match_ui`+0x9a87 := 1. `bomb_secs` = 0: `hud_banner` shows CRes string id 0xbcb (white, 3000 ms). Non-zero: `hud_bomb_timer` starts with `bomb_secs` seconds and is shown (cshell+0x6ff50), `hud_banner` shows the layout's planted notice (cshell+0x573c0 / 0x574f0 / 0x57480, cshell+0xbda50); two layout values stored at `match_ui`+0x9a8c / +0x9a90. Consumed | cshell+0x4afee..0x4b23f (0x4b061 `seek(0)`, 0x4b06a `read(32)`) |
| 0x50 | `GS_BOMB_DEFUSED` | (none) | `hud_bomb_timer` hidden; `hud_banner` shows the layout's defused notice (one of two variants chosen by the object at VA 0x37560d54 slot +0x08); two layout values re-read. Consumed | cshell+0x4aeab..0x4afe4 |
| 0x4f | `GS_BOMB_EXPLODED` | (none) | `hud_bomb_timer` hidden; `hud_banner` shows the layout's explosion notice. Consumed | cshell+0x4adfa..0x4aea1 |
| 0x56 | `GS_OBJECTIVE_ITEM` | `[item:objref16]` | `hud_radar` marks the object as objective item for the own team (cshell+0xb8870(item, 0, own team, 2)). Consumed | cshell+0x4b2ca..0x4b304 (0x4b2e1 reader slot +0x68) |
| 0x66, `kind` = 0x0c | `GS_SCREEN_FLASH` | `[kind:u8 = 0x0c][fade_out:u8][unused:u8][flash_secs:f32]` | white screen flash: duration `flash_secs * 1000` ms, start time now; `fade_out` non-zero = fading from full white, zero = the inverse ramp (cshell+0x47bb0; per-frame cshell+0x485a0 uses weapon-table intensity values of the own current weapon and lowers a sound volume). Consumed. Any other `kind`: the reader is rewound to bit 0 and stage A declines, so stage C reads the same message (section 4.3) | cshell+0x49aa8..0x49b03 (three `read(8)`, one `read(32)`, `seek(0)`) |

### 2.3 Teams

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0x5e | `GS_TEAM_SWAP` | `[score_team1:u8][score_team2:u8]` | `hud_scoreboard` scores (cshell+0x92bb0); `match_ui`+0x9a78 / +0x9a7c := the scores; the own team in `session_info` is switched to the other team (1 -> 2, 2 -> 1, cshell+0x1b1330); roster re-sorted for the new team (cshell+0x293f0); spectator HUD refresh; `match_ui`+0x9a94 := 1. **Not consumed** | cshell+0x4b4d5..0x4b568 |
| 0x5f | `GS_CLAN_STATUS` | `[kind:u8]` then kind 1: `[score_team1:u8][score_team2:u8][no_swap:bool1]`; kind 2 and 3: nothing more | kind 1: team counters and scoreboard scores set as in 0x4a / 0x46; if `no_swap` = 0: the team swap of 0x5e plus team-score, spectator, scoreboard and roster refresh (cshell+0xc35a0, cshell+0xaa170, cshell+0x90820(0), cshell+0x93e60, cshell+0x29bb0). Kind 2: chat line with CRes string id 0x14ab; kind 3: chat line with CRes string id 0x14ac (layout colours cshell+0x50810 / cshell+0x508d0, 10000 ms). **Not consumed** | cshell+0x4b56d..0x4b8d8 |

### 2.4 Chat and text

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0x68 | `GS_CHAT` | `[sender_name:zstr16(32)][text:zstr16(376)][sender_id:u32][team_only:u8][whisper:u32]` | `sender_id` = 0xFFFFFFFF: system notice, `hud_banner` shows `text` in red (0xFFFF0000) for 10000 ms (cshell+0xbd920), consumed. Otherwise the roster record of `sender_id` is looked up (cshell+0x29f70); if there is none the message is **not consumed** and nothing is shown. Colour: `team_only` non-zero -> team-chat colour (cshell+0x50510); else own team (`local_player` slot +0xb0) equal to the record's team (+0x70) -> friendly colour (cshell+0x50690), else enemy colour (cshell+0x50750); a `sender_name` that starts with the 2-unit tag str16(VA 0x374cd9cc; 69bb7a1f3feb2eec; 2) gets the moderator colour (cshell+0x505d0). `whisper` non-zero: whisper line (cshell+0x75d60(text, name, whisper, colour, 10000)); zero: normal line (cshell+0x75b70(text, name, colour, 10000, 1, `sender_id` == own client id, 0)) in `hud_chat_log`. Consumed | cshell+0x4b30e..0x4b4cb (0x4b34e, 0x4b362 slot +0x50, 0x4b36b `read(32)`, 0x4b376 `read(8)`, 0x4b386 `read(32)`) |
| 0x79 (base cshell+0x1b90e0) | `GS_VOICE_LINE` | `[p1:u32][p2:u32][sound_no:u32]` | `sound_no` = 0: nothing. Otherwise a wave path is formed from the configured dialogue directory and the number (cshell+0x27d850; numbers below 1000 give an empty path) and played through cshell+0x1b1e20 (priority arguments 6, 0, 100, volume 1.0). `p1`, `p2` are read and ignored. Consumed | cshell+0x1b90e0 (C: three `read(32)`) |

### 2.5 HUD, overlays and statistics

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0x36 | `GS_HELD_ITEM` | `[weapon_label:zstr8(256)]` | `hud_held_item`+0x10 := the u16 id (+0x04) of the `weapon_db` row whose name matches case-insensitively (cshell+0x1723b0), +0x24 := 0; an empty or unknown name changes nothing (cshell+0x9b380). Consumed | cshell+0x4b244..0x4b2bb (0x4b283 slot +0x4c, size 0x100) |
| 0x77 (base cshell+0x1b90e0) | `GS_OVERLAY` | `[overlay_slot:u8][on:u8]` | `overlay_slot` >= 8: ignored (still consumed). `on` non-zero: if slot `overlay_slot` is free (`match_ui`+0x5340 + 4 x slot), an overlay sprite is created from the sprite name of the own current weapon's table row (cshell+0x1b88f0, through `local_player` slot +0x4c and cshell+0x2046f0 / cshell+0x1724e0; PLAUSIBLE: scope overlay); `on` = 0: the overlay in that slot is removed (cshell+0x1b37b0). Consumed | cshell+0x1b90e0 (two `read(8)`) |
| 0x92 | `GS_WEAPONS_USED` | `[w1:u8][w2:u8][w3:u8][w4:u8]` | `local_player` slot +0x30 returns its `match_stats` part (+0x278, cshell+0x5ca80); that object's slot +0x3c (cshell+0x633d0, DISASM: no decompiler function there) marks each weapon index that is below the `weapon_db` count (+0x15c8) as used in two per-match flag arrays (+0x30, and +0x40 when the row's field +0x78 is set). Consumed | cshell+0x4b8dd..0x4b937 (four `read(8)`) |

## 3. Stage B: player roster object (cshell+0x2c430)

Handler: `roster` slot +0x0c = cshell+0x2c430 (decompiler output present but its argument lists are garbled; every layout
below is read from the disassembly, DISASM grade for widths and stack slots, PROVEN for control flow). The switch is
`id - 0x65` through a byte table (cshell+0x2c468; byte table VA 0x3712e980, jump table VA 0x3712e924, PE-DATA). Only
0x6b, 0x6c, 0x70, 0x7d and 0xa2 return 1; every other case ends in the base default cshell+0x207120 (`return 0`),
so it is **not consumed** after taking effect (section 1.1). Several cases are gated by `conn_ok` = cshell+0x1a0470:
true when `conn_mgr` exists and its state (`conn_mgr`+0x04) is neither 4 nor 5 **(client state)**; when false the
message is ignored without being read.

### 3.1 The roster record

One record per player, found by `pid` = record +0x04 (a `u8` on the wire in 0x65, 0x6b, 0x6c and 0x6d, a `u16` in
0xa2, a `u32` in 0xc3, 0xd6 and 0xd7; compared with the own engine client id from `engine_client` slot +0x84, so a server must keep engine client ids below 256). The
record fields used by this catalogue (writers: this section; readers: the result screen cshell+0x137720 and the
roster sort cshell+0x29a50):

| record off | content (our name) | written by | evidence |
|---|---|---|---|
| +0x00 | `ping_ms` as float | 0xa2 | cshell+0x2d77b |
| +0x04 | `pid` | 0x6b | cshell+0x2a950 |
| +0x08 | `account_no` | 0x6b | cshell+0x2a950 |
| +0x0c | `name` (8-bit string) | 0x6b | cshell+0x2a950 |
| +0x28 | `character` (the player's world object, from `objref16`) | 0x6b | cshell+0x2a950 |
| +0x2c | `kills`; the roster list is kept sorted by this field, highest first (cshell+0x29a50) | 0x6b (absolute), 0x6d (+1 per enemy kill, cshell+0x2aad0) | PROVEN |
| +0x30 | `deaths` (+1 per death, cshell+0x2ab70) | 0x6b, 0x6d | PROVEN |
| +0x34 / +0x38 | current kill streak / current death streak; +0x3c / +0x40 best streaks (cshell+0x2aad0, cshell+0x2ab70) | 0x6d | PROVEN |
| +0x44 + 4 x k | per-kind kill counters, k = `kill_kind` of 0x6d (cshell+0x2ac30) | 0x6d | PROVEN |
| +0x60 | head-shot counter (+1 when `hit_zone` = 1) | 0x6d | cshell+0x2dbaf |
| +0x64, +0x68, +0x6c | three `u8` profile values shown on the result screen (meaning UNPROVEN; PLAUSIBLE rank / level class values) | 0x6b | cshell+0x2a950, cshell+0x137720 |
| +0x70 | `team` (1, 2; 0 and 0xff = no team) | 0x6b, mode sub-messages | cshell+0x2a950 |
| +0x74 | `dead` flag (set by 0x6d for the victim) | 0x6b, 0x6d | cshell+0x2dac2 |
| +0x18c..+0x19c | match summary: experience before, experience gained, experience bonus, GP gained, GP bonus | 0x65 | section 3.4 |
| +0x1a0 / +0x1a4 / +0x1a8 | three values of the `look` block (its first, fifth and sixth byte) | 0x6b | cshell+0x2c8d1 |
| +0x1ac, +0x1af, +0x20c | flags (meaning UNPROVEN; +0x1af = "has group" flag of 0x6b) | 0x6b | cshell+0x2a950 |
| +0x1b0 | `band_name` (16-bit string; PLAUSIBLE clan name) | 0x6b | cshell+0x2a950 |
| +0x1cc..+0x1dc | group values (low 32 bits of a 64-bit number at +0x1cc) | 0x6b | cshell+0x2a950 |
| +0x1e0 | measured round-trip time in ms | 0xc3 sub 3 | cshell+0x2adb6 |
| +0x1f0 | own-record flag | 0xca | cshell+0x2e602 |
| +0x1f4, +0x1f8, +0x1fc, +0x200, +0x204, +0x208, +0x210 | mode values (co-op points and others) | 0x6b, 0x65, 0xd7 | cshell+0x2a950, cshell+0x2e423 |

`roster`+0x08 holds the own client id (cshell+0x2a950 refreshes it), `roster`+0x10 the id of the player who last
killed the local player (0x6d), `roster`+0x3c / +0x40 the two team round counters (0x7d, 0xb5), +0x18..+0x24 the
group match values (0xc0).

### 3.2 Players joining, leaving, settings

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0x6b | `GS_PLAYER_INFO` | gate `conn_ok`; then see 3.2.1 | adds or refreshes a roster record and every HUD list | cshell+0x2c729..0x2cf98, 0x2cf9d..0x2d2df |
| 0x6c | `GS_PLAYER_LEFT` | gate `conn_ok`; `[pid:u8][kicked:u8]` | `pid` = own id: ignored. Unknown `pid`: ignored. Otherwise the player is removed from `hud_team_score`, `hud_coop_score`, `hud_spectator`, `hud_scoreboard` (cshell+0xc58f0, 0x7edc0, 0xa9240, 0x939d0), from radar and both maps by its object (cshell+0xb8540, 0xc7c70, 0xae010), from the markers and the vote panel (cshell+0x8aa00, 0x89ea0, 0xce9e0); if its team is not 0 / 0xff a chat line is added: CRes string id 0x185b when `kicked` is non-zero, else 0xbb9, 5000 ms (cshell+0x75b70); then the record is deleted (cshell+0x2bc70). Consumed | cshell+0x2d4d5..0x2d733 (two `read(8)`) |
| 0x7d | `GS_MATCH_SETTINGS` | `[lobby_caption:zstr8(64)][unused:u8][own_team:i8][own_value:u32][goal:u8][score_team1:i8][score_team2:i8][round_limit:i8][round_no:i8]` then `if mode_class is 7 or 8: [n:u32] n x [flag_team:u32][carrier:u32][carrier_team:u32]` | `lobby_caption` is read into a local buffer and not used further; `local_player`+0x1fc := `own_value`; `hud_team_score`+0x70 := `goal`, +0x74 / +0x78 := the two scores, +0x7c := `round_limit`, +0x80 := `round_no`, +0x12c := `mode_class`; `roster`+0x3c / +0x40 := the two scores; `mode_rules` reset (cshell+0x5b9e0, cshell+0x2b7e0) and, for mode 7 / 8, n flag-carrier triples `[flag_team][carrier][carrier_team]` passed to `mode_rules` slot +0x58 (GAME_MODES_2009 C-GM-6, PROVEN cshell+0x5bcd0: the same carrier record as 0xc6); own team set (`local_player` cshell+0x5d9e0(0, `own_team`, 0, 0, 0)), `local_player`+0x47a := 0; `hud_side_switch` reset; `hud_scoreboard` mode layout (cshell+0x92570(`mode_class`)) and header (cshell+0x92e20(`goal`, team scores, `round_limit`, `round_no`): +0x114 `goal`, +0x124 / +0x138 and +0x120 / +0x13c the scores, +0x118 `round_limit`, +0x11c `round_no`); `hud_mode_notice` mode (cshell+0xcd110); co-op panel (cshell+0x76df0(`goal`)); `local_player`+0x268 := 1. Consumed. `goal` is drawn as a number on the scoreboard header (cshell+0x916a0 for mode classes 1 and 4, round limit text for the round modes 2, 3, 12, 13, cshell+0x91310); PLAUSIBLE: the kill or round target | cshell+0x2c4ff..0x2c724 |
| 0xa2 | `GS_PING_TABLE` | `{[pid:u16][ping:u16]}` repeated, ended by `pid` = 0xFFFF | for each known `pid`: record +0x00 := `ping` as float (shown on the scoreboard, PLAUSIBLE). Consumed | cshell+0x2d73a..0x2d798 |
| 0xca | `GS_SPRINT_ALLOWED` (formerly `GS_OWN_FLAG`) | `[allowed:bool1]` | own record (cshell+0x2aca0) +0x1f0 := `allowed`, the byte that the sprint test of `MOVEMENT_2009.md` §7 requires (SPRINT control bit **and** this byte). **PROVEN-CAPTURE by the client's reaction (tdm1, tdm2):** the reference stack answered each client 0xca whose `sprinting` bit changed with this message 15-79 ms later (48 times, 24 x 1 and 24 x 0, only to the sender); in the 22 updates that carried SPRINT but were sent before the echo `[1]` arrived the speed stayed exactly at the run plateau; the first update after the echo was 60-100 units faster and the sprint plateau (about 1.39 x run) followed. Without `[1]` the client reports SPRINT but runs at run speed. When to refuse is server policy (stamina is a DeCombat rule). The causal step is PLAUSIBLE (the pre-echo window is short) | cshell+0x2e5d8..0x2e602 |
| 0xd5 | `GS_OBJECT_LABEL` | `[obj:objref16][flag:bool1][label:zstr8(65)]` | the shell object of `obj` (client shell slot +0xd4, cshell+0x20ff10) gets the label and flag (cshell+0x201ea0; PLAUSIBLE: a name tag over a character) | cshell+0x2e627..0x2e698 |

#### 3.2.1 `GS_PLAYER_INFO` (0x6b) layout

Reads in order (DISASM cshell+0x2c75f..0x2c9fa; record offsets from the record writer cshell+0x2a950, its 30
arguments traced back to the read slots):

| # | field | record | notes |
|---|---|---|---|
| 1 | `name:zstr8(64)` | +0x0c | |
| 2 | `pid:u8` | +0x04 | |
| 3 | `account_no:u32` | +0x08 | |
| 4 | `flag_a:bool1` | +0x20c | for the own player also `session_info`+0xac |
| 5 | `character:objref16` | +0x28 | the player's world object (it must already exist on the client) |
| 6 | `look` block: 15 x `i8` then `look_a:zstr8(128)`, `look_b:zstr8(128)` | +0x1a0, +0x1a4, +0x1a8 (bytes 1, 5, 6) | parsed by cshell+0x1c2a90 into a character-look object; byte 5 is clamped to >= 0 and byte 6 to >= 1; meanings UNPROVEN |
| 7 | `kills:i16` | +0x2c | |
| 8 | `deaths:u8` | +0x30 | |
| 9-11 | `v64:u8`, `v68:u8`, `v6c:u8` | +0x64, +0x68, +0x6c | |
| 12 | `team:u8` | +0x70 | |
| 13 | `flag_b:bool1` | +0x74 | |
| 14 | `kind:u8` | | 0 = new player with notice, 1 = add or refresh (cshell+0x2a950), 2 = add or update **without a notice** (cshell+0x2b820 updates the record of `pid` in place and falls back to cshell+0x2a950 when it has none; clean decompile, corrected by tdm2), other = no record change |
| 15 | `has_group:bool1` | +0x1af | |
| 16 | `if has_group: [band_name:zstr16(128)][group_no:u64][g1:u32][g2:u32][g3:u32]` | +0x1b0, +0x1cc (low half), +0x1d0, +0x1d4, +0x1dc | absent: name empty, +0x1cc = 0, +0x1d0 = +0x1d4 = -1, +0x1dc = 0 |
| 17 | `h1:u32`, `h2:u32` | +0x1d8, +0x210 | **CORRECTION 2026-10-26 (client-hudgaps):** `h2` (record +0x210) is the **rank** of the world mark: the HUD mark entry (cshell+0x8a760) takes it from this field (DISASM cshell+0x2c983..0x2c990, cshell+0x2ce8e) and shows the level table's small rank icon for that rank number (cshell+0x2683f0 / 0x267dc0 / 0x268810), extension cut, plus the "_O" form (alive) or "_G" form (dead flag) (cshell+0x88a40 / 0x88bf0), 29 x 29 pixels, left of the centred caption (DISASM cshell+0x8bb94..0x8bd03). Rank 0 is the trainee icon, which shows as a yellow "T": it is the **rank icon, not a team tag**. |
| 18 | `e1:bool1`, `e2:bool1` | (read, not stored by cshell+0x2a950) | |
| 19 | `e3:bool1` | +0x1ac | |
| 20 | `unused:u8` | | read and dropped |
| 21 | `m1:u8`, `m2:u32`, `m3:u32`, `m4:u16` | +0x1f8, +0x1f4, +0x1fc, +0x200 | mode values |

Effects: if `pid` is the own client id, the own team becomes `team` (`local_player` cshell+0x5d130, `hud_marker`+0x2c,
`session_info` cshell+0x1b1330, `hud_scoreboard` cshell+0x90820) and `session_info`+0xac := `flag_a`. Kind 0 and
`pid` not own: the record is added and a chat notice is formed from CRes string ids 0x3f7 / 0xbb8 (cshell+0x1a0190,
cshell+0x1a0280). For kinds 1 and 2 the roster then calls cshell+0x2bd40(`pid`) when `character` is none and cshell+0x2bac0(`pid`) otherwise (DISASM cshell+0x2cade..0x2cbcd).
Then the character object gets the `pid` and `team` (client shell slot +0xd4, cshell+0x20fbe0(0x18, obj), its slots
+0x88 and +0x80), and the player is entered into the team score, co-op score, scoreboard, target-name, spectator,
radar, marker and both map lists (cshell+0xc5590, 0x7ea90, 0x91c90, 0xca6d0, 0xa9800, 0x93f50, 0xb8870, 0xb8590,
0x8a760, 0xc8030, 0xc7cc0, 0xae420, 0xae080). Consumed. Grade: layout DISASM; field meanings PLAUSIBLE except
`name`, `pid`, `kills`, `deaths`, `team`, which are PROVEN by their use.

#### 3.2.2 Observed roster, identity and join sequence (tdm1, tdm2, tdm4)

- **0x72 `GS_IDENTIFY_REQUEST`** (`0d 72`, no body): PROVEN-CAPTURE (client reaction) that the real client answers at once, 0x72 then 0x85 in the same 16 ms tick (tdm1 seq 406 / 408;
  tdm2 and tdm4 likewise). The 3000 ms identification fallback of DSV-12 is never needed with this client.
- **0x9e `GS_GAME_ENDPOINT`** (`0d 9e 01 ...`): REFERENCE-CONFIRMED layout with `mode_class` 1 for TDM, a 13-character host text and the game port; the intro block of 0x45 then
  parses with class 1 and the client went on. The client branch that would prove class 1 on its side (0x89 after a death) was exercised only by the bot (tdm4).
- **0x7d `GS_MATCH_SETTINGS`** (tdm1 seq 406, 152 bits, exact parse): `lobby_caption` = 5 characters (the receiving player's own nickname in tdm2), `unused` 1 for the room host and 0 for the
  guest, `own_team` 0 (corrected by the following own 0x6b: team 1), `own_value` 0, `goal` 100 = the room's goal set by the lobby (0x3035 type 8), scores 0 / 0, `round_limit` 1, `round_no` 1.
  REFERENCE-CONFIRMED (layout and the source of `goal`); the display is not visible on the wire.
- **0x6b `GS_PLAYER_INFO`:** three records for pid 0 in tdm1 (486 bits each, exact parse): `account_no` 1002, `character` 408, the `look` block equal to the client's own 0x72 look bytes (the reference stack
  forwards them unchanged: REFERENCE-CONFIRMED pairing), `team` 1, `kind` **2**, `has_group` 0. **Ordering (PROVEN-CAPTURE by the client's reaction):** a 0x6b whose `character` the client has not
  created yet makes the client queue the pid and ask with `UP_ROSTER_QUERY` 0x8c `[pid]` about 1 s later (tdm1: pid 0, the own id; tdm2 conn 6: pids 0 and 1 at 64.172 after both 0x6b arrived
  before the creates); on a link where every 0x6b named an object that already existed (tdm2 conn 7) no 0x8c was sent. Send the create record first and answer 0x8c with the 0x6b of that pid.
- **Kinds (REFERENCE, accepted; tdm2):** the joiner receives kind 2 for itself (first with `dead` 1, again with `dead` 0 after its spawn set) and kind 2 for every other player; every other client
  receives the joiner's record as kind 0 (notice, `dead` 1) directly followed by kind 2 (`dead` 0), in a starting room and in a running match alike, after the joiner's 0x72 and before its 0x87
  (47-62 ms after 0x72, 63-125 ms before 0x87). Kind 1 was never sent. A later 0x6b on a 0x8c query is kind 2, to the sender only.
- **Spawn set before 0x87:** the reference stack sends the whole spawn set, including **0x67 `[1]`**, before the joiner's 0x87 (tdm1: `0d 67 01` at seq 410, 0x87 47 ms later; tdm2: 63-109 ms) and no 0x67 state 3 first;
  the client accepted this as the in-world condition (UDP_SESSION_2009_A 8.3 d, PROVEN-CAPTURE). DC-C2S-10's state 3 before 0x87 stays a valid DeCombat choice; it is the untested path.
- **0x6c `GS_PLAYER_LEFT`** `0d 6c 01 00` (pid 1, not kicked) went to the remaining client in the datagram that removed the leaver's objects (REFERENCE).
- **0xa2 `GS_PING_TABLE`:** `[pid 0][ping 0]` + end marker `ff ff`, **unreliable**, every 3.0 s (2.97-3.06 s; 52 in tdm1). REFERENCE only; the client shows no reaction either way (PD-S2C-4 is not
  contradicted).
- **0x68 `GS_CHAT`:** the echo of each client line arrived 47 ms later with `sender_id` 0 (the own id), `team_only` 0, `whisper` 0, `sender_name` 5 units and a `text` of the form name, space, colon,
  space, line (the reference stack prefixes the name). REFERENCE-CONFIRMED layout; whether the 2009 client prints the sender name itself (so that the prefix would show twice) is not visible on the wire
  (screenshot needed). DeCombat sends the line without a prefix, as 2.4 reads.
- **0x90 / 0x88 / 0x9d:** see 7.2.

### 3.3 Kills, deaths, multi-kills

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0x6d | `GS_KILL` | gate `conn_ok`; see 3.3.1 | kill feed, kill / death counters, streaks, revenge, kill sounds | cshell+0x2d922..0x2e2ed |
| 0xc8 | `GS_MULTI_KILL` | `[kills:u16][head:bool1][combo:u32]` (the first field is a kill count, **not** a weapon) | DISASM (dserver-killforms, 2026-10-25; fold: spec-fold-9): `combo` > 1: slot 10 of `hud_multi_kill`, the "N COMBO" banner with the digits of `combo` (cshell+0xa52a0(`head`, `combo`, 0)); its sound is the head-shot key only when `head` is set, otherwise the banner is silent. `combo` <= 1: cshell+0xa57b0(`head`, `kills`, 0): `head` set gives the head-shot banner and voice, else `kills` > 0 gives the kill-count banner with the digits of `kills` and the one-kill key, else nothing. No name test: it shows on whichever client receives it. 0xc8 is a combo banner the reference stack never sent, and DeCombat does not send it either (the multi-kill level travels in the `kill_kind` of 0x6d); its 2009 use is a mode-specific combo counter that DeCombat does not need | cshell+0x2d8ca..0x2d918 |

#### 3.3.1 `GS_KILL` (0x6d) layout and effects

| # | field | width | use |
|---|---|---|---|
| 1 | `victim:u8` | read(8) | victim `pid` |
| 2 | `killer:u8` | read(8) | killer `pid`; 0xff = no killer (world / fall) |
| 3 | `kill_kind:u8` | read(8) | the **multi-kill level** 0..7+, chosen by the server (the client keeps no time window): index of the killer's per-kind counter (record +0x44 + 4 x `kill_kind`, cshell+0x2ac30) and the first argument of the announcement (step 9). 0 = nothing is announced (this is the **team-kill form**: with `streak_flag` 0 and `f_feed2` 0 it suppresses the banner, the head-shot banner, the voice and the note); 1 = single kill, 2 = double kill, 3 = "multi kill", 4 / 5 / 6, 7 and above the top level. Only the killer's own client shows the banner and voice; other clients get a note line for levels 2..7 |
| 4 | `weapon:u16` | read(16) | weapon number for the kill-feed icon (kill feed argument 3) |
| 5 | `hit_zone:u8` | read(8) | 1 = head (head-shot mark, killer +0x60 += 1); 5 = **groin** (the kill feed's groin icon and the groin banner decoration; the own profile kill sound is not played) |
| 6 | `f_relation:bool1` | read(1) | when set, the kill feed gets the victim's relation to the own team (1 = same team, 2 = other team); the relation picks the **flag-carrier icon** of the feed line (a CTF carrier kill), not a team-kill mark |
| 7 | `f_feed2:bool1` | read(1) | the **ice-crush** form: the feed line's ice-crush icon and the ice-crush banner (slot 12) with its voice |
| 8 | `keep_hud:bool1` | read(1) | when clear, the victim is greyed out in the team score, co-op score, scoreboard, radar, maps and markers (cshell+0xc3110, 0x7d910, 0x93970, 0xb8590, 0xc7cc0, 0xae080, 0x89d30) |
| 9 | `f_feed6:bool1` | read(1) | the feed line's **gold icon** |
| 10 | `streak_flag:u8` | read(8) | non-zero selects the **first-kill banner** form (cshell+0xa5fe0 is called with level 0 and its second argument 1: first-kill slot and first-kill voice) |
| 11 | `feed_value:u8` | read(8) | picks the feed line form: 5 or 10 = the suicide line; with an empty killer 7 = barrel, 3 = burn, 9 = poison death images; any other value = the normal line (the reference stack and DeCombat send 2) |
| 12 | `no_count:bool1` | read(1) | when set, kills / deaths are **not** counted for this event |
| 13 | `has_revenge:bool1` | read(1) | |
| 14 | `if has_revenge: [revenge_value:u32]` | read(32) | the local victim gets the revenge panel (`hud_revenge` cshell+0xbb670(1, killer, `revenge_value`, 7000)) |
| 15 | `f_revenge_done:bool1` | read(1) | when set and the killer is the local player: revenge panel closed (cshell+0xbaff0) and the victim's marker removed |
| 16 | `coop_a:u32`, `coop_b:u32` | read(32) x 2 | victim values for `hud_coop_score` (cshell+0x7f050, cshell+0x7f0d0); `coop_b` non-zero also lets a new local kill sound start while the previous one still plays |

Effects in order (cshell+0x2da33..0x2e297):

1. `mode_class` 5: co-op multi-kill note (cshell+0xa70e0(`victim`)).
2. Victim record: +0x74 := 1 (dead); greyed out unless `keep_hud`; co-op values.
3. If the victim is the local player: `roster`+0x10 := `killer` (or -1 when the local player killed himself).
4. `hit_zone` 1: killer record +0x60 += 1.
5. If the killer is the local player and `hit_zone` != 5: a kill sound chosen from the own profile (cshell+0x1e2010,
   cshell+0x223430, cshell+0x223a10) is played (cshell+0x1b1e20), its handle kept in `roster`+0x14.
6. Counting (only when `no_count` = 0): killer known, `killer` != 0xff, `killer` != `victim` and the two records on
   different teams: killer kills +1 and streak (cshell+0x2aad0), victim deaths +1 (cshell+0x2ab70), team / co-op score
   updates (cshell+0xc5960, 0xc5ae0, 0x7ee50, 0x7eff0), for mode class 6 the scoreboard value (cshell+0x909a0),
   and an infection-HUD note (cshell+0xb10d0) when the killer is the local player and the victim's character object
   has its flag +0x5bc set. Same team: nothing is counted. No killer or self
   kill: victim deaths +1 only. (Mode class 9 with `mode_rules`+0xb4 = 1 counts nothing.)
7. Victim is the local player: notice text CRes string id 0xbba.
8. Kill feed line (`hud_kill_feed` cshell+0x9d4a0) with: killer name (wide), victim name (wide), `weapon`,
   `f_revenge_done`, relation (step 6 rule), `f_feed6`, head-shot flag, killer colour, victim colour (HUD layout
   colours: friendly / enemy, or team 1 / team 2 when the local player is in state 6 = spectator, cshell+0x514d0,
   0x51590, 0x5a870, 0x5a880), `feed_value`, `f_feed2`, groin flag (`hit_zone` 5).
9. Own kill (the killer's name equals the own name, cshell+0x28d80 and a wide compare): own multi-kill counter
   (cshell+0xa3320(head-shot flag)). Any known killer: per-kind counter +1 (cshell+0x2ac30(`killer`, `kill_kind`))
   and a `hud_multi_kill` announcement with the killer's name, team, group flag and group name, its form chosen by
   `streak_flag` (cshell+0xa5fe0).

Kill-feed icons (cshell+0x9c3d0, dserver-killforms 1.1): relation 1 / 2 = flag image, `f_feed6` = gold, `f_feed2` = ice-crush, groin = nut-shot, head = head-shot (**CORRECTION 2026-10-26, client-hudgaps: there is no team-kill mark; a skull in a team-kill line is the head-shot icon of a zone-1 hit**; the other marks of the line builder cshell+0x9d4a0 in order: payback mark, weapon icon, head icon (zone 1), groin mark (zone 5), then at one x the flag mark (victim's team equals the own team: the enemy-flag image, else the team-flag image; DISASM cshell+0x2e21e..0x2e245), bounty mark, freeze mark, then the victim name; special forms by feed value 5 / 10 self-kill, and with an empty killer name 7 barrel, 3 fire, 9 toxic). There is **no team-kill form in the feed**: a team kill shows the normal line with both names in the same colour. The announcement's voice line is played by the client itself from the 0x6d (no separate sound message); the one-kill key has no file in the 2009 table, so the one-kill banner is silent.

The client therefore **keeps its own kill and death counts** from the stream of 0x6d messages; a server only sends
absolute values in 0x6b (join / refresh) and the team totals in 0x4a / 0x46 (section 2).

#### 3.3.2 Observed kill message and what surrounds it (REFERENCE; tdm4, botduel3/4)

0x6d of a rifle kill, 151 bits to both links in the same block (head `0d 6d 00 01 01 91 00 03 10 20 ...`): `victim` 0, `killer` 1, `kill_kind` 1, `weapon` 145, `hit_zone` 3 (a body hit, not 1), `f_relation` 0,
`f_feed2` 0, `keep_hud` 0, `f_feed6` 0, `streak_flag` 1 on the first kill of a life and 0 afterwards, `feed_value` 2, `no_count` 0, `has_revenge` 0, `f_revenge_done` 0, `coop_a` = `coop_b` = 0: a well-formed 3.3.1
message, REFERENCE-CONFIRMED (the real client kept its 15 Hz stream and its session for 60 s afterwards, sent no roster query, and walked to the dropped HiSec case, which the reference stack registered). What these fields mean is **resolved** (DISASM, dserver-killforms, spec-fold-9; table 3.3.1: `kill_kind` 1 = a single kill, `streak_flag` 1 = the first-kill banner, `feed_value` 2 = the normal feed line); which values the
2009 server itself sent is still unknown (11.2 item 8). **Team kill totals travel in 0xb5** `[kills team 1][kills team 2]` sent to everyone right after each 0x6d (and `[0][0]` at the
match start, after 0x44); **0x4a and 0xc8 were never sent** in any capture (0xc8 is a combo banner the reference stack never sent). The client counts a kill itself from 0x6d (3.3.1 step 6), so 0x4a is not needed for the HUD team counters; 0xb5 feeds the scoreboard values
(cshell+0x92bb0). The kill messages go out **before** the victim's own 0x74, 0x66 and 0x67 of the same hit. A kill block in the reference stack's order (bot link, 25 entries): 0xa6 damage note, file lists and variables for
the dropped case and the corpse, 0xd6 sub 0 and sub 2, 0xa6 type 0x17 note, **0x6d**, **0xb5**, 0x74, 0xa6 health and armour notes, 0x66 health 0, 0x66 armour, **0x67 `[3]`**, corpse files, **0x67 `[2]`**.
At the match end the reference stack refreshes every player with 0x6b kind 2 (absolute kills and deaths) before the 0x65s (2.1.3).

tdm6 (seven kills: the real client was killed six times and killed once; REFERENCE, both links identical; capval-tdm6 section 7, folded by spec-fold-8): every 0x6d had
`kill_kind` 1, `f_relation` 0, `feed_value` 2 and no revenge block, as in tdm4, and **`streak_flag` was 1 only on the first kill of the match** (190.531, seq 3006), 0 on the
other six, among them the bot's first kill after its respawn (254.250) and the user's first kill (246.609). This replaces the tdm4 reading "1 on the first kill of a life"
above (tdm4 had one kill; botduel3, whose killer never died, and botduel4 fit both readings); meaning DISASM: `streak_flag` non-zero is the first-kill banner form (3.3.1 row 10), so the reference stack announced "first kill" once per match. `hit_zone`
equals the `[Skeleton0]` location of the skeleton node that the killing 0x82 named (tdm4 node 11 -> 3, tdm6 head node 53 -> 1, the bot's node 24 -> 3; C2S 4.1, 4.1.1), so
the user's head kill carried the head-shot mark (3.3.1 step 4). The HiSec case messages 0xd6 sub 0 and sub 2 came in the kill block before 0x6d (`HISEC_2009.md` 3.5). Our
rust3 round, with the first-kill rule of that build, set `streak_flag` 1 on 6 of 7 kills and the user saw the first-kill announcement on every kill (OURS + playtest report;
capval-rust3 R4).

DeCombat kill form (D, DSV-33 proposed; `dc-dserver` slice 4, TESTED: the writers equal the tdm4 heads byte for byte; RUN: 7 kills in a loopback round): 0x6d with
`kill_kind` 1, `hit_zone` from the hit (DSV-30: the ROM location of the skeleton node the client named, else 1 head / 2 torso by the box), `f_relation` 0, `streak_flag` 1 on
the match's first kill only (DSV-33 correction of dserver-fixes, PENDING in `DC_DEFAULTS.md`; the first wording "1 on the killer's first kill since its
spawn and 0 afterwards" is superseded by tdm6, above), `feed_value` 2, the other flags 0; then 0xb5 `[team 1 kills][team 2 kills]` to everyone (and `[0][0]` in every round
start set); **0x4a is never sent** (DC-KILL-3 now reads "never 0x4a"; the earlier wording "in addition to 0x4a" is superseded); the dserver-killforms branch (merged into master, not yet verified with the real client) moves the multi-kill rule into the 0x6d: `kill_kind` = the chain level (+1 per foe kill within 4 s of the killer's previous foe kill, reset by the killer's death, minimum 2 to announce, cap 7; D, DSV-105 staged), 0 for a team kill (counts nothing, announces nothing; DSV-106 staged), and **0xc8 is not sent** (a combo banner the reference stack never sent; DeCombat does not send it; DC-C2S-13 amended). The kill messages precede the
victim's own 0x74 / 0x66 / 0x67 of the lethal hit. The meanings of `kill_kind` 1, `f_relation` 0 and `feed_value` 2 are now DISASM (3.3.1); the user's view of the banners and the kill feed on the Rust stack is the pending check.

### 3.4 Match summary and result screen data

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0x65 | `GS_PLAYER_SUMMARY` | `[pid:u8][char:objref16][exp_before:u32][exp_gain:u32][exp_bonus:u32][gp_gain:u32][gp_bonus:u32][flag:bool1]` then `if mode_class == 5: [c1:u32][n:u32] n x [bool1] [c2:u32][c3:u32][c4:u32]` | stores into the record of `pid`: +0x18c `exp_before`, +0x190 `exp_gain`, +0x198 `exp_bonus`, +0x194 `gp_gain`, +0x19c `gp_bonus`, +0x1ad `flag`, +0x1ae := (`exp_bonus` + `gp_bonus` > 0), co-op +0x208 `c1`, +0x204 `c2`, +0x1f4 `c3`, +0x1fc `c4` (0 when not co-op); re-sort (cshell+0x29a50); marker update (cshell+0x8aa00). The result screen (cshell+0x137720) later shows per record: experience = (`exp_gain` - `exp_bonus`) + `exp_bonus`, GP = (`gp_gain` - `gp_bonus`) + `gp_bonus` (cshell+0x136920), a rank-up mark when the rank of `exp_before` + `exp_gain` differs from the rank of `exp_before` (rank function cshell+0x267f10), and adds the own record's kills (+0x2c) and deaths (+0x30) to the profile totals (`session_info` +0xfc / +0x100). Not consumed. Meanings PROVEN by the result screen use; the debug line of the handler names the two gain fields (str(VA 0x374ce890; 5f51548cc42232f0; 53)) | cshell+0x2e2fd..0x2e4c6 |
| 0xc0 | `GS_GROUP_SUMMARY` | `[exp_team1:i8][exp_team2:i8][gp_team1:i8][gp_team2:i8]` | `roster`+0x18 / +0x1c := group experience of team 1 / 2, +0x20 / +0x24 := group GP of team 1 / 2 (PLAUSIBLE: clan-match result). Not consumed | cshell+0x2e4cb..0x2e585 |
| 0xb5, 0xc4 (same body) | `GS_ROUND_WINS` | `[wins_team1:u8][wins_team2:u8]` | `roster`+0x3c / +0x40 := the values; `hud_scoreboard` scores (cshell+0x92bb0). Not consumed. Carrier of the TDM team kill totals after each 0x6d (REFERENCE-CONFIRMED, 3.3.2); DeCombat sends it after every kill and `[0][0]` at each round start (D, DSV-33) | cshell+0x2d894..0x2d8be |

### 3.5 Mode-specific roster messages

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0xd7 | `GS_COOP` | `[sub:u8]` then: sub 1 `[pid:u32][obj:objref16][x:u32][points:u32]`; sub 2 `[a:u8][b:u8]`; sub 3 `[pid:u32][x:u8][points:u32]`; sub 4 `[running:bool1][secs:u32]`; sub 5 nothing; other subs: a debug line | sub 1: record +0x1f4 := `points`; if `pid` is own: marker (cshell+0x8ad40(`x`)) and scoreboard value (cshell+0x91c90); co-op score (cshell+0x7ef20(`pid`, `points`)). Sub 2: own record +0x1f8 := `b`, `local_player` cshell+0x5df50(`a`, `b`), target-name cshell+0xca6d0(`b` - 1). **Sub 2 in detail** (C-FT-5, spec-fold-7; `FIRETEAM_2009.md` 2.1-2.3): cshell+0x5df50 re-arms the 7000 ms respawn wait only when the own player state is 2, 3, 5 or 6 **and** `a` equals the stored `b` (PROVEN; to a living player sub 2 only sets the count); cshell+0xca6d0 shows CRes string id 0x5a0 (a respawns-remaining text) with `b - 1`, and a negative `b - 1` hides the count (DISASM cshell+0xca715..0xca72b, cshell+0xcbea0); so `b` counts the lives including the current one. The same field +0x1f8 is written by 0x6b field 21 `m1` (3.2.1, cshell+0x2a950), so the first count can come with the roster record. The class-5 death branch reads +0x1f8 when it processes 0x67 state 2 (GAME_MODES 4.2), so a new count must arrive before the death message. Sub 3: as sub 1 without the record. Sub 4: `hud_round_timer` shown and set to `secs`, running when `running` = 1, frozen when 0 (cshell+0xbe0c0 third argument). Sub 5: `hud_multi_kill` reset (cshell+0xa4df0). Not consumed | cshell+0x2d2e4..0x2d4d0 (sub-id jump table VA 0x3712e9f8) |
| 0xd6 | `GS_HISEC_CASE` | `[sub:u8]` then sub 0 nothing; sub 1 `[case_key:u32][player:u32]`; sub 2 `[case_key:u32][owner:u32]` (owner 0xFFFFFFFF = nobody); other sub-ids read nothing | sub 0: chat line CRes string id 0x18c1 (yellow, 5000 ms); sub 1: chat line with the player's name and CRes string id 0x18c2, `hud_multi_kill` note, `hud_hisec` entry removed (cshell+0x9a3e0), own player: cshell+0x993e0; **a `player` that is not in the roster does nothing at all** (the case entry is not removed); sub 2: `hud_hisec` cshell+0x9a620(`case_key`, `owner`): insert or update the owner of the case entry. `case_key` is the key of the HiSec pickup object (type 0x17 kind 5, 8.2). Not consumed | cshell+0x2e6a4..0x2e834; HISEC_2009 3.2 (DISASM) |
| 0xdb | `GS_RESPAWN_TOKEN` | `[k:u8][sub:u8]`, ignored unless `k` = 0; sub 1 `[n:u32]`, sub 6 `[n:u32]`, other subs nothing | `hud_respawn_token`: sub 1 cshell+0xba6d0(`n`); sub 2 state 1 or 2 depending on cshell+0xba6f0; sub 3 state 6; sub 4 state 0; sub 6 cshell+0xbac00(1, 1, `n`); sub 7 cshell+0xbac00(0, 1, 0); sub 5 nothing. Not consumed | cshell+0x2e83b..0x2e8db (jump table VA 0x3712ea0c) |
| 0xcb | `GS_BOUNTY_MODE` | `[sub:u8]` (0..6) then: sub 0 `[value:u32][team:u8][pid:u32]` + `look` block (corrected 2026-10-24, GAME_MODES_2009 C-GM-1, DISASM cshell+0x2be52..0x2bec3: the roster lookup compares the **third** field; the first two were listed swapped); sub 1 `[a:u32][b:u32][c:u32]`; sub 5 `[k:u8]`, if `k` = 1: `[title:zstr8(260)][text:zstr8(260)]`; sub 6 `[a:u32][b:u32]`; subs 2, 3, 4 nothing | role and team changes of the bounty mode (`hud_bounty` cshell+0x71c30 states, `mode_rules` notes, banner cshell+0xbbfc0 for sub 5). Detailed effects UNPROVEN. Not consumed | cshell+0x2bd80 (jump table VA 0x3712c2dc) |
| 0xd3 | `GS_INFECTION_MODE` | `[sub:u8]` (0..4) then: sub 0 `[team:u8]`, `if team == 2: [infector:u32]`, `[pid:u32]` + `look` block and nothing more (corrected, C-GM-2, DISASM cshell+0x2a26a..0x2a2a6); sub 1 `[a:u8][b:u8][flag:bool1][secs:f32]` (C-GM-3: the last field is used as a float, the seconds of the infection countdown, PROVEN cshell+0xb1190); sub 3 `[e:u8]`; subs 2, 4 nothing | infection-mode team changes and the two `hud_infection` counters (cshell+0xb1190, 0xb0e30, 0xb0f70, 0xb10f0). The sub 0 field list is now complete (C-GM-2); the meanings of the team values and of sub 3 stay UNPROVEN (GAME_MODES_2009 5.7). Not consumed | cshell+0x2a1e0 (jump table VA 0x3712a79c) |

### 3.6 Diagnostics and sounds

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0xc1 | `GS_NET_STATS_CLIENT` | `[client_id:u16][8 x u32]` | read and ignored (engine-generated, `UDP_SESSION_2009_B.md` §9.3). Not consumed | cshell+0x2c489..0x2c4da |
| 0xc2 | `GS_NET_STATS_SERVER` | `[3 x u32]` | read and ignored. Not consumed | cshell+0x2c4e1..0x2c4fa |
| 0xc3 | `GS_PERF_STATS` | `[sub:u8][pid:u32]` then sub 1 `[x:u8][a:u16][b:u16]`; sub 2 `[n:u16][3 x u16]`; sub 3 (only when `pid` is the own id) `[stamp_secs:f32][3 x u32]` | sub 1: cshell+0x29300(`x` == 1, `pid`, `a`, `b`); sub 2: cshell+0x2a180(`pid`, `n`, values); sub 3: own record +0x1e0 := (now - `stamp_secs`) x 1000 ms (round-trip time; `stamp_secs` is in the client's game-time seconds, `engine_client` slot +0x6c); other subs: debug warning. Not consumed | cshell+0x2acd0 (widths from the disassembly) |
| 0xc7 | `GS_SERVER_LOG` | `[text:zstr8(1025)]` | non-empty text is printed to the client debug console with a prefix. Not consumed | cshell+0x2e59c..0x2e5cb |
| 0xbe | `GS_POINT_SOUND` | `[pos:pos48][set:u8][kind:u8][c:u8][d:u8]` | a positional sound: name from the sound table object VA 0x375790d4 (cshell+0xf6ff0(`set`, `kind`, `c`, `d`)), flags (cshell+0xf6c90) and radius (cshell+0xf6d20), played through `sound_mgr` slot +0x10. Meanings PLAUSIBLE (an impact / surface sound). Not consumed | cshell+0x2d79f..0x2d88a (reader slot +0x5c) |
| 0x70 | `GS_NOP_70` | (none) | nothing. Consumed | cshell+0x2e2f3 |

## 4. Stage C: local player object (cshell+0x60d90, base cshell+0x1f0470)

Handler: `local_player` derived slot +0x24 = cshell+0x60d90 (decompiler output present, arguments garbled; layouts from the
disassembly). It handles 0x5a, 0x66, 0xb2, 0xb3, 0xb4, 0xd8, 0xd9, 0xda itself; every id (including those) then
falls into the base handler cshell+0x1f0470 and the two results are OR-ed (cshell+0x6153f..0x6154b), except the
cases that return directly (0x66 kind 1 and kind 0x14, 0xb2, 0xb3). The base handler forwards 0x67, 0x6a, 0x74, 0x7f,
0xa0, 0xa3, 0xa4, 0xad, 0xaf to `local_player` slots (derived table VA 0x374d327c, PE-DATA):

| id | slot | derived code | notes |
|---|---|---|---|
| 0x67 | +0xb8 | cshell+0x61870 (DISASM: not a decompiler function) | calls the base cshell+0x1f97d0 first |
| 0x6a | +0xc8 | cshell+0x5cca0 (DISASM) | calls cshell+0x1f4ed0 |
| 0x74 | +0xc4 | cshell+0x5cd20 (DISASM) | calls cshell+0x1f7d20 |
| 0x7f | +0xc0 | cshell+0x1f4ec0 = `jmp` cshell+0x1f3400 | |
| 0xa0 | +0xbc | cshell+0x1f27f0 (DISASM) | MOVEMENT_2009 §12.4 |
| 0xa3 | +0xb4 | cshell+0x1f4e80 | |
| 0xa4 | +0xcc | cshell+0x1f0680 (DISASM) | ignores the body |
| 0xad | +0xd0 | cshell+0x5ccd0 | |
| 0xaf | +0xd4 | cshell+0x1f2820 | |

The `local_player` object is the "controller" of `MOVEMENT_2009.md`; its own-character pointer is +0x10, its weapon
inventory +0x18 (an array of weapon objects by inventory slot, count +0x3e4), its status block +0x278 (section 4.3).

### 4.1 Player state: the carrier of the in-world test (resolves `UDP_SESSION_2009_A.md` §8.3 d and S12)

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0x67 | `GS_PLAYER_STATE` | `[state:u8]` | `local_player`+0x30 := `state` (base cshell+0x1f97d0, C: `read(8)` stored at `in_ECX[0xc]`). This is the field that `in_world_test` cshell+0x1f3950 requires to be non-zero, so **shell id 0x67 with a non-zero state is the player-state carrier of S12**. Consumed | cshell+0x1f0470 case 0x67 -> slot +0xb8 (PE-DATA VA 0x374d3334) -> cshell+0x61870 (DISASM: `call 0x372f97d0` with the reader) -> cshell+0x1f97d0 |

Why the earlier search failed: the reader cshell+0x1f4ab0 named in S12 is the object's slot +0x10, a full
serialisation of the controller (save / restore style); it also writes +0x30 but it is not on the network path. The
network path is the base switch cshell+0x1f0470 -> slot +0xb8, whose derived entry cshell+0x61870 lies in a gap of
the decompiler function table (between cshell+0x60d90 and cshell+0x61b50), which is why no C caller was found.

State values (both handlers; meanings are interpretations, grade PLAUSIBLE unless noted):

| state | base cshell+0x1f97d0 | derived cshell+0x61870 | our meaning |
|---|---|---|---|
| 0 | (nothing) | (nothing) | not in play; fails the in-world test (PROVEN) |
| 1 | movement enabled on the own character (+0xa8 := 1, +0x84 := 1), camera mode 3, view angles restored, reticle and HUD reset (cshell+0x9eff0(1), cshell+0x9f0c0(0)), player list refresh | own roster record bound (+0x434), marker and target-name reset, sound group stopped, `client_shell`+0x108 := 1, an engine camera call, cshell+0x60680(0), radar refresh (cshell+0xb4f00(2)) | alive / spawned |
| 2 | 12 words at +0x15c cleared, interface camera mode set (screen manager slot +0x30) | own character movement disabled (+0xa8 := 0, +0x84 := 0), `hud_target_name`+0x08 := 2, death time stored (+0x440), +0x470 := 1; with an equipped weapon of table type 0xb the radar marker is cleared | dead (death camera) |
| 3 | camera mode 0 or 2, cshell+0x1f3b30, match_ui+0x14b0 := 3; if slot +0xac: slot +0xa4(0,0) and +0xa8(0) | `hud_radio_call`+0x15b4 := 0, camera mode 2, cshell+0x1f0a40(1), target name set to the own view object | waiting / follow camera (movement frozen, MOVEMENT_2009 §12.5) |
| 4 | cshell+0x1f3380(1, 1), camera mode 0, view angles restored, cshell+0x22d350 | (nothing) | UNPROVEN |
| 5 | (nothing) | weapons reset (cshell+0x1f3b30, cshell+0x22d5b0, cshell+0x22d260, cshell+0x22d350), camera mode 0, +0xac := 1, movement enabled, body rebuilt (cshell+0x1fd200(1)), effects reset | revived in place (UNPROVEN) |
| 6 | (nothing) | camera mode 2, +0x42c := 1, +0x430 := 1, an engine console command str(VA 0x374d355c; 81eb69ed7e2fd54c; 10) that hides the first-person weapon, cshell+0x60680(1), cshell+0x603e0 | spectator (the kill feed treats state 6 as spectator, 3.3.1 step 8) |

A server sends 0x67 with state 1 when a player spawns (including the first spawn that ends the loading screen, S12; the
whole spawn set with state 1 may come before the client's 0x87, PROVEN-CAPTURE accepted, 3.2.2) and state 2 when it dies;
state 3 may accompany the death (the reference stack sends 3 then 2 in one block, REFERENCE) but never replaces state 2, because only
state 2 writes the death time that arms the client's own respawn wait (4.1.1, DISASM). The client then asks for its respawn
itself with 0x89 / 0xc5 / 0xd2 (C2S 3.3, 3.4) and the server answers that request with the spawn set; no arming message is
needed. Proposal **PD-S2C-2'** (rewritten 2026-10-25, spec-fold-5, as `DC_DEFAULTS.md` "Capture notes 2026-10-25" demand;
D, staged): states on the wire 1 alive, 2 dead (arms the client's wait), 3 dead and waiting (sent with 2, never alone),
6 spectator; the respawn is triggered by the client's request, not by a server timer (DSV-32). *Superseded wording of
PD-S2C-2:* "player states on the wire: 1 alive, 2 dying, 3 waiting for respawn, 6 spectating", read with DC-C2S-10 as
"state 2 at the death, state 3 2000 ms later, the server respawns 7000 ms after the death".

#### 4.1.1 Observed life states and the respawn model (DISASM for the client rule, REFERENCE for the server; tdm4, botduel)

**The 2009 client arms its own respawn wait and asks; the server arms nothing.** The wait is armed by the client's per-frame update (cshell+0x61e40, DISASM cshell+0x62467..0x62494) every frame
while `life_state` is 2, 3, 5 or 6 and the death time `local_player`+0x440 is non-zero; that time is written by 0x67 **state 2** (derived handler, jump table VA 0x37161b38, DISASM cshell+0x619d5..0x619de),
cleared by state 1 and **not touched by state 3** (cshell+0x61975). It calls the derived slot +0x70 = cshell+0x60ac0 (a region outside the decompiler function table), which in observer mode 0 starts the death
camera and arms 7000 ms for `mode_class` 1, 4, 6, 7 and 8 (5 when the own roster record +0x1f8 is above 0; 9 when `mode_rules`+0xb4 is 0; 1500 ms for 10; nothing for 2, 3, 11, 12, 13), once per death. The request
(DISASM cshell+0x6203e..0x62132) is one reliable shell message of only the id, **0x89** for classes 1, 4, 5, 6, 9, when `GetTickCount() - start` is strictly greater than the wait (`C2S §3.3`, `§3.4`). The server needs
**no** arming message in the respawn classes; 0xd7 sub 2 and 0xdb sub 6 only re-arm. This replaces the earlier reading that those two messages are the only arming path.

Server side, as the reference stack behaved (REFERENCE, never a rule):
- **Death:** 0x67 `[3]` and 0x67 `[2]` in the **same** reliable block (3 first, 0 ms apart) with the corpse's file and variable messages between them; nothing more followed for the remaining 104 s of tdm4. For the 2009
  client either order arms the wait (state 2 stores the death time, state 3 does not clear it). DC-C2S-10's order (2 at once, 3 after 2000 ms) stays valid; state 3 alone to a dying player would never arm.
- **No server-side respawn timer:** the reference stack respawned a dead player **only on request**: 37 of 37 requests made while the round ran (29 in botduel3, 5 + 3 in botduel4) were answered 30-80 ms later with the spawn set
  (0x6a, 0xa6 x2, **0x67 `[1]`**, 0x66 kind 0x13, 0xb2 / 0x7f x4 slots, 0xa0 `0x0009` body and friction, 0xa0 `0x0020` teleport with a new `move_seq` and a new spawn point, 0x66 kind 0x13 again, 0xb2 / 0x7f again,
  0x66 kinds 0x0f and 0x10, 0x6b kind 2 refresh to every client, 0x67 `[1]` again, 0xa6 notes, 0x66 kind 1 for slots 2 and 3, 0x66 health and armour, kinds 0x18 and 6); a bot that waited 7001-7016 ms before asking
  got state 1 7047-7112 ms after the death. A dead bot that never asked stayed dead (tdm4: 104 s). It sent no 0xd7, 0xdb or 0xd4 in any run, and ignored requests after the round end.
- DeCombat's response is DSV-32 (D, proposed; it refines DC-KILL-1 and supersedes DC-C2S-3 and the respawn parts of DC-C2S-10 and GMD-3): in the classes whose client arms
  its own wait (1, 4, 6, 7, 8: 7000 ms; 10: 1500 ms) there is **no server respawn timer**; a dead player's 0x89 / 0xc5 / 0xd2 that arrives no earlier than the wait minus 250 ms
  after the death is answered at once with the full spawn set; earlier requests, requests from living players and requests after the round end are logged and ignored.
  Classes 5 and 9 (the client arms only under a client state) also respawn on a server timer 10 s after the death; round classes 2, 3, 11, 12, 13 have no respawn inside a round.
  Death: 0x67 `[3]` then `[2]` in one block (the reference stack order; `-deathstates spec` keeps the older 2, then 3 after 2000 ms). Implemented in `dc-dserver` slice 4: TESTED
  (`game_sim` respawn timeline) and RUN: 7 of 7 deaths answered on the bot's self-armed 0x89, 7048-7059 ms after the death on the server clock; 14 extra fallback requests of the
  bot were ignored as "not dead". The 250 ms guard is ours (D); no real client had died in any capture when this was written (tdm6 since holds five such deaths, last bullet; the round-end part of
  V-KILL-1 stays open).
- **Observed with the real client (PROVEN-CAPTURE, tdm6; capval-tdm6 4.2 / 4.3, folded by spec-fold-8):** the real client died six times. After each of the first five it
  sent exactly one reliable `0b 89` 7.015-7.031 s after the datagram that carried 0x67 `[2]` (strictly more than 7000 ms plus at most two tap ticks; `mode_class` 1), with **no**
  0xd7, 0xdb or 0xd4 from the server at any time; the reference stack answered 16-63 ms later with the spawn set (0x67 `[1]` at 197.593, 226.828, 261.281, 304.640, 338.578): the set of
  the botduel runs above plus the fifth slot (0xb2 slot 4, weapon 125, `class_pos` 1), each with a new spawn point and `move_seq` 3 .. 7. The client answered in the next
  datagram with the reliable 0x64 state form `[0x0010][1]`, four 0x76 `[3][2]` + 0x7f pairs (one per server 0x7f of the first half of the set) and its 15 Hz stream from the
  teleport position. It sent no 0x64 while dead and, at each death, one `C2S_RATE` with rate 640 and one u16 sound id (PLAUSIBLE: the acknowledgement of the death sound). It
  left during its sixth wait (0xbd 4.67 s after the death) without a request. The client rule of this section is therefore PROVEN-CAPTURE as well as DISASM; DSV-32's 250 ms
  guard is met by 265-281 ms, while a server timer of exactly 7000 ms would race the request. *Superseded wording (spec-fold-4):* "a death of the real client in any capture
  (tdm1, tdm2, tdm4 all survived; ...). The 2009 client's 0x89 timing after a state 2 is therefore DISASM-derived and bot-confirmed, not PROVEN-CAPTURE." Still not captured:
  a death of the real client near a round end (the remainder of V-KILL-1).

### 4.2 Movement and camera inputs

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0xa0 | `GS_MOVER_UPDATE` | `[flags:u16]` then flag blocks | as `MOVEMENT_2009.md` §12.4 (teleport and box 0x0020, container list 0x0004, body 0x0001, friction 0x0008, freeze 0x0080, release 0x0100); then controller+0x13c := 1. Consumed | cshell+0x1f27f0 (DISASM) -> cshell+0x1ff430 |
| 0x6a | `GS_VIEW_ANGLES` | `[kind:u8][angles:vec96]` | kind 0: pitch, yaw, roll := angles (controller +0x48..+0x50); kind 1: yaw only; body angles copied; then +0x410 := 0, +0x40c / +0x408 := the new pitch / yaw. Consumed | cshell+0x5cca0 (DISASM) -> cshell+0x1f4ed0 |
| 0xa3 | `GS_CAMERA_KICK` | `[kick:vec96]` | added to controller +0xa0..+0xa8, each component capped at 10.0. Consumed | cshell+0x1f4e80 -> cshell+0x1f05f0 |
| 0xaf | `GS_CHARACTER_EVENT` | `[pos:vec72][a:u32][b:u32][c:u32][d:u32]` | when the own state is not 2, 3, 5 or 6: an entry (pos, a, b, c, d) is appended to a list of the own character object (cshell+0x201a50 on `local_player`+0x10). Meaning UNPROVEN | cshell+0x1f2820 (reader slot +0x58) |
| 0xa4 | `GS_WORLD_RENDER_RESET` | (body ignored) | the client shell re-applies its world render variables (far plane, fog values, light animation) with the flag `local_player`+0x150 (cshell+0x1a6900). Consumed | cshell+0x1f0680 (DISASM: the reader argument is replaced by +0x150 before `jmp` cshell+0x1a6900) |
| 0xd9 | `GS_MUSIC` | `[op:u8]` then op 1: `[loop:bool1][p:u32][file:zstr8(261)]` | op 0: `engine_client` slot +0x340 (stop); op 1: the file name prefixed with the game folder str(VA 0x374d3408; 3f8205e1018a1345; 5) is passed with `p` and `loop` to `engine_client` slot +0x33c (PLAUSIBLE: start a music track). Not consumed | cshell+0x5de30 |

### 4.3 Health, armour and the status block

`local_player`+0x278 is a status block (method table VA 0x374d3874; the same class as `match_stats`): +0x0c current
health, +0x14 maximum health, +0x10 current armour, +0x18 maximum armour, +0x1c per-slot ammo counts, +0x08 the
clip of the drawn weapon (setters cshell+0x62eb0, cshell+0x62ef0, cshell+0x62f20, cshell+0x62f50, cshell+0x64140,
all DISASM; each change notifies `hud_mgr` slot +0x10 with 2 for health and 4 for armour). The health / armour
reading of +0x0c / +0x10 is PLAUSIBLE (two capped values with separate HUD notifications; the damage accumulator
+0x44 grows only when +0x0c falls).

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0x66 | `GS_PLAYER_STATUS` | `[kind:u8]` then by `kind` (stage A takes `kind` 0x0c, 2.2): kind 1: `[weapon:u16][ammo_kind:u16][ammo:f32][slot:u8][extra:u16]`; kind 0x14: `[b1:u8][b2:u8][v:u32][money:u32]`; every other kind: `[b1:u8][b2:u8][value:f32]` | kind 1: `client_shell`+0xc3b0 := 7; if `weapon` != 0xFFFF the inventory slot is touched (cshell+0x22d580); status slot +0x38 (cshell+0x62f80, DISASM; arguments slot, weapon, ammo kind, ammo, pickup-notice flag 0, 0, `extra`) records `weapon` in `slot` and sets the ammo count of `slot` to `ammo` (capped by the maximum of the ammo-table row `ammo_kind`; `extra` use UNPROVEN); consumed. Kind 0x14: status +0x14c := `money` and `session_info`+0x104 := `money` (cshell+0x635c0; PLAUSIBLE: the player's GP); consumed. Other kinds go to cshell+0x5cb80(kind, b1, b2, value): 2 = set health to `value` (cshell+0x62eb0), 3 = set armour (cshell+0x62ef0), 0x0f = set maximum health (cshell+0x62f20), 0x10 = set maximum armour (cshell+0x62f50), 4 = inventory selection `b2` (cshell+0x22f2a0, accepted only inside a weapon-table range), 0x13 = `client_shell`+0xc3b0 := 5 and status reset (cshell+0x63160); any other kind: nothing, not consumed | cshell+0x60dda..0x60f1b, cshell+0x5cb80 |
| 0x74 | `GS_DAMAGE_TAKEN` | `[from_dir:vec96][unused:u8][mode:u8][ammo_kind:u16][dmg_kind:u8][zone:u8][part:i8][damage:u32]` | ignored when the own state is 2, 3, 5 or 6. Otherwise `from_dir` is stored (controller +0x1ec..+0x1f4, knockback, `MOVEMENT_2009.md` §12.5), `dmg_kind` -> controller +0x190, `zone` -> +0x194, `damage` -> +0x198; the direction relative to the view is turned into one of the damage-indicator sectors (+0x15c + 4 x sector += magnitude, capped at 1.0); nearby effect objects are told (cshell+0x1acce0 and cshell+0x26e3b0, `mode` selects which list); for `damage` non-zero a view punch depends on `zone` (zones 2..4 and 8..10 pitch, 0..1, 5..7 and 0xb yaw; cshell+0x2b1530 / cshell+0x2b15a0) and a camera shake on the ammo table row of `ammo_kind` scaled by the part row of `part` (cshell+0x1fd670); finally (cshell+0x5cd20) a reticle hit flash (cshell+0x835d0) when `damage` > 0 and `zone` != 0x25 (for `dmg_kind` 1 only when the own character has its +0x304 object). Health itself is **not** changed here (that is 0x66 kind 2). Consumed. Observed and DeCombat forms: 4.3.1. **CORRECTION 2026-10-26 (client-recoil):** the view punch is not picked by `zone`, and it is a jar behind the console value "Reality" (default 0, so off): only when that value is above 0 and `|from_dir|` > 0 are the indicator sector and the jar computed, from the 12-sector index of `from_dir` against the view (not from `zone`); the jar needs `damage` != 0, the damage type's jar flag (the byte before `mode`; Bullet and Choke carry it in the ROM) and picks pitch (sectors 8..10 down, 2..4 up) or roll (5..7 and 0, 1, 11; roll, not yaw) at 2 degrees, rising in 0.1 s and gone after 0.35 s, whatever the damage (full rule `WEAPON_FIRE_2009_addendum_recoil.md` R7; cshell+0x1f7d20, cshell+0x2b1530 / cshell+0x2b15a0). The real client of rust6 ran with the option off | cshell+0x5cd20 (DISASM) -> cshell+0x1f7d20 |

#### 4.3.1 Observed damage and status forms (REFERENCE; client reaction PROVEN where stated)

- **0x74 `GS_DAMAGE_TAKEN`** (200 bits, REFERENCE-CONFIRMED layout; hit 1 head `0d 74 ad eb 6c 3e f1 3b 67 bb 34 f1 09 3e 02 01 ff ff 03 02 00 1b 00 00`): `from_dir` is the shot direction scaled by (health loss / 100): its
  magnitude was 0.268 for a 26.775 loss, 0.02 for a 2-point loss straight up, 0.60 for a 60-point grenade loss; the client adds the magnitude to its damage-indicator sector (capped at 1.0) and normalises it for
  knockback, so a unit vector lights the indicator fully on every hit. `unused` repeats `zone` (2, 5, 7 in the six samples). **`zone` is a damage type, not a body zone**: for the rifle hits it equals the
  ammunition row's damage-type key and for the grenade the area damage-type key (read from the ROM at run time; four hits on different model parts carried the same `zone`). `damage` is the rounded health loss;
  `ammo_kind` was 0xFFFF in every message (the client's camera shake by ammunition row therefore does not run with the reference stack); `dmg_kind` 3 for a weapon hit and 0 otherwise, `mode` 1, `part` 0 (meanings UNPROVEN).
  The reference stack's per-hit amounts are below the ROM `InstDamage` of the ammunition and identical at 72 and 1330 units: reference stack rule values, not adopted (DSV-11 stays the DeCombat rule; for the amounts this is superseded by the project decision of 2026-10-25 to keep the 2009
  damage, next bullet).
- **0x74 in tdm6** (REFERENCE; capval-tdm6 2.1 / 8, folded by spec-fold-8; the hits were the real client's own claims with `extra_kind` = the skeleton node, C2S 4.1):
  **`dmg_kind` = the location of the named node** for a weapon hit (`[Skeleton0]` `Node<k>Location`: node 6 torso -> 2, node 53 head -> 1, nodes 24 / 11 arms -> 3; the six
  real-client hits of tdm4 / tdm6 and the bot's 24 reports agree), **1 for a blast on another player** (the second grenade on the bot: `dmg_kind` 1, `zone` 7, `damage` 42,
  `|from_dir|` 0.4232) and 0 for the thrower's own blast (tdm4); **`damage` is the hit's health damage before the clamp to the remaining health** (210 for the head shot on a
  victim with 70.25 left; tdm4's lethal arm hit carried 27 with 19.675 left), and **`|from_dir|` = min(1, damage / 100)** (1.0, 0.2975, 0.4232). The head shot and the blast
  did not reduce the armour (no 0x66 kind 3 after them). The client only tests `damage` > 0 and stores it (4.3), so the clamp shows only in the indicator magnitude.
  Amounts: reference stack values; under the project decision "keep the 2009 damage" the DSV-30 replacement of damage-calibrate (`DC_DEFAULTS.md` Wave Z part 1, PENDING) is
  fitted to these per-hit values with residual 0 (all 30 captured rifle hits, the head shot, the blasts' armour) and implements the unclamped `damage` and the scaled
  `from_dir`; its `dmg_kind` stays the constant 3 (the location form above is not part of that change).
- **0x66 after each 0x74:** kind 2 (health) then kind 3 (armour) `[kind][0][0][value:f32]`, 72 bits each. Kinds seen at spawn: 0x0f (maximum health 100), 0x10 (maximum armour 100), 0x17, 0x13 with `b1` 1 then 0,
  1 (ammunition, `weapon` 0 / 7 / 7: 1000.0, 1.0, 0.0 after the throw, `extra` 1), 2, 3, 0x18 and 6. Kinds 6, 0x17 and 0x18 are "any other kind: nothing" in 4.3; a server need not send them.
- **0x66 kind 1 after a throw** (blast question Q9, REFERENCE, ACCEPTED; spec-fold-6 G.1, `UDP_SESSION_2009_B.md` 3.5.4): `[7][9][0.0][3][1]` (`weapon`, `ammo_kind`, `ammo` = rounds left,
  `slot`, `extra` 1) to the thrower only, reliable, in the datagram of the projectile's create (tdm2 seq 8110, tdm4 seq 7690). **DeCombat form** (D, DSV-39 addition (proposed, PENDING in
  DC_DEFAULTS); `dc-dserver` on master 7fbad44, TESTED `game_sim` `the_release_of_a_throw_answers_the_thrower_with_the_grenade_count`, no real client): after a release the thrower gets
  0x66 kind 1 `[weapon][ammo_kind][ammo][slot][extra 1]` with `ammo` = the rounds left in the clip mirror after the throw's round was booked: 0.0 for the ROM grenade (clip 1), the reference stack's
  value. Open: for the about 20 ROM throwables with clip 1 and a larger stock DeCombat also sends 0.0; the reference stack's rule there is not in any capture (one grenade type was thrown). *Amended by aa-grenades (Wave AA, merged; DSV-39 Q9 and grant amendments,
  PENDING in `DC_DEFAULTS.md`; folded by spec-fold-8):* `ammo` = every round the thrower still carries (clip + stock), and a weapon that is its own ammunition (ROM
  `IsAmmo`) counts its granted amount as the total, so a 2-pack reads 2.0 at the grant, 1.0 after the first throw and 0.0 after the second. In rust4 our server had sent 0.0
  after the first of two grenades while the client, by its own 0x83 `[143]` in the release block and its reload event 0x76 `[3][1][1]`, still held one (PROVEN-CAPTURE of the
  difference, seq 15568 / 22892 / 38375); the client then played only the hold, without wind-up sound and without release (C2S 4.6; the reaction PLAUSIBLE).
- **Fall damage:** two 0x74 + 0x66 kind 2 pairs 50-110 ms after the client landed from a fall (trailer vertical speed -766 and -825 just before `on_ground` 0 -> 1): damage 1 and 21, health 99.06 and 77.85,
  `zone` 5, `from_dir` nearly vertical. The reference stack derives this from the 0x64 stream; the rule and numbers are server policy. A client self-grenade did 53 or 60 damage (`zone` 7). The 2-point hit with no shooter at
  tdm4 101.5 s had `zone` 5.
- **0xa6 notes to the victim's character** (type 0x18, 8.3): after each hit sub 0x14 (health percent) and sub 0x15 (armour percent) to every client, rounded; sub 0x20 (damage note) with the attacker's object. The 2009
  type 0x18 class ignores 0x14 and 0x15 (8.3).
- **DeCombat 0x74 form** (D, DSV-33 / DC-KILL-2 proposed; `dc-dserver` slice 4, TESTED byte for byte against the tdm4 heads, RUN): `from_dir` = shot direction x health loss / 100,
  `zone` and the byte before it = the ROM `[Ammo<n>] InstDamageType` of the reported ammunition (0 without a ROM value), `damage` = the rounded health loss, `mode` 1, `dmg_kind` 3,
  `ammo_kind` 0xFFFF, `part` 0 (the last four are the REFERENCE constants; meanings UNPROVEN). The amount itself is DSV-30 (ROM `InstDamage` x the ROM skeleton-node factor of the part
  the client names, armour from the loadout's ROM armour items), not the reference stack's fractions. Health and armour follow as 0x66 kinds 2 and 3. RUN: damage 43, 43, 14 per hit, three
  hits per kill; the start armour without armour items is +0.0 (a -0.0 bit pattern was found and fixed in the same slice). *Status after damage-calibrate (Wave Z, merged; spec-fold-8):* `damage` = the rounded health
  damage before the clamp, `from_dir` = shot direction x min(1, damage / 100), the amount from the calibrated DSV-30; the RUN numbers above are those of the former rule.

### 4.4 Weapons and inventory

Common to 0xb2 / 0xb3: the weapon number is a `weapon_db` index (cshell+0x172300 returns its row); `slot` is the
inventory slot (0 .. count-1); `class_pos` is the position of the weapon in its weapon-class list (cshell+0x22edd0
stores `slot` at class table [class][class_pos] when `class_pos` < 33; the class comes from cshell+0x47da0); the HUD
weapon list is updated (`hud_weapon_slots` cshell+0xa0850(slot, weapon, class_pos)); the four part bytes select
attachments (weapon object slots +0x108 reset three attachment points, then +0x104(part, 0, 1) for each part != -1)
and a finish (+0x124(part4) when != -1); the radar gets the weapon when its row +0x21c != -1 (cshell+0xb5390).

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0xb2 | `GS_WEAPON_GIVE` | `[weapon:u16][ammo_kind:u16][ammo:f32][class_pos:u8][slot:u8]` then (only if accepted, see reaction) `[part1:i8][part2:i8][part3:i8][finish:i8]` | does nothing (consumed) when there is no inventory; otherwise the slot is filled with the weapon (cshell+0x22da30: keeps an existing identical weapon or creates a new weapon object; refused -> **not consumed**, the parts are not read); ammo set through status +0x38 with the pickup-notice flag 1 (which also calls cshell+0x1f0700); class list, HUD list; for weapon class 5 a row flag (+0x480) from the row's +0x224 / +0x228; parts and finish as above; `client_shell`+0xc3b0 := 6; cshell+0x60680(0). Consumed | cshell+0x60f20..0x6118e |
| 0xb3 | `GS_WEAPON_EQUIP` | `[weapon:u16][ammo_kind:u16][ammo:f32][clip:u32][class_pos:u8][slot:u8]` then (if the weapon object was created) `[part1:i8][part2:i8][part3:i8][finish:i8]` | remembers the drawn weapon (cshell+0x22d210), always creates a new weapon object in `slot` (cshell+0x22dae0; failure -> not consumed); ammo, class list, HUD list and parts as 0xb2; status +0x08 := `clip` (cshell+0x64140), +0x04 := 1 (cshell+0x64160); `match_ui` slot +0xac(slot, weapon, previous weapon) switches to it. Consumed | cshell+0x61193..0x61424 |
| 0xb4 | `GS_WEAPON_REMOVE` | `[slot:u8][weapon:u16][ammo_kind:u16][unused:u32]` | status slot +0x44 (cshell+0x632c0, DISASM) clears the flags and the ammo count of `slot` (after looking up the ammo-table row `ammo_kind`) and removes the weapon object (cshell+0x22d500(weapon, slot)); the weapon leaves its class list (cshell+0x22ee70). Not consumed | cshell+0x61427..0x61486 |
| 0x7f | `GS_WEAPON_SWITCH` | `[command:u8][auto:u8][ammo:f32][last:u8]` | weapon number := `weapon_db` command table [`command` - 0x14] (+0x15dc; 0xFFFF when out of range); unknown weapon: debug line only. When `auto` != 0 and the user option str(VA 0x374eea0c; 75c5881057f1a80a; 16) is on (default 1): switch to the weapon (cshell+0x22e930 on the inventory) with the ammo count (`ammo` when >= 0, else the row's default +0x25c) and a value from `match_stats` slot +0x80(`last`, 0). Consumed | cshell+0x1f3400 |
| 0xd8 | `GS_PLAYER_USE` | `[sub:u8]` then sub 5: `[obj:objref16][flag:bool1][use_state:u8]`; sub 7: `[n:u8]` n x `[slot:u32][weapon:u32][clip:u32][ammo_kind:u32][ammo:u32]`; subs 6 and 8: nothing | sub 5: `local_player`+0x204 := `use_state`, reset to 0 when the object being used (+0x1e8, its +0x10) is not `obj`; subs 6 / 8: +0x204 := 4; sub 7 (ammo refill): per entry, if `slot` holds a weapon: its clip := `clip` (weapon slot +0x130; and status +0x08 when it is the drawn weapon), status +0x38(slot, weapon, ammo_kind, ammo, 0, 0, -1), and a HUD note (cshell+0x47c30). Not consumed | cshell+0x5dca0 |
| 0xda | `GS_PLAYER_POINTS` | `[kind:u8]` then kind 1: `[delta:u32]` | kind 1: `local_player`+0x1fc += `delta` (the own value of 0x7d). Not consumed | cshell+0x61521..0x61539 |
| 0x5a | `GS_PLAYER_NOTE` | `[kind:u8]` then kind 0: `[a:zstr8(260)][b:zstr8(260)]` | kind 0: `local_player`+0x487 := 1 (the two texts are read into locals and not used); kind 3: +0x487 := 0. Meaning UNPROVEN. Not consumed | cshell+0x614a5..0x6151f |
| 0xad | `GS_TRACK_OBJECT` | `[obj:objref16][kind:u8][a:u32][hold_secs:f32][c:u32]` (corrected 2026-10-24, GAME_MODES_2009 C-GM-5: the second u32 is the remaining time of the use-progress countdown, stored at +0x04 and lowered by the frame time, DISASM cshell+0x275a40..0x275ac7) | the handler peeks `obj` and `kind`, rewinds to where it started, and when `kind` != 0 or `local_player`+0x424 is set passes the reader to cshell+0x275a40, which reads the same fields and gives the object `obj`, an entry of a 32-byte type table at VA 0x375571c8 selected by `kind`, and `a`, `hold_secs`, `c` to an object of `local_player`+0x1c (slot +0x34); it starts the use-progress countdown (hold-to-use plant / defuse, GAME_MODES_2009 4.5). The meaning of `kind`, `a` and `c` stays UNPROVEN. Consumed | cshell+0x5ccd0, cshell+0x275a40 |

#### 4.4.1 Observed weapon forms (tdm1, tdm2, tdm4; REFERENCE server side, PROVEN-CAPTURE for the client's answers)

At the spawn the reference stack sends, twice and in this order within block 37: 0x66 kind 0x13, four 0xb2 `[weapon][ammo_kind][ammo:f32][class_pos 0][slot][-1 x 4]` for slots 0..3 (weapons 145, 164, 0, 7; ammo 120, 45, 1000, 1)
with 0x7f `[command][auto][ammo -1.0][last]` = (105, 0, 0), (251, 0, 1), (105, 1, 0) between them; no 0xb3. The client answers with four `UP_WEAPON_EVENT [3][2]` + `UP_WEAPON_CHANGE [0][weapon][ammo]` and later selects
exactly these slots and fires them with these ammunition ids (PROVEN-CAPTURE that the slots were filled; the client's own lobby loadout lists the same weapons). `GS_WEAPON_EQUIP` (0xb3), which dc-dserver sends for the
drawn weapon, has not been exercised against the real client. `auto` 0 has no client effect; `auto` 1 draws the slot.

`GS_WEAPON_SWITCH` names a weapon by `0x14 + position` in the client's command table (`weapon_db` +0x15dc), built in cshell+0x180590 from the weapon table's order section: the class names in order, then for each class
its entries in order, each looked up by name; the table is their concatenation (EVIDENCE; the reference stack's automatic switch used exactly that number, ROM-gated replay test).

**A DeCombat client against 0x7f** (D; client-netbye, merged as d9fce0a; TESTED `server_weapon_switch_draws_the_named_slot` with a synthetic table: command 0x15 -> weapon 5 in slot 2
-> `0b760302`, `0b7f0205000000`; `auto` 0 and an unknown command silent): `dc-botclient` reads `[command:u8][auto:u8][ammo:f32][last:u8]` (the 4.4 layout, PROVEN cshell+0x1f3400) as a
weapon-switch event; `dc-client` builds the command table from the ROM weapon table's order section (classes in order, then each class's entries by name) and takes table[`command` - 0x14]
(the rule above, EVIDENCE); when `auto` != 0 and it carries that weapon it draws the lowest slot holding it (`ammo` >= 0 sets that slot's count) with the normal select pair `76 [3][2]` +
`7f [slot][weapon][ammo id]` (C2S 4.4), as a key press does; `auto` 0, an unknown command or a weapon not carried: nothing. Grade of the 2009 side: the client's reaction to `auto` != 0 is
PROVEN in code (4.4 row: it switches through cshell+0x22e930, which sends the client's 0x7f); whether the real client's observed selects at the spawn were that reaction or its own choice of
the lowest granted slot is not decided by the captures (`CAPTURE_tdm4_WEAPON_FORMS.md` 2: the select always follows `76 03 02`; same weapon either way): UNPROVEN by capture.

DeCombat grants (D, DSV-38 proposed; `dc-dserver` slice 4, TESTED; cross references in `CAPTURE_tdm4_WEAPON_FORMS.md` section 6): the weapons come from the pipe's `user ... loadout=<slot>:<item>`
(`DSERVER_PIPE.md` 11, 12.A; item numbers below 1000 are weapon rows, equipment slot i = weapon slot i as observed), else from the operator's `-loadout`, else the ROM default-issue weapon of
every slot, so nobody spawns unarmed. Every spawn sends **one** grant block: one 0xb2 per slot (`slot` byte = the slot, `class_pos` 0 as the reference stack sent), then the lowest slot is drawn with a
0x7f automatic switch when its weapon is in the command table, else 0xb3. The reference stack's doubled block is not copied (no client reaction needs it, PLAUSIBLE).

Backpack slot (REFERENCE + PROVEN-CAPTURE, tdm6 conn 6 seq 733; capval-rust3 1.2 / 2, folded by spec-fold-8): the reference stack granted the shotgun that the client had equipped in its
equipment slot 10 (lobby 0x324 `[uid][125][10]`) as 0xb2 **slot 4**, weapon 125, ammunition 28.0, **`class_pos` 1** (a second weapon of the primary class), in every spawn set, and
sent one 0x7f per gun slot: `[105][0][-1][0]`, `[251][0][-1][1]`, `[91][0][-1][4]`, then `[105][1][-1][0]` (the whole set twice). The real client selected it with 0x76 `[3][2]` +
0x7f `[4][125][125]` (343.281) and fired it (C2S 4.1.1). In rust3 our server granted weapon slots 0..3 only and the client sent no select for slot 4 (PLAUSIBLE: the client
ignores an empty slot). DeCombat: the DSV-38 amendment of dserver-fixes (backpack equipment slots 10..13 -> weapon slots 4..7, `class_pos` per weapon class; PENDING).

## 5. Stage D: connection object (cshell+0x23c570)

Handler: `conn_mgr` slot +0x24 = cshell+0x23c570 (DISASM: no decompiler function; `id - 0x72` through a byte table at VA
0x3733c5d0 and a jump table at VA 0x3733c5bc). It forwards four ids to derived slots of VA 0x374cedc4 and returns
the slot's byte result; every other id returns 0. A null reader returns 0.

### 5.1 Session and integrity

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0x72 | `GS_IDENTIFY_REQUEST` | (none: the slot is called without the reader) | slot +0x3c = cshell+0x23c620 calls slot +0x08 = cshell+0x23ccb0, which answers with the client -> server shell message 0x72 carrying the character data (`UDP_SESSION_2009_A.md` §4.11; layout belongs to GM §11). Consumed when the answer was built | cshell+0x23c5a5 (DISASM) |
| 0x73 | `GS_RELOAD_WEAPON_TABLE` | (none) | slot +0x40 = cshell+0x23c610 -> slot +0x0c = cshell+0x23c8e0: the weapon table is re-read from the ROM attribute file str(VA 0x374e7048; f6276e61a84356df; 22) (cshell+0x17f270) and a second table object (VA 0x3759b9d8) is refreshed (cshell+0x1ed040). Consumed | cshell+0x23c59c, cshell+0x23c8e0 (DISASM) |
| 0x9e | `GS_GAME_ENDPOINT` | `[mode_class:u8]` then `if mode_class != 0: [host:zstr8(256)][port:u32]` | `client_shell`+0x16938 := `mode_class` (this is the wire source of the mode number, section 1.1); `mode_class` = 0: nothing more is read and the message is **not consumed**. Otherwise `conn_mgr`+0x11c0 := `host` (a string object), +0x11c4 := `port`; cshell+0x23c420 later uses them to test whether a "host:port" text names the current game server (case-insensitive host compare and equal port; caller cshell+0x11a0b0). Consumed | slot +0x38 = cshell+0x2f860 (DISASM) |
| 0xac | `GS_INTEGRITY` | `[sub:u8]` then sub 0: `[version:u16]`; sub 1: `[nonce:u32][flag:bool1]`; subs 2, 3, 4: nothing | sub 0: `version` = 0x100 -> the client answers with client -> server 0xac sub 0 (`[0xac][0][u16][u32]`, cshell+0x23c99e..0x23c9f0); any other version -> `conn_mgr`+0x11e0 := 1 and disconnect reason 1 is recorded (`conn_mgr` slot +0x30 = cshell+0x23c460 stores the first reason at +0x11d4). Sub 1: `conn_mgr`+0x11cc := `nonce`; the client mixes the nonce with the account name, the weapon-table checksum (`weapon_db`+0x15f8) and a checksum of its own module file (module handle from engine key str(VA 0x374f1bf0; 056a075652d803ab; 16), checksum cshell+0x297720) and answers with client -> server 0xac sub 2 `[0xac][2][4 x u32]`. Sub 2: +0x11e0 := 1, disconnect reason 1. Sub 4: +0x11e0 := 1, reason 2. Sub 3: nothing. Always consumed | slot +0x34 = cshell+0x23c900 (sub-id jump table VA 0x3733cc78) |

DeCombat note (PD-S2C-3, proposed): a server that does not check client integrity sends 0xac sub 0 with
`version` 0x100 only if it wants the answer, never sub 2 / sub 4 (they make the client record a disconnect reason),
and treats the client's sub 2 answer as informational.

## 6. Stage E: mode-rules object (cshell+0x5ac40)

Handler: `mode_rules` derived slot +0x20 = cshell+0x5ac40. Order inside: the base part cshell+0x22f690 (0x97,
0x99), then the co-op mission object (global VA 0x375603f8, set by cshell+0x30000) through cshell+0x31490 (0xc9),
then 0xc6 itself. It returns 1 for every id that one of them took.

### 6.1 Level change and mode objectives

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0x99 | `GS_LEVEL_EXIT` | `[next_world:zstr8(260)][keep:bool1][now:bool1]` | `mode_rules`+0x90 := `next_world`, +0x8d := `keep`, +0x99 := `now`. If `now`: +0x98 := `keep`; an empty `next_world` switches the interface to state 5 (the screen state: leave the world for the menus, cshell+0x1bb0b0(5)); otherwise +0x8c := 1 and interface state 2 (PLAUSIBLE: load the next level). If not `now` and the connection state (`conn_mgr`+0x04) is 3: interface state 3 (loading screen). Consumed | slot +0x3c = cshell+0x22f950 |
| 0x97 | `GS_NOP_97` | (none) | slot +0x40 = cshell+0x5aad0 = `jmp` cshell+0x22f6d0 = `return 1`. Consumed, no effect | cshell+0x22f6d0 (DISASM) |
| 0xc6 | `GS_OBJECTIVE_STATE` | `[obj:objref16][state:u32][flag_team:u32][carrier:u32][actor:u32][event:u32][at_base:bool1][quiet:bool1]` (field roles corrected 2026-10-24, GAME_MODES_2009 C-GM-4, DISASM cshell+0x5aae0..0x5ac2f; the old names were `value`, `owner`, `player`, `d`, `e`, `f1`, `f2`) | every client mode object of kind 2 (object slot +0x44 = 2) whose key (slot +0x3c, the team) is `flag_team` and whose object (+0x10) is `obj` gets slot +0x4c(`flag_team`, `state`, `at_base`); the own player is told through cshell+0x5d9e0(`is_me`, `flag_team`, `event`, `actor`, `quiet`) with `is_me` = (`carrier` is not -1 and equals the own client id: `carrier` decides "is me", `actor` names the player in notices and scores); `mode_rules` slot +0x54(`flag_team`, `carrier`). The object is a flag of the flag modes (type 0x17 kind 2), not a capture point; event meanings in GAME_MODES_2009 5.5 (PLAUSIBLE). Consumed | slot +0x5c = cshell+0x5aae0 (DISASM) |

### 6.2 Co-op mission (0xc9)

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0xc9 | `GS_COOP_MISSION` | `[sub:u8]` then sub 1: `[n:u32]` n x `[goal_id:u32][goal_label:zstr8(1000)][goal_state:u8]`, `[m:u32]` m x `[timer:f32]`, `[a:u8][b:u8]`; sub 2: `[text:zstr8(1000)][secs:f32]`; sub 3: `[text:zstr8(1000)][v:u32]`; sub 4: `[on:bool1][secs:f32]`; sub 5: `[op:u8][goal_id:u32]` then op 1 `[goal_label:zstr8(1000)]`, op 3 `[goal_state:u8]`, op 2 nothing; sub 6: `[pickup:u8]`; sub 7: `[flag:bool1]`; sub 8: `[a:i8][b:u8]` | sub 1: the mission goal list is rebuilt, the first positive timer starts the mission timer (object +0x18 / +0x1c / +0x24 / +0x28), respawn-token HUD reset, `hud_scoreboard` cshell+0x92140(`a`, `b`); sub 2: banner text for `secs` (cshell+0xbdd20) and a mode sound; sub 3: text note (cshell+0x81fe0) and the mode sound; sub 4: mission timer start (`on`) or clear; sub 5: goal added (op 1), removed (op 2) or its state set (op 3); sub 6: pickup note (cshell+0x80090) with one of three pickup sounds for `pickup` 1, 2, 3; sub 7: `local_player`+0x268 := `flag`; sub 8: scoreboard values and, for `a` != 0, a multi-kill HUD note (cshell+0xa4f80). Consumed for every sub-id; another shell id falls through | cshell+0x31490 (jump table index = sub - 1) |

## 7. Stage F and the shell's fallback switch

### 7.1 Stage F: HUD manager (cshell+0x45050)

Correction to GM §2 and to report correction (3): stage F is **not** an empty stage. The dispatcher calls slot +0x38
on the global VA 0x375acdb8; that global holds the derived HUD manager (`hud_mgr` = `match_ui`+0x6b74, section 1.2),
whose slot +0x38 is cshell+0x45050 (PE-DATA: derived table VA 0x374d06dc +0x38; the base table VA 0x374f4c9c +0x38
is the `return 0` cshell+0x26b220 that GM §2 cites). cshell+0x45050 compares the id as a signed byte in a chain
(DISASM cshell+0x45083 .. 0x45676) and handles the ids below; it returns 1 except where noted. Most of them drive the
vote-kick panels (`hud_vote_request`, `hud_vote`) and radar marks.

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0xd4 | `GS_RESPAWN_TIMER` | `[x:u8][kind:u8]` then kind 1: `[pid:u32]` and, only if `match_ui`+0x9a95 is clear **(client state: the `waiting_banner` flag of 0x45, formerly `first_round`)**, `[secs:u32]` | kind 0: `local_player` slot +0xa0(1) (+0x484 := 1); kind 2: cshell+0x1f4110(0); kind 1: cshell+0x1f4110(`pid` == `session_info`+0xa4: when the player may act, movement re-enabled and a camera reset; for the own player also slot +0x8c), then (if read) +0x484 := 0 and the three-part countdown `hud_strike` cshell+0x9ee80(1, `secs`); when the own character has its flag +0x5bc, two HUD calls (cshell+0x9eff0(0), cshell+0x9f0c0(0)). Always returns 0 (**not consumed**). Meaning PLAUSIBLE: respawn countdown | cshell+0x45083..0x45196 |
| 0x84 | `GS_RADAR_OBJECT` | `[obj:objref16][on:bool1]` | `hud_radar` shows or hides `obj` (cshell+0xb8610) | cshell+0x4519c |
| 0xcd | `GS_RADAR_PING` | `[kind:u8][team:u8]` then kind 2: `[value:u32][unused:u32][text:zstr8(257)]`; kind 1: `[value:u32]` | kind 2: radar mark with text (cshell+0xb8e70(`team`, `value`, ..., `text`)); kind 1: radar ping (cshell+0xb8650(`team`, `value`)); other kinds: returns 0 | cshell+0x451d6 |
| 0xb7 | `GS_VOTE_START` | `[unused:u8][a:u32][b:u32][name1:zstr8(64)][name2:zstr8(64)][unused:u16][unused:bool1][flag:bool1]` | an open vote request is closed (`hud_vote_request`+0x20 := 0) and the vote panel opens with the two names and values (cshell+0xd0f60, layout keys of the vote panel) | cshell+0x45294 |
| 0xb9 | `GS_VOTE_COOLDOWN` | `[unused:u32][b:u32][flag:bool1]` | `flag` set: `session_info`+0x3f9 := 1, +0x3fc := `b`, +0x430 := 0 (cshell+0x1b1360) | cshell+0x45382 |
| 0xd0 | `GS_VOTE_RESULT` | `[a:u32][b:u32][unused:bool1]` | open request closed; result shown (cshell+0xcf840(`a`, `b`, ...)) | cshell+0x453c6 |
| 0xbb | `GS_VOTE_TALLY` | `[a:u32][b:u32][c:bool1][d:bool1][e:bool1][f:u16]` | tally line "a : b" (cshell+0xcf350) | cshell+0x45429 |
| 0xcf | `GS_VOTE_STATE` | `[open:bool1][code:u8]` | `hud_vote_request`+0x110 := `open`; `open` and `code` 2: vote cooldown started (cshell+0x1b13c0) and panel message 5 (cshell+0xcfd70(5, 0)); not `open` and `code` 0: cooldown cleared (cshell+0x1b13e0) | cshell+0x454a9 |
| 0xbc | `GS_VOTE_ERROR` | `[code:u32]` then code 4: `[x:u32]`; code 7: `[y:u8]` | panel message (cshell+0xcfd70): code 0 -> 0, 1 -> 1, 2 -> 4, 3 -> 3, 4 -> 8 with `x`, 7 -> 7 with `y`; any other code: returns 0 | cshell+0x45676 |
| 0x7e | `GS_VOTE_ASK` | `[name:zstr8(64)][value:u32]` | the vote panel stores the name (cshell+0x44a60) and `hud_vote`+0x18 := `value`; the client sends client -> server shell 0xd1 (built with the writer of GM §1.4, cshell+0x29e50) | cshell+0x45518 |
| 0xb6 | `GS_VOTE_PROMPT` | (none) | panel refresh (cshell+0xd0120) and the client sends client -> server shell 0xba | cshell+0x455e3 |

### 7.2 The shell's fallback switch (cshell+0x1a86b0)

Reached only when all six stages declined.

| id | our name | layout | client reaction | evidence |
|---|---|---|---|---|
| 0x00 | `GS_CONSOLE_TEXT` | `[text:zstr8(50)]` | the text (at most 49 characters kept) is formatted into a local buffer by cshell+0x1a39f0 (a `vsprintf` whose result is not used): **no visible effect** in this build. The engine also generates shell id 0 (`UDP_SESSION_2009_B.md` §9.4); a server need not send it | cshell+0x1a3c80 (DISASM: buffer size 0x32) |
| 0x86 | `GS_WEAPON_NOTE` | (body passed on) | the drawn weapon object (`local_player`+0x18 inventory, +0x0c) gets slot +0x04 with (id, reader); for the 2009 weapon class that slot is cshell+0x2f410 = `return 1` (method table VA 0x374cec2c, PE-DATA): no effect; no weapon drawn: nothing | cshell+0x22d1c0 (DISASM) |
| 0x88 | `GS_WORLD_SWITCH_STATE` | `[state:u8]` | `client_shell`+0x1a0c := `state` (the switching-worlds state of `UDP_SESSION_2009_A.md` §4.11; the in-world test needs 2 or 3) | cshell+0x1a58f0 |
| 0x8e | `GS_SHELL_HOOK_8E` | (none) | if `client_shell`+0x177c is set: cshell+0x29a160 (meaning UNPROVEN) | cshell+0x1a5850 |
| 0x90 | `GS_SHELL_FLAG_90` | `[flag:bool1]` | `client_shell`+0x1a09 := `flag` (meaning UNPROVEN). REFERENCE: sent as `0d 90 01` (17 bits) in the first block of a link to a **starting** world, before `S2C_VERSION`, and `0d 90 00` after the client's 0x87 and to every link when the world starts; a link that joins a **running** world gets `[0]` at once (tdm1, tdm2). The client showed no visible reaction; a server need not send it | cshell+0x1a58d0 |
| 0x9b | `GS_SERVER_ERROR` | `[code:u8]` | `code` 1: a debug line, then the interface switches to state 5 (screens; cshell+0x1bb0b0(5) on the object of shell slot +0xac = `match_ui`): the player is taken out of the match to the menus. Other codes: nothing | cshell+0x1a5870 |
| 0x9d | `GS_UNKNOWN_9D` | `[u32]` (body UNPROVEN: REFERENCE sends `0d 9d 00 00 00 00`, 48 bits, value 0, once per join right after 0x9e) | not handled by any stage of the dispatch chain (sections 2-6, 7.1) nor by this switch, so the 2009 client drops it silently (tdm1: no reaction). Present in the reference server's start order and in no handler list; a server need not send it. Meaning and whether another build handles it: UNPROVEN | none (no handler found) |
| 0xa6 | `GS_OBJECT_NOTE` | `[note_kind:u8][obj:objref16][...]` | handed to the object-message router (shell slot +0xd4 getter, section 8) method cshell+0x20fd00: `note_kind` 0x0d: every registered router object whose object (+0x10) is `obj` gets slot +0x30(reader); 0x2d and 0x2f: cshell+0x2b95a0 / cshell+0x2b9ce0 read the rest; any `note_kind` < 0x3d with a known `obj`: the router's client object of that type for `obj` (cshell+0x20fbe0(type, obj)) gets slot +0x30(reader). The remaining layout is that of the target class (section 8.3) | cshell+0x1a3c60, cshell+0x20fd00 |

## 8. Shell object messages: client objects attached to world objects

### 8.1 Path

`UDP_SESSION_2009_B.md` §6.3: a shell object message reaches CShell method +0x60 = cshell+0x1a3d00 with
`(obj, reader)`, where `obj` is the engine object of a create record that carried a blob (the usual case) or none
for engine id 17. cshell+0x1a3d00 (DISASM cshell+0x1a3d1f..0x1a3d47; corrects the "two calls" reading of the
decompiler output, as in report correction (4)):

1. if `obj`: a common-interface call on it (VA 0x375964f0 slot +0x48 with 3, 1, 1; meaning UNPROVEN);
2. shell slot +0xd4 = cshell+0x41c30 returns `router` = shell +0x164e0 (`lea eax, [ecx + 0x164e0]`), and the effect
   dispatcher cshell+0x217760 runs **on `router`** with `(obj, reader)` (`mov ecx, eax; call 0x37317760`);
3. the reader is rewound to bit 0;
4. cshell+0x26e070 runs on shell +0x1a88 with `(obj, reader)` (section 8.4).

`router` keeps one list of client objects per type 0 .. 0x3c (16-byte list headers from +0x04, count at +0x0c of
each header); cshell+0x20fbe0(type, obj) returns the client object of `type` whose engine object (+0x10) is `obj`.
Its slot +0x00 creates a client object: `create(type, create_data, reader, obj)`. The same router receives shell id
0xa6 (section 7.2), which is how a server addresses an existing client object later.

### 8.2 Effect dispatcher (cshell+0x217760)

Reads `[type:u8]` (`read(8)`, cshell+0x2177a0) and switches. Three patterns (PROVEN in the C; widths visible in the C
for this function and checked in the disassembly):

- **Inline create data**: the dispatcher reads a fixed record, then calls `create(type, &record, 0, 0)`.
- **Class-read**: `create(type, 0, reader, obj)`: the new client object reads the rest of the message itself. Types
  3, 5, 6, 0x13, 0x18, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x21, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2c, 0x2e,
  0x31, 0x32, 0x35, 0x37, 0x39. Type 0x37 is the mode-zone client object (GAME_MODES_2009 4.3, C-GM-7: its layout is given there; the objective areas of the bomb, flag and co-op modes). The other layouts are those of the classes' read methods, which were not traced here
  (UNPROVEN; decompiler re-run item: the router create slot and the per-type read methods).
- **Direct**: types 0x2d and 0x2f go to cshell+0x2b95a0 / cshell+0x2b9ce0 with the reader (no object). Any other
  type value is ignored.

The role names are ours (PLAUSIBLE, from what the record fields feed); the evidence column is the create-record
constructor that the case calls first.

| type | our name | inline record (after `type`) | record constructor |
|---|---|---|---|
| 0x01 | `OBJ_CHARACTER` | `[pos:vec96][file_a:u16][file_b:u16][16 x u32][str_a:zstr8(256)][str_b:zstr8(256)][str_c:zstr8(256)][3 x u16][6 x bool1][6 x u8]` then 4 x `[u32][5 x u16]` then `[link:objref16]` | cshell+0x20eb70 (`file_a`, `file_b` are file ids turned into names by cshell+0x19ec10) |
| 0x06 | `OBJ_SHOT_EFFECT` | class-read; engine id 17 relay, see the table after this one and DISASM cshell+0x2c6770 | router factory case 6 (static client object VA 0x3759c4c8; method table VA 0x374f03a4, set by cshell+0x2160c0) |
| 0x0b | `OBJ_VOLUME` | the shared block of cshell+0x2163d0: `[b:bool1][u32][u32][vec96][vec96][vec96][u8][bool1][u8][vec96][u32][u32][u32][u8]` | cshell+0x20ef90 |
| 0x0d | `OBJ_CAMERA` | `[u8][u8][u8][u32][u32]` | inline (method table VA 0x374efffc) |
| 0x10 | `OBJ_DEBRIS` | `[rot:raw128][pos:pos48][u8]` | cshell+0x20f030 |
| 0x11 | `OBJ_CORPSE` | `[u8][u8][vec96][vec96]` | cshell+0x20f120 |
| 0x13 | `OBJ_PROJECTILE` | class-read; blob of a thrown grenade `[0x13][weapon:u8][ammo:u8][1:u8]` (32 bits with the type byte; REFERENCE-CONFIRMED: the same 32 bits `13 07 09 01` in tdm2 seq 8110 / 8111 and tdm4 seq 7689 / 7690, spec-fold-6). The widths of `weapon` and `ammo` for ids above 255 are UNPROVEN (only 7 / 9 observed; the class read method is the 11.3 re-run item; V-PROJ-4). The object's create form, its flight records and its removal: `UDP_SESSION_2009_B.md` 3.5.4 | factory case 0x13 |
| 0x14 | `OBJ_IMPACT_MARK` | `[rot:raw128][pos:vec96][u32][u16][u8]` | cshell+0x20edb0 |
| 0x15 | `OBJ_LIGHT` | `[pos:vec96][3 x u32][u8][4 x u32][u8][2 x u32][name_a:zstr8(257)][name_b:zstr8(257)]` | cshell+0x20f160 |
| 0x16 | `OBJ_SPARKS` | `[pos:pos48][dir:vec96][u8][count:u16]` | cshell+0x20f370 |
| 0x17 | `OBJ_PICKUP` | `[3 x bool1][name:zstr8(256)][kind:u32]` then kind 2: `[state:u32][team:u32][at_base:bool1]` (the flag of the flag modes 7 / 8, C-GM-8); kind 3: `[flag:bool1]` (bounty pickup of mode 9); kind 5: `[case_key:u32]`; kind 6: `[u16][u16]` | cshell+0x20f1c0 (kind 2 DISASM cshell+0x2824d2..0x2824f0, cshell+0x5dad0) |
| 0x18 | `OBJ_CHARACTER_REC` | class-read: the 572-bit character record of 8.2.1 (the object of every player character; blob of a kind-1 object) | factory case 0x18 -> cshell+0x220830 -> constructor cshell+0x206db0 (method table VA 0x374efea4) |
| 0x19 | `OBJ_PLAYER_SOUND` | `[k:u8]` then k = 0x12: `[u8][u32][vec96][u8][u8]`; else `[u16][u32][vec96]`. The relay forms of the reference stack are in the table after this one | cshell+0x20f330 |
| 0x1a | `OBJ_LINE` | `[from:vec96][to:vec96]` | cshell+0x20f3f0 |
| 0x1b | `OBJ_WEAPON_EFFECT` | the shared block of cshell+0x2163d0, then `[u32][u32]` (cshell+0x216e50) | cshell+0x20f420 |
| 0x26 | `OBJ_PLAYER_CORPSE` | class-read (folded by spec-fold-8 from capval-rust3 5.1 and aa-triage-red-dot 1.1): the corpse of a player (REFERENCE, ACCEPTED; every kill of tdm6, tdm4): a kind-1 object (`cf` 0x25bb) created in the tick of the death with a 219-bit blob of this type (head `26 00 ff ff ff 00 00 02 04 00 00 40 12 ..` in tdm4), the victim's body model block (body, child model, five textures, three styles; no face) and one tracker on a death clip (narrow form, `state1` 0), flags 0x1803 / 0 / 0x00600820 / 0, the victim's last position; in the same tick the victim's character gets `cf` 0x0188 (flags 0x0802, empty attachment list) and its weapon child is removed (`UDP_SESSION_2009_B.md` 3.5.5). Field meanings UNPROVEN; DeCombat form DSV-44 (proposed) | factory case 0x26 (class-read list above) |
| 0x30 | `OBJ_WEATHER` | `[pos:vec96][6 x u32][vec96][u8][u8][name:zstr8(257)]` | cshell+0x20f940 |
| 0x33 | `OBJ_GADGET_TARGET` | `[u8][bool1][bool1][name:zstr8(256)]` | cshell+0x20f280 |
| 0x36 | `OBJ_SCATTER` | `[pos:vec96][8 x u32][name:zstr8(256)][flag:u32]` | cshell+0x20f990 |
| 0x39 | `OBJ_DOOR` | class-read (dserver-round2 G1, folded by spec-fold-8): `[a:u8][b:bool1][c:u8]` after the type byte (stored at +0x38, +0x39, +0x3c; meanings UNPROVEN); the object registers in a client list (VA 0x375b5ff8, cshell+0x2e21b0). Observed (REFERENCE; tdm4 conn 6, tdm1, tdm2 conn 7): the blob of the kind-2 named brush of each sliding door, 25 bits `39 00 02 00` (`a` 0, `b` 0, `c` 1); the doors' AWMType property is 1 in that world (PLAUSIBLE source of `c`); the door records: `UDP_SESSION_2009_B.md` 3.5.1 | factory case 0x39 -> cshell+0x2216d0 -> constructor cshell+0x2e2090 (method table VA 0x374fb06c, read slot +0x10 = cshell+0x2e2230, DISASM) |
| 0x3c | `OBJ_PLAIN` | (none: only the object) | inline (method table VA 0x374cd5f0) |

**V-S2C-1 settled for players (REFERENCE-CONFIRMED, tdm2; consistent with tdm1, tdm4 and the dserver-slice3 reference run).** For a TDM server: every player character is a
kind-1 model object (`kind_byte` 0xc1, `UDP_SESSION_2009_B.md` 3.5) whose blob is the **type 0x18** character record (572 bits after the type byte, 8.2.1), with a second kind-1 object for the held weapon attached at socket
2; weapon and item pickups are kind-1 objects with a **type 0x17** blob (116 bits; kind 2 = flags of the flag modes, GAME_MODES 4.4); type 0x37 mode zones (409 or 1081 bits, five in the TDM world, GAME_MODES 4.3); type 0x0b
volumes on 60 kind-9 objects; type 0x64 on 16 (8.4); 0x39 x3, 0x0d x1, 0x13 x1 (a thrown grenade, `13 07 09 01`). **Type 0x01 was never seen**; the earlier reading "0x01 on every player character" is wrong (GM
§5.1's "players (type 0x01 / 0x18)" matches the reference stack's 0x18). Effects and relays (engine id 17, no object; REFERENCE, accepted, all unreliable and sent to every client including the sender):

| trigger (client message, PROVEN-CAPTURE) | relayed as | layout |
|---|---|---|
| `UP_WEAPON_SOUND` 0x80 `[sound_no][weapon_id][own_id:u8][weapon_pos]` | type **0x19** 32-46 ms later (3 of 3 in tdm2, 6 of 6 in tdm4) | `[0x19][k = sound_no][weapon_id:u16][player:u32][pos:vec96]` (160 bits after the engine id byte, 168 with it): the client message with `own_id` widened to 32 bits; the inline reader above reads it bit for bit |
| a jump, `UP_MOVE_EVENT` 0x76 `[1][1]` | type 0x19, `k` 0x0f | `[0x19][0x0f][0:u16][player:u32][pos:vec96]`; one relay per jump (26 of 26 in tdm4, 13 of 13 in tdm2: tdm2's table read them as landings, but its 16 landings gave 13 relays; tdm1 shows event 3 is no landing marker) |
| the release 0x82 of a throwable (`UP_FIRE` with the thrown weapon's ids) | type 0x19, `k` 0x12 | `[0x19][0x12][6:u8][player:u32][pos:vec96][1:u8][0:u8]` (176 bits) |
| each bullet 0x82 (hit or miss; two records for a hitting shot in tdm4: the impact and a miss-style continuation) | type **0x06** shot effect | `[0x06][first:objref16][shooter:objref16][weapon:u16][ammo:u16][a:u8][b:u16][c:u8][from:vec96][to:vec96][normal:vec96][d:u8][e:u8][f:u8][g:u8]` = 424 bits after the engine id byte (432 with it); `first` 0xFFFF or 0 (what the trace hit: PLAUSIBLE), `shooter` the shooter's character, `from` = the 0x82 `weapon_pos` exactly (the blast record instead carries the release's `origin`, next row), `to` = the reference stack's own trace end (a hit point within 0.8 units of the 0x82 extra block, or 104-1340 units along the ray), `normal` a unit vector ((0,1,0) floors, (0,0,-1) walls), `a` 110, 71, 20, 10 or 6 (6 on the hit records of tdm4), `b` 0x5e10 / 0x4f10 / 0x4f00 (tdm2 bullets), 0x4be0 / 0x4bf0 / 0x0f01 / 0x0f11 (tdm4 bullets), 0x59f0 / 0x5ff0 (the two blasts), `c` 1, `d`..`g` = 0x00, 0xff, 0xff, 0xff (meanings UNPROVEN). Type 0x06 is class-read; the layout was traced in DISASM cshell+0x2c6770..0x2c68f6 and matches all captured records bit for bit |
| the explosion of a thrown grenade: 2.52 s after the release 0x82 in both captures (2.47 / 2.48 s after the create; the fuse is a server rule, not adopted) | type 0x06 in the form above, every field answered from the two blasts (tdm2 seq 8516 / 8517, tdm4 seq 8064 / 8065; REFERENCE, ACCEPTED; spec-fold-6): `first` 0xFFFF, `shooter` = the thrower's character, `weapon` / `ammo` = the throw's ids (7 / 9), `a` 71, `b` 0x59f0 (tdm2) / 0x5ff0 (tdm4), `c` 1, `from` = the release 0x82's **`origin`** (the eye point, bit for bit in both; not its `weapon_pos`), `to` = the projectile's last streamed position (within 0.9 / 1.3 units: its rest point, `UDP_SESSION_2009_B.md` 3.5.4), `normal` exactly (0, 1, 0) in both (bullet records on floors carry the surface's own slanted normal, so the blast's is PLAUSIBLY a fixed up vector), `d`..`g` 0x00, 0xff, 0xff, 0xff. Meaning of `a` / `b` UNPROVEN (V-PROJ-2) | in the tick of the projectile's removal record; with it 0xa6 [0x18][the thrower's character] sub 0x20 (reliable, every link, meaning UNPROVEN) and, to each hurt player, 0x74 + 0x66 kind 2 (in both captures only the thrower). The client accepted the blast before the removal (tdm2: an earlier datagram) and after it (tdm4: one datagram, reliable block first), B 3.5.4 |
| `UP_WEAPON_CHANGE` 0x7f | a `CF_FILES` change record on the weapon child object (not an engine-17 message) | `UDP_SESSION_2009_B.md` 3.5 |
| `UP_SPRINT_STATE` 0xca | 0xca `[allowed]` to the sender only, reliable | 3.2 |
| `UP_WEAPON_EVENT` 0x76 category 3, 0x4d, 0x83 | nothing | |

Shot-effect notes (folded by spec-fold-8). (1) **`a` is the ROM surface id** of what the shot hit (aa-triage-red-dot 2.1; DISASM cshell+0x2c6770: after reading `a` the
reader replaces 4 and 6 by 0 under the low-gore option, cshell+0x2c6808..0x2c6810; the update cshell+0x2c8540 skips the impact for 0x6e and passes `a` to cshell+0x37f0f0,
which returns field +0x38 of the ROM surface row): the reference stack sent 6 (the body surface row) on 4 of 4 rifle hits on the bot in tdm4 (seq 2968, 3019, 3073, 3132; each with a
continuation record `first` 0, `a` 20, `to` 70-90 units farther), while our dserver sent 71 on every record of rust4, so a body hit played a world impact instead of the
body impact (PROVEN-CAPTURE of the difference; the bullet-hole path cannot persist, so this is not the user's "red dot"). DeCombat: DSV-29 amendment (PENDING). (2) tdm6
(REFERENCE; capval-tdm6 5.5): the reference stack relayed exactly 120 effect records per bot life (= the 0xb2 grant of slot 0) and none for the bot's later reports, while it still applied
damage from them; an applied hit's first record carried the claimed hit point and normal (capval-tdm6 section 11). DeCombat refuses reports beyond its clip and stock mirror
(DC-C2S-4, ours, on purpose).

Type 0x65 (engine id 17, 312 bits, carrying a ROM effect name; once) lies above the dispatcher's range (< 0x3d) and is ignored by the client. U5 (which message relays a weapon event) is **DECIDED for the reference stack form
that the real client accepts**; the 2009 server's own form is still unknown.

DeCombat thrown objects (D, DSV-39 proposed; `dc-dserver` `game/projectile.rs` on master 96983bf; TESTED in `game_sim`, not run against the real client). The 0x82 that releases a
throw (`GAME_MESSAGES_2009_C2S.md` 4.6) creates the type 0x13 object in the create form of `UDP_SESSION_2009_B.md` 3.5.4 on every client, the server integrates the flight (ROM
`ProjectileFX` row else the DSV-39 defaults: rule values, ours), and at the fuse it sends the removal, then the type 0x06 blast (`first` 0xFFFF, `from` = the release origin, as the
reference stack did; `to` = the blast point; `normal` (0, 1, 0), as the reference stack did) and the area damage through the DSV-30 path. Against the answered reference stack form (spec-fold-6) it differs in:
the blast's `a` / `b` / `c` / `d..g` are the rifle-shot bytes (`b` 0x4f10, the reference stack's blasts 0x59f0 / 0x5ff0; meanings UNPROVEN, so whether the client draws a different impact is
open: V-PROJ-2); no change records and no stream entries during the flight (V-PROJ-1); no 0x66 kind 1 grenade count to the thrower; no 0xa6 sub 0x20 in the blast tick; the create
rotation is the thrower's yaw (the reference stack's was not). The removal-before-blast order is one the client accepted (tdm4). The 0x66 count and an optional flight stream are assigned to
the dserver-light worker (Wave U); see the staged follow-up list. *Status at master 7fbad44 (spec-fold-7):* both are implemented by dserver-light: the 0x66 kind 1 count (4.3.1, D, TESTED)
and the thrown-object stream behind `-projectile-stream on|off`, default off (`UDP_SESSION_2009_B.md` 4.3, DSV-41 (proposed), TESTED); the blast bytes, sub 0x20, the create rotation and the
late joiner are unchanged (staged follow-ups S-3a, S-3d, S-3e, S-3f). *Status after Wave AA (aa-grenades merged; spec-fold-8):* the stream is **on by
default** and lists every thrown object in every tick from its create to its removal (velocity while it moves, the rest position without velocity until the blast; DSV-41
amendment, PENDING), because rust4 (3 throws, 0 stream entries and 0 change records per throw: PROVEN-CAPTURE of our stream) with the user's report "throwing grenade didn't
show the throw but the explosion happened" is the first observation for V-PROJ-1: the 2009 client does not move a type 0x13 object by itself (OBSERVED, one report). Change
records during the flight are still not sent (DSV-41b, not built); the grenade count follows 4.3.1.

#### 8.2.1 Type 0x18 character record (REFERENCE-CONFIRMED widths; tdm2 and the dserver-slice3 reference run; class path DISASM)

The class is the router's type 0x18 (factory case 0x18 -> cshell+0x220830 -> constructor cshell+0x206db0, method table VA 0x374efea4, PE-DATA). Its create slot +0x10 (cshell+0x203c70) reads the record with
cshell+0x2b6aa0 and hands it to slot +0x0c (cshell+0x205f40), which copies it to the object (cshell+0x202650, `lea ecx, [esi + 0x38]`: object offset = record offset + 0x38). Read order after `[type:u8 = 0x18]`
(widths from the pushed arguments, DISASM cshell+0x2b6ac9..0x2b6ccf; the width of the first read is fixed by the capture: with 1 bit every one of 15 records ends at exactly 572 bits):

| # | field | width | record offset | observed (reference stack) |
|---|---|---|---|---|
| 1 | `roster_bound` (object +0x40; selects the roster branch of the init, cshell+0x1a0470 check) | u1 | +0x08 | 1 |
| 2 | `has_tail` (object +0x44; the NPC marker path, GAME_MODES 6.2) | u1 | +0x0c | 0 |
| 3-5 | three bytes | u8 each | +0x10, +0x14, +0x20 | 0, 0, 0 (+0x24 := 0xff if +0x14 or +0x20 is 0xff, else a lookup of +0x14, cshell+0x1e0d80 on the table object at VA 0x3759a20c) |
| 6-8 | three bytes | u8 each | +0x2c, +0x2d, +0x34 | (1, 255) before the spawn and (3, 2) after it; +0x34: 1 for the team-1 player, 0 for the team-2 player in tdm2, the engine client id in the bot-only run (both fit every capture) |
| 9 | | u64 | +0x38 | 0 |
| 10-15 | six words | u32 each | +0x40 .. +0x54 | +0x40 = 0x3f000000 (the f32 0.5) except in one create to the two real-client links (0); the rest 0 |
| 16 | box (the player's half extents) | vec96 | +0x58 | (15, 50, 15) |
| 17 | offset | vec96 | +0x64 | (0, 0, 0) |
| 18-19 | two words (low halves copied to object +0x5ae / +0x5b0 by the init) | u32 each | +0x70, +0x74 | 0, 0 |
| 20-21 | | u1, u8 | +0x78, +0x79 | 0, 0 |
| 22 | infected flag (object +0x5bc) | u1 | +0xa4 | 0 |
| tail | only if `has_tail`, after the infected bit (C-FT-2, spec-fold-7; names and uses from `FIRETEAM_2009.md` 1.2-1.5, PROVEN field order and targets, DISASM widths): `[npc_rank:u8]` +0x28 (rank-icon index of the marker entry, looked up in the ROM level table's small-icon list), `[npc_team:u8]` +0x98 (team of the marker, radar and map entries; the bar is drawn only for a team other than the viewer's), `[npc_kind:u8]` +0xa0 (radar / map icon kind: 2 gives 8 / 3, else 6 / 1; 4 sets the infected flag; the health bar is shown only for kinds 1 and 2), `[npc_health:u32 = f32 bits]` +0x9c (health fraction; object +0x5ae := trunc(x 100)), `[npc_caption:zstr8(257)]` +0x7c (marker caption, converted with the ANSI code page, no table lookup; an empty caption creates no marker entry, no caption and no bar) | u8, u8, u8, u32, zstr8: 56 + 8 x (len + 1) bits | +0x28, +0x98, +0xa0, +0x9c, +0x7c | absent in every player record |

Total 572 bits (580 with the type byte; `blob_bits` 580). Client use visible in cshell+0x205f40: when +0x40 is non-zero and the connection check passes, the object takes the branch that attaches the chat effect to other
players' characters, otherwise when +0x44 is non-zero it adds a marker entry by itself (the NPC path). PLAUSIBLE therefore: +0x40 non-zero marks a roster player and the tail is the form for characters without a roster
record. All other field meanings are UNPROVEN. The NPC branch itself (marker, health value, kind, map and radar entries, in that order) is read in full in `FIRETEAM_2009.md` 1.3 (PROVEN,
DISASM): an AI character is sent with `roster_bound` 0 and `has_tail` 1, and no 0x6b record (the create in one line: FIRETEAM_2009 1.9, FT-C1 (proposed)). No capture of the NPC form exists.

### 8.3 Shell id 0xa6 (addressing an existing client object)

Layout `[note_kind:u8][obj:objref16]` + class data (cshell+0x20fd00). See 7.2. The class data layouts are the
same per-type read methods as the class-read group of 8.2 (UNPROVEN).

**Type 0x18 objects (`note_kind` 0x18; DISASM, tdm2 and the slice-3 reference run).** The router passes the reader after `obj` to the object's slot +0x30 = cshell+0x206260 (PE-DATA slot), which reads `[sub:u8]` and
switches through a byte table (`sub - 7` against 0x1e; byte table VA 0x373065cc, jump table VA 0x37306590; DISASM cshell+0x2062a2..0x2062bf). Handled sub-ids and their reads:

| sub | reads | reference stack sends |
|---|---|---|
| 0x07, 0x09 | `[v:u8]` | never |
| 0x0c | `[v:u64]` -> +0x70 | never |
| 0x11 | the 572-bit character record again (member +0x38 slot +0x08) | at every join and spawn (reliable; completes the record: bytes +0x2c / +0x2d become (3, 2)) |
| 0x17 | `[text:zstr8(1023)]` | never |
| 0x18 | through cshell+0x202cd0, only for an object that is not the own view object | never |
| 0x1b | `[box:vec96][offset:vec96]` -> object +0x544 | twice per character create (reliable) |
| 0x1d | nothing; **acts** on every object: clears the infected flag +0x5bc and runs two resets (cshell+0x202ed0, cshell+0x205df0(0)); a roster-only part follows (C-FT-1, spec-fold-7; `FIRETEAM_2009.md` 1.6, PROVEN cshell+0x206260). PLAUSIBLY the "cure" of a kind-4 NPC | never |
| 0x1e, 0x1f, 0x25 | nothing; roster-only: they act only when object +0x40 (`roster_bound`) is set and the connection check passes, so they do nothing to an NPC (C-FT-1) | 0x1f at each spawn (unreliable), 0x25 once about 3.5 s after it (reliable); 0x1f / 0x25 set and clear object +0x539 / +0x53a with a tick stamp when the roster check passes (PLAUSIBLY a spawn-protection window) |
| 0x22 | nothing; **acts**: object +0x540 := 1 (meaning UNPROVEN; C-FT-1) | never |
| 0x20 | `[v:u64]` -> +0x530 | once to the grenade victim (the reference stack wrote 80 bits, the client reads 64) |
| 0x24 | `[fraction:u32 = f32 bits]`, read **only** when object +0x44 (`has_tail`) is set; for an object without the tail the four bytes are not read and the rest of the message is left. Object +0x5ae := trunc(fraction x 100.0) (truncation toward zero), the percent of the NPC health bar (C-FT-1, spec-fold-7; `FIRETEAM_2009.md` 1.5: PROVEN chain create -> +0x5ae -> class slot +0x74 -> bar setter, DISASM cshell+0x206548..0x206569) | never |
| **0x08, 0x13, 0x14, 0x15, 0x1a** | **not handled** (table default exit) | 0x08 `[u8]` after each 0x87; 0x13 `[3]` at create; 0x14 / 0x15 `[u8]` health and armour percent (every ~2 s per character and after each hit); 0x1a `[u8]` |
| **0x40, 0x80, 0xc0** (above 0x25) | **not handled** | 0x40 `[3 x u8]`, 0x80 `[6 x f32]` (the six aux words of the shooter's 0x64), 0xc0 `[6 x f32][3 x u8]` |
| *correction (remote-aim, 2026-10-27)* 0x40, 0x80, 0xc0 | not handled by **this base handler**, but the 2009 client builds every type-0x18 object as a **subclass** (client-object factory cshell+0x66c10: type 0x18 -> 0x7f0-byte object, constructor cshell+0x28a30, EVIDENCE cshell+0x66c4b) whose message slot cshell+0x26f60 runs the base reader first and then reads the sub again (DISASM): **0x80** `[6 x u32]` (cshell+0x259b0: view pitch, view yaw offset, their rates, their accelerations -> +0x78..+0x8c; also sets the "update now" byte +0x7dc), **0xc0** the same six words then `[flags:u32]` -> +0x7b0, **0x40** `[flags:u32]` -> +0x7b0; also 0x14 / 0x15 `[u16]` -> +0x5ae / +0x5b0, 0x20 `[u64]`, 0x23, 0x26 `[u8]` | these are the remote players' **aim**: the object eases its drawn pitch / yaw offset toward them (cshell+0x256f0) and turns seven body nodes (Upper_torso, Torso, Right_pelvis, Neck, Head, Left / Right_shoulder; cshell+0x266c0, node callback cshell+0x263f0, shares cshell+0x25a50 / cshell+0x24660). The reference server sent 0x80 whenever the sender's six 0x64 aux words changed (tdm2: 593 to object 189, 1 equal consecutive pair, median gap 78 ms, unreliable, to every link). A server **should** send 0x80 (DeCombat: DSV-153) |

So 1,594 of the 1,610 type 0x18 notes on the real client's link (2,004 of 2,028 on the bot's link) had no effect on the 2009 client. A server needs only 0x11 (record refresh after the spawn), 0x1b (box) and, PLAUSIBLY,
0x1f / 0x25; it need not send 0x08, 0x13, 0x1a; it **should** send 0x80 for the remote aim (correction above; the "never send 0x40, 0x80, 0xc0, 0x14, 0x15" of the first read was based on the base handler only). Notes that reach the client before the create record of their object are dropped (cshell+0x20fbe0 finds no object); send them after it. For an NPC character (the `has_tail` form, 8.2.1)
a co-op server sends 0x24 after each damage; 0x11 re-reads the record without re-running the init, so a refreshed health, name, team, rank or icon kind does not reach the HUD
(only `npc_kind` does, for the bar, which is read every frame; `FIRETEAM_2009.md` 1.5, PROVEN control flow).

Type 0x17 kind 5 (HiSec case, HISEC_2009 3.1): the create record is `[case_key:u32]` (record +0x48), the key shell 0xd6 uses
to address this case. Its server message through 0xa6 (object slot +0x30, cshell+0x2828e0, PROVEN) is `[sub:u8]`: sub 1
`[fx_name:zstr8(256)]` replaces the attached client FX (the old one is removed first; none when empty); sub 2
`[remaining:u32]` sets the countdown the HUD shows (a negative value reads as 0; cshell+0x2823b0). The unit of `remaining` and
whether the object counts it down by itself are UNPROVEN (HISEC_2009 Q-1).

### 8.4 Second consumer (cshell+0x26e070)

After the rewind, cshell+0x26e070 (on shell +0x1a88) reads `[type:u8]` again and handles only `type` = 0x64, and
only with an object: `[name:zstr8(256)][value:u32][has_target:u8]` then, when `has_target` != 0,
`[target:objref16][pos:pos48]`; it attaches the named item to the object through the engine model interface
(object slots +0xa0, +0x9c, +4 then +0x44(obj, 2, ...)). Meaning PLAUSIBLE (an attachment by name). No other type
is consumed here, so type 0x64 never collides with the dispatcher's types (< 0x3d).

## 9. Subsystem index

Every server -> client shell id of this catalogue, grouped by what it drives (section in brackets).

| subsystem | ids |
|---|---|
| match and round flow, timers | 0x45 match begin (2.1.1), 0x44 / 0x3a round begin, 0x46 / 0x3c round result (2.1.2), 0x48 / 0x3d round close, 0x41 timer stop, 0x47 objective layers off, 0x40 training end, 0x3f result screen (2.1), 0x99 level exit (6.1), 0xd7 sub 4 timer (3.5), 0xd4 respawn countdown (7.1) |
| teams, scores, scoreboard | 0x43 clear, 0x4a team score, 0x5e team swap, 0x5f clan status (2.1, 2.3), 0x7d match settings, 0xb5 / 0xc4 round wins, 0xa2 ping table, 0xc0 group summary (3.2, 3.4) |
| players | 0x6b player info, 0x6c player left, 0xca sprint permission, 0xd5 object label (3.2), 0x67 player state (4.1) |
| kill feed, deaths, streaks | 0x6d kill (3.3.1), 0xc8 multi-kill (3.3) |
| health, armour, damage | 0x66 kinds 2, 3, 0x0f, 0x10, 0x13, 0x14 (4.3), 0x74 damage taken (4.3), 0x66 kind 0x0c screen flash (2.2) |
| weapons, inventory, pickups | 0xb2 give, 0xb3 equip, 0xb4 remove, 0x7f switch, 0xd8 use / refill, 0x66 kinds 1 and 4 (4.3, 4.4), 0x92 weapons used, 0x36 held item, 0x77 overlay (2.5), 0x86 weapon note (7.2), object type 0x17 pickup (8.2) |
| movement and camera | 0xa0 mover update, 0x6a view angles, 0xa3 camera kick, 0xaf character event, 0xa4 world render reset (4.2), 0xad track object (4.4) |
| chat, text, sound | 0x68 chat (2.4), 0x79 voice line (2.4), 0xc7 server log, 0xbe point sound (3.6), 0xd9 music (4.2), 0x00 console text (7.2) |
| mode objectives | 0x51 bomb planted, 0x50 bomb defused, 0x4f bomb exploded, 0x56 objective item (2.2), 0xc6 objective state, 0xc9 co-op mission (6.1, 6.2), 0xd7 co-op, 0xd6 HiSec case, 0xdb respawn token, 0xcb bounty mode, 0xd3 infection mode (3.5) |
| match end, result screen | 0x46 round result, 0x65 player summary, 0xc0 group summary, 0x3f result screen, 0x99 level exit |
| vote kick | 0xb7, 0xb9, 0xd0, 0xbb, 0xcf, 0xbc, 0x7e, 0xb6 (7.1) |
| radar | 0x84, 0xcd (7.1), 0x56 (2.2) |
| connection, session | 0x72 identify request, 0x73 reload weapon table, 0x9e game endpoint, 0xac integrity (5.1), 0x88 world switch state, 0x9b server error, 0x90, 0x8e (7.2) |
| diagnostics | 0xc1, 0xc2 network statistics, 0xc3 performance statistics (3.6) |
| no effect | 0x42, 0x3b (2.1), 0x70 (3.6), 0x97 (6.1), 0x86 with the 2009 weapon class (7.2); 0x00 has no visible effect (7.2) |
| world-object client objects | shell object message types (8.2), 0x64 attachment (8.4), shell id 0xa6 (7.2, 8.3) |
| player notes (meaning UNPROVEN) | 0x5a, 0xda (4.4) |

Ids that appear in no stage and are dropped silently: every id not listed above (dispatch chain of GM §2 plus the
corrections of section 7.1).

## 10. Messages needed for one TDM round (server -> client)

Assumptions: one Team Deathmatch round on a world the client already has, two teams, no clan match, no co-op.
`mode_class` is the TDM value of the user's ROM (GM §1.2: none of 2, 3, 5, 6, 9, 10, 11; read at run time, never
hard-coded); where a layout depends on `mode_class` the TDM branch is used. Engine messages are those of
`UDP_SESSION_2009_A.md` §9.2 and `_B.md`; they are named here only where game messages must follow them. Every game
message below goes inside `S2C_SHELL_MSG` (engine id 13), reliable and in this order (PD-S2C-1). "per player" means
to every client in the match; "own" means only to the player concerned.

Join and load (per joining client):

1. 0x9e `GS_GAME_ENDPOINT` `[mode_class][host][port]` (5.1) before any message whose layout depends on
   `mode_class` (0x7d, 0x45, 0x46, 0x65). Own.
2. 0x72 `GS_IDENTIFY_REQUEST` (5.1); the client answers with its character data (client -> server 0x72). Own.
   Optional: 0xac sub 0 with version 0x100 (5.1).
3. Engine world load and the object creates of every player character (`UDP_SESSION_2009_A.md` §9.2 S4..S10;
   each character create record carries a shell object blob of type 0x18 and a second object for the held weapon, 8.2, 8.2.1; V-S2C-1 settled).
4. 0x7d `GS_MATCH_SETTINGS` (3.2): `goal` = the kill target, scores 0 / 0, `round_limit` 1, `round_no` 0,
   `own_team`; for `mode_class` 7 or 8 also `n` = 0. Own.
5. 0x6b `GS_PLAYER_INFO` (3.2.1) for every player in the match including the joining one (kind 1; kind 0 for a
   player who joins a running match, to the others, so they get the join notice); each only after that player's
   character object exists on the receiving client. To the joining client for everyone; to everyone else for the
   joining player. Observed order (REFERENCE, accepted; 3.2.2): the joiner gets kind 2 for itself and for every other player; the others get the joiner's record as kind 0 directly followed by kind 2; the
    spawn set and 0x67 `[1]` precede the joiner's 0x87.
6. 0x88 `GS_WORLD_SWITCH_STATE` `[2]` (7.2; `UDP_SESSION_2009_A.md` S11). Own.
7. 0x66 kinds 0x0f, 0x10, 2, 3 (4.3): maximum health, maximum armour, health, armour. Own.
8. 0xb3 `GS_WEAPON_EQUIP` for the drawn weapon and 0xb2 `GS_WEAPON_GIVE` for every other loadout weapon (4.4),
   with ammo counts. Own.
9. 0xa0 `GS_MOVER_UPDATE` with flag 0x0020 (spawn position, box, new `seq`) and 0x6a `GS_VIEW_ANGLES` kind 0 (4.2).
   Own.
10. 0x67 `GS_PLAYER_STATE` `[1]` (4.1; `UDP_SESSION_2009_A.md` S12): with steps 3, 6 and the engine view object the
    client closes its loading screen. Own.

Round start (per player; also to a player who joins later, right after step 10):

11. 0x45 `GS_MATCH_BEGIN` (2.1.1): `round_secs` = the round time in seconds, `show_intro` 1, `waiting_banner` 0,
    `countdown_secs` (0 = no start countdown), the intro block built from the world's map and radar data of the ROM
    (titles may be empty: an empty `notice_body` shows no banner), then `round_no` (the last field, 0 or 1).
12. 0x4a `GS_TEAM_SCORE` `[0][0]` (2.1).
13. 0xa2 `GS_PING_TABLE` (3.2), then about every few seconds (DeCombat choice, PD-S2C-4: every 5 s).

Each hit and each kill:

14. To the victim: 0x74 `GS_DAMAGE_TAKEN` (4.3) and 0x66 kind 2 with the new health (kind 3 for armour).
15. Per player: 0x6d `GS_KILL` (3.3.1) with `victim`, `killer`, `weapon`, `hit_zone`, `no_count` = 0; the clients
    count kills and deaths themselves.
16. To the victim: 0x67 `[2]` (dead), later 0x67 `[3]` while it waits; optional 0xd4 kind 1 with the respawn
    countdown (7.1). The client reads its `secs` field only while the `waiting_banner` flag of the last 0x45 is
    clear (client state), so after step 11 with `waiting_banner` = 0 a 0xd4 kind 1 carries `secs` (DeCombat sends
    no 0xd4 today).
17. Per player: 0x4a `GS_TEAM_SCORE` with the new team kill totals (DeCombat: 0xb5, never 0x4a, 3.3.2). 0xc8 `GS_MULTI_KILL` is a combo banner that the reference stack never sent and DeCombat does not send; the multi-kill level rides the `kill_kind` of the 0x6d of step 15.
18. Respawn (to the respawning player): steps 7 to 10 again (0x66, weapons as needed, 0xa0 / 0x0020, 0x6a,
    0x67 `[1]`), sent when the client's own respawn request (0x89, about 7000 ms after it processed 0x67 `[2]`) arrives; the 2009 client arms that wait itself and the server sends no arming message (4.1.1).

Any time: 0x68 `GS_CHAT` (2.4); 0x6b for a joining player and 0x6c `GS_PLAYER_LEFT` for a leaving one (3.2);
0xb4 / 0xd8 sub 7 for weapon removal and ammo refills (4.4).

Round and match end (per player):

19. 0x46 `GS_ROUND_RESULT` (2.1.2): `round_secs` 0, `win_team` (1, 2, or 0 for a draw), `wins_team1`,
    `wins_team2` (the team totals of the round: PLAUSIBLE for TDM).
20. 0x65 `GS_PLAYER_SUMMARY` (3.4) for every player: experience before, gains and bonuses (the values come from
    the lobby's reward rule, `DC_DEFAULTS.md` CD-54).
21. 0x3f `GS_MATCH_RESULT_SCREEN` `[result_flag][result_value]` (2.1): opens the result screen, which shows the
    roster with the 0x65 values (field meanings UNPROVEN; V-S2C-3).
22. 0x99 `GS_LEVEL_EXIT` `[""][0][1]` (6.1): back to the menu screens, when the server ends the session itself
    (PLAUSIBLE; the lobby flow of `PROTOCOL_2009.md` decides what the player sees next).

Observed instead (REFERENCE, 2.1.3): steps 19-21 as 0x46 (with the deciding kill, or with 0x6c on a forfeit), 3 s later 0x47 / 0x6b refresh / 0x65 / 0x48, 3 s later 0x3f; no 0x99, no server bye; the 2009 server's order and
delays are unknown. Step 17's 0x4a was never sent by the reference stack, which carries team kills in 0xb5 (3.3.2). DeCombat follows the observed set (D, DSV-35 / DSV-33): steps 17 (0x4a), 19 (0x41) and 22 (0x99) are not sent;
step 22 stays the only way for a server to send a client to the menu without a leave of its own (PLAUSIBLE) and is unused.

### 10.1 Observed start order of the reference server (bot reference run, dserver-slice3 S3-7; REFERENCE)

Per bot, in the reliable order that `dc-dserver -startorder reference stack` follows (0x9d and the 0xa6 notes omitted):

1. Link: shell 0x90 `[1]`, shell 0x88 `[2]`, then `S2C_VERSION`, `S2C_CLIENT_ID` and the batched `S2C_FILE_LIST` (8 entries in the bot run, 10 in tdm1).
2. After the client's `C2S_LOAD_STATE(1)`: single-entry file lists, shell 0x72, 0x9e `[mode 1][host][port]`, 0x9d `[u32 0]`, 0x7d, `S2C_VIEW_OBJECT`.
3. After the client's 0x72 / 0x85: 0x6b (own), 0x6a, 0x67 `[1]`, 0x66 kinds 0x0f, 0x10, 0x17, 0x13 (`b1` 1), 0xb2 x4 with 0x7f after the first two (`auto` 0), 0x7f (`auto` 1, the drawn weapon), 0xa0 `0x000d` (body),
   0xa0 `0x0020` (seq 1), 0x66 kind 0x13 (`b1` 0), the 0xb2 / 0x7f set again, 0x66 kinds 0x0f / 0x10, 0x6b (own, again), 0x43, 0xa0 `0x0080` `[1]`, 0x44 `[0][1][1][0]`, 0xb5 `[0][0]`, 0x6b (other) x2, 0xa0 `0x0080` `[0]`, 0x45,
   then the initial object group (80 creates, `UDP_SESSION_2009_B.md` 3.5.1).
4. After the client's 0x87: 0x90 `[0]`, 0x88 `[3]`, 0xa0 `0x0020` (seq 2, same position), 0x66 kinds 1 (slots 2, 3), 2, 3, 0x18 and 6, engine id 22, 0xa0 `0x0008` (friction).

0x66 kinds 0x17, 0x18 and 6 reach no handler of the 2009 client (4.3). The 0x7f with `auto` 0 has no client effect (4.4). The reference stack sent 0x6b before the create records of the characters it names; 3.2.1 field 5 requires the
object to exist, and the type 0x18 init (`roster_bound` 1) binds the character when its create arrives later. The real client then queried the pid with 0x8c (3.2.2); DeCombat keeps creates first. The real-client captures show
the same order with the kinds and the doubled spawn set of 3.2.2 and 4.4.1.

DeCombat start and join order (D, DSV-36 proposed, refining DSV-22; `dc-dserver` slice 4, TESTED in `game_sim` "reference stack start and late-join order", RUN with two bots): at the link
0x90 `[1]`, 0x88 `[2]`; at the placement 0x72, 0x9e, 0x9d `[0]` (`-no9d` omits it; the 2009 client drops it, 7.2), 0x7d, then the creates (characters and the world-file objects of
`UDP_SESSION_2009_B.md` 3.5.2) and `S2C_VIEW_OBJECT`; after the player info the 0x6b records (others kind 0 then 2, the joiner kind 2), the spawn set with 0x67 `[1]` (one grant block
per spawn, DSV-38), then 0x43, 0x44 (remaining seconds), 0xb5, 0x45 **only to the round's first spawn**, 0xc3 sub 1; all before the client's 0x87; after 0x87 0x90 `[0]` and
0x88 `[3]`. A joiner of a running round gets 0x43, 0x44, 0xb5 and no 0x45, and 0x90 `[0]` / 0x88 `[3]` already at the link. Every spawn ends with the player's kind-2 record to
everyone; 0xa2 goes out every 5 s (PD-S2C-4, the reference stack's 3 s is not adopted). Differences that remain from the reference stack: creates before 0x6b (3.2.1 field 5; the reference stack sent 0x6b first),
one grant block instead of two, no 0xa6 notes; 0xc3 is still sent once and the real client's reaction to it is unverified (UNPROVEN).

## 11. Corrections, UNPROVEN items and the re-run / capture lists

### 11.1 Corrections to sibling specs (for their authors and the coordinator)

| # | spec and place | correction | evidence |
|---|---|---|---|
| C1 | `GAME_MESSAGES_2009.md` §2 stage F row, and correction (3) of its report | stage F is the derived HUD manager; its slot +0x38 is cshell+0x45050, which handles 0x7e, 0x84, 0xb6, 0xb7, 0xb9, 0xbb, 0xbc, 0xcd, 0xcf, 0xd0, 0xd4 (section 7.1). cshell+0x26b220 is only the base table's slot | PE-DATA VA 0x374d06dc +0x38; DISASM cshell+0x46ed1..0x46ed9 |
| C2 | `GAME_MESSAGES_2009.md` §1.2 (the mode number "is not on the wire") | shell id 0x9e sets `client_shell`+0x16938 from a `u8` (section 5.1) | DISASM cshell+0x2f86e..0x2f87c |
| C3 | `GAME_MESSAGES_2009.md` §2 gating ("PLAUSIBLE that it is set once the in-game interface exists") | `match_ui`+0x6aa4 is set at the end of the interface init cshell+0x1b7ff0 (DISASM cshell+0x1b8864), i.e. at client start, and cleared only by the interface teardown (cshell+0x1b4e00 at 0x1b4ff3, cshell+0x1ba923): stage A is open during the whole game session | DISASM |
| C4 | `UDP_SESSION_2009_A.md` §8.3 d, S12, V-A4 | the player-state carrier is shell id 0x67 `[state:u8]` (section 4.1); cshell+0x1f4ab0 is a serialisation method, not the network path | cshell+0x1f0470, PE-DATA VA 0x374d3334, DISASM cshell+0x61870, cshell+0x1f97d0 |
| C5 | `UDP_SESSION_2009_B.md` §7.1 fallback row (0x00 "PLAUSIBLE: console print") | CShell reads at most 49 characters and only formats them into a local buffer: no visible effect | DISASM cshell+0x1a3c97 (`push 0x32`), cshell+0x1a39f0 |
| C6 | `UDP_SESSION_2009_B.md` §7.1 stage d row | 0x72 is handled without reading its body; 0x73 reloads the weapon table; their slots are +0x3c / +0x40 -> +0x08 / +0x0c | DISASM cshell+0x23c620, cshell+0x23c610 |

### 11.2 UNPROVEN items

1. Reliability of every server -> client game message (not visible in the client; PD-S2C-1).
2. Field meanings marked UNPROVEN or PLAUSIBLE in sections 2-8, in particular: 0x3f `result_flag` / `result_value`;
   0x6b `v64`, `v68`, `v6c`, `look` bytes, flags, mode values; 0x74 `mode`, `part`; 0x66 kind 1 `extra`; 0xa0 is covered by `MOVEMENT_2009.md`;
   0xaf, 0xad, 0x5a, 0xda, 0x8e, 0x90; the bounty and infection sub-messages (0xcb, 0xd3) beyond their first fields;
   the vote messages' value fields.
3. Player state values 4 and 5 (4.1).
4. The layouts of the class-read object types of 8.2 and of 0xa6 class data (8.3), except type 0x18 (8.2.1, widths REFERENCE-CONFIRMED, meanings UNPROVEN) and its 0xa6 sub-ids (8.3), and the relay forms of types 0x06 and 0x19 (8.2; type 0x06 bytes `a`, `b`, `c`, `d`..`g` UNPROVEN; their observed values per record kind, the blast's included, are listed in 8.2 since spec-fold-6). Type 0x13's blob widths above 255 (V-PROJ-4).
5. Which shell object type the 2009 server attached to each world object (8.2): settled only for the reference server (players 0x18, flags 0x17 kind 2, zones 0x37); the 2009 server's own choice is unknown.
6. The TDM `mode_class` number: REFERENCE-CONFIRMED as 1 in 0x9e (tdm1, tdm2, tdm4; also the ROM mode row); read from the ROM at run time, GM §1.2.
7. The order of 0x65 / 0x3f / 0x99 at match end for the **2009** server: the reference stack sends 0x46 at the end of a single-round TDM match and the order of 2.1.3 (goal and forfeit ends); the real client's display of them is
   not covered by any capture (the receiver was a bot); 0x99 never observed.
8. Which values the 2009 server used in 0x6d (`kill_kind`, `streak_flag`, `feed_value`), 0x3f (`result_value`), 0x45 (`waiting_banner`, formerly `first_round`: the reference stack sent 0 and a 1 draws the waiting banner, 2.1.1 step 3) and 0x74 (`dmg_kind`, `mode`, `part`). DeCombat sends
   the REFERENCE values the real client accepted (DSV-33, DSV-35, D), which leaves the 2009 values open.
9. Closed by capture: the sprint permission 0xca (3.2); the player character form (8.2); the respawn arming (4.1.1, DISASM).

### 11.3 decompiler re-run list (functions cited here that have no clean decompiler function; DISASM grade until created)

| rva | role (ours) |
|---|---|
| cshell+0x61870 | `local_player` derived slot +0xb8 (0x67) |
| cshell+0x5cca0, cshell+0x5cd20 | derived slots +0xc8 (0x6a), +0xc4 (0x74) |
| cshell+0x1f27f0, cshell+0x1f0680 | base slots +0xbc (0xa0), +0xcc (0xa4) |
| cshell+0x23c570 | stage D handler |
| cshell+0x23c610, cshell+0x23c620, cshell+0x23c8e0, cshell+0x2f860 | stage D slots (0x73, 0x72, weapon-table reload, 0x9e) |
| cshell+0x22f6d0 | base mode-rules slot +0x40 (0x97) |
| cshell+0x2f410 | weapon object slot +0x04 (0x86) |
| cshell+0x62eb0, cshell+0x62ef0, cshell+0x62f20, cshell+0x62f50, cshell+0x62f80, cshell+0x632c0, cshell+0x633d0, cshell+0x64140, cshell+0x64160 | status-block setters (health, armour, maxima, ammo, weapon removal, weapons used, clip) |
| the router create slot (shell +0x164e0 object, slot +0x00) and the per-type read methods of the class-read object types | 8.2 |

### 11.4 Capture validation items (for the coordinator; no live run in this wave)

| id | what to record | settles |
|---|---|---|
| V-S2C-1 | the create-record blobs of player characters and pickups | 8.2 types per object. **SETTLED** for players, pickups and zones (REFERENCE-CONFIRMED, tdm2, tdm1; 8.2, 8.2.1) |
| V-S2C-2 | the game messages around a spawn, a kill and a respawn | section 10 steps 7-18, 0x67 values. PARTLY: spawn set, a kill (4.1.1, 3.3.2) and a bot respawn recorded; a death and respawn of the **real** client not yet |
| V-S2C-3 | the end of a TDM match | 0x46 / 0x65 / 0x3f / 0x99 order and the 0x3f fields. **SETTLED for the reference stack** (goal and forfeit ends, 2.1.3), no 0x99; the real client's display not covered |
| V-S2C-4 | 0x7d and 0x45 at match start | `goal`, `round_no`, intro block contents. PARTLY: 0x45 only to the first spawn of a round, a joiner of a running round gets 0x43 / 0x44 / 0xb5 (2.1.3); `goal` = room goal |
| V-S2C-5 | reliability flags of game messages (transport block of each) | 11.2 item 1. PARTLY (REFERENCE): 0xa2, most 0xa6 and engine id 17 unreliable; roster, state, status, weapon, round and result messages reliable |
| V-PROJ-1 | a throw over open ground against a DeCombat server (create only, no stream), with a screen recording | whether the 2009 client's type 0x13 object (or the engine) moves the grenade by itself (gravity, bounce) or freezes it at the create position when no stream follows (`UDP_SESSION_2009_B.md` 3.5.4). First observation (rust4, OBSERVED, one playtest report; spec-fold-8): it does not move without the stream; the DeCombat default is now the stream (DSV-41 amendment); a screen recording still settles it |
| V-PROJ-2 | several blasts on different floors, slopes and walls (reference stack or DeCombat) | the meaning of the blast's `a` / `b` (8.2) and whether `normal` is always (0, 1, 0) |
| V-PROJ-3 | a throw while turning or looking up | the source of the create rotation (not the thrower's yaw in tdm2 / tdm4) |
| V-PROJ-4 | a throwable with a weapon or ammunition id above 255, if the ROM has one (else the 11.3 re-run of the type 0x13 read method) | the blob widths of row 0x13 |
| V-PROJ-5 | a player joining while a grenade is in flight | whether a late joiner gets the projectile's create (no capture has it; DeCombat sends none) |

## 12. Names registered here (for `GLOSSARY_UDP.md`, new section S.G)

Message names: `GS_` + our name in sections 2-7 (for example `GS_ROUND_BEGIN` 0x44, `GS_MATCH_BEGIN` 0x45,
`GS_ROUND_RESULT` 0x46, `GS_PLAYER_INFO` 0x6b, `GS_KILL` 0x6d, `GS_PLAYER_STATE` 0x67, `GS_PLAYER_STATUS` 0x66,
`GS_DAMAGE_TAKEN` 0x74, `GS_WEAPON_GIVE` 0xb2, `GS_WEAPON_EQUIP` 0xb3, `GS_PLAYER_SUMMARY` 0x65,
`GS_MATCH_RESULT_SCREEN` 0x3f, `GS_GAME_ENDPOINT` 0x9e, `GS_LEVEL_EXIT` 0x99, `GS_CHAT` 0x68); object types:
`OBJ_` names of 8.2. Objects: `match_ui`, `roster`, `local_player`, `conn_mgr`, `mode_rules`, `hud_mgr`, `router`,
`session_info`, `weapon_db`, `match_stats` and the `hud_*` elements of 1.2. Proposed defaults: PD-S2C-1 (all game
messages reliable and ordered), PD-S2C-2' (player states 1 / 2 / 3 / 6 with the client-armed respawn of 4.1; the older PD-S2C-2 wording is superseded, Appendix F.2), PD-S2C-3 (integrity messages,
5.1), PD-S2C-4 (ping table every 5 s); the coordinator assigns their CD numbers.

## Appendix O: reference stack notes (spec-fold-3, 2026-10-24)

Source: `UDP_REFERENCE_FINDINGS_botclient.md` items O1-O13 (the evidence record; runs and captures are cited there). Grade used here:
**REFERENCE-CONFIRMED** = the reference server stack (a second, independent writer or reader, NOT the 2009 server) behaves as the spec text
says when `dc-botclient` talks to it; it is black-box evidence under REFERENCE_TESTING.md section 1, distinct from PROVEN (2009 code or
captures). **Reference stack differs** = the reference stack does something else; the base text and its grade stand and the 2009 evidence decides (a capture
of the real 2009 client). No base grade was changed by this appendix and no reference stack value is a DeCombat rule value. No ROM names are quoted.

| item | spec place | note | state |
|---|---|---|---|
| O2 | 5.1 identify request 0x72, 7 shell ids | the reference stack's shell ids in order of first appearance (0x90, 0x88, 0xa6, 0x72, 0x9e, 0x9d, 0x7d, 0x6b, 0x6a, 0x67, 0x66, 0xb2, 0x7f, 0xa0, 0x43, 0x44, 0xb5, 0x45, 0xa2) were all handled by the bot's reader; the identify request 0x72 arrives right before the view object and the client's answer (0x72 character data) was accepted. **Gap found:** shell 0x9d (`0d 9d 00 00 00 00`) has no row in this catalogue, and 0x90's single bit is UNPROVEN (row 0x90); both are listed as open in the findings | REFERENCE-CONFIRMED (0x72 flow); 0x9d uncatalogued |
| O6 | 7.2, row 0x88, row 0x90 | **Reference stack differs:** 0x88 state 2 and 0x90 arrive before `S2C_VERSION` and are sent again when the world starts; 0x88 state 3 follows the client's world-ready 0x87. Record: 0x88 may arrive before step S2 (game layer, harmless) | Reference stack differs |
| O9 | 4.1 player state, PD-S2C-2 | **Reference stack differs:** state 1 is sent directly (no state 3 first); PD-S2C-2 (proposed) says 3 then 1, DC-C2S-10 (proposed) likewise (both wordings superseded 2026-10-25, Appendix F.2); both pass the client's test. Mover update, facing, weapons and status messages follow as listed in the session A appendix | Reference stack differs |
| O10 | 11.2 item 1, PD-S2C-1 | **Reference stack differs:** the reference stack sends some shell messages in the UNRELIABLE block: 0xa2 (ping table, about every 3 s) and part of the 0xa6 traffic. The client does not show the server's choice, so this neither confirms nor refutes the 2009 server; a DeCombat server may keep PD-S2C-1 (all game messages reliable) | Reference stack differs; reliability UNPROVEN |

Count: REFERENCE-CONFIRMED 1 item (O2), reference stack differs 3 (O6, O9, O10).

## Appendix C: capture notes (spec-fold-4, 2026-10-25)

Sources (evidence records, kept in full): `CAPTURE_tdm1_GAME_VALIDATION.md` section 3, `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md` sections 1-4, `CAPTURE_tdm4_KILL_VALIDATION.md` sections 3-4 and 6, `REFERENCE_FINDINGS_botduel.md`,
`CAPTURE_tdm4_WEAPON_FORMS.md`, and `SESSION_B_S2C_addendum_dserver3.md` (stub since this fold). Server bytes are the reference stack's (REFERENCE); the grade REFERENCE-CONFIRMED needs the real client's next messages as proof of
acceptance. The shell ids the real client received in tdm1, with counts: 0x90 x2, 0x88 x2, 0x72, 0x9e, 0x9d, 0x7d, 0x6b x3, 0x6a, 0x67, 0x66 x16, 0xb2 x8, 0x7f x6, 0xa0 x6, 0x43, 0x44, 0xb5, 0x45, 0xca x48, 0xa2 x52, 0x74 x2,
0x68 x2, 0xa6 x2,975 and engine id 17 x140; every one whose layout this catalogue gives parsed exactly (0 bits left) with `mode_class` 1; not checkable: the class data of 0xa6 and 0x9d. Never received in tdm1: 0x6d, 0x67 `[2]` / `[3]`,
0xd4, 0xd7, 0xdb, 0x46, 0x65, 0x3f, 0x99, 0x6c, 0xc3, 0xc8, 0xb3, 0x4a, 0x5e, 0xac, 0x73.

| id | spec place | observation | grade |
|---|---|---|---|
| 0x72 | 5.1, 3.2.2 | answered by 0x72 + 0x85 in the same tick | PROVEN-CAPTURE (reaction) |
| 0x9e | 5.1 | `mode_class` 1, host, port | REFERENCE-CONFIRMED |
| 0x7d | 3.2, 3.2.2 | layout, `goal` = room goal | REFERENCE-CONFIRMED |
| 0x6b | 3.2.1, 3.2.2 | layout; create-before-roster rule; kinds 0 / 2 order | REFERENCE-CONFIRMED; rule PROVEN-CAPTURE by the 0x8c reaction |
| 0x67 | 4.1, 4.1.1 | state 1 before 0x87 accepted; death 3 then 2 in one block; client arms on state 2 only | PROVEN-CAPTURE (carrier), DISASM (client rule), REFERENCE (order) |
| 0x66 | 4.3, 4.3.1 | kinds 1, 2, 3, 0x0f, 0x10, 0x13 layouts | REFERENCE-CONFIRMED |
| 0x74 | 4.3, 4.3.1 | layout; `from_dir` magnitude, `zone` = damage type | REFERENCE-CONFIRMED; semantics from the client use (S2C 4.3, DISASM) |
| 0xb2 / 0x7f | 4.4, 4.4.1 | slot filling used by the client | PROVEN-CAPTURE (use), REFERENCE (form) |
| 0xa0 / 0x6a | 4.2 | 0x0020 teleport: `move_seq` echo 1 then 2 and the position; 0x6a kind 0: the first 0x64 yaw is bit-identical | PROVEN-CAPTURE (reaction) |
| 0x43 0x44 0xb5 0x45 | 2.1.3 | layouts incl. the TDM intro branch; 0x45 only to the first spawn | REFERENCE-CONFIRMED |
| 0xa2 | 3.2.2 | every 3.0 s, unreliable | REFERENCE |
| 0xca | 3.2 | sprint permission | PROVEN-CAPTURE (reaction) |
| 0x68 | 3.2.2 | echo layout | REFERENCE-CONFIRMED |
| 0x6d 0xb5 | 3.3.2 | kill message and team kills in 0xb5; no 0x4a; 0xc8 is a combo banner the reference stack never sent | REFERENCE-CONFIRMED (weak) |
| 0x46 0x47 0x65 0x48 0x3f | 2.1.3 | goal and forfeit end order; no 0x99 | REFERENCE (receiver was a bot) |
| 0x88 0x90 0x9d | 7.2 | 0x88 state 2 before 0x87 is sufficient; 0x90 flag values; 0x9d dropped | PROVEN-CAPTURE (0x88 sufficiency), REFERENCE |
| object blobs | 8.2, 8.2.1, 8.3 | player character type 0x18, pickups 0x17 kind 2, zones 0x37, relays 0x19 / 0x06 | REFERENCE-CONFIRMED |

Undecided or not in any capture: the death and respawn of the real client; a hit on a player by the real client (`has_hit` of 0x82 was set only against the bot, tdm4); team chat and radio; the class data of the
0xa6 notes other than type 0x18; the 2009 server's choices for every REFERENCE row above. dc-dserver changes that follow from this fold are listed in the staged proposal files of the report, not here.

Count: PROVEN-CAPTURE 6 rows (0x72, 0x6b rule, 0x67 carrier, 0xb2 / 0x7f use, 0xa0 / 0x6a, 0xca; plus 0x88 sufficiency), REFERENCE-CONFIRMED 9, REFERENCE 3.

## Appendix F: fold record (spec-fold-5, 2026-10-25)

Sources: the reports `reports/dserver-slice4.md` (DSV-30 .. DSV-38, live rounds 1-3), `lobby-loadout.md` (LDO-1 tokens on the pipe), `client-reference stack.md` section 4
(the Rust client against the real client's PROVEN-CAPTURE forms), the staged `dserver-slice4_DC_DEFAULTS.md` (its rows are now in `DC_DEFAULTS.md` as PENDING), and the
capture-validation ids of `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md` (D1-D18) and `CAPTURE_tdm4_KILL_VALIDATION.md` (K1-K17, V-KILL-1) that no base spec cited by id. Grades: header
plus **D** (defined there). No new 2009 evidence is added by this fold: every PROVEN-CAPTURE / REFERENCE-CONFIRMED / DISASM statement below was already graded in an evidence record
or an earlier appendix; this fold records where it now lives and what DeCombat chose.

### F.1 DeCombat server forms folded into the base text (all D, proposed; `dc-dserver` slice 4 on master e191a26)

| row | place | what DeCombat sends | evidence the form must satisfy | test state |
|---|---|---|---|---|
| DSV-31 | 2.1.3 | team empty for 10 s ends the round for the other team | REFERENCE: forfeit end 0.31-0.34 s after the leave (the delay is ours) | TESTED, RUN |
| DSV-32 | 4.1, 4.1.1 | respawn only on the client's request (guard = wait - 250 ms); fallback timer 10 s only in classes 5 / 9; death 0x67 3 then 2 | DISASM (client arms itself, cshell+0x60ac0); REFERENCE (respawn on request, 37 of 37) | TESTED, RUN 7 / 7 |
| DSV-33 | 3.3.2, 3.4 row 0xb5, 4.3.1 | 0x6d observed values, 0xb5 team kills, never 0x4a, never 0xc8 (0xc8 is a combo banner the reference stack never sent; DeCombat does not send it); 0x74 observed form | REFERENCE-CONFIRMED forms (tdm4); 0x6d field meanings DISASM (spec-fold-9), 0x74 meanings UNPROVEN | TESTED (byte-equal heads), RUN |
| DSV-35 | 2.1.3, section 10 | end set 0x46; +3 s 0x47 / 0x6b / 0x65 / 0x48; +3 s 0x3f; never 0x41 / 0x99; no server bye | REFERENCE timing and forms; the real client's display of them is in no capture | TESTED; RUN (team-empty end only) |
| DSV-36 | 10.1 | start and join order, 0x45 only to the first spawn, joiner without 0x45 | REFERENCE-CONFIRMED (tdm1, tdm2: the real client accepted both orders) | TESTED, RUN |
| DSV-37 | `UDP_SESSION_2009_B.md` 3.5.2 | world-file zones (type 0x37) and capture flags (type 0x17 kind 2) | REFERENCE-CONFIRMED forms (tdm2); ROM-gated bit identity, see B 3.5.2 | TESTED (ROM-gated) |
| DSV-38 | 4.4.1 via `CAPTURE_tdm4_WEAPON_FORMS.md` section 6 | weapons by equipment slot from the pipe's `loadout=`; ROM default issue without it | PROVEN-CAPTURE (slot i = weapon slot i, the client draws the lowest granted slot) | TESTED; RUN: every spawn granted slots 0-3, attributed by the slice report to the ROM default issue (its lobby sent no `loadout=`), so the `loadout=` path is TESTED only |
| DSV-30, DSV-34 | 4.3.1 (amount only) | damage amount and the shot trace are server rules, not wire forms | none (rule values; the reference stack's fractions are not used) | TESTED, RUN |

### F.2 Proposal rewrites (the corrected respawn model)

`DC_DEFAULTS.md` "Capture notes 2026-10-25" demand that DC-C2S-10, PD-S2C-2 and GMD-3 be rewritten before approval: the 2009 client arms its **own** respawn wait when it processes
0x67 state 2 and then asks once with 0x89 / 0xc5 / 0xd2 (DISASM cshell+0x60ac0, cshell+0x6203e; bot-confirmed against the reference stack, not yet PROVEN-CAPTURE); the server respawns on that
request. Rewritten here and in the sibling specs, old text kept and marked superseded:

| row | superseded wording | rewritten wording (D, staged) | where |
|---|---|---|---|
| PD-S2C-2 -> PD-S2C-2' | "player states on the wire: 1 alive, 2 dying, 3 waiting for respawn, 6 spectating" (with DC-C2S-10: 3 two seconds after 2, server respawn 7000 ms after the death) | 1 alive; 2 dead, arms the client's wait; 3 dead and waiting, sent with 2 and never alone; 6 spectator; the respawn follows the client's request (DSV-32) | 4.1, section 12 |
| DC-C2S-10 -> DC-C2S-10' | state 3 before 0x87; death 2, 3 after 2000 ms, respawn 7000 ms after the death | the spawn set with 0x67 `[1]` may come before 0x87 (the tested path); death 0x67 `[3]` + `[2]` in one block; respawn on request | `GAME_MESSAGES_2009_C2S.md` 12.3, Appendix A A4 / A11 |
| GMD-3 -> GMD-3' | the server arms the 7000 ms wait with 0xd7 sub 2 in classes 1, 4-9 and respawns 2000 ms after the wait without a request | no arming message in classes 1, 4, 6, 7, 8, 10; 0xd7 sub 2 only in class 5 (it sets the value the client tests); respawn on request; server timer only in 5 / 9 | `GAME_MODES_2009.md` 4.2 |

### F.3 tdm4 kill items (K-ids) in the server -> client text

| id | content | now in | grade |
|---|---|---|---|
| K8 | 0x74 + 0x66 to the victim, `from_dir` scaled, reference stack amounts not taken | 4.3.1 (and its DeCombat form) | REFERENCE |
| K9 | 0x6d fields, team kills in 0xb5, no 0x4a; 0xc8 is a combo banner the reference stack never sent (DeCombat does not send it) | 3.3.2 | REFERENCE (weakly REFERENCE-CONFIRMED) |
| K10 | 0x67 3 then 2 in one block, nothing more for 104 s | 4.1.1 | REFERENCE |
| K11 | no 0xd7 / 0xdb arming; the client arms itself | 4.1.1, `GAME_MESSAGES_2009_C2S.md` 3.3, `GAME_MODES_2009.md` 4.2 | DISASM |
| K12 | the bot stayed dead: the reference stack respawns only on request | 4.1.1 | DISASM + REFERENCE |
| K15 | round end and leave order and timing | 2.1.3 | PROVEN-CAPTURE (leave) / REFERENCE (end set) |
| K17 | relays: engine 17 type 0x19 sounds, type 0x06 impacts | 8.2 | REFERENCE |

K1-K7, K13 and K14 (client -> server) are cited in `GAME_MESSAGES_2009_C2S.md` Appendix F; K16 (lobby 0x394 / 0x3004) in `PROTOCOL_2009.md` section 7.2. V-KILL-1 (a capture in which the
real client dies and respawns) stays the open capture item: it would turn the respawn rows of 4.1.1 from DISASM + REFERENCE into PROVEN-CAPTURE.

### F.4 tdm2 differences D1-D18 (dc-dserver against the accepted forms) and where their wire facts live

The D-ids of `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md` section 8 compare a crate with the accepted forms; the wire facts belong to these base places (all REFERENCE-CONFIRMED unless noted). The crate
state of each id on master e191a26 is in the staged follow-up list `staging/spec-fold-5_CRATE_FOLLOWUPS.md` (not a spec).

| id | wire fact | base place |
|---|---|---|
| D1, D2, D4 | character create record (kind 1, type 0x18 blob, model block counts, weapon child at socket 2, flag life cycle) | `UDP_SESSION_2009_B.md` 3.5, 3.5.1; 8.2.1 here |
| D3 | remote stream `uf` 13, `yaw8`, three trackers | `UDP_SESSION_2009_B.md` 4.3 |
| D5 | unwrapped client yaw; repair with 0x6a kind 0 | `GAME_MESSAGES_2009_C2S.md` 2.1.1 (runaway yaw), 4.2 row 0x6a here |
| D6 | `CF_FILES` record of the weapon child on a weapon change | `UDP_SESSION_2009_B.md` 3.5 |
| D7 | type 0x19 sound relays (0x80, jump `k` 0x0f) to everyone including the sender | 8.2 |
| D8 | type 0x06 shot effect for every bullet | 8.2 |
| D9 | thrown projectile object (type 0x13 blob) and its removal | `UDP_SESSION_2009_B.md` 3.5; 8.2 |
| D10 | removal of character and weapon child in one kind-3 record with 0x6c | `UDP_SESSION_2009_B.md` 5.1, 3.5 (CV2-8) |
| D11, D16 | forfeit end and the end set | 2.1.3 |
| D12 | roster kinds 0 / 2 | 3.2.2 |
| D13 | 0xca sprint permission echo | 3.2 row 0xca (PROVEN-CAPTURE reaction) |
| D14 | 0x45 only to the round's first spawn | 2.1.3 |
| D15 | 0x65 second field = the character object | 3.4, 2.1.3 |
| D17 | spawn set before 0x87 (no state 3 first) | 3.2.2, 4.1 (PROVEN-CAPTURE: accepted) |
| D18 | 0xa6 notes mostly ignored by the client | 8.3 |

### F.5 From the Rust client's comparison with the real client (`client-reference stack.md` section 4)

Only two of its listed differences touch this catalogue; both stay open:
- L11: our client still sent 0x89 when its wait ran out after 0x46 (the reference stack ignored it, as DSV-32 does). Whether the 2009 client's round-end handling (0x46 / 0x48 / 0x3f, 2.1)
  disarms its wait is not read: UNPROVEN.
- L12: 0x46 carries its own pair `[wins_team1][wins_team2]` (0 / 0 in TDM) after the 0xb5 kill pair; which pair the 2009 HUD shows on the result banner is not read: UNPROVEN.

### F.6 Not folded

Crate state, recommendation lists and live-run logs (they describe our code, not the wire): left in the reports and in the staged follow-up list. The DSV rule VALUES (damage,
armour, 10 s, 250 ms, 120 s) are DC_DEFAULTS rows and are cited, not restated as wire facts.

Count: rows added with grade D 8 (F.1), proposal rewrites 3 (F.2), id cross references 7 K + 18 D; no existing grade changed.

## Appendix G: fold record (spec-fold-6, 2026-10-25)

Sources: the captures tdm2 (`20261025_001409_tdm2`) and tdm4 (`20261025_002236_tdm4`) decoded with our own readers (the `dc-botclient` capture reader `capdecode` and a scratch record / stream dump built on the same crate, outside the tree; identifiers and seq numbers only, no reference stack log was read); the reports `reports/{dserver-tidy, stack-verify, bot-respawn, bot-roundend, client-dserver-fix, render-styles}.md`; the addenda `GAME_MESSAGES_2009_S2C_addendum_blast_questions.md` and `RENDER_2009_addendum_styles.md` (both now stubs); `DC_DEFAULTS.md` rows DSV-39, DSV-40, GAIN-1, BOT-18, REN-1..11 (PENDING). Grades as in the header plus **D** (README). No existing grade was lowered; no REFERENCE value became a rule.

### G.1 The blast questions (dserver-tidy Q1-Q9), answered from the captures

| q | answer | place | grade |
|---|---|---|---|
| Q1 | blast `a` 71, `b` 0x59f0 (tdm2 seq 8516 / 8517) / 0x5ff0 (tdm4 seq 8064 / 8065), `c` 1, `d..g` 00 ff ff ff | 8.2 blast row | REFERENCE, ACCEPTED; meanings UNPROVEN (V-PROJ-2) |
| Q2 | `normal` exactly (0, 1, 0) in both blasts | 8.2 blast row | REFERENCE, ACCEPTED |
| Q3 | `from` = the release 0x82's `origin` (bit for bit in both), not `weapon_pos` | 8.2 blast row; B 3.5.4 | REFERENCE, ACCEPTED (PROVEN-CAPTURE for the 0x82 side) |
| Q4 | create `flags3` 0 (flags 0x1201 / 0x0100 / 0x00400020 / 0) | B 3.5.4 | REFERENCE, ACCEPTED |
| Q5 | animation block: main tracker only, narrow form, `anim` 0, time 0; rotation a full small quaternion, not the thrower's yaw | B 3.5.4 | REFERENCE, ACCEPTED; rotation source UNPROVEN (V-PROJ-3) |
| Q6 | yes: stream entries `uf` 4 in every stream message and reliable change records during the flight (one follow-up in tdm2; 5 + 9 + 1 in tdm4) | B 3.5.4 | REFERENCE, ACCEPTED; the client's own extrapolation UNPROVEN (V-PROJ-1) |
| Q7 | blob widths for ids above 255: not answerable from the captures (7 / 9 only) | 8.2 row 0x13 | UNPROVEN (V-PROJ-4, 11.3) |
| Q8 | removal and blast share the tick; the blast came first in tdm2 (earlier datagram), the removal first in tdm4 (one datagram, reliable block first); both accepted | B 3.5.4 | REFERENCE, ACCEPTED |
| Q9 | 0x66 kind 1 `[7][9][0.0][3][1]` to the thrower only, reliable, in the datagram of the create (tdm2 seq 8110, tdm4 seq 7690) | B 3.5.4; 4.3 row 0x66 | REFERENCE, ACCEPTED |

### G.2 Other changes

| place | change | grade |
|---|---|---|
| 8.2 row 0x13 | the 32-bit blob in both captures; widths open; pointer to B 3.5.4 | REFERENCE-CONFIRMED |
| 8.2 type 0x06 row | the observed `a` / `b` value lists extended with tdm4 (bullets and hits) and the blasts; `from` of a blast is the `origin` | REFERENCE |
| 8.2 DeCombat paragraph (new) | DSV-39 forms and the five differences from the answered reference stack form (blast bytes, no flight records / stream, no 0x66 count, no 0xa6 sub 0x20, create rotation) | D (proposed), TESTED (`game_sim`) |
| 2.1.3 | RUN record of stack-verify (team-empty and goal ends, both bots, timings, gains); "goal end not reached live" replaced | RUN (our stack) |
| 2.1.3 | DSV-35 consequence: each remaining client gets 0x6c for every earlier leaver before its link closes; `dc-client` keeps departed tracks | D; TESTED (dc-client `net_dserver`) |
| 11.2 item 4, 11.4 | blast values pointer; new capture items V-PROJ-1..5 | - |

### G.3 Notes on the evidence records (kept in full, not edited)

- `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md` 3.4 lists the 0xa6 [0x18][189] sub 0x20 of 112.062 under "s2c 6 only"; the capture has it on conn 5 too (seq 8516), so it went to every
  link; the 0x74 / 0x66 of that row are conn 6 only, as written.
- `GAME_MESSAGES_2009_C2S.md` 4.6 says "2.5-2.9 s later"; measured from the release 0x82 the blast came 2.515 s (tdm2) and 2.516 s (tdm4) later, 2.48 / 2.47 s after the create.

Count: questions answered 9 (Q7 as UNPROVEN with a capture item), new capture items 5, RUN records 1, D paragraphs 1.

## Appendix H: fold record (spec-fold-7, 2026-10-25)

Sources: `FIRETEAM_2009.md` 5.2 (corrections C-FT-1, C-FT-2, C-FT-5, C-FT-6; C-FT-3 / C-FT-4 went to `GAME_MODES_2009.md`), the reports
`reports/{dserver-light, client-netbye}.md`, the rows DSV-39 (addition, Q9) and DSV-41 of `DC_DEFAULTS.md` (Wave U section, PENDING; staged first in `staging/dserver-light_DC_DEFAULTS.md`). No capture
was re-read; no new RVA was introduced (every RVA comes from FIRETEAM_2009 or the existing text). Grades as in the header plus **D** (README). No existing grade was lowered; no REFERENCE
value became a rule.

| place | change | grade |
|---|---|---|
| 2.1.2 co-op block | C-FT-6: `stage` 2 / 3-4 / 5 select the complete / failed / timed-out banner; the panel field targets of the six values | PROVEN (banner order), DISASM (field targets); labels UNPROVEN (V-FT-11) |
| 3.5 row 0xd7 sub 2 | C-FT-5: re-arm only in own state 2, 3, 5, 6 with `a == b`; CRes 0x5a0 shows `b - 1`; 0x6b `m1` writes the same field; the count must precede 0x67 state 2 | PROVEN, DISASM |
| 4.3.1 | Q9 bullet: the reference stack's 0x66 kind 1 after a throw and the DeCombat form (dserver-light) | REFERENCE, ACCEPTED; D (proposed), TESTED |
| 4.4.1 | DeCombat client's handling of 0x7f (client-netbye): command table rule, `auto` draw; the real client's spawn select UNPROVEN as a reaction | PROVEN (code), EVIDENCE (table), D TESTED, UNPROVEN (capture) |
| 8.2 DeCombat paragraph | status note: count and optional flight stream done (DSV-41) | D |
| 8.2.1 tail row and the paragraph under the table | C-FT-2: tail field names and uses; pointer to the NPC branch | PROVEN (order, targets), DISASM (widths) |
| 8.3 type 0x18 table and the paragraph under it | C-FT-1: 0x1d / 0x22 act, 0x1e / 0x1f / 0x25 roster-only, 0x24 tail-only health percent; NPC note on 0x11 / 0x24 | PROVEN, DISASM |

Count: corrections folded 4 (C-FT-1, -2, -5, -6), REFERENCE bullets 1, D paragraphs 2, status notes 1.

## Appendix I: fold record (spec-fold-8, 2026-10-25)

Sources: `specs/UDP_SESSION_2009_B_addendum_round2.md` G1 / G4 (now a stub), `CAPTURE_tdm6_VALIDATION.md` sections 2-9 and 11, `CAPTURE_rust3_RUSTSTACK_VALIDATION.md` sections 1, 2,
4, 5, 9 and 11, `reports/{aa-triage-start-state, aa-fix-start-state, aa-triage-grenades, aa-fix-grenades, aa-triage-red-dot, damage-calibrate, dserver-round2}.md`
and the PENDING rows they staged in `DC_DEFAULTS.md` (Wave Z, Wave AA and dserver-fixes sections). No new capture was decoded by this fold; every seq / t is the source's.

| place | change | grade |
|---|---|---|
| 2.1 row 0x45, 2.1.1 steps 4 / 7, 2.1.2, 2.1.3, 7.1 row 0xd4, 11.2 item 8 | the second head flag is `waiting_banner` everywhere (formerly `first_round`; aa-start-state had renamed step 3 and section 10 only) | DISASM (unchanged) |
| 2.1.1 step 5 | intro field sources (G4); rust3 without intro; DSV-50 | REFERENCE; OURS; D |
| 3.3.2 | tdm6 kill feed: `streak_flag` only on the match's first kill, `hit_zone` = node location; DeCombat sentence corrected (DSV-33) | REFERENCE; D |
| 4.1.1 | the real client's deaths and respawn requests (tdm6), superseding "not observed with the real client" | PROVEN-CAPTURE (upgrade from DISASM + bot) |
| 4.3.1 | tdm6 0x74 forms (`dmg_kind` by location, unclamped `damage`, `from_dir`); DSV-30 replacement status; 0x66 kind 1 count amended (aa-grenades) | REFERENCE; D; PROVEN-CAPTURE (difference) |
| 4.4.1 | backpack slot 4 with `class_pos` 1 and the 0x7f set (tdm6); DSV-38 amendment | REFERENCE + PROVEN-CAPTURE; D |
| 8.2 rows 0x26, 0x39 | `OBJ_PLAYER_CORPSE`, `OBJ_DOOR` (G1); names registered under section 12 ("`OBJ_` names of 8.2") | REFERENCE, DISASM; meanings UNPROVEN |
| 8.2 shot-effect notes | `a` = ROM surface id (red-dot); the relay stops after the granted rounds (tdm6) | DISASM + PROVEN-CAPTURE (difference); REFERENCE |
| 8.2 DeCombat thrown objects, 11.4 V-PROJ-1 | stream on by default; first observation for V-PROJ-1 | D; OBSERVED |

No grade was lowered; every superseded sentence is quoted where it was replaced.
