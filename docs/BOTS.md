# Bots on your own PC

A few bots that run on your PC and join you on a public lobby.

## Simple use

On Windows the bots are a download of their own (`DeCombat-win-<tag>-bots.zip`): unzip it into the same place as the
game zip. With your Combat Arms folder next to `DeCombat.exe` and `bots.bat`:

```text
Windows   bots.bat 3 YourNick          three bots that sit in whatever room YourNick sits in
          bots.bat list                what runs
          bots.bat stop                end them (Ctrl-C in their window works too)
macOS     ./bots.command 3 YourNick    (the same; list and stop too)
```

Open a room in the game and the bots come in. Leave it and they leave. At most 7: that is the lobby's
`bots.pool_per_ip` (default 7), the number of bot seats one address may hold. Without your nick (`bots.bat` alone) the
bots sit with the first person found.

A bot needs no account. Each one takes a free seat of the lobby's bot pool when it logs in and gives it back when it
ends. Its name in the game is the seat's: `PoolBot17`.

## The command

```text
dc-botclient swarm --server <host[:port]> --rom <client folder> [--count 1..7] [--server-id N] [--channel N]
                   [--room any | <part of a title>] [--follow <nick>] [--room-password <p>] [--skill off | 0..100 | lo-hi, default 30-65]
                   [--mod-cache <dir>] [--mod-set-hash <hex>] [--content-hash <hex|0>] [--dir <folder>]
dc-botclient swarm list | stop
```

- `--server` is remembered after the first run. Port 10000 without one.
- `--server-id` picks the entry of the server list, `--channel` the channel; they must match where your room is.
- Without `--follow` and `--room` a bot takes the first listed room with a person in it (any name without the `PoolBot`
  prefix).
  `--follow` waits for that player's room and leaves when they do. To find them a bot enters each listed room for a
  moment, as a filler bot does.
- Without `--rom` (or `CA_ROM`) the bots have no map to walk on: they stand still and hold fire.
- `DC_SERVER_FINGERPRINT` pins the lobby's certificate. Otherwise the first one seen is remembered in the client's own
  `known_servers.txt`.

## Where things are

Everything is in `bots` in the DeCombat profile folder (`%APPDATA%\DeCombat`, `~/Library/Application Support/DeCombat`,
`~/.config/decombat`; `--dir` or `DC_BOT_DIR` to move it):

