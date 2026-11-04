# GAME_MODES_2009: what the 2009 client expects from the server in every game mode (clean-room)

- **Title:** GAME_MODES_2009, the per-mode contract between a DeCombat game server (`dc-dserver`) and the 2009
  EU-test client: for every row of the ROM game-room mode table, the client's mode class, team model, what the
  client enforces or only displays, the server -> client messages beyond the TDM list of
  `GAME_MESSAGES_2009_S2C.md` §10, the client -> server messages the mode adds, the objective objects of the object
  stream, the HUD, the respawn model, round end and spectating.
- **Builds on:** `specs/GAME_MESSAGES_2009.md` (cited `GM §n`), `GAME_MESSAGES_2009_S2C.md` (`S2C §n`),
  `GAME_MESSAGES_2009_C2S.md` (`C2S §n`), `HISEC_2009.md`, `MOVEMENT_2009.md`, `UDP_SESSION_2009_A/_B.md`. Message
  names (`GS_*`, `UP_*`, `OBJ_*`) and client object names (`match_ui`, `roster`, `local_player`, `mode_rules`,
  `hud_*`, `router`, `session_info`) are those of S2C §1.2 and §12.
- **Evidence:** clean decompiler tree `CShell.dll/<rva>.c` (CShell image base 0x37100000, so
  `FUN_3715a910` is cshell+0x5a910), read-only capstone disassembly of the user's `Game/CShell.dll` and PE-DATA reads
  of its method tables (scratch scripts outside the tree, tag **DISASM**), and a scratch program over `dc-bute`
  (`GameRoomData::modes()`, `maps()`) that printed only numbers, booleans and string hashes of the user's ROM mode
  table. No ROM string is quoted. Strings that the client code uses are described, not quoted. Captures of the real client exist
  for TDM only (tdm1, tdm2, tdm4, Appendix C); apart from those, nothing here is OBSERVED except the ROM table numbers of section 2 (OBSERVED: ROM table key `GameType`,
  `EnablePlay`, `DefaultGoal`, `GoalMin`, `GoalMax`, `GoalInc`, `MaxPlayers`, `MinPlayers`, `DefaultMapID`, read
  2026-10-24).
- **Status:** DRAFT 1. TDM (class 1) is checked against three captures of the real client against the reference server (tdm1, tdm2, tdm4, 2026-10-25; Appendix C; grades PROVEN-CAPTURE = the real client's own bytes or reaction,
  REFERENCE-CONFIRMED = a reference server form the client accepted, REFERENCE = what the reference server did and never a rule); every other mode is not capture-validated. **Author agent:** game-modes-spec (Claude, clean-room spec writer).
  **Date:** 2026-10-24.

Grades as in the sibling specs: **PROVEN** (visible in the decompiler output, widths checked where they matter),
**DISASM** (visible only in the disassembly or PE-DATA, or the decompiler output argument lists were garbled and the reading
comes from the disassembly), **PLAUSIBLE** (one step reconstructed, or a meaning read off the effects),
**UNPROVEN** (not visible; capture needed). Every rule value that the client does not show is a DeCombat default:
proposals GMD-1..n are staged in `staging/game-modes-spec_DC_DEFAULTS.md` and cited
`GMD-n (proposed)`.

## 0. Contents

1. The mode class: where the client gets it and how it uses it
2. The ROM mode table (numbers only) and our mode names
3. Index of the client's mode-class switches
4. Shared machinery: round flow, respawn arming, mode zones, pickups, hold-to-use, side switch
5. Per-mode chapters (5.1 TDM ... 5.10 class 10)
6. Fireteam: the co-op AI mode (what the client renders and expects from AI objects)
7. Movement: mode-specific rules
8. Implementation order for `dc-dserver`
9. Capture items V-GM-n
10. Corrections to sibling specs
11. Open questions

## 1. The mode class

### 1.1 Source of the number

`mode_class` is the 32-bit value at `client_shell`+0x16938 (global VA 0x37597e50; GM §1.2, S2C §1.1).

| source | what | grade |
|---|---|---|
| ROM row | the game-room attribute loader cshell+0x265780 reads key `GameType` of every `[Mode<n>]` and `[ClanMode<n>]` section into field +0x104 of the mode row (two lists, plain at table +0x15e0, clan at +0x15f0) | PROVEN cshell+0x265780 (lines reading `GameType` into +0x104) |
| room entry | cshell+0x1630a0 picks the list by the clan flag (`session_info`+0x12c != 0 selects the clan list) and stores `row[room mode index] +0x104` into +0x16938 through cshell+0x264050 | PROVEN cshell+0x1630a0, cshell+0x264050 |
| wire | `GS_GAME_ENDPOINT` 0x9e `[mode_class:u8]...` overwrites it (S2C §5.1) | DISASM (S2C) |
| offline paths | cshell+0x118d70 sets 1 or 2 (a local/training start path; not used by a network server) | PROVEN |

The server must therefore know the same value: `dc-dserver` takes the ROM row's `GameType` (the lobby passes it in
the room line, DC_DEFAULTS Wave F note), sends it first in 0x9e, and never hard-codes it (ROM model). PROVEN by the
layout dependencies listed in section 3.

### 1.2 What depends on it

Thirty-two clean functions use +0x16938 directly (two of them, cshell+0x118d70 and cshell+0x1630a0, write it) and
twenty-three more use the integer index form (`[0x5a4e]` of the shell, the same field; cshell+0x41820 writes 1 there
at initialisation): HUD elements, the kill counter rules, the respawn request, the observer
camera, the result screen, the map and radar layers, the mode-zone tests, and four message layouts (0x7d, 0x45,
0x46, 0x65). Section 3 lists every site. PROVEN (search of the clean tree for both forms).

## 2. The ROM mode table and our mode names

