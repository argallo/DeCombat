# bot-shooter: bot-vs-bot reference stack rounds (reference lobby + reference game server, no real client)

- **Sessions** (all `python -I evidence/capture_session.py --start --no-client --label botduelN`, loopback, the
  tool's ports, two `dc-botclient` processes from branch `bot-shooter`, `--rom` = the user's read-only client folder,
  goal set by the host bot to the ROM mode row's smallest goal (0x38c type 8 = 30, echoed by 0x3035 type 8 value 30);
  TDM, `mode_class` 1 in 0x9e and from the ROM row):

  | attempt | capture folder | Bot1 (host, team 1, pid 1) | Bot2 (guest, team 2, pid 0) | result |
  |---|---|---|---|---|
  | 1 | `captures/20261025_010543_botduel1` | `--shooter --walk` | standing | 120 reports, 0 damage: every line of sight was blocked (section 1); stopped by PID after 3.5 min |
  | 2 | `captures/20261025_011446_botduel2` | `--shooter --hunt` (way-finder v1) | standing | the way-finder stuck at a ledge after 10 s; stopped by PID after 80 s |
  | 3 | `captures/20261025_012331_botduel3` | `--shooter --hunt` (way-finder v2) | standing | **30 kills, goal reached, round ended, both bots left after 0x3f** |
  | 4 | `captures/20261025_013028_botduel4` | `--shooter --hunt` | `--shooter --hunt` | **both died and respawned**: Bot1 3 kills / 5 deaths, Bot2 5 kills / 3 deaths; the round ended when both quit at `--duration` (0x46 to the one still linked) |

  Both bots ran with `--respawn-after 8000 --duration 300`. Every process was stopped (bots exit 0 or stopped by
  PID; the tool stopped reference game server and the lobby and reported "all session ports are free"); `tasklist` / `netstat`
  confirmed nothing was left after each attempt. A stale reference lobby of the aborted `run_tdm3` (PID 41356, not
  started by this task) was running the whole time on its own ports; left alone.
- **Evidence:** `// OBSERVED: captures/2026102501xxxx_botduelN/capture.jsonl` and the bots' own
  logs `logs/Bot{1,2}.events.jsonl` / `.report.json`, decoded with `dc-botclient` `examples/capdecode.rs` /
  `weaponforms.rs` (both directions replayed into receiving Links). The capture's lobby / reference game server logs were not read
  (timestamps only rule).
- **Grades:** client -> server bytes here are **our bot's** (not 2009 evidence); server -> client bytes are
  **REFERENCE** (what the reference stack does, never a rule value to copy). Where the 2009 client's own behaviour is the
  point, the evidence is cited separately (tdm4 PROVEN-CAPTURE, capval-kill DISASM).
- **ROM content:** none quoted; ROM table values were read at run time and are cited by key; message heads that
  carry ROM file or variable names (engine ids 0x32 / 15) are described by form only.

## 0. Summary

| # | question | verdict | grade |
|---|---|---|---|
| F1 | Does the reference stack validate a reported hit against the world? | **Only its effect record** (REWORDED 2026-10-25 after tdm6): the reference stack's type-0x06 record of a reported shot ends at the first wall along the reported ray, but its DAMAGE decision is not that trace and not a line-of-sight test in the level model: botduel1's 120 claims through walls did no damage, yet tdm6 applied 24 claims whose reported ray met a solid level wall 46-109 units from the muzzle, three of them kills on a fully hidden victim (CAPTURE_tdm6_VALIDATION §5.3); botduel1's refusals have another, UNPROVEN cause (dserver-hits note) | REFERENCE |
| F2 | Does the reference stack respawn a player on 0x89 without having sent 0xd7 / 0xdb? | **YES**: 37 of 37 requests made while the round ran (29 in attempt 3, 5 + 3 in attempt 4) answered with the spawn set 30-80 ms later; it never sent 0xd7, 0xdb or 0xd4 and never respawned anyone unasked | REFERENCE |
| F3 | When does the 2009-like bot ask? | 7001-7016 ms after it processed 0x67 state 2 (self-armed, the capval-kill DISASM rule); death -> state 1: 7047-7112 ms | bot + REFERENCE |
| F4 | Is a request after the round end answered? | no (2 requests after 0x46 ignored) | REFERENCE |
| F5 | Damage per hit vs ROM | identical for 120 hits at 72-648 units and for tdm4's 4 hits at 1330 units: `damage` 27, health -26.775, armour -5.25, `zone` 2, `dmg_kind` 3, `mode` 1, `ammo_kind` 0xFFFF, `part` 0; health loss = 0.765 x the ROM key value `[Ammo145] InstDamage`, armour loss = 0.15 x it, no range falloff inside `[Weapon145] Effectrange0` | REFERENCE |
| F6 | Kill message 0x6d | 151 bits: victim, killer, `kill_kind` 1, weapon 145, `hit_zone` 3, four flags 0, `streak_flag` 1 on the first kill of a life then 0, `feed_value` 2, `no_count` 0, no revenge; followed in the same block by **0xb5 [team 1 kills][team 2 kills]**; never 0x4a, never 0xc8 | REFERENCE |
| F7 | How did the round end? | goal 30 reached: 0x6d + 0xb5 [30][0] + **0x46** [3][1][0][0] in one tick; **+3.000 s** 0x47 [0], 0x6b x 2, **0x65** x 2, 0x48 [3]; **+3.000 s** **0x3f** [0][1]; no 0x41, no 0x99, no server bye (the bots left after 0x3f + 3 s) | REFERENCE |
| F8 | TCP 0x3004 | one 0x3004 to the killer's account only (exp total 63, GP total 10040, popup 1), in the tick of 0x65, then 0x345; none to the account with 0 gain; 0x3042 [id][0] for both after they left | REFERENCE |
| F9 | Our 0x82 / 0x7f / 0x76 / 0x83 forms | accepted: all 120 0x82 (887 bits) damaged the target; 3 x 0x83 [ammo 145] after each 30-round clip, shooting went on | bot form, REFERENCE acceptance |