- `logs/seat<n>.out`: what each bot printed, with the seat it got. A log above 5 MB is deleted and
  started over when that bot next starts (the managed bots' logs below keep a `.1` instead). The command prints a line
  when a bot ends for any other reason than a full pool: `seat2 ended (exit 1), starts again in 20 s: <last log line>`.
  A bot that ends is started again after 10 s, doubling to 2 min while it keeps ending quickly; starts are 3 s apart.
  With `--follow`, a hint is printed once after 3 minutes (the name must match the in-game name exactly).
- `swarm.json`, `last_server.txt`: the running command's state for `list` and `stop`, and the last lobby.

An `accounts.json` from an older build is no longer read. It can be deleted.

## The bot pool

A lobby makes `bots.pool_size` accounts at its start (default 100; 0 = no pool), `poolbot1` and so on, in its own DB. A
stock bot logs in with the bot mark and no name, and the lobby gives it a free one.

- A seat is taken until the bot's connection ends, a crash included. Between the auth and the lobby connection the
  session key holds it.
- One address holds at most `bots.pool_per_ip` seats (default 7). The lobby's own machine has no limit.
- With no seat free the bot says so and tries again: `the lobby's bot seats are all taken`, or `this address holds as
  many bot seats as the lobby allows`. A lobby without a pool or older than the pool stops the command with `has no bot
  seats`. A bot of an older build on a new lobby goes on as before, on the accounts it registered.
- A seat logs in by the mark only: not by name, not with a password, not from the lobby's machine. It enters rooms and
  plays. It cannot make a room, change team, buy, trade, mail, chat, make friends or join a clan, and it cannot pick
  the clan server. In-match chat goes through the game server, not the lobby: a changed binary is out of this reach.
- Each lobby instance has its own pool. A modded instance works the same; the bots pass its `--mod-set-hash` as before.
  A featured entry works too, under "A server entry with a pack".

Named accounts still work for an operator: `dc-botclient --name VpsBot1 --login-password ...` on any account, as
`tools/vps/bots.sh` does.

## A seat for people

From `bots.keep_seat_online_players` people online on the lobby (default 8, bots not counted; 0 = never), rooms with bots
keep a seat open. Eight is a full room's worth of people: below it, people want the company more than the seat.

- No bot takes the last free seat of a room.
- When a person's entry fills a room with a bot in it, one bot leaves at once, in the waiting room or a running match.
- Below that number bots may fill a room, and a person who wants into a full room still gets in: a bot makes way. At or
  above it the same person sends a second bot out, for the seat that stays open.

The bot that leaves is one that is not the host, on the larger team, the newest. The others see a normal leave. A bot host
leaves only when no other bot is there, and a room passes to a person before a bot. Clan rooms are left out, GMs never
leave, and the team rule reads the room's own teams, so a mode without sides (free-for-all, Fireteam) is only a matter of
which bot. When only the smaller team holds bots, one of them leaves anyway: the seat comes first.

A bot sent away passes over that room for a minute and looks elsewhere, and one refused at the door waits 20 seconds per
room. It keeps its seat and does not reconnect. A person coming in during a match does not wait for the bot: the game
server has its own, larger limit of `room.max_players_cap`.

A room with a weapon limit its guns fail (a "Sniper Only" room and a bot without a sniper rifle) is left the same way: a
bot cannot change what it carries, so the first refused ready is the last. It leaves, passes that room over for 5 minutes,
logs `weapon_limit`, and readies under that limit nowhere else for the rest of its run (BOT-156).

## A server entry with a pack

A bot can join an entry with a mod set: it repeats the hashes the lobby pushes for that entry (BOT-101). For a room on a
community map it finds the map in `--mod-cache`, by default the unpacked packs of your own client
(`mods/unpacked` in the profile folder), so play on that entry once first. A pack that adds guns would need more: a bot
holds no mod content.

## What bots earn

Nothing, and the people with them are paid as usual, however many bots there are (LG-320). The lobby treats these as
bots (LG-317):

- a seat of the bot pool, from anywhere. It also keeps no record: no match count, no kills on a sheet;
- an account named `...bot<digits>`;
- an account a stock `dc-botclient` has ever logged into, under any name. The mark stays on the account;
- a `--test-bot-account`.

Connected from another machine, the last three get no EXP or GP. On the lobby's own machine they are the operator's and
are paid as before. The daily NX bonus and operator NX events count people only. Clan NX counts the people on each side
and needs `rewards.nx_clan_min_players` of them, and a match with a bot from another machine pays none.

What this cannot catch: a changed `dc-botclient` that leaves its mark out under a person's name, or a second real client
on a second account. The server cannot tell those from a player.

The mark is permanent, so do not point a bot at your own account. An operator clears a wrong one in the lobby's DB:
`UPDATE accounts SET self_bot = 0 WHERE login = '<name>'`.

## Bots the lobby runs

A lobby can run bots itself, as child processes, and you control them on the dashboard's **Bots** tab (docs/OPS.md
section 15; also on the admin web). This replaces `tools/vps/bots.sh` on the VPS. Status: implemented and unit-tested
with a stand-in bot program; not yet run on the VPS.

Turn it on in the lobby's `server.toml` (each lobby instance has its own switch and its own groups), then restart it:

```toml
[bots]
managed = true            # off by default: a lobby without it starts no bot
managed_max_total = 21    # most bots of all groups together
```

`DC_BOTS__MANAGED=true` works as well. The log then says `bots: managed by this lobby (bots.managed): .../dc-botclient, logs in ..., at most 21 in all`;
it says `(MISSING: no bot can start)` when `dc-botclient` is not beside `dc-lobby-server` (the server tarball installs both in
`/opt/decombat/bin`).

**A group** is a set of bots that share their settings:

- a name prefix: the bots are `<prefix>1` .. `<prefix><count>` (2..12 letters, digits or `_`, a letter first, no digit
  last, not `PoolBot`);
