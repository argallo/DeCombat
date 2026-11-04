# Network lab

Bad real-world connections, reproduced on one PC before players meet them. Headless bots play real matches against a
local lobby and game server, each bot on its own impaired path (Wi-Fi bursts, a 4G hotspot, a player in Australia, a
campus firewall, a carrier NAT that changes the port, blackouts), and `dc-netlab` judges every player from what the
server and the bot saw. No admin rights, no OS tools, no game window: the impairment sits inside the process, around
the UDP socket, the same on Windows, Linux and macOS.

## Quick start

Build once (release, so a full room does not starve the machine):

```text
cargo build --release -p dc-netlab -p dc-botclient -p dc-dserver -p dc-lobby-server
```

Then, with your Combat Arms folder (the ROM) in `CA_ROM` or `--rom`:

```text
target/release/dc-netlab quick --rom <client folder> --parallel 2     every mode once, 16 bots a room (about 15 min)
target/release/dc-netlab soak  --rom <client folder> --parallel 2     every mode on 3 maps for 6 min, then 30 min mixed
target/release/dc-netlab campus --rom <client folder>                   one campus player, then 7 bots join the match
target/release/dc-netlab ci --direct                                   no ROM: a synthetic world (what CI runs)
target/release/dc-netlab run --rom <dir> --mix good:8,wifi:4,cgnat_rebind:4 --mode CTF --minutes 5
target/release/dc-netlab list --rom <dir>                              profiles, matrices, the ROM's modes and maps
```

Each run writes `netlab-out/<date>/report.md` and `report.json` (`--out` to choose), plus one folder per scenario
with every bot's event log (`*.events.jsonl`), report (`*.report.json`) and output, and the game servers' logs. Exit
status 0 when every scenario passed, 1 when one failed, 2 on a usage error.

Options: `--minutes <m>` per scenario, `--players <n>` per room (the mix is scaled, up to 16), `--parallel <n>` rooms
at once (one lobby, a game server per room), `--only <name,...>`, `--seed <n>` (every path is seeded: a run repeats),
`--dserver-args <text>` (passed to every game server), `--xfail <profile,...>` / `--xfail none`, `--strict` (a known
failure fails the run too), `--bin-dir <dir>` (default: next to `dc-netlab`).

## Profiles

All numbers are one way, per direction (a round trip is about twice the latency).

| profile | what it models | numbers |
|---|---|---|
| `good` | a wired home line | 10 ms, jitter 0-2 ms |
| `wifi` | a busy home Wi-Fi | 8 ms, jitter 0-25 ms, loss 0.5 %, bursts (1 % chance to enter, 25 % to leave, 50 % lost inside), 0.2 % reordered, 0.1 % duplicated |
| `mobile_hotspot` | a phone's 4G hotspot | 35 ms, jitter 0-30 ms, loss 1 % + bursts (70 % inside), up 1.5 Mbit/s with a 400 ms queue, down 6 Mbit/s with 250 ms, 0.5 % reordered, a 1-3 s outage every 90 s (handover) |
| `australia` | a far player | 150 ms, jitter 0-8 ms, loss 0.3 % |
| `campus_firewall` | a campus network | 12 ms, 100 packets/s policer and 2 Mbit/s cap per direction with a 50 ms queue, datagrams over 1200 B lost, the flow killed after 4 s without a datagram from the server (a new one 30 s later, from a new port), NAT mapping idle timeout 30 s |
| `cgnat_rebind` | a carrier-grade NAT | 25 ms, the client's source port changes every 60 s, an idle mapping expires after 20 s |
| `lossy_burst` | a saturated line | 30 ms, loss 1 % + long bursts (2 % to enter, 10 % to leave, 80 % lost inside) |
| `blackout` | a line that drops out | 20 ms, dead for 2-10 s every 45 s |
| `clean` | nothing (the shim only counts) | |

Overrides follow the profile, comma-separated: `wifi,loss=2%,latency=40`, `australia,down.loss=20`. Keys (both
directions, or one with `up.` = client to server, `down.` = server to client):

- `loss` (per thousand, or percent with `%`), `burst=enter/leave/bad` (per thousand: the chance per datagram to enter
  the bad state, to leave it, and the loss inside it), `latency`, `jitter` (ms, uniform 0..jitter, first in first out),
  `reorder` (per thousand, delivered `reorder_gap` ms late, default 30), `dup`, `mtu` (bytes; larger datagrams are
  lost), `kbps` with `queue` (ms; a full queue drops), `pps` (packets per second policer); `off` clears a key;
