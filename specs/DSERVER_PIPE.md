# DSERVER_PIPE.md: lobby <-> dedicated-server stdin/stdout line contract

Status: DRAFT spec, 2026-10-24. Implemented by `dc-lobby-server` and `dc-dserver`; spec-fold-5 (2026-10-25) verified the coordinator's folds of the lobby-loadout and dserver-slice4 addenda (sections 11, 12), linked them from the base sections 3.1, 4.5, 4.7, 5 and 6, and added section 13;
spec-fold-3 (2026-10-24) folded the end-to-end findings (addendum A1-C5, section 9) and marks every point where the spec text was
brought in line with what both implementations do as "ALIGNED 2026-10-24". Grades TESTED / RUN / READ are those of the e2e record (section 9). spec-fold-4 (2026-10-25) adds section 10: what the captures of the real client (tdm1, tdm2, tdm4) say about this channel; grade PROVEN-CAPTURE = the real client's own bytes or reaction.
Scope: the text-line control channel between the Rust lobby (`dc-lobby`) and the dedicated game server process
(`dc-dserver` later; today the reference game server `reference game server`, or a stand-in script in tests).
Header convention (no `specs/README.md` existed when this was written; this header is the proposed convention):
Status / Scope / Evidence legend / numbered sections / Open questions.

## 0. Evidence legend and provenance

Sources read for this spec:

- `reference game server header` lines 1-23 only (our own header comment). Tag: `HARNESS: reference game server header:1-23`.
- `lobby_harness`: the `FAKE_DSERVER` script (lines 485-515) and the suites that drive it
  (`test_persist_restart`, lines 524-655). Tag: `HARNESS: lobby_harness:<line>`.

Field tags used below:

- **H** = harness fact: a script in `lobby_harness` or the reference game server header header comment sends or checks it.
- **D** = ours by design: the Rust implementation chooses it; nothing outside this file requires it.
- **O** = optional, DeCombat design: the 2009 client does not need it and the harness never exercises it. The audit says
  the older user-line field set has no 2009 evidence, so every user-line field beyond what the harness reads is O,
  not a requirement.

Not read, hence NOT specified with evidence: the lobby-side parser and the lobby-side `room` line builder
(`reference lobby` is out of bounds for this task apart from the argparse block). Statements about lobby internals are
design (D), not a description of the Python code.

## 1. Process model

- One dedicated-server child per running room. The lobby owns it: spawns it on game start, writes control lines to its
  stdin, reads event lines from its stdout, kills it when the room is over or on lobby shutdown.
  (H: reference game server header:8-9, "the lobby spawns this process with pipes".)
- Transport: UTF-8 text, one command per line, `\n` terminated (accept `\r\n` on read). stdin and stdout are the only
  channel. stderr is not part of the contract (D: drained and logged, never parsed).
- Tokens are separated by ASCII spaces; the stand-in uses whitespace split (H: lobby_harness:494), so extra
  spaces are tolerated on read. Emit single spaces.
- Every stdout line that does not start with `@` is engine console / host log (H: reference game server header:21). The lobby copies
  it into its log only with `--dserver-verbose` (H: argparse help) and never parses it.

## 2. Launch

```
<dserver-exe> -port <udp> -world <WorldPath> -guid <ca|legacy> -idle <secs> [-log <file>] [<extra args>]
```

| arg | type | meaning | tag |
|---|---|---|---|
| `-port` | u16 | UDP game port the server binds (loopback unless `-bindip`) | H (header usage; the stand-in reads it, lobby_harness:489) |
| `-world` | path | world to load; derived from the room map, or `--dserver-world` for every room | H (header usage, lobby flag) |
| `-guid` | `ca` (the legacy alternative token is not used by DeCombat) | network identity to present; `ca` = the real 2009 client | H (header usage; `--dserver-guid` default `ca`) |
| `-idle` | u32 seconds, 0 = never stop | stop after this long with no clients (ALIGNED 2026-10-24, B1: a negative value is a usage error of the server, exit 64 with `@end`, RUN; the LOBBY must clamp `--dserver-idle` to >= 0, small lobby change, not yet made) | H (header usage; `--dserver-idle` default 60) |
| `-game -fps -maxplayers -name -bindip -netfmt` | various | engine host options named in the header usage | H (names), header defaults |
| `-bindip` (B2, READ) | address | the server binds 127.0.0.1 unless given. The LOBBY must pass `-bindip` when the advertised game address (`--gameip`) is not loopback, or the operator puts it in `--dserver-args`; today a LAN `--gameip` without it advertises an address nobody listens on (lobby change needed) | D |
| `-log` | file | host log file, placed in `--dserver-logdir` | H (header) |

Port rule (ALIGNED 2026-10-24, B3, READ): the lobby always passes a concrete `-port`; a server must bind exactly that port or fail with an
error line and `@end` (`dc-dserver` exits 3). `@ready port=` is informational only (the lobby ignores it and advertises the port it chose).