- how many, 1..15 (a room holds up to 16: `room.max_players_cap`; all groups together stay within `bots.managed_max_total`, default 21);
- the server entry (every entry of this lobby's server list; an entry that names another lobby, such as the modded instance, is
  listed greyed out: its bots log in there, so add that group in that instance's dashboard) and one or more of its channels. A new
  group starts with the first channel ticked. Bot 1 takes the first ticked channel, bot 2 the next, and so on
  round the list: three channels and seven bots give 3, 2 and 2 bots;
- the room: any room with a person in it, the room of one player (they follow that player and leave with them, see
  "Following a player" below), or the first room whose title holds a text;
- the skill: a level 0..100, a range such as `30-65` (each bot gets its own level from its name), or `off`. Three bands
  (BOT-170 in DC_DEFAULTS.md): up to 30 a beginner (about 1 s to react, few hits, stands while it fires), 30..65 an average
  player (the default range), from 65 a good player (0.3 s to react, tracks a strafing target, long bursts, pushes and
  crouch-strafes, keeps firing at a target that ducks behind cover for a moment). 80 and up is meant to be dangerous;
  20 is meant to be easy;
- a loadout, or nothing to leave their guns alone: a dropdown of gun names per slot (primary, secondary, melee, throwing; the ROM's
  weapons and the camo variants of the loaded mod sets), or typed below them: `random`, or the items as `account loadout` takes
  them (`0:27` is the L96A1 in slot 0). Each bot gets it once, after its first login made the account (or at once when it has one), and
  restarts so the kit counts. The status shows `loadout 0:27`, `after the first login` or why it was refused. A refused kit is tried again
  after 10 minutes, or at once when the group is edited; a bot still waiting for its first login is checked when it logs in, else every 10 s.

The tab lists each group and its bots: the process (running, restarting with the seconds left, stopped), how long it has
run, its restarts and last exit, its channel, and from the lobby whether it is logged in and in which room or match.
**Edit** changes a group: only the bots whose settings change restart (a new channel list moves the bots one by one).
**Stop** ends its bots and keeps the group; **Start** runs them again; **Delete** ends them and removes the group. A
change that would run more than `managed_max_total` bots is refused. Every change is an event of kind `bots` in the
dashboard's events list, by group number and prefix. Groups can be saved while `managed` is off; they start once it is on. A group whose bots do not fit under `managed_max_total` (the
later groups lose out) shows "N bots over bots.managed_max_total" on its card and those bots stay waiting. When `dc-botclient` is
not next to the lobby executable the tab shows a red banner (`exe_missing` in `GET /ops/bots`); no bot can start until it is there.

**Following a player.** A group that follows a player goes where the lobby sees that player: when they are on a channel
of another entry or another channel of this lobby, the group's bots restart there (all of them on the player's channel,
with that entry's `--server-id`; a curated entry's packs come from the same `--mod-cache`) and take a seat in the
player's room. While the player is on a channel in no room, the bots wait on that channel; while they are offline, on
the server or channel list, or in the clan lobby, the bots stay where they are (where the group was set up, until the
first move). A group moves at most once every 10 s, so a player hopping channels does not restart the bots on every hop;
each move restarts the bots 2 s apart. A room with a password is not joined (the bots know none) and a full room has no
seat. Editing the group's entry or channels puts the bots back there. The group's card says where the player is, or
that they are waiting. A group follows within its own lobby only: the lobby sees its own sessions, so a player on an entry
of another lobby (the modded instance) is "not online on this lobby"; to follow them there, make the group in that
instance's Bots tab. Rule: OPS-15 in DC_DEFAULTS.md.

What each bot runs: `dc-botclient --parent-pipe --lobby 127.0.0.1:<auth port> --name <name> --room any --duration 3600
--match-wait 3600 --shooter --hunt --respawn-after 0 --skill <skill> --matches 0 --filler <prefix>` (or `=<player>`),
`--room-title <text>` for a title choice, `--server-id`, `--channel`, `--rom` (the lobby's ROM), `--mod-cache` (the
featured entries' packs, when there are any), `--mod-set-hash` / `--content-hash` on a modded lobby, `--log` and
`--known-servers` in the bots' folder. That is what `bots.sh` passed. The output goes to `bots/<name>.out` and
`bots/<name>.events.jsonl` beside the lobby's log (`/opt/decombat/logs/bots` on the VPS; above 5 MB a file is moved to
`.1`: the lobby does it for `.out` at a start, the bot does it for its events file while it runs, `--log-max 5242880`).
The events file holds the summary events only; the per-message transport records (`tcp_open`, `tcp_in`, `tcp_out`) and
game-message records (`msg_in`, `msg_out`) are written only with `--log-transport`, which no managed or swarm bot gets.
`--respawn-after 0` asks for a respawn once the mode's wait passed and then at most every 5 s while the bot is still dead;
where the round or the mode decides (Search and Destroy, Elimination, Quarantine, co-op without a life) it never asks and
logs one `respawn_wait` with the reason. A leave the lobby refuses (a start countdown) ends the run with exit code 0.

A bot that ends starts again 10 s later (one game session lasts an hour, then it starts over; a bot whose hour passed
with no room to join exits with code 0 and the Bots tab shows `ran its hour`, a real failure keeps exit code 1); one that keeps ending
within 2 minutes waits twice as long each time, up to 5 minutes. Starts are 2 s apart. Its stdin is a pipe from the
lobby: when the lobby ends, also by a crash or a kill, the bots end too. A graceful stop, Stop, Delete or a smaller
count close the pipe and kill a bot that has not ended 2 s later.

