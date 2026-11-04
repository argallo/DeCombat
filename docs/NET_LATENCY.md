# NET_LATENCY: where the felt latency goes, and what we can cut (2026-10-30)

Setting: dc-lobby-server and dc-dserver on a VPS in the US Midwest, the player on the US west coast on Ethernet.
ICMP round trip PC to VPS: min 57, avg 61, max 68 ms, 0 % loss. In-game ping shown: about 65 ms.
This is an assessment only; no product code was changed.

## How it was measured

- Loopback on the dev PC, release x86_64 builds of master 3e4f7c62: dc-dserver on the JUNK_FLEA world, 4 dc-botclient
  hunters/shooters and one headless dc-client (offscreen, 8 ms frames, `--net-log`), all through a UDP delay proxy
  (our own script, 30 ms each way +- 4 ms uniform jitter, so a 52..68 ms path like the real one; the proxy's own
  scheduling error was 0.04 ms mean, 0.11 ms p99). 60 s per run, ticks 10, 20, 30, 60; plus one 10 Hz run with 2 bots
  and no jitter. Sources: the dserver `link 10 s` and `link summary` lines, the client `remote_interp` events, and
  the dserver's process CPU time over the last 50 s of each run.
- The VPS tick: the systemd unit passes only `-bindip 0.0.0.0` and the VPS server.toml / env set no `[dserver]
  tick_hz`, so the lobby passes no `-tickhz` and the VPS runs the built-in default of **10 Hz** (CD-63).

## Measured numbers (30 ms each way, +-4 ms jitter, 5 players)

| tick | dserver CPU (one desktop core) | stream gap seen by client | playout delay (client) | server rtt window mean | datagrams out per client |
|---:|---:|---:|---:|---:|---:|
| 10 Hz | 0.66 % | 99.7 ms | 150.1 ms | 60.6 ms | 36 /s |
| 20 Hz | 0.72 % | 50.2 ms | 74.7 ms | 60.9 ms | 53 /s |
| 30 Hz | 0.66 % | 33.3 ms | 50.3 ms | 61.8 ms | 70 /s |
| 60 Hz | 1.31 % | 16.3 ms | 50.0 ms | 60.9 ms | 121 /s |

Input (move updates) arrived at 13.3..14.6 per second per client, longest gap 74..88 ms, in every run. No holds and
no late frames in any run. Jitter q95 seen by the client: 8 ms (2 ms without proxy jitter).
Ping overhead of our stack: with an exact 60.0 ms path the server's round trip read 60.3 ms (window min 60, max 64).
Per window the headless dc-client read about 1 ms more than the bots (61..62 vs 60..61): its network thread shares
a lock with the frame loop (likely cause, not traced). One window per run can show a start-up outlier (max 254 / 730 ms in the window that
covers the world load); it lifts that window's mean only.

## Latency budget, US Midwest VPS, US west coast player (stage by stage)