OBSERVED (scratch program over `dc-bute` against the user's ROM, 2026-10-24; values are numbers from the ROM and
are read at run time by DeCombat, never compiled in). The ROM has 9 plain rows (`[Mode0]`..`[Mode8]`) and 9 clan
rows (`[ClanMode0]`..`[ClanMode8]`), and 19 map sections. "maps" counts the map sections that carry the row's
`GMS_<Key>` key (the room's map list for that mode, `dc_bute::GameRoomData::map_mode_masks`).

| row | `GameType` = mode class | `EnablePlay` | maps | team model (section 5) | goal kind (what `DefaultGoal` counts, PLAUSIBLE from the client's use) | our name |
|---|---|---|---|---|---|---|
| Mode0 | 1 | yes | 17 | two teams | team kills | `TDM` |
| Mode1 | 2 | yes | 6 | two teams, attack (team 1) / defend (team 2) | rounds | `BOMB` |
| Mode2 | 5 | yes | 2 | one human team vs AI | mission / difficulty (0..2) | `COOP` (Fireteam) |
| Mode3 | 6 | yes | 14 | free for all | player kills | `FFA` |
| Mode4 | 7 | yes | 14 | two teams | flag captures | `FLAG` |
| Mode5 | 9 | yes | 7 | hunters vs one marked player | points | `BOUNTY` |
| Mode6 | 10 | **no** | 2 | two teams | kills (UNPROVEN) | `CLASS10` |
| Mode7 | 11 | yes | 5 | humans (team 1) vs infected (team 2) | rounds | `INFECT` |
| Mode8 | 12 | yes | 17 | two teams, one life per round | rounds | `ELIM` |
| ClanMode0 | 4 | yes | 17 | two clans | team kills | `TDM_CLAN` |
| ClanMode1 | 3 | yes | 6 | two clans, attack / defend | rounds | `BOMB_CLAN` |
| ClanMode2, 3, 5, 6 | -1 | no | 0 | (disabled rows) | | none |
| ClanMode4 | 8 | yes | 14 | two clans | flag captures | `FLAG_CLAN` |
| ClanMode7 | 11 | no | 0 | (disabled) | | none |
| ClanMode8 | 13 | yes | 17 | two clans, one life per round | rounds | `ELIM_CLAN` |

Further ROM keys per row that a server reads at run time (values not reproduced): `DefaultGoal`, `GoalMin`,
`GoalMax`, `GoalInc` (room goal range), `MaxPlayers`, `MinPlayers`, `PlayersInc`, `RecommendMaxPlayers`,
`DefaultMapID`, `SortingOrder`, and four room-option switches `EnableWeaponUseSetting`, `EnableJoinCombatSetting`
(join a running match), `EnableTeamKillSetting`, `EnableViewEnemyCameraSetting` (all PROVEN as keys of the loader
cshell+0x265780; their values per row are OBSERVED, and two rows have the team-kill switch off: the `FFA` and
`INFECT` rows, and the `COOP` row has all four off).

Mode classes 1..13 are therefore all in use; class 0 means "no mode" (0x9e with 0 is not consumed, S2C §5.1).
The names in the last column are ours. The goal kind is graded per chapter.

## 3. Index of the client's mode-class switches

Every client site that branches on `mode_class` (PROVEN by the search of section 1.2; rva = function start).
"pairs" lists the classes the code treats alike.

| cshell+ | what it does | classes |
|---|---|---|
| 0x5a910 | `mode_rules` slot +0x0c: offline start-world choice | 1 / 4, 2 / 3, 5, other (not network relevant) |
| 0x22d2b0, 0x22ef50 | default "special" weapon class chosen when switching to the mode item: class 9 for 2 / 3, class 8 for 1, 4, 6, 7, 8, 10 | 2 / 3 vs 1, 4, 6, 7, 8, 10 |
| 0x61e40 | respawn request after the 7000 ms wait: 0x89 for 1, 4, 5, 6, 9; 0xc5 for 7, 8; 0xd2 for 10 (C2S §3.4); none for 2, 3, 11, 12, 13 | as stated |
| 0x2aad0, 0x2ab70, 0xc5960, 0xc5ae0 | roster kill / death and team-score counters skip the event when class 9 and `mode_rules`+0xb4 = 1 | 9 |
| 0x2c430 (0x6d case) | FFA scoreboard note when class 6; co-op note when class 5 | 5, 6 |
| 0x2c430 (0x7d case) | flag-carrier list read only for 7 / 8; `hud_team_score`+0x12c := class | 7 / 8 |
| 0x2c430 (0x65 case) | co-op block when class 5 | 5 |
| 0x49a40 (0x44, 0x45, 0x46) | measure flag for 5; caption for 5, 6, 9, 11; base markers unless 6; co-op data for 5; round-begin reset for team 1 in 2 / 3 | 2 / 3, 5, 6, 9, 11 |
| 0x60010 | observer target search without team filter for 6 and 9 | 6, 9 |
| 0xa9800, 0xa9e60 | spectator panel: single list for 6 and 9 | 6, 9 |
| 0x134b80, 0x137720, 0x137dd0, 0x132b80, 0x1326b0, 0x130220 | result screen: single ranking list for 6, 9, 11; team result block otherwise; co-op summary for 5; flag columns for 7 / 8 | 5, 6, 9, 11, 7 / 8 |
| 0xc1a50 | round-win banner: infection sounds and pictures for 11 (team 1 vs team 2 win); team-win sounds for every class except 5, 6, 9 | 5, 6, 9, 11 |
| 0xcd210 | half-time "teams change sides" picture: 3 (at exactly half the rounds), 4 (half the goal), 8 (once), 13 (half the rounds) | 3, 4, 8, 13 |
| 0xbe7c0, 0xbe0c0, 0xbe570 | round-timer variants: flag panels for 7 / 8, infection panel for 11 | 7 / 8, 11 |
| 0xc05a0, 0xc0ac0, 0xc5590, 0xc53e0 | team score panel: points layout for 11, bounty layout for 9, flag layout for 7 / 8 | 7 / 8, 9, 11 |
| 0xc6930, 0xc8160, 0xc81f0, 0xae640, 0xae6d0, 0xb8990, 0xb8f40 | overview / spectator map and radar: flag pickups (7 / 8), bounty pickups (9), co-op map background (5) | 5, 7 / 8, 9 |
| 0xb27b0, 0x28dd40 | infection HUD per-frame update (countdown, sounds) | 11 |
| 0x82860 | carried-item notices: mode item 0x1cc for team 1 and 0x21f for team 2 in 2 / 3; item 0x1c6 notice except for team 2 in 11 and bounty phase rules in 9 | 2 / 3, 9, 11 |
| 0x5d160, 0x5d530, 0x5d610, 0x5d880 | mode-zone tests (section 4.3): class 3 is mapped to 2 | 2 / 3 |
| 0x5dad0 | flag capture prompt (section 5.5): class 8 mapped to 7 | 7 / 8 |
| 0x2e1010 | mode-zone object init: map / radar icon only when the zone's class byte equals the current class | all |
| 0x86f10 | flag HUD element update returns at once unless 7 / 8 | 7 / 8 |
| 0x8c420 | notice of item 0x1c6 suppressed in class 9 outside bounty phase 1 | 9 |
| 0x8a420, 0x96b30, 0x9d4a0, 0x7caf0, 0xcbea0, 0x81930 | co-op variants of marker, health, kill feed, co-op score and notice elements | 5 |
| 0x2fcd0 | copies the class (as a byte) into the connection object +0x1201 | all |

## 4. Shared machinery

### 4.1 Round flow messages (all modes)

The round flow ids of S2C §2.1 are mode-independent; what changes per mode is which ones a server sends and the
conditional blocks.

| step | message | single-round modes (1, 4, 6, 9, 10; 5 per mission) | round modes (2, 3, 7, 8, 11, 12, 13) | grade |
|---|---|---|---|---|
| match start | 0x45 `GS_MATCH_BEGIN` `[round_secs][show_intro][waiting_banner][countdown_secs]` + intro + `[round_no]` | once, `waiting_banner` (formerly `first_round`) 0 | once at match start with `waiting_banner` 0 | PROVEN (layout, S2C 2.1.1); the flag 1 draws the "waiting for players" banner (S2C 2.1.1 step 3) |
| round start | 0x44 `GS_ROUND_BEGIN` `[round_secs][round_no][round_limit][keep_team]` | not needed (PLAUSIBLE) | once per round | PROVEN (layout); the per-round use is PLAUSIBLE (it resets the timer, the kill feed, the bounty and infection HUD and stores `round_no` / `round_limit`, which the half-time picture compares) |
| round result | 0x46 `GS_ROUND_RESULT` `[round_secs][win_team:i8][wins_team1][wins_team2]` (+ co-op block for 5) | once at the end | once per round | PROVEN (layout) |
| round close | 0x48 `GS_ROUND_CLOSE` `[round_secs]` | optional | between rounds: shows the between-round banner timer (`hud_banner` timed state) | PLAUSIBLE |
| objectives off | 0x47 `GS_ROUND_OBJECTS_END` | optional | after each round: clears the objective layers of radar and maps | PLAUSIBLE |
| team totals | 0x4a `GS_TEAM_SCORE` (kill totals) or 0xb5 / 0xc4 `GS_ROUND_WINS` (round wins on the scoreboard) | 0x4a | 0xb5 after each round (PLAUSIBLE) | PROVEN (layouts) |
| match end | 0x65 per player, 0x3f (0x99 in the first draft) | observed (REFERENCE): 0x46, +3 s 0x47 / 0x6b refresh / 0x65 / 0x48, +3 s 0x3f, no 0x99; DeCombat sends that set (DSV-35, D) | not observed | PLAUSIBLE order for the 2009 server (V-S2C-3); REFERENCE for the reference server |

Observed in TDM (class 1; REFERENCE, accepted by the real client; details in S2C 2.1.3, 3.3.2, 4.1.1): match start is 0x43, 0x44 `[0][1][1][0]`, 0xb5 `[0][0]`, then 0x45 with `waiting_banner` (formerly `first_round`) **0** and `round_no` 0 (the reference stack; S2C §10 step 11
now sends the flag 0 as well, since a 1 draws the waiting banner for the whole round, S2C 2.1.1 step 3; for `round_no` it allows 0 or 1, which no client reaction separates); a player who joins a running round gets 0x43, 0x44 and 0xb5 but no 0x45; team kill totals travel in 0xb5 `[kills team 1][kills team 2]` after each 0x6d and 0x4a was never sent; the TDM room had no
time limit (`round_secs` 0), so the goal end (0x46 with the deciding kill, +3 s 0x47 / 0x6b / 0x65 / 0x48, +3 s 0x3f) and the forfeit end (0x6c + 0x46 0.3 s after the leaver's 0xbd, then the same) were observed, a time-limit end was not;
no 0x41, no 0x99, no server bye. The 2009 server's own order and delays are unknown.
DeCombat (D, proposed; `dc-dserver` slice 4, TESTED, RUN with two bots): DSV-35 sends the observed end set with its 3 s pauses and never 0x41 / 0x99 / a server bye (the session
ends when the last client left, or 120 s after the result screen); DSV-31 ends the round for the other team when a team has had no player for **10 s** (the reference stack: 0.3 s; the delay is
ours); DSV-36 sends 0x45 only to the round's first spawn and gives a joiner of a running round 0x43 / 0x44 (remaining seconds) / 0xb5. These cover the single-round classes; the round
classes (2, 3, 7, 8, 11, 12, 13) have no DeCombat round loop yet (section 8).
RUN record (stack-verify, release Rust stack of master 692de66, two bots, loopback; S2C 2.1.3 has the table): a team-empty end 10.001 s after the leave with 0x46, +3.014 s
0x47 / 0x6b / 0x65 / 0x48 and +2.986 s 0x3f; a goal end (goal 3) with 0x46 in the millisecond of the deciding kill, +2.988 s and +3.007 s; the 0x65 summaries carried the lobby's
gains (GAIN-1); no server bye. Our stack only; the real client's display of the end set is still in no capture (V-S2C-3).

Layout traps that depend on `mode_class` (PROVEN, S2C):

- 0x7d `GS_MATCH_SETTINGS`: `if mode_class is 7 or 8: [n:u32] n x [flag_team:u32][carrier:u32][carrier_team:u32]`
  (section 5.5 gives the meaning of the triples).
- 0x45 intro block: `mode_caption` only for 5, 6, 9, 11; team bases only when not 6; `[x:u8][y:u8][z:bool1]` only
  for 5.
- 0x46: co-op block only for 5.
- 0x65: co-op block only for 5.

`goal` (0x7d) is drawn on the scoreboard header as a number for classes 1 and 4 and as a round limit for the round
classes 2, 3, 12, 13 (S2C §3.2, cshell+0x916a0 / cshell+0x91310). PLAUSIBLE: the server sends the room goal there
(the ROM `DefaultGoal` range keys bound it in the lobby).

What the client **enforces** in any mode: nothing about the goal or the clock. The round timer (`hud_round_timer`)
counts down from the seconds the server sent and is only a display; no client code ends a round or a match on
its own (PROVEN negative: the timer element has no sender; the round flow ids are all server -> client). The goal
and the time limit are server rules (GMD-1, GMD-2 (proposed)).

### 4.2 Respawn arming and the three respawn requests

The client arms its own respawn wait after a death and sends a respawn request when it is over. **The wait is armed by the client itself** when it processes server 0x67 state 2 (DISASM cshell+0x60ac0, called every frame from cshell+0x61e40
while the death time is set; C2S §3.3, corrected 2026-10-25): **7000 ms** in classes 1, 4, 6, 7 and 8, 7000 ms in class 5 only when the own roster record's +0x1f8 (written by 0xd7 sub 2 `b` and by 0x6b field 21 `m1`) is above 0 at that moment, 7000 ms in class 9 only when `mode_rules`+0xb4 is 0,
**1500 ms in class 10**, and no wait in classes 2, 3, 11, 12 and 13. Server 0xd7 sub 2 (with `a == b`, and only while the own player state is 2, 3, 5 or 6: PROVEN cshell+0x5df50) and 0xdb sub 6 only re-arm it. (The first draft said these two were the only arming paths; that was wrong.) Which request it sends
depends on the class (cshell+0x61e40, PROVEN); the request goes out when the elapsed time is strictly greater than the wait:

| classes | request after the armed wait | meaning (PLAUSIBLE) |
|---|---|---|
| 1, 4, 5, 6, 9 | `UP_RESPAWN_REQUEST` 0x89 | free respawn |
| 7, 8 | `UP_RESPAWN_REQUEST_B` 0xc5 | respawn in the flag mode (a separate id lets the server apply flag rules, for example a base spawn) |
| 10 | `UP_RESPAWN_REQUEST_C` 0xd2 `[0]`, only while the own team has a living member (cshell+0x293d0) | team-life respawn |
| 2, 3, 11, 12, 13 | none | no respawn within a round: the dead player stays an observer until the next 0x44 / spawn |

A server needs **no** arming message in classes 1, 4, 6, 7, 8 (and 10): it answers the request. The reference stack respawns a player only on request (37 of 37 bot requests answered 30-80 ms later with the spawn set; a bot that never
asked stayed dead for 104 s; no 0xd7, 0xdb or 0xd4 was ever sent: S2C 4.1.1, REFERENCE).

**GMD-3'** (rewritten 2026-10-25 by spec-fold-5 as `DC_DEFAULTS.md` "Capture notes 2026-10-25" demand; D, staged, not approved): the server sends **no arming message** in classes 1, 4, 6, 7,
8 and 10 (the client arms itself on 0x67 state 2); in class 5 it sends 0xd7 sub 2 as before, because the client arms only when the stored value is above 0; in class 9 the client arms
only while `mode_rules`+0xb4 is 0. The respawn follows the client's request (DSV-32): a request no earlier than the class wait minus 250 ms after the death is answered at once with the
spawn set; earlier requests, requests from living players and requests after the round end are ignored; a server timer respawns 10 s after the death only in classes 5 and 9 (where the
client may not ask); round classes 2, 3, 11, 12, 13 have no respawn inside a round. *Superseded wording of GMD-3:* "in the respawn classes (1, 4, 5, 6, 7, 8, 9) the server arms the
client's 7000 ms wait with 0xd7 sub 2 (`a == b`) right after the death; it respawns on the request (0x89 / 0xc5) or 2000 ms after the wait if no request arrives" (it rested on the
first-draft reading of C2S 3.3 that 0xd7 sub 2 / 0xdb sub 6 are the only arming paths). DC-KILL-1's 500 ms guard and 10,000 ms fallback for every class are refined by DSV-32.
Implemented in `dc-dserver` slice 4 (TESTED; RUN: 7 of 7 bot deaths answered on the bot's own 0x89 at 7048-7059 ms). No capture contains a death of the real client, so the wait length of
each class is DISASM, not PROVEN-CAPTURE (V-GM-10).

**Class 5 lives** (C-FT-5, folded by spec-fold-7 from `FIRETEAM_2009.md` 2.1-2.3; DISASM cshell+0x60ac0..0x60bcc, PROVEN cshell+0x5df50, cshell+0x2a950): the death branch takes the own
roster record when it processes 0x67 state 2; no record or +0x1f8 = 0 (unsigned) means **no wait and no request ever**. So the count must reach the client **before** the death message:
0x6b `m1` at the join (no 0xd7 needed for the first life), then on each death 0xd7 sub 2 `[n-1][n-1]` before 0x67 state 2 (`n` = the lives counting the current one). With 0 the player
stays an observer for the round; a later sub 2 `[k][k]` with `k` > 0 while he is dead re-arms the wait (a life given back). The client shows `b - 1` with CRes string id 0x5a0 (a
respawns-remaining text) and hides it when negative (cshell+0xcbea0). DeCombat: FT-L1 (proposed, FIRETEAM_2009 4.3). The GMD-3' wording "in class 5 it sends 0xd7 sub 2 as before" is to be read
in this order (before the death message, not after it); `docs/FIRETEAM_AI_DESIGN.md` 2.3, which sent it after the kill, is marked superseded.

Death and observer camera: the dead player gets 0x67 state 2 (which arms the client's wait) and state 3 (S2C §4.1; the reference stack
sends 3 then 2 in one block, DeCombat likewise by default, DSV-32; the older "state 3 2000 ms later" is `-deathstates spec`). In the round classes the server
keeps him in state 3 (or 6, spectator) until the next round's spawn. The observer target search ignores teams in
classes 6 and 9 (cshell+0x60010) and otherwise allows the enemy team only with the user's "enemy dead camera"
option (C2S §3.5; the room switch `EnableViewEnemyCameraSetting` is the lobby side of the same option, PLAUSIBLE).

0xd4 `GS_RESPAWN_TIMER` kind 1 with `secs` starts the three-part `hud_strike` countdown only while the
`waiting_banner` flag (formerly `first_round`) of the last 0x45 is clear (S2C §7.1, PROVEN): a round-mode server that sends `waiting_banner` = 0
in later 0x45 messages can show a pre-round countdown with it (PLAUSIBLE use).

### 4.3 Mode zones: object type 0x37

Objective areas (bomb sites, plant and defuse points, flag capture zones, base zones, use targets) are **client
objects of type 0x37** created from the object stream. The evidence chain:

1. The router (S2C §8.1) keeps one list per type with 16-byte headers from +0x04; the list that the zone tests walk
   is at `router`+0x374 with count +0x380, which is header 0x37 (0x04 + 0x37 x 0x10 = 0x374). PROVEN by arithmetic
   on the code of cshell+0x5d160, cshell+0x5d530, cshell+0x5d610, cshell+0x5d880, cshell+0x5dad0.
2. The factory cshell+0x222480 case 0x37 calls the allocator cshell+0x221610, which constructs the object with
   cshell+0x2e12d0 (method table VA 0x374fafcc, PE-DATA). PROVEN.
3. Type 0x37 is a class-read type of the effect dispatcher (cshell+0x217760: `create(type, 0, reader, obj)`, S2C
   §8.2), so the layout below follows the `[type:u8 = 0x37]` byte of a shell object blob. PROVEN.
4. The object's init (slot +0x10 = cshell+0x2e1010, PE-DATA) stores the engine object and calls the read method
   of its embedded record at object +0x68 (record method table VA 0x374fafbc, slot +0x08 = cshell+0x2b67a0).
   PROVEN; widths DISASM cshell+0x2b67b5..0x2b6896.

Layout (after `[type:u8 = 0x37]`; object offsets = record offset + 0x68):

| # | field (our name) | width | object off | use | grade |
|---|---|---|---|---|---|
| 1 | `z_a` | u32 | +0x70 | not traced | DISASM (width) |
| 2 | `z_b` | u32 | +0x74 | not traced | DISASM |
| 3 | `z_flag` | bool1 | +0x78 | not traced | DISASM |
| 4 | `z_pos` | vec72 (reader slot +0x58, GM §1.3) | +0x7c..+0x84 | position of the flat ground marker the init creates (a ROM square model and texture) | DISASM |
| 5 | `z_icon` | u8 | +0x88 | 0xff = none; else an icon index looked up by cshell+0x2e5050 | PROVEN |
| 6 | `z_team` | u8 | +0x94 | owning team: 1, 2; 0xfe = any team; 0xff = none (zone ignored) | PROVEN |
| 7 | `z_class` | u8 | +0x95 | the mode class the zone belongs to; the tests compare it with the current class, mapping 3 -> 2 and 8 -> 7 | PROVEN |
| 8 | `z_c` | u8 | +0x96 | not traced | DISASM |
| 9 | `z_value` | u32 | +0x98 | copied to `local_player`+0x444 by the enemy-zone test (PLAUSIBLY a site number) | PROVEN |
| 10 | `z_kind` | u32 | +0x9c | zone kind (table below) | PROVEN |
| 11 | `z_d` | u32 | +0xa0 | not traced | DISASM |
| 12 | `z_sub` | u8 | +0xa4 | sub-kind of kind 5: 1 = plant point, 2 = defuse point (section 5.2); non-zero also gives the zone a map icon (map kind 7 for 1, 6 otherwise) and a radar icon (10 for 1, 9 otherwise) | PROVEN cshell+0x2e1010 |
| 13 | `z_e` | u32 | +0xa8 | not traced | DISASM |
| 14 | `z_text` | zstr8(260) | +0xac | notice text the zone tests show (cshell+0xbda50 with it) | PROVEN |
| 15 | `z_text2` | zstr8(260) | +0x1b0 | not traced | DISASM |

The object also has `active` (+0x2c4, set by its update; zones are skipped while it is 0) and `marked` (+0x2c5,
set by the init when `z_sub` != 0). PROVEN.

Zone kinds the client tests (PROVEN unless noted):

| `z_kind` | test | client effect |
|---|---|---|
| any | cshell+0x5d160: active, `z_team` is the own team or 0xfe, `z_class` = current class | `local_player`+0x43c := 1 (PLAUSIBLE "inside own base") |
| 4 | cshell+0x5d530: active, `z_team` is **not** the own team (or 0xfe), class matches, and the bomb is not planted (`match_ui`+0x9a87 = 0, set by 0x51) | `local_player`+0x1c7 := 1 ("inside an enemy objective area"), +0x444 := `z_value` |
| 5 | cshell+0x5d610: active, marked, class matches | the zone becomes the use target (`local_player`+0x1e8); `z_sub` 1 sets +0x1c9 (may start a plant), `z_sub` 2 sets +0x1c8 (may start a defuse); when the player may not use it (two `local_player` checks, slots +0x6c and +0x64) a notice is shown instead |
| 6 | cshell+0x5dad0 (classes 7 / 8 only): active, `z_team` is the own team (or 0xfe) | flag capture prompt (section 5.5) |
| 7 | cshell+0x2e1010 init | radar icon kind 0xb and map icon kind 8 at the zone; no test (PLAUSIBLE: a visible marker only) |
| 8 | cshell+0x5e050 (C2S §7.1): active, `z_team` own or 0xfe | the target of `UP_USE` 0x59 |

Observed in the TDM world (REFERENCE-CONFIRMED, tdm1; the client accepted them): the reference server created **five type-0x37 zones** at load (blobs of 409 or 1081 bits, every one parses exactly with the layout above): `z_kind` 4, 4, 6, 6, 8,
`z_team` 2, 2, 1, 2, 2, `z_class` 1, 1, 1, 1, 9 (so one zone belonged to class 9, not 1), `z_a` and `z_b` read as floats -1.0, one `z_d` = 15.0 read as a float (so `z_a`, `z_b`, `z_d` are PLAUSIBLY f32). It thus attaches mode objects of other
classes to the world's objects in TDM too; the kind-8 zone (the target of `UP_USE`) belonged to team 2, so the use test (own team or 0xfe) failed for the team-1 player and no 0x59 / 0xd8 / 0xad / 0xc5 / 0xd2 was sent: the negative is
PROVEN-CAPTURE. Where the 2009 server took them from stays open (V-GM-2).

A server therefore creates one type-0x37 object per objective area of the map's mode data, with the zone's
`z_class` set to the class it serves. Where the 2009 server took these areas from (the world's objects or the
room's GMS data) is UNPROVEN (V-GM-2); DeCombat reads them from the ROM world / GMS data at run time (GMD-4
(proposed)).

DeCombat zones from the world file (D, DSV-37 proposed; `dc-dserver` slice 4): the world file's objective objects become type-0x37 zones on kind-0 objects in every mode (`z_kind` 4 for a
bomb site, 6 for a flag capture zone; `z_team` from the object's team text through the ROM team names, else 0xfe; `z_value` the zone number; `z_text` the object name; `z_a` / `z_b` -1.0 as
observed). `z_class` is the class the zone serves (2 bomb, 7 flag; `-zoneclass own`, default) or the room's class (`-zoneclass room`, the reference stack's form); because the zone tests compare
`z_class` with the current class (mapping 3 -> 2 and 8 -> 7), with `own` a bomb or flag zone in a TDM room is inert for the client's zone tests, while with `room`
its tests run in TDM as they did with the reference stack (REFERENCE-CONFIRMED; the client effects of 4.3 in TDM were not observed). ROM-gated test (TESTED): with `-zoneclass room` the four kind-4 / kind-6
zones of the tdm2 world equal the reference stack's blobs bit for bit; the reference stack's fifth zone (kind 8, the use target, class 9) is not created by DeCombat. Details in
`UDP_SESSION_2009_B.md` 3.5.2.

### 4.4 Pickups: object type 0x17 kinds

`OBJ_PICKUP` (type 0x17, inline create record, S2C §8.2; object method table VA 0x374f086c; HISEC §3.1). The object
copies record +0x30 (`pickup_kind`) to +0x80, +0x34 to +0x84, +0x38 to +0x88 and +0x3c to +0x8c (DISASM
cshell+0x2824d2..0x2824f0). Accessors: slot +0x44 returns +0x80 (kind), slot +0x40 returns +0x84, slot +0x3c returns
+0x88, slot +0x48 returns +0x94 (HiSec key); slot +0x4c(key, value, flag) sets +0x84 := value and +0x8c := flag when
+0x88 = key (DISASM cshell+0x21a460..0x21a490, cshell+0x282380).

| `pickup_kind` | extra fields | mode | client use | grade |
|---|---|---|---|---|
| 2 | `[state:u32][team:u32][at_base:bool1]` (+0x84, +0x88, +0x8c) | flag classes 7 / 8 | the team's flag; radar icon kind 3 and map icons for classes 7 / 8 (cshell+0xb8f40, cshell+0xc81f0, cshell+0xae6d0); state changes through 0xc6 (section 5.5) | DISASM (layout), PROVEN (uses) |
| 3 | `[flag:bool1]` (+0x8c) | bounty class 9 | radar icon kind 4 and map icons for class 9 (cshell+0xb8990, cshell+0xc8160, cshell+0xae640) | PROVEN (uses); meaning PLAUSIBLE (a bounty item) |
| 5 | `[case_key:u32]` | any | HiSec case (HISEC §3) | DISASM |
| 6 | `[item:u16][b:u16]` | any | weapon / item pickup with its model | DISASM |

Observed in the TDM world (REFERENCE-CONFIRMED, tdm1): two type-0x17 objects with `pickup_kind` 2 (the flags, 116 bits each, exact parse with the kind-2 layout above) were created at load although TDM does not use them, and 60 kind-9 objects
carried type 0x0b blobs; the client accepted all of them. HiSec cases (kind 5) appeared when a player died (S2C 3.3.2).
DeCombat (D, DSV-37): the world file's capture flags are created as kind-1 model objects with a type-0x17 kind-2 blob `[state = team][team][at_base 0]`, model = the object's model file,
textures and styles from the ROM attachment rows of the flag look; the two flags of the tdm2 world equal the reference stack's blobs and file names (TESTED, ROM-gated). Not created: kind-9
volumes with type 0x0b, pickups of kinds 3 / 5 / 6 at load, doors (`UDP_SESSION_2009_B.md` 3.5.2).

### 4.5 Hold-to-use (plant, defuse) and the use progress

PROVEN control flow, PLAUSIBLE meanings:

1. The zone test (4.3, kind 5) sets the use target and +0x1c9 / +0x1c8.
2. Holding the use command sends `UP_USE_HOLD` 0xd8 phase 1 `[1][target]` (C2S §7.1).
3. The server answers with **S2C 0xad** `GS_TRACK_OBJECT`, which starts the client's use-progress object
   (cshell+0x275a40, DISASM cshell+0x275a40..0x275ac7): `[obj:objref16][kind:u8][a:u32][hold_secs:f32][c:u32]`;
   the progress object stores `obj` as its target (+0x08), the entry `kind` of a 32-byte code table at VA
   0x375571c8 (+0x00, its +0x08 to +0x0c), `hold_secs` as the remaining time (+0x04), `a` (+0x14), `c` (+0x18), sets
   running (+0x10 := 1) and hands `obj` to the object at `local_player`+0x1c (slot +0x34). The handler passes the
   message on only when `kind` != 0 or the in-match flag `local_player`+0x424 is set (S2C §4.4). Correction: S2C
   §4.4 lists the fields as `[a][b][c]`; the second u32 is the remaining time (DISASM: it is stored into +0x04,
   which cshell+0x275ad0 decrements by the frame time).
4. While running, cshell+0x275ad0 lowers the remaining time by the frame time and sends `UP_USE_PROGRESS_END` 0xad
   `[target][remaining:f32]` when the hold stops or reaches 0 (C2S §7.2).
5. On release the client sends 0xd8 phase 4.
6. The server decides: a completed plant -> 0x51, a completed defuse -> 0x50 (section 5.2).

`GS_PLAYER_USE` 0xd8 sub 5 `[obj][flag][use_state]` sets `local_player`+0x204 (S2C §4.4): PLAUSIBLY the server's
use state for the HUD (`hud_progress_bar`). The hold time is a server rule (GMD-6 (proposed)).

### 4.6 Side switch (clan modes)

cshell+0xcd210 (PROVEN) decides when to show the "teams change sides" picture, from `round_limit` (0x44 / 0x7d)
and the two win counters of 0x46: class 3 at exactly half the rounds played (sum of the two win counters), class 4
when either win counter reaches half of `round_limit` (once), class 8 once, class 13 when `round_no` = half the
rounds. It never swaps anything itself: the
server swaps teams with 0x5e `GS_TEAM_SWAP` (the client flips its own team 1 <-> 2) or 0x5f kind 1 with
`no_swap` = 0 (S2C §2.3). PLAUSIBLE: the clan modes play two halves with swapped sides; DeCombat default GMD-7
(proposed).

## 5. Per-mode chapters

Each chapter lists only what the mode adds to the TDM sequence of S2C §10 (join, load, spawn, kill, respawn, end).

### 5.1 `TDM` (class 1) and `TDM_CLAN` (class 4)

- **Teams:** two teams, ids 1 and 2 (S2C §3.1 record +0x70). The client counts a kill only when killer and victim
  are on different teams (cshell+0x2c430 0x6d case, PROVEN): same-team kills change no counter.
- **Goal / time:** the `goal` of 0x7d is shown as a number on the scoreboard header (classes 1 and 4). Display only.
  The ROM keys `DefaultGoal`, `GoalMin`, `GoalMax`, `GoalInc` give the room's range. Rule GMD-1 (proposed).
- **Extra messages:** none beyond S2C §10, with these observed additions (REFERENCE): the sprint permission 0xca `[allowed]` after each client 0xca (the client sprints only after `[1]`, PROVEN-CAPTURE, S2C 3.2), 0xb5 `[kills1][kills2]` after
  each 0x6d instead of 0x4a (S2C 3.3.2), 0x9d and 0x90 (ignored by the client, S2C 7.2). Class 4 adds the side switch (4.6): the picture at half the goal, then 0x5e from the server (GMD-7).
- **Client -> server:** none extra. PROVEN-CAPTURE (tdm1, 157 s): the client sent exactly 0x4d, 0x64, 0x68, 0x72, 0x76, 0x7f, 0x80, 0x82, 0x83, 0x85, 0x87, 0x8c, 0xbd and 0xca; nothing mode-specific (no 0x59, 0xd8, 0xad, 0xc5, 0xd2) even
  with five type-0x37 zones in the world. The respawn request 0x89 follows the client's own armed wait about 7000 ms after 0x67 state 2 (4.2; DISASM, not yet seen from the real client).
- **Objects:** players are kind-1 model objects with a **type 0x18** blob plus a weapon child at socket 2 (REFERENCE-CONFIRMED, tdm2; `UDP_SESSION_2009_B.md` 3.5, S2C 8.2.1; type 0x01 was never seen), pickups 0x17 kind 6, HiSec 0x17 kind 5;
  the TDM world also carried two flag pickups (0x17 kind 2) and five type-0x37 zones (4.3, 4.4).
- **HUD:** `hud_team_score` (two counters, 0x4a), `hud_round_timer`, `hud_kill_feed`, scoreboard.
- **Respawn:** the client asks (0x89, 7000 ms after 0x67 state 2); DeCombat keeps no server timer in classes 1 and 4 (DSV-32, GMD-3', 4.2). **Round end:** one round: 0x46, then 0x47 / 0x6b refresh / 0x65 / 0x48 3 s later, then 0x3f 3 s later; 0x99 was never observed (REFERENCE, 4.1).
- **Spectator:** dead players follow team mates; enemy only with the "enemy dead camera" option.
- Grade: PROVEN for the layouts and the counter rule; REFERENCE-CONFIRMED for the TDM start sequence, the character form and the kill / round-end messages the reference server sent (Appendix C); the per-step use of the rest is the S2C §10 proposal.

### 5.2 `BOMB` (class 2) and `BOMB_CLAN` (class 3)

- **Teams:** team 1 attacks (carries the bomb item), team 2 defends (carries the defuse item). Evidence: the
  carried-item notice cshell+0x82860 shows its notice for weapon-table item 0x1cc only for team 1 and for item 0x21f
  only for team 2, both only in classes 2 / 3 (PROVEN); the mode-item weapon class is 9 for classes 2 / 3
  (cshell+0x22d2b0, cshell+0x22ef50, PROVEN). That 0x1cc is the bomb and 0x21f the defuse kit is PLAUSIBLE (the
  numbers are weapon-table indices of the user's ROM; the server reads those rows at run time).
- **Round start:** 0x44 per round; with `keep_team` = 0 the client runs an extra reset for an own team 1 player
  (cshell+0x22e0c0 on the weapon inventory and `match_ui` slot +0xb0, PROVEN): PLAUSIBLY the bomb item is removed
  and must be given again (0xb2 with item 0x1cc to one attacker, GMD-8 (proposed)). `hud_marker`+0x59 := 1 when
  `waiting_banner` = 0 in classes 2 / 3 (S2C 2.1.1 step 7).
- **Objective areas:** type 0x37 zones with `z_class` = 2: kind 4 (an enemy area, used for the "inside the target
  area" state while the bomb is not planted), kind 5 `z_sub` 1 (plant point; also a map icon kind 7 and radar icon
  10). After the plant the server creates (or turns on) a kind 5 `z_sub` 2 zone at the bomb (defuse point; map icon
  6, radar icon 9). PLAUSIBLE (the client tests both sub-kinds the same way and only the server can place the
  planted bomb).
- **Client -> server:** 0xd8 phase 1 / 4 and 0xad (4.5) for plant and defuse; `UP_USE` 0x59 is not part of this
  mode (PLAUSIBLE).
- **Server -> client beyond TDM, in order** (PLAUSIBLE order, PROVEN layouts):
  1. per round: 0x44, spawn sequence, 0xb2 bomb item to the carrier;
  2. plant start: 0xad `GS_TRACK_OBJECT` to the planter with the hold time (GMD-6), 0xd8 sub 5 use state;
  3. plant done: 0x51 `GS_BOMB_PLANTED` `[bomb_secs:u32]` to all (the round timer is hidden and set to 0, the
     bomb timer counts down `bomb_secs`, `match_ui`+0x9a87 := 1 so the enemy-area state stops; `bomb_secs` = 0
     shows CRes string id 0xbcb instead), 0xb4 removing the bomb item from the planter, the defuse zone (type 0x37,
     kind 5 sub 2) and PLAUSIBLY a world object of the bomb;
  4. defuse done: 0x50 `GS_BOMB_DEFUSED`; or timer out: 0x4f `GS_BOMB_EXPLODED` (and damage to players in range,
     server rule GMD-9);
  5. round end: 0x46 with the winner, 0xb5 round wins, 0x47, 0x48.
  The objective-item radar mark 0x56 `GS_OBJECTIVE_ITEM` `[item:objref16]` marks a dropped bomb for the own team
  (S2C §2.2; PLAUSIBLE use: the bomb dropped by a dead carrier).
- **HUD:** `hud_bomb_timer`, `hud_banner` (planted / defused / exploded notices from the HUD layout), radar and map
  site icons, `hud_progress_bar` (PLAUSIBLE).
- **Respawn:** none within a round (no request for classes 2 / 3, 4.2). Dead players observe until the next round.
- **Round end:** win when all enemies are dead, the bomb explodes (attackers) or is defused / the time runs out
  (defenders) (server rule GMD-9). `goal` = rounds; scoreboard shows the round limit (classes 2, 3).
- **Class 3:** half-time picture at exactly half the rounds, then 0x5e / 0x5f (4.6).

### 5.3 `COOP` (class 5): see section 6.

### 5.4 `FFA` (class 6)

- **Teams:** none visible: the intro block carries no team bases (S2C 2.1.1 item i), the observer may follow
  anybody, the spectator and result screens use one ranking list, the round-win banner plays no team sound
  (PROVEN). The 0x45 intro carries `mode_caption` (shown by `hud_team_score`).
- **Counter rule trap:** the client counts a kill only if the two roster records have **different** team values
  (5.1). A server that puts every FFA player on the same team gets no kill counts on the client. Two ways out
  (GMD-10 (proposed)): give every player a distinct team value, or send absolute kills / deaths with 0x6b kind 2
  after each kill. Which one the 2009 server used is UNPROVEN (V-GM-3). The scoreboard has an FFA-only refresh on
  each counted kill (cshell+0x909a0 / cshell+0x909e0, PROVEN).
- **Goal:** player kills (PLAUSIBLE; the ROM row's goal range differs from TDM). Room team-kill switch is off.
- **Respawn:** 0x89 (4.2). **Round end:** one round; result screen with one list.
- **Spectator:** any player (cshell+0x60010 no team filter for 6).

### 5.5 `FLAG` (class 7) and `FLAG_CLAN` (class 8)

- **Teams:** two teams, each with a flag: a type 0x17 pickup of kind 2 `[state][team][at_base]` (4.4).
- **Capture zones:** type 0x37 zones of kind 6, `z_team` = the team, `z_class` = 7 (8 is mapped to 7).
- **Client-side capture prompt** (cshell+0x5dad0, PROVEN): while the local player carries a flag
  (`local_player`+0x479), stands in his own team's kind-6 zone and his own team's flag is at base (no kind-2
  pickup of his team with `at_base` = 0), the client shows the capture notice (flag HUD event 5 and banner event
  5). It sends nothing: the capture is the server's decision (PLAUSIBLE).
- **Flag events: 0xc6 `GS_OBJECTIVE_STATE`** (handler cshell+0x5aae0, DISASM cshell+0x5aae0..0x5ac2f; correction
  of the S2C field names): `[obj:objref16][state:u32][flag_team:u32][carrier:u32][actor:u32][event:u32][at_base:bool1][quiet:bool1]`.
  - every kind-2 pickup whose `team` is `flag_team` and whose engine object is `obj` gets `state` and `at_base`
    (slot +0x4c);
  - `local_player` cshell+0x5d9e0(`is_me`, `flag_team`, `event`, `actor`, `quiet`) with `is_me` = (`carrier` !=
    -1 and `carrier` = own client id): when `flag_team` is not the own team or `event` = 4, the carrying flag
    +0x479 := `is_me`; unless `quiet`: event 1 adds one to the actor's first flag counter on the team score panel
    (+0x804, cshell+0xc5b80), event 3 to the second counter (+0x80c, cshell+0xc5bc0); the flag HUD
    (cshell+0x86da0) and banner (cshell+0xbdb50) show the event picture for own / enemy team and "me"; events 1,
    2, 3 add a kill-feed style line with the actor's name (cshell+0xa6bd0);
  - `mode_rules` slot +0x54(`flag_team`, `carrier`) records the carrier for the HUD (cshell+0x5bc40, PROVEN).
  Event meanings (PLAUSIBLE from the effects): 1 = captured (scores), 2 = taken, 3 = returned (second counter),
  4 = dropped / reset (clears the carrier regardless of team), 5 = local capture prompt only.
- **Late joiners:** 0x7d for classes 7 / 8 carries `n` triples `[flag_team][carrier][carrier_team]` that the client
  passes to `mode_rules` slot +0x58 (cshell+0x5bcd0: the same carrier record as above, PROVEN), so a joining client
  learns who carries which flag. `n` = 0 when no flag is carried.
- **Client -> server:** `UP_RESPAWN_REQUEST_B` 0xc5 after an armed wait (4.2). No pickup request exists: flag
  pickup is decided by the server from positions (PLAUSIBLE, no C2S sender found, C2S §7).
- **HUD:** `hud_flag`, flag panels on the round timer and team score (cshell+0xbe7c0, cshell+0xc53e0), flag icons on
  radar and maps.
- **Round end:** `goal` = captures (PLAUSIBLE, ROM goal range); the result screen has flag columns (cshell+0x130220).
  Class 8: half-time picture once (4.6).
- **Respawn:** 0xc5 (respawn allowed).

### 5.6 `BOUNTY` (class 9)

- **Roles:** the class-9 message **0xcb** `GS_BOUNTY_MODE` (handler cshell+0x2bd80, DISASM
  cshell+0x2bdc2..0x2c2b7) assigns a player to team 1 or team 2. Sub 0 layout (correction of S2C §3.5, which has
  the first two fields swapped): `[value:u32][team:u8][pid:u32]` + `look` block (cshell+0x1c2a90, the 0x6b look
  block). The record of `pid` gets `team`; team 1: the player is removed from one team-score list
  (cshell+0xc59c0, `mode_rules` cshell+0x5ba20) and, if own, `local_player`+0x47a := 0; any other team: the player
  is added with `value` (cshell+0xc5a50, `hud_multi_kill` cshell+0xa6e70, `mode_rules` cshell+0x2bc50) and, if own,
  `hud_bounty` shows `value` (cshell+0x71850) and the own state (1 when `mode_rules`+0xb4 = 1, else 0),
  `local_player`+0x47a := 1; for another player the bounty HUD state becomes 3 (phase 1) or 2. Radar, target-name,
  spectator-map and marker lists are refreshed; the own team in `session_info` follows. PLAUSIBLE: team 2 (or
  "not 1") is the marked player with a bounty `value`, team 1 the hunters.
- **Other sub-ids:** 1 `[a:u32][b:u32][c:u32]` -> `hud_bounty` values (cshell+0x71940(a), cshell+0x71670(b),
  cshell+0x71760(c)); 2 (no fields) -> weapon slots, inventory and `match_ui` resets (cshell+0xa0bd0, cshell+0x22db70,
  cshell+0x1b31c0, cshell+0x1b5380; PLAUSIBLE "new life / new loadout"); 3 / 4 -> bounty HUD states 4 / 5 unless
  phase 1; 5 `[phase:u8]` -> `mode_rules`+0xb4 := `phase`, and for `phase` = 1 `[title:zstr8(260)][text:zstr8(260)]`
  as a 4000 ms banner, bounty HUD state 1 (own player marked) or 3, marker refresh; 6 `[a:u32][secs:u32]` -> timer
  (cshell+0xbe0c0(a, secs, 1, 1)). PROVEN control flow, meanings UNPROVEN.
- **Phase 1 effect** (PROVEN): while `mode_rules`+0xb4 = 1 the roster kill / death counters and the team score
  counters are **not** changed by 0x6d (cshell+0x2aad0, cshell+0x2ab70, cshell+0xc5960, cshell+0xc5ae0), and the
  item-0x1c6 notice is shown only in phase 1 for the marked player (cshell+0x5d880, cshell+0x82860). So in phase 1
  the score comes from the server's bounty values (sub 1 / sub 0 `value`), not from kills (PLAUSIBLE).
- **Pickups:** type 0x17 kind 3 (bounty item, radar icon 4) shown only in class 9 (4.4).
- **Teams on screen:** one ranking list (result, spectator), observer follows anybody, no team sounds (as FFA).
- **Respawn:** 0x89 (4.2). **Goal:** points (PLAUSIBLE; ROM goal range). **Round end:** one round.
- Rule values (who becomes the bounty, how much it is worth, how long a phase lasts): GMD-11 (proposed).
- **Addendum (modes-spyhunt, 2026-10-28).** Sub 6 is `[secs:u32][colour:u32]`: cshell+0xbe0c0 takes the first value
  as the clock's seconds and sets the second as the digits' colour (the handler logs the second in hex). The element
  cshell+0x70880 (layout section of the mode) names its items: notices "seize intel", "seize all", "stolen",
  "stolen all", "drop intel", "respawn intel" (cshell+0x71c30 states 0..5, sounds intel get / get all / drop), stage
  messages per stage and role (cshell+0x71a90: phase 0 = stage 1, phase 1 = stage 2; the own spy flag picks the spy
  form), dropped-intel and spy counters (sub 1 `b`, `c`), the own seized / total intel (sub 0 `value`, sub 1 `a`) and
  a seize animation (own sub 0 with a team other than 1). So `value` is the seized intel count, team 2 the intel
  holders ("spies"), and phase 1 the agent's stage. The ROM GMS files of the mode name 5 intel objects, an agent body
  row with three weapon rows and a 180 s stage 2; DeCombat's rules: DC_DEFAULTS.md modes-spyhunt (DSV-230..DSV-236).

### 5.7 `INFECT` (class 11)

- **Teams:** team 1 = humans, team 2 = infected. Evidence: the round-win banner of class 11 picks the human win
  sound and picture for `win_team` 1 and the infected ones otherwise (cshell+0xc1a50, PROVEN; the sound keys are
  ROM sound-table entries, not quoted).
- **0xd3 `GS_INFECTION_MODE`** (handler cshell+0x2a1e0, DISASM cshell+0x2a21e..0x2a296; correction of S2C §3.5):
  - sub 0 `[team:u8]` then `if team == 2: [infector:u32]`, then `[pid:u32]` + `look` block, and nothing more
    (the "further fields" of S2C do not exist). The record of `pid` gets `team`; the character's client object is
    told (cshell+0x201e40(1) for team 2, (0) otherwise: PLAUSIBLY the infected look, flag +0x5bc); team 2 for the
    own player: camera and movement reset, an infection notice (cshell+0xb10f0(3, 5.0)), a profile value applied
    to `match_stats` slot +0x104 (PLAUSIBLY the infected speed or health factor, UNPROVEN); when the own player is
    the `infector`: the infection counter +1 (cshell+0xb2260) and a sound. Team and score lists are refreshed.
  - sub 1 `[a:u8][b:u8][flag:bool1][secs:f32]`: the infection countdown starts with `secs` (cshell+0xb1190; an
    intro sound when 19 < `secs` <= 20) and the two two-digit counters show `a` and `b` (cshell+0xb0e30,
    cshell+0xb0f70). `secs` is used as a float (the handler passes the raw 32 bits to a float parameter, DISASM).
    PLAUSIBLE: seconds until the first infection, and the round number / round limit (0x44 writes the same two
    counters with `round_no` / `round_limit`).
  - sub 2 (no fields): weapon slots, inventory and `match_ui` resets (as 0xcb sub 2).
  - sub 3 `[v:i8]`: own profile record +0x2ac := `v` (UNPROVEN meaning).
  - sub 4 (no fields): a notice by the own state (cshell+0xb10f0(2 or 1, 5.0) for human / infected).
- **Kills:** the client's 0x6d counting adds an infection-HUD note when the local player kills a character whose
  client object has the infected flag +0x5bc (S2C §3.3.1 step 6). Health / damage of infected players is a server
  rule (GMD-12 (proposed)).
- **HUD:** `hud_infection` (countdown in minutes:seconds, a countdown sound below 11 s, the two counters; per-frame
  cshell+0xb27b0), team score with the points layout (cshell+0xc05a0), round timer infection panel
  (cshell+0xbe7c0, cshell+0xbe570).
- **Respawn:** none (no request for class 11): a killed human becomes infected by 0xd3 sub 0 instead (PLAUSIBLE);
  an infected player's death respawns him by the server's own timer (GMD-12).
- **Round end:** rounds (`goal`); humans win if one survives the round time, infected win when no human is left
  (server rule GMD-12). Result screen: one ranking list (classes 6, 9, 11).
- **Room:** team-kill switch off (ROM).

### 5.8 `ELIM` (class 12) and `ELIM_CLAN` (class 13)

- **Teams:** two teams; the scoreboard header shows the round limit for 12 and 13 (S2C §3.2).
- **Respawn:** none within a round (4.2): one life per round (PLAUSIBLE: no request is ever sent).
- **Extra messages:** the round flow of 4.1 (0x44 per round, 0x46, 0xb5); no objective objects.
- **Round end:** a team wins the round when the other team has no living player, or by time (server rule GMD-13).
- **Class 13:** half-time picture when `round_no` reaches half the rounds; then 0x5e (4.6).
- Grade: PLAUSIBLE (the class has no dedicated handler; its behaviour is the generic round flow without respawn).

### 5.9 `TDM` variants of the clan list

Clan rows reuse the plain rules of their class family (4 = 1, 3 = 2, 8 = 7, 13 = 12) plus the side switch and the
clan status messages 0x5f, 0xc0 (S2C §2.3, §3.4). The disabled clan rows (`GameType` -1) are never offered.

### 5.10 `CLASS10` (class 10, `EnablePlay` off in the ROM)

The row is disabled in this ROM (two maps carry its key). The client still has its code: respawn request 0xd2 only
while the own team has a living member (cshell+0x61e40, cshell+0x293d0), mode-item weapon class 8. No dedicated
handler or HUD was found. Meaning UNPROVEN (PLAUSIBLY a team-life mode). DeCombat does not offer it (GMD-14).

## 6. Fireteam: the co-op AI mode (class 5)

### 6.1 What the client expects

The co-op mode is the only mode with a mission flow and AI opponents. The 2009 client has **no AI code**: it
renders what the server streams and reads mission state from messages. Everything the AI does is a DeCombat design
(server rules, HOSTING_AND_MODS §3; AI behaviour, waves and difficulty are GMD-15..GMD-18 (proposed)).

Client-side facts (PROVEN unless noted):

| area | client expectation | evidence |
|---|---|---|
| mission flow | 0xc9 `GS_COOP_MISSION` (S2C §6.2): sub 1 builds the goal list (`goal_id`, `goal_label`, `goal_state`) and timers, sub 2 / 3 texts with a sound, sub 4 mission timer, sub 5 goal add / remove / state, sub 6 pickup note (three pickup sounds), sub 7 `local_player`+0x268, sub 8 scoreboard values | cshell+0x31490 |
| points and per-player values | 0xd7 `GS_COOP` sub 1 / 3 points (with a floating points popup for the own player, cshell+0x8ad40), sub 2 the lives count (4.2: sent before the death message; it re-arms only a dead player), sub 4 the mission timer on the round-timer element, sub 5 multi-kill HUD reset | cshell+0x2d2e4 |
| match start | 0x45 with `mode_caption` (co-op score panel) and the co-op `[x:u8][y:u8][z:bool1]` tail | S2C 2.1.1 |
| results | 0x46 co-op block `[stage][c1..c6]` and for `stage` = 2 the name list, title, text, panel time; 0x65 co-op block | S2C 2.1.2, 3.4 |
| level chain | `GS_LEVEL_EXIT` 0x99 with the next world, the client's 0x96 / 0x98 / 0x99 answers (C2S §7.4) | cshell+0x22f950, cshell+0x5b1a0 |
| HUD | `hud_coop_score`, `hud_coop_result`, `hud_coop_item`, co-op layouts of marker, health, kill feed (cshell+0x8a420, 0x96b30, 0x9d4a0), co-op map background, radar icons for a boss, health and ammo (layout keys of the co-op HUD section, cshell+0xb3e30) | section 3 |
| room options | the `COOP` row has all four room switches off and a fixed player count; 2 maps carry its key (section 2) | OBSERVED (ROM table key `EnablePlay`, `MaxPlayers`) |

### 6.2 AI-controlled characters as the client sees them

- **Body:** an AI enemy is a character world object with the character client object of type 0x18, the type the
  roster looks characters up by (cshell+0x20fbe0(0x18, obj)); for **players** the server's create path is the type 0x18 blob
  (REFERENCE-CONFIRMED, tdm2; S2C 8.2.1, the same router class); for AI the client side is now read in full (`FIRETEAM_2009.md` 1.1-1.3, PROVEN, spec-fold-7): the same blob with `roster_bound` 0 and `has_tail` 1 takes the NPC branch, and the tail
fields are `npc_rank`, `npc_team`, `npc_kind`, `npc_health`, `npc_caption` (S2C 8.2.1); what the 2009 server sent is unknown (no capture, V-FT-1). The class: factory
  case 0x18 -> cshell+0x220830 -> constructor cshell+0x206db0, method table VA 0x374efea4, PE-DATA). Its init
  (slot +0x0c = cshell+0x205f40, PROVEN) copies the create record, keeps the engine object, copies the infected
  flag (object +0x5bc from +0xdc), and, when the object's field +0x40 (filled from the create data by cshell+0x202650)
  is 0 or a check (cshell+0x1a0470) fails, and +0x44 is set, adds a **marker entry by itself** with the name string
  at +0xb4 and the team +0xd0 (cshell+0x8a760). PLAUSIBLE: this is the path for non-player characters: they need no 0x6b roster record.
- **Health bar:** every marker entry gets an extra element (cshell+0x878b0: the co-op HUD's NPC health-bar
  texture, rectangle, offset and alpha keys); it is drawn only in class 5 and only for entries whose team is not
  the own team (cshell+0x8a420: `mode_class` == 5 and entry team != own team). So AI enemies need a team value
  different from the players' team (GMD-15 (proposed): players team 1, AI team 2). *Superseded first-draft
  sentence:* "Which value feeds the bar's fill is UNPROVEN (V-GM-6; PLAUSIBLY a value of the character object that the server updates through 0xa6
  object notes)." **Settled (C-FT-3, spec-fold-7; `FIRETEAM_2009.md` 1.5):** the fill is object +0x5ae, the percent 0..100 that the create sets from the
  tail's `npc_health` (trunc(fraction x 100)) and 0xa6 [0x18] sub 0x24 changes later; the marker update reads it every frame through class slot +0x74 and
  draws the bar only while `npc_kind` (object +0xd8) is 1 or 2 (PROVEN chain cshell+0x206d30 -> cshell+0x8b2e0 -> cshell+0x6d800; DISASM
  cshell+0x8b565..0x8b58b). An empty `npc_caption` creates no marker entry and therefore no bar (FIRETEAM_2009 1.4). The conversion truncates, so a
  server sends `(percent + 0.5) / 100` (FT-W1 (proposed)). V-GM-6 now only confirms this on screen (V-FT-5).
- **Movement / animation:** AI bodies move by the ordinary object stream (positions, rotations, animation state
  of the engine object, `UDP_SESSION_2009_B.md`), like remote players. No AI-specific message exists. PLAUSIBLE.
- **Kills of AI:** the kill feed 0x6d addresses roster ids (u8 `victim`, `killer`), so an AI without a roster
  record cannot appear in 0x6d (the handler looks both up and skips unknown ones). Co-op points for AI kills go
  through 0xd7 sub 1 / 3 (`points`) and 0xc9 sub 8 (PLAUSIBLE). Damage taken from AI uses 0x74 as from players.
  **No kill-feed rows at all in class 5** (C-FT-4, spec-fold-7; `FIRETEAM_2009.md` 1.7, PROVEN cshell+0x9d4a0): the kill-feed add called from the 0x6d
  case returns at once when `mode_class` is 5, so in co-op 0x6d still counts the victim's death but shows no line, also for players killed by players or AI.
  No client path removes an AI's marker, radar or overview-map entry (only 0x6c and 0x65 do, for roster pids), so a server hides and reuses AI objects instead
  of removing them until V-FT-6 shows removal is safe (FT-R1 (proposed)).
- **Bosses and items:** radar and map icons for a boss and for health / ammo items exist in the co-op HUD layout
  (cshell+0xb3e30, cshell+0xc5ce0); what marks an object as a boss is UNPROVEN (V-GM-7).
- **Waves:** the client has no wave concept beyond the mission goals and timers of 0xc9 and the round-timer use of
  0xd7 sub 4. Waves, spawn points, counts and difficulty (`goal` 0..2 of the row, PLAUSIBLY difficulty) are DeCombat
  design (GMD-16, GMD-17).

### 6.3 What `dc-dserver` needs for a first co-op slice

1. Players on team 1 with the normal spawn sequence; 0x45 with `mode_caption` and the co-op tail.
2. AI bodies as character objects (the same create path as player characters, V-S2C-1) on team 2 with a
   name, no roster record (the NPC create form of `FIRETEAM_2009.md` 1.9, FT-C1 (proposed): `npc_kind` 1 so the bar shows, FT-K1); positions streamed every tick by the server's own AI (dc-sim movement, GMD-16).
3. 0xc9 sub 1 at mission start (one goal "eliminate the wave", one timer), sub 5 op 3 when a goal completes.
4. 0xd7 sub 1 for points per AI kill, 0x6b `m1` and 0xd7 sub 2 for the lives, each new count before the 0x67 state 2 of the death (4.2, FT-L1); respawn request 0x89.
5. End: 0x46 with the co-op block (`stage` 2 complete, 3 or 4 failed, 5 timed out: S2C 2.1.2, C-FT-6; the first draft's "`stage` 1 or 2" is superseded), 0x65 with the co-op block, 0x3f, 0x99.

## 7. Movement: mode-specific rules

None of the 55 functions of section 1.2 lies in the mover code of `MOVEMENT_2009.md` (cshell+0x1f0470..0x202000 region:
the controller and mover functions cited there). PROVEN by the search (direct and index forms); a mode-dependent
movement value reached through another pointer would not be found by it (residual risk, UNPROVEN). Mode effects on
movement are indirect:

- the round classes freeze movement through player states 2 / 3 / 6 (S2C §4.1), not through the class;
- infection: 0xd3 sub 0 for the own player applies a profile value to `match_stats` slot +0x104 and resets
  movement and camera; whether this changes the speed is UNPROVEN (V-GM-5);
- the character model (look block) changes for infected players, and the stand box comes from the model
  (`MOVEMENT_2009.md` 9.2), so an infected model may have another box (PLAUSIBLE).

## 8. Implementation order for `dc-dserver`

Cheapest first, assuming the TDM slice of DC_DEFAULTS Wave F works (S2C §10 sequence, DSV-10..21):

| step | mode(s) | new work | cost |
|---|---|---|---|
| 1 | `TDM_CLAN` (4) | 0x9e with 4; side switch: 0x5e at half the goal (GMD-7); clan status 0x5f / 0xc0 | very low |
| 2 | `ELIM` (12), `ELIM_CLAN` (13) | round loop: 0x44 / 0x46 / 0xb5 / 0x48 per round, no respawn, observer state 3 until the next round, round winner (GMD-13); 13 adds the side switch | low; it builds the round loop that 2, 3, 7, 8, 11 reuse |
| 3 | `FFA` (6) | team values per GMD-10, 0x45 caption, no bases; single-list result | low |
| 4 | `FLAG` (7), `FLAG_CLAN` (8) | type 0x17 kind 2 flag objects, type 0x37 kind 6 capture zones, 0xc6 events, 0x7d carrier list, 0xc5 respawn, pickup by proximity (GMD-19) | medium |
| 5 | `BOMB` (2), `BOMB_CLAN` (3) | type 0x37 zones kinds 4 / 5, hold-to-use (0xd8 / 0xad both ways), 0x51 / 0x50 / 0x4f, bomb item 0xb2 / 0xb4, explosion damage (GMD-6, GMD-8, GMD-9) | medium |
| 6 | `INFECT` (11) | 0xd3 subs 0 / 1 / 4, infected look and team change, countdown (GMD-12) | medium |
| 7 | `BOUNTY` (9) | 0xcb subs 0..6, kind-3 pickups, phase rules, points (GMD-11); most meanings UNPROVEN: needs V-GM-4 first | medium-high |
| 8 | `COOP` (5) | AI bodies, mission messages 0xc9 / 0xd7, co-op result blocks, level chain (section 6; GMD-15..18) | high (the AI is new design) |
| - | `CLASS10` (10) | disabled in the ROM | none |

Steps 1-3 need no new object types; step 4 introduces type 0x37 zones and 0x17 kind 2, which step 5 reuses.

## 9. Capture items V-GM-n

Three TDM captures exist (tdm1 solo, tdm2 two players, tdm4 kill; Appendix C); no capture of any other mode. Captures use `evidence/captap.py` against a DeCombat server with a human driving the client (never an agent-launched game client).

| id | what to record | settles |
|---|---|---|
| V-GM-1 | a round mode (class 12) match: the messages between two rounds | 4.1 order of 0x46 / 0xb5 / 0x48 / 0x47 / 0x44 and the respawn of all players |
| V-GM-2 | the object stream of a bomb or flag map at load | where type 0x37 zones and kind-2 flags come from (world objects or server-made), the unread zone fields (4.3) |
| V-GM-3 | an FFA match: kill counts on the client scoreboard with distinct team values vs one team | GMD-10 |
| V-GM-4 | the client's bounty HUD for each 0xcb sub-id and state value | 5.6 meanings |
| V-GM-5 | an infected player's movement speed and box after 0xd3 sub 0 | section 7 |
| V-GM-6 | the NPC health bar with a type 0x18 character of team 2 in class 5 | 6.2 health source (settled statically by spec-fold-7, C-FT-3; the capture confirms it on screen, V-FT-5) |
| V-GM-7 | co-op radar icons for a boss | 6.2 boss marking |
| V-GM-8 | hold-to-use: 0xad server -> client values and the progress bar | 4.5 fields `a`, `c`, `kind` table |
| V-GM-9 | the flag HUD and banner for 0xc6 events 0..5 | 5.5 event meanings |
| V-GM-10 | the real client killed and left to respawn (TDM), then a class with a different wait | the 0x89 time after 0x67 state 2 (7000 ms), the reference stack's answer, the arming rule of 4.2 per class |

None of V-GM-1 .. V-GM-9 is settled by tdm1, tdm2 or tdm4 (they are about other modes). Partial input for V-GM-2: the TDM world carried five type-0x37 zones (kinds 4, 6, 8) and two kind-2 flags (4.3, 4.4).

## 10. Corrections to sibling specs

Status 2026-10-25 (spec-fold-4): the respawn arming of 4.2 and the TDM chapter 5.1 are corrected by the tdm4 disassembly pass and the captures (C2S §3.3); nothing else here changed. Status 2026-10-24 (spec-fold-3): C-GM-1..8 are all applied to `GAME_MESSAGES_2009_S2C.md` (rows 0x7d, 0xcb, 0xd3, 0xad, 0xc6, the type 0x17 and 0x37 lines of 8.2) and `HISEC_2009.md` (3.1 row 6a). The table below is kept as the record.

| # | spec and place | correction | evidence |
|---|---|---|---|
| C-GM-1 | `GAME_MESSAGES_2009_S2C.md` §3.5 row 0xcb sub 0 | layout is `[value:u32][team:u8][pid:u32]` + look block (the roster lookup compares the **third** field) | DISASM cshell+0x2be52..0x2bec3 |
| C-GM-2 | S2C §3.5 row 0xd3 sub 0 | `[team:u8]`, `if team == 2: [infector:u32]`, `[pid:u32]`, look block, nothing more | DISASM cshell+0x2a26a..0x2a2a6 |
| C-GM-3 | S2C §3.5 row 0xd3 sub 1 | the last field is used as a float (seconds of the infection countdown) | PROVEN cshell+0xb1190 |
| C-GM-4 | S2C §6.1 row 0xc6 | field roles: `[obj][state][flag_team][carrier][actor][event][at_base][quiet]`; `carrier` decides "is me", `actor` names the player in notices and scores | DISASM cshell+0x5aae0..0x5ac2f |
| C-GM-5 | S2C §4.4 row 0xad | `[obj][kind:u8][a:u32][hold_secs:f32][c:u32]`, it starts the use-progress countdown | DISASM cshell+0x275a40..0x275ac7 |
| C-GM-6 | S2C §3.2 row 0x7d (7 / 8 triples) | the triples are `[flag_team][carrier][carrier_team]` (flag carrier records) | PROVEN cshell+0x5bcd0 |
| C-GM-7 | S2C §8.2 type list | type 0x37 is the mode-zone client object (layout 4.3); type 0x17 kinds 2 / 3 are the flag and bounty pickups | section 4.3, 4.4 |
| C-GM-8 | HISEC §3.1 type 0x17 kind 2 row | kind 2 is the flag of the flag modes: `[state][team][at_base]` | DISASM cshell+0x2824d2..0x2824f0, cshell+0x5dad0 |

## 11. Open questions

1. Where the 2009 server took the objective areas (type 0x37) from, and whether the world's own objects carried
   them (V-GM-2).
2. Bounty meanings (team value, phases, sub 1 values) (V-GM-4).
3. FFA team values (V-GM-3).
4. Class 10 purpose (disabled row).
5. The fields of type 0x37 marked "not traced" and the `kind` table of the use-progress object.
6. The goal kinds are PLAUSIBLE from the client's display; the lobby offers the ROM range per row and the server
   applies GMD-1.
7. The 2009 client's actual respawn request in each class (V-GM-10), and the 2009 server's round-end order and delays (the reference stack's are in 4.1).

## Appendix C: capture notes (spec-fold-4, 2026-10-25)

Sources (evidence records, kept in full): `CAPTURE_tdm1_GAME_VALIDATION.md` section 5, `CAPTURE_tdm4_KILL_VALIDATION.md` sections 4 and 6, `REFERENCE_FINDINGS_botduel.md`, `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md` section 4.

| place | spec text | observation | grade |
|---|---|---|---|
| 5.1 teams | two teams, ids 1 and 2 | own roster record team 1 (0x6b), client 0x72 `team` 1 | REFERENCE-CONFIRMED |
| 5.1 goal / time | goal shown as a number | 0x7d `goal` 100 = the room's goal | REFERENCE-CONFIRMED (layout and source); display not visible |
| 5.1 client -> server | none extra | nothing mode-specific sent | PROVEN-CAPTURE (negative) |
| 5.1 objects | players type 0x01 / 0x18 | players type 0x18 (+ weapon child), 2 flag pickups, 5 zones | REFERENCE-CONFIRMED |
| 4.1 round flow | 0x45 with `first_round` 1 | reference stack `first_round` 0; joiner of a running round: no 0x45 | REFERENCE |
| 4.1 / 5.1 round end | 0x46, 0x65, 0x3f, 0x99 | 0x46, +3 s 0x47 / 0x6b / 0x65 / 0x48, +3 s 0x3f; no 0x99 | REFERENCE |
| 4.2 arming | only 0xd7 sub 2 / 0xdb sub 6 | the client arms itself on 0x67 state 2 (DISASM); the reference stack never arms and respawns on request | DISASM, REFERENCE, bot-confirmed |
| 4.3 zone layout | 15-field type 0x37 | five zone blobs parse exactly; `z_a`, `z_b`, `z_d` f32-like | REFERENCE-CONFIRMED |
| 4.4 pickup kind 2 | `[state][team][at_base]` | two flag pickups of 116 bits parse exactly | REFERENCE-CONFIRMED |
| 7 movement | no mode-specific movement | nothing mode-specific seen | consistent |

Count: PROVEN-CAPTURE 1 (the negative on client messages); REFERENCE-CONFIRMED 5; REFERENCE 2; DISASM 1 (arming).

## Appendix F: fold record (spec-fold-5, 2026-10-25)

Sources: `reports/dserver-slice4.md` (DSV-31, DSV-32, DSV-35, DSV-36, DSV-37), `DC_DEFAULTS.md` "Capture notes 2026-10-25" (rewrite GMD-3), `client-reference stack.md` L13.
No new 2009 evidence; grades as in the header plus **D** (DeCombat design, a DC_DEFAULTS row; TESTED / RUN = crate test / loopback run of our own stack).

| place | change | grade |
|---|---|---|
| 4.1 table row "match end" and the observed paragraph | the observed end set replaces "0x65, 0x3f, 0x99"; DSV-35 / DSV-31 / DSV-36 named | REFERENCE (forms), D (DeCombat timing: 10 s team-empty, 3 s pauses, 120 s linger) |
| 4.2 GMD-3 -> GMD-3' | rewritten to the client-armed model; the old wording kept as superseded | D (staged); client facts DISASM; reference stack facts REFERENCE |
| 4.2 death paragraph | 3 then 2 in one block by default | D; REFERENCE (order) |
| 4.3, 4.4 | DSV-37 world-file zones and flags, ROM-gated bit identity with the reference stack (tdm2 world) | D; the forms REFERENCE-CONFIRMED (Appendix C) |
| 5.1 respawn line | no server timer in classes 1 / 4 | D |
| V-GM-10 | still open (no death of the real client in any capture) | - |
| L13 (client-reference stack) | the test rooms used goal 5, below the ROM goal minimum a 2009 host can pick; the reference lobby accepted it | REFERENCE; GMD-1 (the lobby's range check) decides for DeCombat |

Not folded: the round loop of the round classes (no crate implements it yet); GMD rows other than GMD-3 (unchanged).

## Appendix G: fold record (spec-fold-6, 2026-10-25)

Sources: `reports/stack-verify.md` (release Rust stack of master 692de66, two rounds), `DC_DEFAULTS.md` GAIN-1. Grades as in Appendix F.

| place | change | grade |
|---|---|---|
| 4.1 (after the DeCombat paragraph) | RUN record: team-empty end (10.001 s, +3.014 s, +2.986 s) and goal end (+2.988 s, +3.007 s), gains in 0x65, no server bye; table in S2C 2.1.3 | RUN (our stack only; no real client) |

## Appendix H: fold record (spec-fold-7, 2026-10-25)

Sources: `FIRETEAM_2009.md` 5.2 (C-FT-3, C-FT-4, C-FT-5; with C-FT-2 / C-FT-6 for the pointers in 6.2 / 6.3). No capture exists for class 5 and no
new RVA was introduced (all from FIRETEAM_2009). Grades as in Appendix F. No existing grade was lowered; the replaced first-draft sentences are kept in place as superseded.

| place | change | grade |
|---|---|---|
| 4.2 first paragraph | class 5 arms from +0x1f8 (0xd7 sub 2 `b` or 0x6b `m1`); re-arm only in own state 2, 3, 5, 6 | DISASM, PROVEN |
| 4.2 new paragraph "Class 5 lives" | C-FT-5: the count must precede the death message; 0 = observer; give a life back; CRes 0x5a0; FT-L1 | DISASM, PROVEN; D (proposed) |
| 6.1 points row | sub 2 = lives count | PROVEN |
| 6.2 body | the NPC path read in full; tail names (S2C 8.2.1) | PROVEN |
| 6.2 health bar | C-FT-3: +0x5ae, kinds 1 / 2, empty caption, truncation; V-GM-6 reduced to an on-screen check | PROVEN, DISASM; FT-W1 (proposed) |
| 6.2 kills | C-FT-4: no kill-feed rows in class 5; no HUD removal for AI (FT-R1) | PROVEN; D (proposed) |
| 6.3 items 2, 4, 5 | FT-C1 / FT-K1 / FT-L1; result `stage` values | D (proposed); PROVEN (banner order) |
| 9 V-GM-6 | settled statically | - |

Count: corrections folded 3 (C-FT-3, -4, -5), superseded sentences kept 2 (6.2 health source, 6.3 `stage` 1 or 2).

## Appendix I: fold record (spec-fold-8, 2026-10-25)

Source: the aa-start-state correction (`reports/aa-fix-start-state.md`; S2C 2.1.1 step 3). The match-start row, the 5.1 paragraph, the 4.2 note on 0xd4 and
the 4.4 round-start bullet now name the flag `waiting_banner` (formerly `first_round`); the 5.1 paragraph no longer says that S2C section 10 proposes 1. Appendix C keeps the
spec-fold-4 wording as a record. Grades unchanged (PROVEN layout, DISASM meaning).