Limits: a bot keeps to rooms with a "person", and a person is any member without its own group's prefix, so the bots of
two groups in the same room keep each other company. Give groups that share channels a room choice that keeps them
apart, or use one group. A lobby that listens on one address only (`server.bind` not `0.0.0.0`) is reached there, and
the bots then log in from that address, not loopback.

**Moving from `bots.sh`:** stop it first (`sudo bash /opt/decombat/tools/bots.sh stop`, and
`sudo systemctl disable --now decombat-bots` if you used the unit), or each name logs in twice. Then add the same groups
on the tab: the same prefix reuses the same accounts, with their characters, guns and history. A `--args "--server-id 2
--channel 4"` becomes entry 2 with channel 4 ticked; a loadout given by hand with `account loadout` stays until a group
names another. Rules: LG-388 .. LG-391 in DC_DEFAULTS.md.

## Evaluating the bots' gameplay

`tools/bot_eval.py` (Python 3, standard library) plays headless rooms of bots through the network lab
([NETLAB.md](NETLAB.md)) and scores how much they play like an average person, per bot and per room.

```text
python -I tools/bot_eval.py run --out <dir> [--parallel 3] [--bots 8] [--only TDM,TMM] [--minutes M] [--bin <dir>] [--rom <dir>]
python -I tools/bot_eval.py score <dir> [--md] [--csv] [--partial]
```

`run` plays every mode on every map it has (Fireteam excluded): 6 minutes for the respawn modes, 8 for the round
modes, `--bots` good bots per room, `--parallel` rooms at once. The ROM is `--rom` or `CA_ROM`, the binaries
`--bin` or `target/release` of this checkout (build `dc-netlab`, `dc-botclient`, `dc-dserver` and `dc-lobby-server`
in release first). Each room lands in `<dir>/<KEY>_<map index>_<map name>/`; a room that already has `report.json`
is skipped, so a stopped run continues where it was. `progress.txt` gets a line per room; the folder is scored at
the end.

`score` takes one netlab `--out` folder or a folder of them. It reads the bots' event logs, the game server's log
and `report.json`, and writes `score.md` / `score.json` per room and `summary.md` / `scores.csv` for the folder
(`--partial` also scores rooms still running). The room report lists every rubric line that applies to the mode
with PASS / FAIL and its numbers, then a table per bot (kills, deaths and who killed it, world deaths, shots,
stuck and idle time, goals reached, reaction times), the rounds and how each ended, and the stuck, idle and
unreached-goal episodes with their positions. The metric definitions are in the script's header and its
thresholds are constants at the top of the file:

- mobility: stuck episodes (a goal more than 96 units away, the body within 48 units for 8 s or more), idle time
  (no goal and no movement for 15 s; a defender on a Hold job before the plant may stand 90 s), deaths without a
  killer (a death at the bomb's detonation is the bomb, not a fall), goals not reached within 60 s, floor coverage
  per team (256-unit cells), team mates standing within one body width (the first 3 s after a spawn do not
  count), jumps (non-face stuck jumps per alive minute; ladder mounts and ledge climbs are logged as `face` and do
  not count; pogo runs of 4 or more jumps within 96 units with gaps under 6 s, judged for bots with 3 or more alive
  minutes and reported with their positions), and confinement (a bot alive 60 s or more inside a 512-unit box
  while its goal is more than 300 units away, or the nav island count growing by 20 inside one life: none
  allowed, the box and times are reported);
- combat (respawn modes): kills per 2 minutes alive, team kill shares, hits per shot for bots with 100 or more
  shots (Spy Hunt: hit rate by distance band, and at most a tenth of the hits closer than 20 units), the time from
  seeing an enemy to the first shot (median of at least 20 samples, none instant, a short tail of slow ones) and
  the spread of the bots' planned reaction delays;
- objectives: Search and Destroy plants, defuses (including whether a defuse starts when a defender could reach
  the site in 25 s) and round ends; Capture the Flag captures, dropped flags and balance (each team takes the
  enemy flag, capture margin); Spy Hunt pickups, stage times, stage 2 ending by upload or death rather than the
  timer, and how many hunters work the agent; Quarantine after the outbreak (length of the phase, how close the
  humans stay together, how often they hunt instead of grouping, how fast the first claw lands, how much they
  shoot when they see an infected); Elimination Pro round ends. Win shares (round wins in Search and Destroy and
  Elimination Pro, human wins in Quarantine, agent wins in Spy Hunt) are judged pooled per mode in the "Per mode"
  section of `summary.md`, since a single room has too few rounds;
- flow: dropped bots, failed joins, panics, refusals by the server.