- path-wide: `blackout=every_s:min_s-max_s` (`off`), `rebind=s` (new source port every s), `nat_idle=s` (mapping expires
  after s without a send; the next send leaves from a new port), `silence_kill=s[:hold_s]` (flow killed after s of
  server silence; dead for hold_s, 0 = for good; then a new flow from a new port), `seed=n`.

Without a profile name the overrides start from `clean`: `--net loss=20,latency=80`.

## By hand

- One bot: `dc-botclient --lobby <host:port> --name Tester --room any --net cgnat_rebind` (also `--udp` for a game
  server directly). The bot's event log gets `netlab_path` at the start and `impairment` at the end: per direction the
  datagrams offered, delivered and dropped by cause, and the blackouts, rebinds and flow kills. The older `--loss` and
  `--drop-over` still work and add to the profile.
- A person: `dc-client --netlab wifi,latency=60` (or `DC_NETLAB=wifi,latency=60` in the environment). Only the match
  socket is impaired (the lobby connection is TCP and is not); `dc-client.out` names the path at the match start and
  the `--net-log` file gets the same `impairment` line at the end. Use it on a test machine, not on a live account you
  care about: a `cgnat_rebind` or `campus_firewall` path drops matches today (see Known failures).

## The scenarios

A run with the ROM starts one `dc-lobby-server` (fresh database, loopback ports) which starts a `dc-dserver` per room
as in production (default server settings; `--dserver-args` to change them). The first bot of a scenario creates the
room (mode, map, 16 seats) and starts when the bots of its start wave are in; bots of a later wave (`campus`) look for
the room by its title and join the running match. All bots shoot and hunt (`--shooter --hunt --skill 30-65`) and leave
after the scenario's minutes. Without the ROM (`--direct`) `dc-netlab` starts the game server itself on a synthetic
world with the roster of the scenario and the bots join it directly; they walk and do not shoot there (no stream
positions without a world).

| matrix | scenarios |
|---|---|
| `quick` | every playable mode of the ROM once (each on another map), 16 bots of the mixed set, 2 min; then `nat-and-firewall` (10 good, 2 `cgnat_rebind`, 2 `campus_firewall`, 2 `blackout`, 3 min) |
| `soak` | every mode on up to 3 maps, 6 min each, then a 30 min mixed room |
| `ci` | no ROM: a mixed room, a late wave (8 good bots join a campus player after 40 s), a harsh room (bursts, hotspots, blackouts); 2 min each |
| `campus` | the 2026-11-02 report: a campus-firewall player and a good one start, 7 good bots join after 45 s, 4 min |

The mixed set of 16: 6 `good`, 2 `wifi`, 2 `mobile_hotspot`, 2 `australia`, 2 `lossy_burst`, 1 `blackout`, 1
`campus_firewall`.

## Reading the report

`report.md` opens with one line per scenario (bots passed / failed / known failures, kills, verdict), then a table per
scenario, one row per bot:

| column | from | meaning |
|---|---|---|
| join s | bot | socket open to spawn |
| rtt p50/p95 | server | the round trip of the 10 s windows (the samples' mean per window) |
| input/s p50 (min) | server | player updates per second per window |
| gap ms | server | longest gap between two player updates (reported only: a death holds updates for the respawn wait) |
| srv resend % | server | resends per reliable block the server sent (its loss proxy for server to client) |
| bot resends | bot | resends of the bot's reliable blocks |
| dead 5s | bot | 5 s windows in the world in which nothing arrived |
| drops up/down | shim | datagrams the path dropped, client to server / server to client |
| rebinds | shim | source port changes |
| kills | bot | kills the bot made |
| end | bot | how the session ended (`quit (...)` = the bot left; anything else is a drop) |

Then `Why:` lists the reasons of every failing bot. `report.json` holds the same and more per bot (`server`, `bot`,
`path`, `limits`).

A bot fails when any of these holds; the limits follow its path (`dc_netlab::report::Expect`):

- the session did not end by the bot's own quit or the server's normal close: a link closed, server gone, a join that
  failed or timed out, the server's silent drop (DSV-490), its start-up deadline, a close with code 3;
- it never spawned, or the spawn took longer than 20 s (45 s on an impaired path, plus the longest blackout);
- a reliable block was given up (`rel_abandoned`);
- round trip median above `150 + 1.5 x path round trip + 2 x jitter + queues` ms (+200 ms on a bursty path), or p95
  above that base plus 1.5 s on a bursty path plus the longest blackout (the 2009 transport samples the round trip from a block's first send, so a
  block resent after a burst or a blackout gives one long sample);
- the server counted fewer of the bot's player updates than the path delivers (upstream loss and blackout share) minus
  5 points. Update rates and gaps alone are not judged: a dead player sends none (elimination modes keep a player dead
  for the rest of the round), and the bots' pace differs per platform (a 15.6 ms timer tick makes their 30/s about
  21/s on Windows);