| # | stage | added delay now (10 Hz) | at 30 Hz | 2009 or ours |
|---|---|---:|---:|---|
| 1a | own movement and aim on own screen | 0 (local simulation) | 0 | 2009: the client moves itself, the server only corrects (C2S spec 2.1, MOVEMENT_2009) |
| 1b | own move state to the wire | avg 33, 0..67 (15 per second, frame-bound) | same | 2009: 15 updates per second, moving or standing (C2S spec 2.1, captures); our network thread polls every 1 ms (CLI-60) |
| 1c | own shot to the wire | 1..2 (next network poll) | same | 2009: one fire message per pellet from the fire routine (C2S spec 4.1); no coalescing in our link, a queued message leaves on the next poll |
| 2 | path client to server | 30 | 30 | physics (half of the 60 ms round trip) |
| 3a | server receive to processing | <= 1 (1 ms reader wait, handled on arrival) | same | ours |
| 3b | wait for the next world tick before others see it | avg 50, 0..100 | avg 17, 0..33 | ours: CD-63 default 10 Hz; 2009 has no fixed server rate and its client expects 60 per second (SESSION_B 9.1) |
| 4 | path server to other client | 30 | 30 | physics |
| 5 | playout (interpolation) delay | 150 measured | 50 measured | ours: CLI-11 (1.5 x stream interval, 50..250 ms) and CLI-1330 (jitter-sized, server-clock stamps) |
| 6 | frame and present | about one frame (not measured) | same | ours (frame cap 1000 fps in match) |
| | **a remote player's move on your screen (sum of 1b..5)** | **avg about 293 ms** (210..377) | **avg about 160 ms** | |
| 7 | your hit confirmation (hit sound, name tag flash) | 1 round trip + 1..2 = about 62 | same | the server relays shot records the moment it judges the claim, not at the tick (CLI-98 sound on the server's record) | **Since 2026-10-31 (CLI-1603) the hit sound, the tag flash and the blood play at the shot from the client's own hit test; the record only confirms.**
| 8 | damage and kills on the victim | one-way + one-way = about 60 | same | judged on arrival, sent at once |

Hit registration: the 2009 client decides hits itself; each fire message carries the hit object, point and ray
(C2S spec 4, "hit detection is client-side"). Our server checks every claim (origin, box with 48 units tolerance,
wall trace, cone) and has a bounded rewind of 200 ms of the victim's reported path (DSV-42, our rule). The age of
the victim position the shooter aimed at, on the server clock, is stage 3b + 4 + 5 + 2: about 260 ms at 10 Hz
(up to 310), so at 10 Hz the 200 ms rewind is too short and the 48-unit tolerance carries the rest (a runner moves
about 17..31 units in 60..110 ms at 285 units per second; a sprinter can fall out). At 30 Hz the age is about
127 ms (up to 143) and the rewind covers it.

Displayed ping (DSV-410, PD-S2C-4): the server sends the 0xa2 table every 5 s; each value is the transport's mean of
the last 16 reliable-block round trips (first send to ack, UDP_TRANSPORT 12.1, 2009 rule). Acks go out on the next
poll on both sides, not at the tick, so tick quantisation is not in the ping. Our stack adds about 0.3..1 ms. The
65 vs 61 ms gap is therefore mostly the path itself (game traffic under load, the 16-sample mean catching the
57..68 spread, any resent block counted from its first send). The displayed ping is honest; it cannot be lowered
by netcode, only by a shorter path.

## What cannot be fixed

US west coast to the Midwest is about 3,000 km in a straight line: 30 ms round trip at the speed of light in fibre,
about 45 ms on real fibre routes, 57..68 ms on this ISP path today (general knowledge, labelled as such). Any server
in Chicago keeps the SF player one round trip (about 60 ms) behind the server, and every remote player he sees at
least one path length plus an interpolation buffer behind real time. Only moving the server changes that number.

## Options, ranked by felt gain per risk

1. **Raise the dserver tick to 30 Hz on the VPS (config only).** Add `tick_hz = 30` under `[dserver]` in
   /opt/decombat/config/server.toml (or `DC_DSERVER__TICK_HZ=30` in decombat.env) and restart the unit. Gain: a
   remote player's motion shows about 133 ms sooner (293 to 160 ms avg: tick wait -33, playout -100); the rewind
   window then covers the shooter's view. Cost: measured CPU unchanged (0.66 % of a desktop core with 5 players),
   datagrams out per client 36 to 70 per second (a few KB/s per player). Risk: low; the Rust client measured clean
   at 30 Hz here, the 2009 client accepts 30 Hz on the wire (CD-63 data point) and expects 60.
2. **Then 60 Hz** if the VPS CPU and traffic stay low: a further 8 ms (tick wait); playout stays at the 50 ms floor.
   Cost: CPU 1.31 % here, 121 datagrams per second per client (budget the VPS traffic quota). Low risk.
3. **Send the own move update faster or on change (client, ours).** The 2009 rate is 15 per second; sending at
   30 / 60 per second (or at once when the control word changes: start, stop, jump, crouch) cuts stage 1b from avg 33
   to 17 / 8 ms. Gain 17..25 ms, worth it only after option 1. Cost: 2..4 x the client-to-server datagrams; needs a
   DC_DEFAULTS row (departs from the 2009 rate) and a check of the server's move validation at the shorter dt.
4. **Lower the playout floor from 50 to about 30 ms (CLI-11 minimum, ours).** At 60 Hz the jitter-sized term is about
   29 ms (16.7 interval + 8 ms jitter q95 + 4), so the 50 ms floor costs about 20 ms; at 30 Hz the gain is about 5 ms.
   Risk: more holds on jittery links (alice's p95 round trip 276 ms); CLI-1330's jitter term still raises the delay
   there. Do it only with option 2, and check `remote_interp` holds per minute on the next WAN match.
5. **Server location (no code).** General knowledge, typical figures: a Silicon Valley VPS gives the SF player about
   3..8 ms RTT (Los Angeles about 10..15, Seattle about 20); an east-coast friend (New York area) has about 18..25 ms
   to Chicago, 65..75 ms to Silicon Valley, 35..45 ms to Dallas. Chicago: SF 60 + east 22; Silicon Valley: SF 5 +
   east 70. For a mixed SF / east-coast group Chicago or Dallas is the fair middle; for an SF-only group a west-coast
   VPS saves about 50 ms of round trip, all of it felt.
6. **Nothing to gain** in hit acks, shot sends or ping math: hit records, damage and shots already leave on arrival
   (not on the tick), and the ping is the real round trip.

Recommended now: option 1 (config change on the VPS, restart between matches), then read the next match's dserver
`link` lines and the client `remote_interp` lines with tools/net_report.py.

## 2026-10-31: the VPS at 30 Hz, the Mac over Wi-Fi

Option 1 was applied on the VPS on 2026-10-31 (`tick_hz = 30` under `[dserver]`, docs/VPS.md); every game server
since says `tick 30 Hz`. The first match from the MacBook felt worse, not better. Numbers (dserver_c1_r0_g1.log on
the VPS, the client's `remote_interp` lines in ~/Library/Logs/DeCombat/dc-client.net.jsonl):

| | Windows PC (Ethernet), earlier matches | MacBook (Wi-Fi 6E, -41 dBm), this match |
|---|---:|---:|
| round trip mean (server view) | 63 ms | 78 ms (min 64, max 114) |
| input jitter (server view) | 2.6..7 ms | 17.9 ms (16.7..23.2 in the other Mac matches) |
| resends | 0..2 of ~1000 blocks | 11 of 1189 |
| ICMP ping to the VPS, 20 probes | (57..68 in the first note) | min 60.5, avg 85.4, max 189.8, stddev 35.3 |
| playout delay (client) | | 53..62 ms (rule: 33 ms interval + lateness q95 4..19 ms + 4) |
| late frames (client) | 0 on loopback | 3..7 % of the frames, 30..100 late runs per minute per remote player |

The 30 Hz tick cut the delay from 150 to about 56 ms as predicted; on this link that delay is too short. The
lateness statistic was the 95th percentile of the last 64 samples (2 s at 30 Hz): a Wi-Fi spike beyond it came back
every 1..4 s, after the window had forgotten the previous one, and each one drew the remote body extrapolated for a
few frames and then corrected (CLI-1331): a hitch. At 10 Hz the 150 ms delay hid all of it, which is why 10 Hz felt
smoother on the Mac while being 130 ms more behind.

Change (CLI-1330 amended): the delay now covers the largest lateness of the last 256 samples (8.5 s at 30 Hz), the
clock map keeps its 64-sample minimum. Expected on this link: delay about 90 ms (33 + the spike + 4), no periodic
hitch; on the Ethernet PC nothing changes (lateness ~3 ms, the 50 ms floor wins). The honest fix for the Mac is a
cable: its round trip would read about 63 ms like the PC's and the delay 50 ms. To check after the next match:
`python3 tools/net_report.py ~/Library/Logs/DeCombat/dc-client.net.jsonl` (late/min should be near 0, "late max" is
the new column).

The same match's frame report: `Apple M4 Pro (Metal, IntegratedGpu) (Immediate), 104191 frames in 206 s = 505 fps`
on a 120 Hz panel, which is where the fan noise came from; see docs/MACOS.md (CLI-1575: Fifo by default on macOS).

### Second Mac match the same day: lost updates, not only late ones

With the 256-sample maximum the delay sat at 72..75 ms and no arrival was later than 35 ms, yet Bot1 was still drawn
late 84..120 times a minute (5..8 % of the frames; Bot2, mostly standing, 0). The user saw it as the running bots
looking blurry: at 120 Hz an extrapolate-then-correct every half second is a doubled body. Cause: Bot1 delivered 266
samples per 10 s where the 30 Hz tick sends 300, so a tenth of the updates never arrived (unreliable stream datagrams
are not resent), and after each missing one the next sample came a full 66 ms step after the previous; the rule
covered the mean interval (35 ms) plus the lateness, not the doubled step. The rule now covers the longest *wait*
of the window (arrival minus the previous sample's stamp: the step plus the lateness), so a loss raises the delay to
about 70 ms and a loss plus a spike to about 100. Waits above 250 ms are silences (death, culling) and are ignored;
they no longer enter the interval average either (before, a reappearing player got 1.5 x a 700 ms "interval" = the
250 ms cap, decaying over seconds). Verify with `late/min` in tools/net_report.py (target near 0) and
`late max` (now the longest wait beyond the mean interval).

## 2026-10-31: what shipped shooters do, and what we changed to match

Third Mac match of the day: 0 late frames at a 130 ms delay, yet the running bots had a rubber-band feel. The
remaining sources of drawn-speed wobble were in our own rules, and the fix follows what shipped engines do.

What the references say (read 2026-10-31; wiki pages, a blog series and secondary write-ups, no engine source):

- Valve, Source Multiplayer Networking and Interpolation (developer.valvesoftware.com): remote entities are drawn
  a fixed interpolation period behind, `cl_interp 0.1` = 100 ms by default, chosen so that two snapshots are
  always available and one lost packet does not stall the view; `cl_interp_ratio` 2 means two update intervals,
  players with loss raise it to 3 or 4; when snapshots run out the client extrapolates linearly for at most
  `cl_extrapolate_amount` 0.25 s; the server's lag compensation subtracts each client's interpolation delay so
  nobody leads their aim. The buffer is fixed, not reactive, and the client clock is corrected gradually.
- Gabriel Gambetta, Fast-Paced Multiplayer part III (gabrielgambetta.com/entity-interpolation.html): other
  players are shown "in the past", one update interval at least, interpolating only between real server data,
  never predicting; dead reckoning is for cars, not shooters, because speed and direction change abruptly.
- Overwatch (GDC 2017, Tim Ford; Edgegap's write-up): no snapping; when buffers run dry the server asks the
  client to simulate slightly faster for a while (time dilation), i.e. speed changes of a few percent, never a jump.
- Unreal's CharacterMovement network smoothing: a remote body's correction is spread over time (linear or
  exponential mesh offset), never applied as a step.
- Apex Legends (Respawn, Samy Duc's post, via DOTEsports / PC Gamer): 20 Hz snapshots are enough when the
  interpolation and lag compensation are right; 60 Hz would save about two frames of latency for 3x the cost.

Our rules measured against that:

| ours before today | the shipped practice | change (CLI-1330 / CLI-1331 amended) |
|---|---|---|
| delay 1.5 x interval, floor 50 ms (one and a half updates: one loss runs the buffer dry) | two intervals or more (Source ratio 2, Gambetta "at least one behind" plus a loss margin) | floor two intervals + 4 ms: 70 ms at 30 Hz |
| delay sized by the 95th percentile of lateness over 2 s | fixed buffers; adaptive ones grow on trouble and shrink slowly | the longest wait (step + lateness) of the last 8.5 s |
| clock map = minimum offset of the last 64 samples, a rise taken at once | the client clock is corrected gradually | 256-sample minimum, rises and drops both slewed at 10 % of real time |
| extrapolate 100 ms then ease back (unchanged) | Source extrapolates up to 250 ms | kept: a shooter's remote player rarely keeps a velocity for 250 ms |
| no measure of the drawn motion | engines expose smoothing stats | `remote_interp` reports speed_cv, backsteps_per_min, jumps_per_min; tools/net_report.py columns back/min, jump/min, speed cv |

Why each wobble mattered: a 64-sample window's minimum left the window every ~2 s on a jittery link, and every
change of the map stretched (rise) or squeezed (drop) one 33 ms segment by 20..30 ms, which is a visible speed
hitch twice a second on a running body; the 1.5 x floor made every single lost update a late frame (an
extrapolate-then-correct); and the reactive delay made the body run at 90 % speed for up to a second each time the
delay rose. The latency the user feels is unchanged by the clock map change and up by 20 ms on a clean link from
the floor (50 to 70 ms); on the Wi-Fi link the delay was already 70..130 ms.

To judge the next match: `python3 tools/net_report.py ~/Library/Logs/DeCombat/dc-client.net.jsonl`; back/min and
jump/min near 0 mean the drawn motion never reverses or leaps. Baseline on loopback (WARHEAD, 2 bots, 45 s):
back/min 0, jump/min 0, speed cv 0.48..0.55 for the hunting bot, which is the bot's own stop-turn-sprint rhythm
over a 10 s window, not netcode; compare the Wi-Fi match against that, not against 0. A remaining rubber-band feel
with back/min and jump/min at 0 and the cv near the baseline points at the bots' own movement (their evade and
route steps), not at the netcode.

## 2026-10-31, later: the stop-go stream (CLI-1577 / CLI-1578 / BOT-96) and the frame clock (CLI-1576)

Fourth and fifth Mac matches: the playout statistics were clean (0 late frames, 0 reversed steps, 0 jumps) and
the user still saw doubled runners, rubber banding and a jittery own body. Two causes, both found by measuring:

1. **Repeated positions.** A new `repeats` count in `remote_interp`: in a local match 151 of 301 stream samples
   of a running bot repeated the previous position while the velocity said it moved. Every player (bots and
   dc-client alike) reported 15 times a second, the server streams 30 times a second, so every other tick carried
   the old position. Linear interpolation between positions then moves the body for 33 ms and stands it for 33 ms:
   a 15 Hz stop-go that reads as blur and rubber banding at 120 Hz. The server's own `input 14.8/s` link lines had
   shown this rate all along. Fix at the source: dc-client and the bots send 30 updates a second (CLI-1578,
   BOT-96); and for the repeats that remain (a lost update) the client places the sample along the velocity and
   straightens it when the next reported position arrives (CLI-1577).
2. **Uneven CPU frames under vsync.** The exit reports showed `frame 0.67..88 ms` under Fifo: the swap chain
   blocks one frame and lets the next start at once, while the panel shows them evenly. Everything simulated or
   interpolated at the CPU time moved in uneven steps on an even display: the own body jittered. The frame clock
   (CLI-1576) predicts the presentation time and the own sim, the remote playout, effects and sounds use it.

What to read after the next match (tools/net_report.py and the client's exit report): `repeats` near 0 for every
player on a 30 Hz-reporting server, back/min and jump/min near 0, the exit report's raw short / long frame counts
small, and the dserver link lines reading `input 29.x/s`. The bots on the VPS must be rebuilt and restarted for
their 30 Hz (the lobby and the game server are unchanged).