## 1. F1: hits through walls (attempt 1): no damage; the effect record ends at the wall

Bot1 circled near its base (`--walk`) and reported 120 hits on Bot2's object (2,330-2,520 units away) at 150 ms
intervals. Bot2 received **no 0x74 and no 0x66 health change** in 3.5 minutes.

The reference stack answered each report with one unreliable engine-17 type 0x06 record to every client (432 bits):

| capture | record per report | first two u16 | example head |
|---|---|---|---|
| tdm4 (real client, clear line) | two: the hit record, then the miss-style record | `[408 = victim][189 = shooter]`, then `[0][189]` | `11069801bd009100910006110f01...`, `11060000bd009100910014f04b01...` |
| botduel1 (bot, through walls) | **one**: only the miss-style record (kind byte 0x14 x 89, 0x0a x 31) | `[0][252 = shooter]` | `11060000fc009100910014104f01...` |
| botduel3 (bot, clear line) | two, as tdm4 | `[252 = victim][255 = shooter]`, then `[0][255]` | `1106fc00ff009100910006110f01...`, `11060000ff00910091000af04b01...` |

In botduel1 the record carries the muzzle point (582.5, 78.8, 923.6) and an end point (551.6, 81.8, 527.7) about
400 units along the shot; a thin sweep through the ROM world's collision (dc-sim, the same world file the bot loaded)
between the bots' positions hits geometry at 7-19 % of the distance. The reference stack therefore traces each reported shot
against the world itself and stops it at the first wall (PLAUSIBLY the same way the 2009 server did; the 2009
client's own trace never reports such a hit anyway).

Consequence (bot fixed): the shooter reports a hit only along a clear line (`hunt::line_clear` on the loaded ROM
world), and `--hunt` walks toward the target until the line is clear. Way-finder v1 (feeler sweeps) got stuck at a
ledge in attempt 2 (the feeler said "free" where the body could not move); v2 probes each heading on a copy of the
body with the same movement code, with a visit penalty; offline on the same world it reaches a clear line in 18-33 s
from either base, live in 31 s (attempt 3).

## 2. F2-F4: respawn on request

Attempt 3, Bot2 (the victim, `mode_class` 1), 30 deaths:

| step | timing | messages (Bot2 link) |
|---|---|---|
| death | the 4th hit, in one reliable block | 0xa6 notes, 0xd6 sub 0 / sub 2 (HiSec case), 0xa6 type 0x17, **0x6d**, **0xb5** [n][0], 0x74, 0xa6 health / armour notes, 0x66 health 0, 0x66 armour, **0x67 [3]**, the corpse's file list and variables, **0x67 [2]**; then two engine-17 impact records |
| bot self-arms | 16 ms after the block (`respawn_armed` via "self after 0x67 state 2", wait 7000) | - |
| request | 7001-7016 ms after arming: **`0b 89`** (reliable, 16 bits) | c2s |
| reference stack answer | **30-80 ms** after the request (n = 29) | 0x6a, 0xa6 x 2, **0x67 [1]**, 0x66 kind 0x13, (0xb2, 0x7f) x 4 slots, 0xa0 flags 0x0009 (body + friction), **0xa0 0x0020** (teleport, new `move_seq`, a new spawn point each time), 0x66 0x13, (0xb2, 0x7f) again, 0x66 0x0f / 0x10 (maxima 100), **0x6b** (refresh of the respawned player, also sent to the other client), **0x67 [1]** again, 0xa6 notes, 0x66 kind 1 for slots 2 / 3, 0x66 health 100, armour 100, kinds 0x18 and 6 |
| death -> state 1 | 7047-7112 ms | - |

- **No 0xd7, 0xdb or 0xd4** reached either bot in any attempt; between a death and the request nothing changed the
  victim's state (the reference stack has no own respawn timer, as in tdm4 where the bot never asked and stayed dead 104 s).
- The fallback request (`--respawn-after 8000`) fired once in the whole round: after the round end (F4).
- **F4:** the 30th death (the goal) came in the same tick as 0x46; the self-armed 0x89 7 s later and the fallback
  0x89 1 s after that were **not answered** (no 0x67, no spawn set) before both bots left.
- The other player learns of a respawn from the 0x6b refresh (and the object's jump to the spawn point in the
  stream); the shooter re-armed on the position jump every time (29 `target_up` "moved").

## 3. F5: damage per hit

| capture | hits | distance | 0x74 to the victim | 0x66 after each hit |
|---|---|---|---|---|
| tdm4 (real client) | 4 | 1326-1330 | `damage` 27, `zone` 2, `dmg_kind` 3, `mode` 1, `ammo_kind` 0xFFFF, `part` 0, `from_dir` = shot direction x 0.268 | health 100 -> 73.225 -> 46.45 -> 19.675 -> 0; armour 100 -> 94.75 -> 89.5 -> 84.25 -> 79 |
| botduel3 (bot) | 120 | 72-648 | identical (`0d74...0201ffff0302001b0000` tails) | identical sequence for all 30 lives |
| botduel4 (both bots) | 35 (20 on Bot1, 15 on Bot2) | 75-662 | identical | identical |

- Base value: the ROM key value `[Ammo145] InstDamage` of the reported ammunition row (weapon 145's `AmmoName0`), read
  at run time. Health loss = 0.765 x it, armour loss = 0.15 x it, the 0x74 `damage` = the rounded health loss; four
  hits kill from 100 / 100. The same amounts at 72 and at 1330 units: no range falloff inside `[Weapon145]
  Effectrange0`. These are reference stack rule values (armour share, a further factor): **not taken**; DSV-11 stays the
  DeCombat rule.
- `zone` 2 equals the ROM key value `[Ammo145] InstDamageType` (as capval-kill found); our bot always reported
  `extra_kind` 24 and the reference stack's `zone` did not change with it.

## 4. F6: the kill message

`0d6d0001019100031020000000000000000000` (first kill of a life, `streak_flag` 1) and
`0d6d0001019100030020000000000000000000` (`streak_flag` 0), 151 bits, to both links in the same block; fields
(S2C §3.3.1): victim 0, killer 1, `kill_kind` 1, weapon 145, `hit_zone` 3, `f_relation` 0, `f_feed2` 0, `keep_hud` 0,
`f_feed6` 0, `streak_flag` 1 / 0, `feed_value` 2, `no_count` 0, `has_revenge` 0, `f_revenge_done` 0, `coop_a` /
`coop_b` 0. Counts: 30 kills = 30 x 0x6d to each bot, each followed by 0xb5 [kills of team 1][kills of team 2]
(`0db50100` .. `0db51e00`); 0xb5 [0][0] once at the match start; 0x4a and 0xc8 never.

## 5. F7 / F8: the round end and the lobby results (attempt 3)

| t (s) | to both bots (REFERENCE) |
|---|---|
| 293.297 | the 30th kill: 0x6d, 0xb5 [30][0], **0x46** `0d4603000000010000` = [round_secs 3][win_team 1][0][0] |
| 296.297 | 0x47 [0], 0x6b (Bot2 record), 0x6b (Bot1 record), **0x65** pid 0 `0d6500fc0000...` (all gains 0), **0x65** pid 1 `0d6501ff00000000003f000000000000002800...` (exp_gain 63, gp_gain 40), 0x48 [3] |
| 296.281 | lobby (TCP, Bot1's account only): **0x3004** `0430120000003f0000003827000001000000` = exp total 63, GP total 10040, popup 1; then 0x345 |
| 299.297 | **0x3f** `0d3f0200000000` = [result_flag 0][result_value 1 = the winning team] |
| 302.3 | both bots: `leave_after_result` (3000 ms, BOT-16), `C2S_BYE`; the reference stack closed each link with bye code 5; Bot2 got 0x6c [pid 1] when Bot1 left |
| 302.3-302.6 | lobby: 0x3042 [1001][0] and [1002][0] to both, 0x3035 type 7 (room state) |

- Order: 0x46 with the goal kill; 0x47 / 0x6b / 0x65 / 0x48 3.0 s later; 0x3f 3.0 s after that. Same order as tdm4
  (round ended by a player leaving, where 0x6c came first); `round_secs` = 3 in 0x46 and 0x48 both times.
- **No 0x41, no 0x99** (no level-exit request), no 0xb5 after 0x46, no server bye after 0x3f (tdm4: the link stayed
  up 38 s): leaving is the client's job.
- During the match the lobby sent Bot1 four 0x3014 (`inbox_stale_push`) at the times it walked over the HiSec cases
  the victim dropped (0xd6 sub 1 / sub 2 on the game link); no other lobby message during the match.

## 6. F9 / round 4: both bots shoot (attempt 4)

- Both bots hunted and fired: Bot1 28 reports (3 kills), Bot2 52 reports (5 kills, one 0x83 reload). Bot1's
  way-finder made little progress for long stretches (96 progress checks without progress against 6 for Bot2), so
  only 8 kills happened in 300 s and the goal was not reached.
- Every death of either bot followed the same path as attempt 3: 0x6d + 0xb5, 0x67 [3] then [2] in one block, the
  bot's self-armed 0x89 7001-7015 ms later, the spawn set 31-64 ms after it (Bot1 5 of 5, Bot2 3 of 3), death ->
  state 1 7048-7095 ms. Kill forms: victim 1 / killer 0 (5) and victim 0 / killer 1 (3), all `kill_kind` 1,
  `hit_zone` 3, weapon 145; `streak_flag` 1 only on the first kill.
- Each spawn gave the bot its weapons again (0xb2 x 4 slots, 0x7f forced-switch notes) and the bot sent the select
  pair `76 03 02` + `7f 00 9100 9100` once per life (6 and 4 times).
- **Round end by leaving:** both bots reached `--duration` at the same instant and sent `C2S_BYE`; the reference stack closed
  both links with bye code 5. Bot2's link still received **0x6c [pid 1] and 0x46 [3][2][0][0]** (win team 2, the side
  still connected) 312 ms after Bot1's bye, as in tdm4 (0.344 s after the leaver) - then nothing more: 0x65 / 0x3f
  would have followed 3 s later, but Bot2 was gone. No 0x3004 to either account; the lobby sent 0x3042 [id][0] for
  both and 0x3035 type 7 (room state 0), and 0x3035 type 2 (host hand-over to 1002) to the remaining room member.
- No time limit exists in this TDM room (0x44 / 0x45 `round_secs` 0, as in tdm4), so "the time limit ending the
  round" cannot happen here: the two endings observed are the goal (attempt 3) and a team leaving (attempt 4, tdm4).

## 7. What dc-dserver should take from this (for its next slice)

| # | item | evidence |
|---|---|---|
| D1 | Validate reported hits with a world trace from `origin` to the reconstructed point (a hit behind geometry does no damage); keep the box tolerance (the 2009 client's hit points lie on the model, 30 units in front of the box centre) | F1 (REFERENCE), tdm4 geometry |
| D2 | Make the request the trigger: answer a dead player's 0x89 (0xc5 / 0xd2 by class) with the spawn set at once once `respawn_ms` minus a margin has passed; keep the own timer only as a fallback (capval-kill DC-KILL-1). Today dc-dserver respawns on its own 7000 ms timer, which races a 2009 client's request 7001-7016 ms after it processed state 2 (a request from a player the timer already revived is logged as "too early") | F2, F3; loopback test (the bot without a ROM class hint sends nothing) |
| D3 | Ignore requests after the round end | F4 |
| D4 | Send 0xb5 [team kills] after each kill (the reference stack never sends 0x4a) | F6 (DC-KILL-3) |
| D5 | Round end set: 0x46 with the deciding kill, 0x47 / 0x6b refresh / 0x65 / 0x48 after a pause, 0x3f after another; no server bye | F7 (pauses are reference stack rule values: DeCombat picks its own) |
| D6 | Lobby: push 0x3004 to every account whose totals changed, in the 0x65 tick | F8 |