| extra args | string | `--dserver-args` split on whitespace and appended | H (argparse help) |

Environment:

- `CA_ROM` (D): path of the read-only 2009 client folder. The lobby exports it to the child unchanged so the server can
  read world and game tables at run time. The stand-in ignores the environment. A server that needs it and does not
  find it prints one error line, then `@end`, and exits non-zero.
- Working directory (D): `--dserver-logdir` if given, else a per-room temp directory (C4, READ: the lobby creates `%TEMP%\dc-lobby-dserver-<pid>-<game>`
and does not yet remove it; the contract is: removed when the game is reaped, a lobby task).
- The harness may point `--dserver-exe` at a `.py` file (H: comment above FAKE_DSERVER, lobby_harness:480-484:
  "runs a .py --dserver-exe with the same Python"). A Rust lobby that must pass the persist suite launches a path ending
  in `.py` through a Python interpreter (`CA_PYTHON` if set, else `python` from PATH) and execs any other path directly.

Port choice (D; DC-DEFAULT: CD-10 for the long-term range): one free UDP port per room, passed as `-port` and
advertised to clients in the game-start packet. The harness asserts the advertised port equals `--gameport` when
`--port-per-room` is off (H: lobby_harness:541-545 passes `--gameport`, 603 checks the packet field at offset 0x28).
"Free" means a bind on 127.0.0.1 succeeds; a 0.0.0.0-only probe misses a loopback holder on Windows (H: docstring of
`test_stale_port`, lobby_harness:445-447). A stale child of our own whose match is over is killed and its port
reused; a foreign holder is skipped and another port is chosen.

## 3. Lobby -> server lines

### 3.1 `user` (one per expected player)

```
user <uid> <sid> <key> <team> <exp> <nick>
```

| # | name | type | meaning | tag |
|---|---|---|---|---|
| 1 | uid | u32 | account/user number; echoed in `@join` and `@player` | H (stand-in reads `u[1]`, lines 507, 513) |
| 2 | sid | u64 | session/connection token of the player, opaque to the server (ALIGNED 2026-10-24: both implementations carry a u64, A3, READ) | H as a position (echoed as `u[2]`, line 502); meaning D |
| 3 | key | u32 | per-room entry key the lobby generated for the player; RESERVED: no server checks it (ALIGNED 2026-10-24, A2, TESTED) | H as a position (echoed as `u[3]`); meaning D |
| 4 | team | 0 or 1 | 0 = Alpha, 1 = Bravo (0-based here; `@player team` is 1-based) | H (`u[4]=='0'` is Alpha, line 512) |
| 5 | exp | u32 | lobby experience at entry, informational | H as a position (header usage); use D |
| 6 | nick | string, no spaces | display name; the lobby substitutes `_` for spaces | H (`u[6]` becomes `name=`, line 514) |