- a 10 s window with fewer than 5 datagrams from the client (a live client acks at least every second, NET-12, dead
  or alive in the game; not judged on a path with blackouts);
- server resends above `5 % + 4 x the path's downstream loss`, +10 % with bursts, +2 x the blackouts' share of the time in percent, +5 % with a
  policer, a bandwidth cap or a size limit;
- more 5 s windows without a datagram at the bot than the path's blackouts explain (none on a path without them);
- a server datagram over 548 bytes (NET-10).

A scenario fails when a bot fails, a bot process did not end in time, or (ROM runs, 2 min or longer) the bots claimed
more than 20 hits and no kill was registered. The summary shows shots, hits claimed and kills per scenario.

`dc-netlab judge --out <run folder>` judges an earlier run again from its files (after a threshold change).

`xfail` marks a failing bot whose profile is a known open issue (`DEFAULT_XFAIL` in `crates/dc-netlab/src/lib.rs`,
`--xfail`); it does not fail the run unless `--strict`. `xpass`: a known failure that passed this time.

## CI

`.github/workflows/netlab.yml` runs nightly and by hand (Actions, "netlab", with a matrix, players and minutes): the
transport tests over simulated paths (`cargo test -p dc-net-engine -p dc-netlab`), then `dc-netlab ci --direct
--players 8` on ubuntu. CI has no ROM, and the worlds are ROM content, so CI runs only the synthetic world; the report
is in the job summary and the `netlab-report` artifact. The ROM run (`quick` / `soak` on real maps through a lobby) is
a local or beta-server step: run it on a machine with the ROM before a build goes to players.

## Findings and known failures

What the lab found on its first runs (2026-11-03, Windows, release build, `quick --parallel 2`, 9 rooms of 16):

| finding | status |
|---|---|
| NAT rebinding (`cgnat_rebind`) dropped the player at every port change: the server ignored the new port as an unknown peer and sent to the old one, both sides went silent, the bot gave up after 15 s ("server gone: no datagram from the server"), the server dropped the client after 20 s (DSV-490). The same shape as the 2026-11-02 campus report, where client and server each saw nothing from the other. | fixed: NET-23, the server moves the link to the client's new port (same IP, old address silent 250 ms, the datagram carries the link's move secret, NET-24: our clients only; 2026-11-03 rerun with the secret: `run --direct --mix good:4,cgnat_rebind:4 --minutes 3`, 12 moves, 8 of 8 pass); `tests/nat_rebind.rs`; rerun: 3 port changes per bot, no drop |
| Round trip samples after a loss burst or a blackout are long (p95 up to 2.6 s on a 60 ms path): the 2009 transport samples from a block's first send, so a resent block reports the whole wait, and the resend timer grows with it. | 2009 behaviour, kept; the p95 limit allows it. A Karn-style sample (only from blocks sent once) would be a NET row of its own |
| The bots send about 21 updates/s on Windows, not 30: their loop waits on a 15.6 ms timer tick. | bot pace only (the client is frame-paced); the lab judges updates delivered, not the rate |
| Hits claimed vs kills per mode differ widely (mode keys of the ROM: TDM 12 %, FFA and CTF 9 %, TSV and TMM 2 %, BNT and QRT under 1 %). | not judged; worth a look by mode (health rules, infected health) |

A path the lab cannot save: `campus_firewall` with a flow kill (a firewall that blocks the flow for 30 s after 4 s of
server silence) ends the session; the server is never silent that long in a running match, so it did not trigger in
these runs.