Fields 1-6 are the whole line the harness feeds. Anything else a design wants (loadout summary, clan tag, country, rank
icon index) is **O** and goes as trailing `k=v` tokens AFTER field 6, never inserted earlier, so a consumer indexing
positions 1-6 keeps working (D). The tokens in use (`loadout=`, `face=`, `mod=`, and the reader's `char=`, `slot=`, `model=`, `skin=`) are listed in
sections 11 and 12.A (D, TESTED on both sides).
Semantics (D): announces an expected player. ALIGNED 2026-10-24 (A2, TESTED): the lobby sends a random `key` per player, but the server
never reads it: it identifies a connecting client by the NAME in the client's hello against the roster nick (`_` for spaces) and refuses
unknown names unless started with `-open`. Name matching is the de-facto contract; `key` stays reserved until a capture shows what
the 2009 client presents on connect (OPEN, needs a capture; a server that wants key checking must define it as a new DeCombat rule). Late `user` lines (rejoin) are accepted any time before `quit`. The stand-in restarts its 1.5 s settle timer on
each `user` line (line 507).

Seat ticket (D; DC-DEFAULT: LG-327): the Rust lobby adds `ticket=<32 lower-case hex>`. With the token, a server seats that name only
for a hello with this ticket (`dc_wire::seatticket`), which replaces the name-only rule above for that entry. Without it the name-only rule holds. The
value is a secret: neither side logs it.

### 3.2 `room` (match settings)

```
room key=<mode> type=<n> goal=<n> ff=0|1 time=<s> train=<s> exp=<..> gp=<..>
```

H: only the reference game server header:11-12 header lists these. The stand-in ignores `room` (it handles `user` and `quit` only,
lines 505-510), so no harness assertion depends on it. Types and meanings below are our reading, not proof.

| key | type | meaning (our words) | tag |
|---|---|---|---|
| `key` | token | mode code from the ROM mode table (the stand-in's result line uses `TDM`) | H header, `@result mode=TDM` |
| `type` | u32 | mode variant number | H header; meaning D |
| `goal` | u32 | score / kill / round target (the stand-in's TDM is documented as "goal 2", `limit=2` in `@result`) | H |
| `ff` | 0/1 | friendly fire off/on | H header |
| `time` | seconds | match time limit | H header |
| `train` | seconds | warm-up period before play | H header; meaning D |
| `exp` | comma list (7 fields, last = map factor) | scale data for exp; the server's `@player exp=` is advisory (4.4, CD-54 proposed) | H header; semantics D (CD-03); shape ALIGNED 2026-10-24 (A4) |
| `gp` | comma list (7 fields, last = map factor) | scale data for GP; same | H header; semantics D (CD-04); shape ALIGNED 2026-10-24 (A4) |

ALIGNED 2026-10-24 (A4, TESTED: the forwarded line equals `dc_bute::room_line_with_vfs` for the room settings): the line the lobby actually sends
carries, besides the keys above, `arms cap rec trig` and four display-only keys `cmt large obj radar` (O: UI resource paths with `|`
separators; the server ignores them), and `exp` / `gp` are 7-field comma lists whose LAST field is the map factor (meaning of the other
fields: OQ-2), not one number. Values never contain spaces, so whitespace tokenising stays valid. The server keeps every pair and uses only
`key` and `goal`.

Requested keys with NO evidence in allowed sources: `arms`, `cap`, `rec`, `trig`, clan `weight` / `alpha` / `bravo`.
The header does not list them and `reference lobby` is out of bounds for this task. They are **O, DeCombat design**: if the
Rust lobby wants them it defines them itself as further `k=v` tokens and servers ignore unknown keys. Proposed own
names (proposals, not evidence): `arms` = weapon restriction class, `cap` = player cap, `rec` = recommend flag,
`trig` = objective/trigger set id, `clanw` / `clana` / `clanb` = clan-match weight and the two clan ids. Confirm each
against the 2009 room-setup screen before use.
Parsing (D): tokens after `room` are `k=v`; unknown keys ignored; missing keys keep server defaults; a bad value is
logged and ignored, never fatal. The last `room` before match start wins.

### 3.3 `con <console string>`
H: reference game server header:13. Runs one engine console string; the rest of the line (spaces included) is the string. Operator
use only (D); not exercised by the harness.

### 3.4 `quit`
H: reference game server header:14; the stand-in exits 0 on it (lines 509-510). The server stops the world, prints `@end`, exits.
The lobby also treats closing stdin as quit (D) and force-kills after a 5 s grace period (D).

## 4. Server -> lobby events (stdout)

All event lines start with `@`. Parsing (D): whitespace split; first token is the event; unknown events and unknown
`k=v` keys are ignored and logged at debug level; a malformed known event is logged and skipped. Servers flush after
every line (the stand-in uses `flush=True`).

Seat refusal (D; DC-DEFAULT: LG-327): `@seat_refused <uid> noticket`, for a hello with a listed name, no ticket and that seat free. An older
lobby ignores it as an unknown event.

### 4.1 `@ready port=<p> world=<w>`
H: reference game server header:17, lobby_harness:490. World loaded; clients may connect. With `--dserver` the lobby sends the
game-start packet only after this (D). `port` informational; `world` display token. Wait bounded by `--dserver-timeout`
(default 30 s).

### 4.2 `@connect <idx>`
H: stand-in prints it before each `@join` (line 501). Not in the header. **O**: ignorable; do not depend on it.

### 4.3 `@join`
Header form (H): `@join <client_slot> <uid> <key> <team> <name>`. ALIGNED 2026-10-24 (B4, TESTED: the lobby logs `@join client N uid Some(uid)` for both players):
the real server sends the header form and its `<team>` is the 0-based roster team (like the `user` line), NOT the 1-based team of `@player`;
the lobby ignores it today.
Stand-in form (H, line 502): `@join <idx> <uid> <sid> <key> <team>` (five tokens after `@join`).
They disagree after token 2. Contract (D, tolerant): the lobby reads token 1 as `client_slot` and token 2 as `uid` and
ignores the rest. New servers send the header form. Meaning: that client entered the game. See OQ-1.

### 4.4 `@player` (per-player match result, before `@result`)

```
@player uid=<u32> kills=<n> deaths=<n> team=<1|2> exp=<n> gp=<n> tk=<n> hs=<n> name=<nick>
```
H: lines 511-515.

| key | type | meaning | tag |
|---|---|---|---|
| uid | u32 | player (matches the `user` line) | H |
| kills | u32 | kills this match; added to `characters.kills` | H |
| deaths | u32 | deaths this match; added to `characters.deaths` | H |
| team | 1 or 2 | 1-based: 1 = Alpha, 2 = Bravo (the stand-in adds 1 to the user-line team, line 513) | H |
| win | 0 or 1 | optional, Spy Hunt only (DSV-472, DeCombat): the server's verdict for this player; when present the lobby's win / loss and win bonus follow it instead of `@result win` (which is 0 then) | D |
| agent | 0 or 1 | optional, Spy Hunt only (DSV-472, DeCombat): the agent whose upload won; the lobby pays the CD-86 agent extra | D |
| exp | u32 | experience the server counted: ADVISORY (ALIGNED 2026-10-24, A1, TESTED); the lobby computes the payout itself (CD-03, CD-54 proposed); the real server omits it (DSV-7 proposed) | H (the old stand-in suite expected `characters.exp == 3`, line 593; with the recompute that no longer holds) |
| gp | u32 | GP the server counted: ADVISORY, same | H (old stand-in expectation GP 10000-3920+1, line 589) |
| tk | u32 | team kills | H as a key; storage D (no harness column) |
| hs | u32 | headshots; added to `characters.headshots` | H |
| name | string | nick, LAST key, runs to end of line | H |

ALIGNED 2026-10-24 (A1, TESTED: a stand-in reporting exp=3 gp=1 for Alpha is paid CD-03/CD-04 over the ROM map factor by the lobby;
e2e_dserver.rs `stand_in_result_win_split_advisory_numbers_and_exactly_once`): the lobby ALWAYS computes the payout (CD-54, proposed;
open question 1 of that row restated with a test). The reported numbers are advisory unless the operator switch `economy.trust_server_rewards`
is on (trusted-server mode), in which case they are added as given. A server may omit `exp`/`gp`. `@player` lines come only for players that
connected and were identified (C3, TESTED indirectly): a player who started in the lobby but never reached the game server gets no result
line, no reward and no match count; whether he counts a loss is a DeCombat rule (DC_DEFAULTS candidate).

### 4.5 `@result`

```
@result mode=<key> win=<team> alpha=<n> bravo=<n> limit=<n> rounds=<a>:<b>
```
H: line 516 (`mode=TDM win=1 alpha=2 bravo=0 limit=2 rounds=0:0`). `win` is on the same 1-based scale as `@player team`
(0 = draw is D; the stand-in only emits 1). `alpha` / `bravo` team scores; `limit` the goal that applied; `rounds` is
`alphaRounds:bravoRounds`. The older free-text form (`@result <text>`, header line 18) is accepted but only the key form
drives wins/losses (D).
`@result` is omitted when no player connected (C1, TESTED): `@end` alone then ends the match, the room goes back to waiting and nothing is stored.
Lobby effect (H, lines 592-596; ALIGNED 2026-10-24: exp/gp per the lobby's computation, A1): one DB transaction per match: per `@player` add exp/gp/kills/deaths/headshots,
`matches += 1`, `wins += 1` if the player's team equals `win` else `losses += 1`; insert exactly one `match_log` row;
then push new exp/GP to the player's client (the suite waits for message 0x3004 after the match, line 590; +6 exp / +0xa GP in the e2e run).
A draw (`win=0`) counts as neither a win nor a loss (TESTED).
Exactly-once (D): a second `@result` for the same game is ignored.
Forfeit and empty teams (observed, REFERENCE, S2C 2.1.3; DeCombat waits 10 s first, DSV-31, D, 12.C): when every player of one team has left a running round, the reference server ended the round at once with the remaining team as the winner; a `win` equal to that team (and the scores as they stood) is the natural `@result` of that case; when no player is left the match ends as a draw or without `@result` (C1). The values and delays are reference stack behaviour, not rules.

### 4.6 `@leave <client_slot>`
H: header line 19; the stand-in prints one per client (line 519). The client left. PROVEN-CAPTURE (tdm1, tdm2, tdm4): the real client announces a deliberate leave with `UP_LEAVE_MATCH` 0xbd and sends `C2S_BYE` 15-16 ms later (first leave), or sends no `C2S_BYE` at all and only the plain transport `CTL_BYE` x 4 when it is back in the waiting room, 7-9 s later (second leave, round already over); after `C2S_BYE` it sends nothing on the link. A server therefore emits `@leave` at the 0xbd (DeCombat: remove at once) and must not wait for the transport bye or a link timeout.

### 4.7 `@end`
H: header line 20; stand-in's last line. The server is stopping. ALIGNED 2026-10-24 (A5, TESTED for the lobby side, stale_port.rs `our_own_finished_server_is_stopped_and_its_port_reused`):
a server MUST exit by itself after `@end` (dc-dserver does within 1 s; since slice 4 it prints `@end` only after the last client left, or 120 s after the result screen with `-linger`, and sends no server bye, 12.C, D, RUN). The lobby marks the room over and closes
stdin; it reaps the process on stdout EOF / exit, and stops a lingering one when its port is needed for a later start (stale-port path) or
after `quit` plus the grace. It does NOT schedule a reap after `@end` today: a stand-in that prints `@end` and keeps running holds its port
until the next start. Proposed lobby task (OPEN): one deferred reap in `close_game` after `@end` plus the grace. `quit` followed by stdin EOF
is one stop (C5, RUN).

### 4.8 `@crash`, `@teamswap`
Named by the task brief, absent from the header and from every harness script: no allowed-source evidence for their
fields. **O, DeCombat design** proposals: `@crash <free text>` (abnormal termination; lobby handles like process exit
without `@end`) and `@teamswap <uid> <newteam>` (mid-match change, 1-based). The lobby tolerates their absence and
ignores them if not implemented.

## 5. Event order (H from the stand-in, otherwise D)

1. Process starts, prints `@ready` at once (stand-in) / after world load (real server).
2. Lobby may already have written `user` lines; the stand-in collects them in a reader thread.
3. Stand-in waits until at least one `user` arrived and 1.5 s passed since the last (lines 498-499). A real server
   reacts to real client connections instead.
4. Per player: `@connect i`, `@join i ...`.
5. 0.5 s later: one `@player` per player, then `@result`.
6. 0.3 s later: `@leave i` for each, then `@end`; process exits 0.

The lobby accepts any interleaving and must not require `@connect` or `@leave` before `@result`. With `dc-dserver` (12.C, D) `@player` / `@result` come in the tick of the round end and
`@leave` / `@end` only when the clients leave the result screen, so `@end` follows `@result` by at least the 6 s of the end set (RUN: 40 s in the slice-4 round 3, where the remaining bot stayed on the result screen). Test timing (H): the
persist suite runs the lobby with `--countdown 0.05` and waits up to 15 s for the game-start packet and the post-match
push; the stand-in match takes about 3 s from the last `user` line.

## 6. Timers

| timer | default | source |
|---|---|---|
| spawn -> `@ready` | 30 s | H `--dserver-timeout` |
| idle with zero clients | 60 s | H `--dserver-idle`; enforced by the server through `-idle` (it prints `@end`) |
| grace after `quit` / stdin close before kill | 5 s | D |
| stale port: kill our own finished child on the next port request | immediate | H (test_stale_port) |
| server: team empty -> round end | 10 s (`-teamempty`) | D, DSV-31 (12.D) |
| server: end-set pauses (0x46 -> 0x47 ... -> 0x3f) | 3 s + 3 s (`-holds`) | D, DSV-35 |
| server: `@end` at the latest after the result screen | 120 s (`-linger`, 0 = for ever) | D, DSV-35 |

## 7. Error handling (D unless marked)

- `@end` before `@ready` = start failed (C2, TESTED, e2e_dserver.rs `a_server_that_cannot_load_its_world_puts_the_room_back`): a server that cannot
  load its world prints an error line and `@end` and exits 2; the lobby treats it as an ordinary end before ready: no 0x3043, room waiting, nothing stored.
- Spawn failure: log; players get the same outcome as a failed game join, room back to waiting
  (H: main suite, lobby_harness around "game join fails -> both report back; room returns to waiting").
- No `@ready` in time: kill the child; same handling.
- EOF on stdout before `@end`: crash. Players go back to the room, room back to waiting, nothing stored (no partial
  `match_log`). Log the exit code.
- Exit after `@result` without `@end`: results kept, room over.
- Malformed event line: log first 200 bytes, skip, keep reading.
- Pipe write error: same as crash.
- Lobby death must not orphan the child: every server implementation exits when stdin reaches EOF (the stand-in does
  not; it relies on `quit` or process kill).
- UTF-8 only; invalid sequences are replaced, never fatal. The harness runs the lobby with a cp1252 console encoding
  and checks a Korean chat line does not take it down (H: `main`, lobby_harness:802-838 region).

## 8. Open questions

- OQ-1: `@join` layouts differ (header vs stand-in). Recommendation: header form for new servers, lobby parses tokens
  1-2 only.
- OQ-2: numeric type of `room exp` / `gp` (scale float vs integer) and exact meaning of `type` / `train` need the 2009
  room-setup screen and ROM mode table; not derivable from allowed text.
- OQ-3: `arms cap rec trig clanw/clana/clanb` have no allowed evidence (section 3.2).
- OQ-4: harness uses `--gameport` 27888 by default; CD-10 says the DeCombat range is 29901-29999. Keep 27888 as the lobby
  default until the harness changes.

## 9. Fold record: end-to-end findings (spec-fold-3, 2026-10-24)

Source: staged addendum `DSERVER_PIPE_addendum_e2e.md` (branch `lobby-e2e`, commits be9c5c9..5de89be; tests in
`crates/dc-lobby-server/tests/` e2e_dserver.rs, stale_port.rs, world_for_room.rs; lines cited there referred to the copy at 59826e4).
"Lobby" = `dc-lobby-server`, "server" = `dc-dserver`. CD-54 stays PROPOSED; its row in DC_DEFAULTS is not edited here.

| item | resolution | which side changes | state |
|---|---|---|---|
| A1 exp/gp advisory | 4.4 and 4.5 rewritten; trusted-server mode is an operator switch | none (both already do it); the spec was wrong | ALIGNED 2026-10-24 |
| A2 `key` unused | 3.1 says the server identifies by hello name; `key` reserved | none; the capture of the client's connect was taken (tdm1, section 10 D1): nothing in the client's traffic can carry `key` | ALIGNED 2026-10-24; key stays reserved (closed by capture, section 10) |
| A3 `sid` u64 | 3.1 row 2 | none | ALIGNED 2026-10-24 |
| A4 room keys | 3.2 lists `arms cap rec trig` and `cmt large obj radar`; `exp`/`gp` comma lists | none | ALIGNED 2026-10-24; field meanings OQ-2 |
| A5 `@end` / reap | 4.7: server must exit after `@end`; lobby reaps on exit or when the port is needed | lobby: add a deferred reap after `@end` (later task) | spec ALIGNED; lobby reap OPEN |
| B1 `-idle` | section 2: u32, 0 = never | lobby: clamp to >= 0 | spec ALIGNED; lobby fix OPEN |
| B2 `-bindip` | section 2 | lobby: pass `-bindip` for a non-loopback `--gameip` | OPEN (lobby) |
| B3 port | section 2: lobby passes a concrete port, server binds exactly it | none | ALIGNED 2026-10-24 |
| B4 `@join` team base | 4.3: 0-based | none (the lobby ignores it) | ALIGNED 2026-10-24 |
| C1 no `@result` without players | 4.5 | none | ALIGNED 2026-10-24 |
| C2 `@end` before `@ready` | section 7 | none | ALIGNED 2026-10-24 |
| C3 unidentified players | 4.4 | none; the loss-counting rule is a DC_DEFAULTS candidate | ALIGNED 2026-10-24; rule OPEN |
| C4 temp directory | section 2 working directory | lobby: remove on reap | OPEN (lobby) |
| C5 `quit` + EOF | 4.7 | none | ALIGNED 2026-10-24 |

Section D of the addendum (checks in agreement: launch line, port choice, `CA_ROM`, user lines, last `room` wins, `quit` path, the 0x3043 push
only after `@ready`, one transaction per result, `@leave` per client, `-idle 2` ending the match) is recorded here as TESTED agreement with
sections 2, 3.1, 3.2, 3.4, 4.1, 4.4-4.7 and 6; no text change was needed. Not re-run in this fold (evidence quoted from the addendum).

## 10. Fold record: capture findings (spec-fold-4, 2026-10-25)

Source: `CAPTURE_tdm1_TRANSPORT_VALIDATION.md` (hello), `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md` sections 2-4, `CAPTURE_tdm4_KILL_VALIDATION.md` sections 6-8, `REFERENCE_FINDINGS_botduel.md` section 5 (evidence records, kept in full).
Grades: PROVEN-CAPTURE = the real client's own bytes or reaction; REFERENCE = what the reference stack did (never a rule). No line of the pipe contract is exercised by the real client itself
(the pipe is between our lobby and our server); these items are the client-visible facts the contract must respect.

| item | finding | grade | consequence for this contract | state |
|---|---|---|---|---|
| D1 identification and `key` | the hello's second string is a 36-character upper-case GUID text that no lobby frame carries (so the lobby cannot hand it to the server as `key`); the UTF-16 name in the hello equals the login name, and the 0x72 message carries `room_slot`, `char_no` and `session_idx` that equal the lobby's values (`UDP_SESSION_2009_A.md` 5.2, `GAME_MESSAGES_2009_C2S.md` 5.1.1) | PROVEN-CAPTURE | the server identifies by the hello name and may cross-check `char_no` / `room_slot` (3.1); `key` stays reserved and is not derivable from the client | closed |
| D2 leave timing | see 4.6 | PROVEN-CAPTURE | `@leave` at the 0xbd; do not wait for the transport bye | text only; whether `dc-dserver` removes at 0xbd was not re-checked here (C2S 5.7 states the rule) |
| D3 lobby 0x394 | the client sends 0x394 {0} on every leave of a running match, in the same millisecond as 0xbd (`PROTOCOL_2009.md` 3.7, 7.1) | PROVEN-CAPTURE | the lobby (not this pipe) must read it as "left the match" and put the member back to the waiting room | OPEN (lobby side) |
| D4 forfeit result | see 4.5 | REFERENCE | `@result win=<remaining team>` is a reasonable default | spec note, no code change required |
| D5 reward push | the reference lobby pushed 0x3004 (and 0x345) only to accounts still in the match, in the tick of the game server's 0x65; none to the leaver (`PROTOCOL_2009.md` 7.1) | REFERENCE | the lobby computes and pushes after `@result` (4.5) to every account with an `@player` line; a leaver has none (C3) | consistent, no change |
| D6 join of a running match | a re-join 15 s after leaving was answered in the same millisecond by 0x393 ok, 0x3042 {id, 3} and 0x3043; the server saw a fresh hello and 0x72 byte-identical to the first (tdm2) | PROVEN-CAPTURE (client), REFERENCE (lobby) | the server must accept a second hello of a known identity after the first link ended (the player stays in its `user` set; whether the lobby sends a fresh `user` line is not visible in the captures) | consistent, no change |

No ungraded item is added by this fold. Not exercised by any capture: `-idle`, `-bindip`, `@crash`, `@teamswap`, the `con` line.

## 11. Addendum: `user` line trailing tokens (lobby-loadout, 2026-10-25, LDO-1 proposed)

Evidence: `crates/dc-lobby-server/tests/loadout_tokens.rs` (TESTED with a stand-in server recording stdin). Tokens are the
`k=v` extension of §3.1; receivers treat them as an unordered set (last wins) and ignore unknown keys.

```
user <uid> <sid> <key> <team 0|1> <exp> <nick> [loadout=<slot>:<item>,...] [face=<n>] [mod=1]
```

| token | sent when | value | notes |
|---|---|---|---|
| `loadout` | at least one item sits in an equipment slot 0..3 | comma list of `<slot>:<item_no>`, ascending slot; `slot` = the lobby's equipment slot (0x323 slot view), `item_no` = ROM item number | NOT weapon-table indices: the server maps item numbers to its own tables. A server that only understands the older plain list (`4,0,7`) reads this as an empty list (dc-dserver slice 3 did; slice 4 must parse `slot:item`). |
| `face` | always while lobby config `dserver.loadout_tokens` is true | stored face index (0-based) | |
| `mod` | the account is a GM and `gm.flag_moderators_to_game` | `1` | CD-64, unchanged |
| `char`, `skin`, `model` | NOT sent | | the lobby's character record holds no class, body or skin field; `char=` is the server's `char_no` check (DSV-19) and the lobby's char_no is the account number, so it stays out until a capture shows what the client presents |

Order emitted: `loadout`, `face`, `mod`. Switch: lobby.json `dserver.loadout_tokens` (default true; false = the bare 7-field
line of slice 1). Open: the item-number to weapon mapping and the default weapon when `loadout` is absent belong to the
server (dserver-slice4).

## 12. Addendum: dserver-slice4 (2026-10-25, DSV-30..38 proposed; tags D/O as §0)

Folded from the staged dserver-slice4 addendum. The `user` token table of §11 is the lobby side of §12.A; §12.A adds the
reader's view (slot ranges, gear encoding by cshell+0x25c010, bare-number compatibility).

### 12.A §3.1 `user` line: trailing tokens (agreed with the lobby-loadout worker)

The lobby-loadout worker's branch (`lobby-loadout`, `dc-lobby-server/src/game.rs` `loadout_tokens`,
LDO-1 proposed) appends, after field 6 and before the `mod=` token:

```
user <uid> <sid> <key> <team> <exp> <nick> [loadout=<slot>:<item>,<slot>:<item>,...] [face=<n>] [mod=1]
```

| token | form | meaning | reader (dc-dserver) | tag |
|---|---|---|---|---|
| `loadout=` | comma list of `<slot>:<item>`, slots ascending, absent when nothing is equipped | the account's equipped items: `<slot>` = the equipment slot of the 0x323 record (0..3 weapon slots; 4..9, 14 gear and backpack), `<item>` = the ROM item number the lobby stores (weapons below 1000 are weapon rows, gear `1000 x k + ItemID`, cshell+0x25c010) | weapons by slot (first claim of a slot wins), gear items for the armour (DSV-30, DSV-38); unknown slots / item kinds ignored; a bare number without `:` is the slice-2 form (weapon row, next free slot) | D |
| `face=` | `<n>` | the stored face index = the `[Face<n>]` row of the ROM model table | character face row (DSV-24) | D |
| `model=`, `skin=` | `<n>` | body row and skin variant | DSV-24 (not sent by the lobby yet: no character class field) | D |
| `char=`, `slot=` | `<n>` | identity check against `UP_PLAYER_INFO` | DSV-19 | D |
| `mod=1` | literal | game moderator | CD-64 | D |

Rules (D): tokens are `k=v` without spaces; the last token of a key wins; the dserver ignores unknown keys and bad
values (logged). With no weapon item the dserver uses its operator default, then the ROM default-issue weapons
(DSV-38), so a player never spawns unarmed.

### 12.B New §3.5 `gain <uid> <exp> <gp>` (lobby -> server, O)

```
gain <uid> <exp_gain> <gp_gain>
```

Sent by a lobby after it stored a `@result` (CD-54: the lobby computes the rewards), one line per `@player` account,
with the experience and GP it added. The dserver shows them in the result screen's `GS_PLAYER_SUMMARY` 0x65 when the
line arrives before the summary (DSV-35: 3 s after `@result`); later lines are kept but have no effect. Without the
line the summary shows 0 gains (TODO in the code, `TODO_SUMMARY_GAINS`). A server that does not know the line logs it
as malformed and ignores it (§7). Values are u32; a negative gain is sent as 0. **Lobby side: implemented 2026-10-25 (config-combat, `dc-lobby-server/src/game.rs::finish`, switch `dserver.send_gain_lines`, TESTED with the stand-in server; end to end with dc-dserver not yet run).** **Verified end to end since 2026-10-25 (stack-verify, release Rust stack of master 692de66, two rounds; RUN):**
the lobby wrote one `gain` line per account 14 ms after it stored the `@result` (round 1: 1001 +23 exp / +24 GP, 1002 +22 / +23; round 2: 1001 +14 / +16, 1002 +28 / +29), the server's
0x65 summaries 3 s later carried exactly these gains for the matching character objects, and `lobby.db` (`match_log`, character totals) agrees. The amounts are DeCombat lobby
rules (CD-54), not 2009 evidence; the 2009 client's display of them is not captured.
(a small addition next to the 0x3004 push in `game.rs::finish`).

### 12.C §4 / §5 event timing changes (D)

- `@player` / `@result` go out with `GS_ROUND_RESULT` 0x46 in the tick the round ends (goal, timer, or DSV-31 team
  empty), unchanged in form.
- The server no longer ends the session a fixed 10 s after the round end (slice 2's DSV-13 holds with 0x99): it keeps
  every game link until its client leaves (no server bye, as the reference stack did) and prints `@leave` per client, then
  `@end` once the last client has left, or 120 s after the result screen at the latest (`-linger`). A lobby that
  waits for `@end` to put the room back to waiting therefore waits until the clients have left the result screen.
- Clients that leave before the result screen do not shorten the end set for the others; when every client is gone
  the session ends at once.

### 12.D §2 launch: new dc-dserver options (D)

`-deathstates reference stack|spec`, `-respawnwait <ms>`, `-respawnfallback <ms>`, `-combat key=value[,..]`, `-linger <secs>`,
`-teamempty <secs>`, `-holds <first_ms>,<second_ms>` (now the two end-set pauses), `-no9d`,
`-worldobjects off|mode|all`, `-zoneclass own|room`. A lobby passes them through `--dserver-args`.

## 13. Fold record (spec-fold-5, 2026-10-25)

Checked against the staged files `staging/lobby-loadout_DSERVER_PIPE.md` and `dserver-slice4_DSERVER_PIPE.md`: every table row and rule of both is present in sections 11
and 12 (the coordinator's fold, master 2dff5fe / e191a26); no text was missing. This fold only linked them from the base sections (3.1, 4.5, 4.7, 5, 6) and graded the timing statements D.

| item | state on master e191a26 | grade |
|---|---|---|
| `loadout=` / `face=` written by the lobby | done (`dc-lobby-server` `game.rs::loadout_tokens`) | D, TESTED (stand-in server) |
| `loadout=` read by the server (`slot:item`, bare list compatibility) | done (`dc-dserver` slice 4) | D, TESTED; RUN only with the default-issue path |
| `gain <uid> <exp> <gp>` (12.B) | server reads it; **the lobby does not send it yet**, so 0x65 shows 0 gains | D; OPEN (lobby) |
| `@end` after the clients leave, `-linger` (12.C) | done (server); the lobby waits for `@end` before the room goes back to waiting | D, RUN |
| `char=` / `skin=` / `model=` from the lobby | not sent (no class or skin field in the lobby's character record) | OPEN (needs a capture of what 0x72 carries for a chosen character) |
| keys in `server.toml` | `dserver.loadout_tokens`, `auth.first_account_no`, `room.settings_change_clears_ready` are lobby.json only; `combat.*` exists only as `-combat k=v` (dc-config denies unknown fields) | OPEN (dc-config schema) |

## 14. Fold record (spec-fold-6, 2026-10-25)

Sources: `reports/stack-verify.md` (release Rust stack of master 692de66, two loopback rounds, `lobby.log` and `lobby.db` read by that worker), the
config-combat merge recorded in `DC_DEFAULTS.md` (GAIN-1 and the config-layering note). Updates of the section 13 rows (that table is kept as the spec-fold-5 record):

| item (section 13 row) | state on master 96983bf | grade |
|---|---|---|
| `gain <uid> <exp> <gp>` (12.B) | the lobby sends it (GAIN-1, `dserver.send_gain_lines`, config-combat); **verified end to end**: gain lines 14 ms after the stored `@result`, the 0x65 summaries 3 s later carry the same gains, `lobby.db` agrees (12.B) | D; TESTED (stand-in) and RUN (end to end) |
| keys in `server.toml` | the lobby keys and `[combat]` are read from `server.toml` / env over lobby.json since config-combat (DC_DEFAULTS layering note: CLI > env > server.toml > lobby.json > default) | D; closed |
| `@end` after the clients leave | RUN again in both stack-verify rounds (`@leave` per client, `@end`, server exit code 0, no leftover process or port) | D, RUN |
| `char=` / `skin=` / `model=` | unchanged: not sent | OPEN |

Not tested by stack-verify: a graceful lobby stop (the lobby was stopped by PID) and the x86_64 build.
