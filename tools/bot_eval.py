#!/usr/bin/env python
"""bot_eval.py: run the bot-gameplay matrix through dc-netlab and score how human-like the bots play.

Tooling of the DeCombat project (docs/BOTS.md "Evaluating the bots' gameplay"). Python 3, standard library only;
safe under `python -I`.

    python -I tools/bot_eval.py run --out DIR [--parallel 3] [--bots 8] [--only TDM,TMM] [--minutes M]
                                    [--bin DIR] [--rom DIR] [--no-score]
    python -I tools/bot_eval.py score DIR [--md] [--csv] [--partial]

`run` plays every mode x map of the matrix below (Fireteam excluded) as one headless dc-netlab room of `--bots` good
bots each, a few rooms at a time. Room folders are `DIR/<KEY>_<map index, 2 digits>_<map name, letters only>/`
(netlab's own layout inside); a folder that already holds `report.json` is skipped, so an interrupted run resumes.
A line per finished room goes to `DIR/progress.txt`. The ROM comes from `--rom` or `CA_ROM`; the binaries from
`--bin` or `<this repo>/target/release`. After the last room the whole folder is scored (unless `--no-score`).

`score` reads one netlab `--out` folder (it holds `report.json`) or a folder of many and writes, per room,
`score.json` + `score.md` into the room folder and `summary.md` + `scores.csv` into DIR. `--md` / `--csv` also print
the summary / the CSV. `--partial` also scores room folders without `report.json` (a room still running).

Inputs per room: the bots' `<nick>.events.jsonl` (kind in the field `ev`), the game server log `dserver/*.log`
(lines `<ms> dc-dserver: ...`; a log with several sessions, from a retried room, is scored from the line of its
last `dc-dserver: world <map> (N bytes, ...` line on), `report.json` (mode, map, minutes, per-bot shots) and the bots' `.out` files.

Time base. Bot events carry `ms`, the bot's session clock from its join request; the server log carries the
server's clock. A bot's event is placed on the server clock as `ms + <the server's "client N linked" ms>` (the
join request arrives then; the error is the one-way latency, ~10-30 ms on loopback). Rounds, pauses and the
warm-up come from the server log, so "in a running round" means between a `round ... started` line and the next
`round N over` (the warm-up counts as running: the bots move in it; the frozen result pause does not).

Metric definitions (the shared definitions every reviewer uses; thresholds are the constants below):

  alive time      the bot's `track` samples (one every TRACK_PERIOD_S while it is alive in the world) x the period.
  life segment    a run of consecutive `track` samples with gaps <= SEGMENT_GAP_S and no `died` between them.
  stuck episode   inside one life segment and one running round: samples i..j, every one with a goal more than
                  STUCK_GOAL_U from the body (3-D) and every body position within STUCK_RADIUS_U (3-D) of sample i,
                  lasting t_j - t_i >= STUCK_MIN_S. Episodes do not overlap (the scan restarts after j). Reported:
                  count, longest (s) and the start positions.
  idle episode    as a stuck episode, but every sample has no goal (`goal` null) and stays within IDLE_RADIUS_U of
                  sample i for >= IDLE_MIN_S. Reported in seconds and by the track `kind` (stand / hold / ...).
  goal episode    the track `goal` is the bot's current leg / destination (it moves along a route and with a
                  hunted target). An episode is a run of samples of one life segment with a goal and the same
                  `kind`, the goal never jumping more than ROUTE_RETARGET_U between two samples (that is a new
                  target); goal-less samples (waits, combat) of up to ROUTE_GAP_S inside it do not end it. It is
                  reached when a sample has `arrived` true or the body is within GOAL_REACH_U (3-D: a goal on
                  another floor is not reached) of the current goal; then it closes. Only episodes whose first goal
                  is under ROUTE_MAX_GOAL_U away are judged: reached within ROUTE_MAX_S = ok; not reached after
                  ROUTE_MAX_S (or reached later) = late, listed with the last position and goal; ended (another
                  kind, a death, the round end) before ROUTE_MAX_S = abandoned (not judged).
  coverage        distinct CELL_U x CELL_U floor cells (x, z of `track` positions) visited by each team and by all
                  bots; team coverage = team cells / all cells.
  stuck_jump      `stuck_jump` events carry `face` (a ladder mount or ledge climb: intended). Judged: the non-face ones per
                  alive minute, and pogo runs: POGO_N or more events within POGO_RADIUS_U of the run's first one with
                  gaps under POGO_GAP_S (bots with >= POGO_MIN_ALIVE_MIN alive minutes), reported with the positions.
  confined        inside one life segment, CONFINED_MIN_S inside a CONFINED_BOX_U (x, z) box while the goal stays more
                  than CONFINED_GOAL_U away (a pit, a pocket, a deck); or the nav `islands` count grows by
                  CONFINED_ISLANDS within one life (a `died` ends a life). None allowed per room.
  reaction        (a) planned: the `reaction` event's `delay_ms` (the skill's delay, + a sniper's settle; first
                  sightings = kind `new`); the IQR line judges these. (b) measured: per `reaction` event (the bot takes a seen enemy as
                  its target; it comes in the same ms as that `target_seen`), the bot's next `fire` before that
                  target's `target_unseen` (or the next `reaction`); latency = fire ms - reaction ms. A
                  `target_seen` without a `reaction` is a second enemy seen while engaged and is not a sighting.
                  Re-acquires (`reacquire` true) are kept apart; the rubric judges first sightings.
                  (`fire.target_obj` is what the shot hit, null on a miss: not the aimed target.)
  shots / hits    the bot's end `report` (`shooter.shots`, `shooter.hit_claims`, `shooter.kills_by_me`), else
                  report.json.
  kills / deaths  the server log (`client V killed by Some(K)` / `None`): K's kills, V's deaths; killer None = a
                  world death (fall, kill volume, water). A kill of a team mate counts as a team kill, not a kill.
                  The bots' own `died` and `target_down` (`mine`) counts are kept as a cross-check.
  accuracy        hit_claims / shots (hits per shot; a rifle kill takes several hits), rifles only: a bot that selected a weapon
                  with `sniper` true is not judged (BOT-123 governs snipers).
  round end       `bomb` (exploded), `defuse`, `elimination` (`team(s) X without a living player`), `forfeit` (a
                  team left), `capture` / `mission` / `goal` (a capture, the spy's upload, a kill goal), `timer`
                  (round ran >= its length - 1 s), else `other`. The warm-up's own result is not a round.
  K/D spread      max - min of the bots' kills/death ratios (deaths 0 counts as 1).

Nick <-> server client id: the bot's own `roster` event (`own` true, `pid`), else the server's `@join` lines.
"""
import argparse
import csv
import glob
import io
import json
import math
import os
import queue
import re
import statistics
import subprocess
import sys
import threading
import time

# ---------------------------------------------------------------------------------------------------------------
# thresholds (the coordinator tunes these)
# ---------------------------------------------------------------------------------------------------------------
TRACK_PERIOD_S = 2.0
SEGMENT_GAP_S = 3.5
STUCK_GOAL_U = 96.0
STUCK_RADIUS_U = 48.0
STUCK_MIN_S = 8.0
STUCK_MAX_PER_BOT_PER_10MIN = 1
STUCK_LONGEST_MAX_S = 20.0
IDLE_RADIUS_U = 16.0
IDLE_MIN_S = 15.0
IDLE_MAX_S = 0.0                # total idle seconds allowed per room ("a human never stands in a corner")
IDLE_GUARD_KINDS = ("stand", "hold")  # a guard / post: camping a flag or a site 15-30 s is normal CA play
IDLE_GUARD_MIN_S = 45.0         # ... so a guard counts as idle only when motionless this long
IDLE_HOLD_JOB_MIN_S = 90.0      # a Hold-job defender before the plant counts as idle only when still this long (TMM_TSV review)
IDLE_HOLD_MAX_S = 30.0          # one hold per bot per 10 min under this, with an enemy seen in the last CONTACT_S, is allowed
CONTACT_S = 30.0
ROUND_COUNTDOWN_S = 20.0        # round modes: the first seconds of a round (the countdown) are not judged for idling
ROOTED_MIN_S = 20.0             # still (16 u) while the target is in sight: a turret, judged apart from idling
ROOTED_MAX_PER_BOT_PER_10MIN = 1
PACE_CELL_U = 128.0             # ledge pacing: within one 128 u circle for >= PACE_MIN_S with the goal > PACE_GOAL_U away
PACE_MIN_S = 20.0
PACE_GOAL_U = 300.0
STUCK_JUMP_PER_MIN_MAX = 2.0    # non-face stuck jumps per bot per alive minute (corner hopping; face jumps are ladder
                                # mounts and ledge climbs) (r2 reviews TDM_FFA, TMM_TSV, CTF, QRT)
POGO_N = 4                      # a pogo run: this many stuck_jump events ... (r2 decision 1; TDM_FFA review)
POGO_RADIUS_U = 96.0            # ... within this distance of the run's first jump ...
POGO_GAP_S = 6.0                # ... with gaps under this
POGO_MAX_PER_BOT_PER_5MIN = 1   # allowed runs per bot per 5 alive minutes (at least 1)
POGO_MIN_ALIVE_MIN = 3.0        # a bot is judged for pogo runs only with this many alive minutes (TMM_TSV review)
CONFINED_BOX_U = 512.0          # confined: alive inside a box this wide (x, z) ... (r2 decision 2; TDM_FFA, CTF reviews)
CONFINED_MIN_S = 60.0           # ... for this long ...
CONFINED_GOAL_U = 300.0         # ... while the goal stays farther than this
CONFINED_ISLANDS = 20           # or the nav `islands` count grows by this much inside one life
CONFINED_MAX_PER_ROOM = 0       # allowed per room
MATE_STACK_U = 36.0             # a team mate within one body width
MATE_STACK_MAX_SHARE = 0.05     # ... for at most this share of a bot's samples (TDM / CTF)
MATE_STACK_SPAWN_S = 3.0        # samples this soon after a spawn / respawn / round start are not counted (TDM_FFA review)
REACTION_IQR_MIN_S = 0.10       # a bot's planned first-sighting delays must vary (a fixed delay is a tell)
REACTION_IQR_MIN_N = 20         # judged on the planned `delay_ms` of `reaction` kind new, n >= 20 (TDM_FFA review)
WORLD_DEATHS_ROOM_PER_6MIN = 3  # respawn modes: falls per room (8 players do not fall 5 times in 6 min)
WORLD_DEATHS_ROUND_ROOM = 2     # round modes: one fall loses a player for the round
ROUND_WIN_POOLED = (0.40, 0.60)  # round modes: either side's share of the round wins, POOLED per mode (TMM_TSV review);
                                 # a single room (8-15 rounds) is coin-flip noise and only informational
POOL_MIN_ROUNDS = 20            # a pooled round-win check needs this many rounds
WORLD_DEATHS_MAX_PER_BOT_PER_10MIN = 1
ROUTE_RETARGET_U = 1500.0       # a goal jump this large between two samples starts a new goal episode
ROUTE_GAP_S = 6.0               # goal-less samples (waits) inside one goal episode
GOAL_REACH_U = 96.0
ROUTE_MAX_GOAL_U = 2000.0
ROUTE_MAX_S = 60.0
ROUTE_LATE_MAX_SHARE = 0.10     # share of judged goals allowed to be late
ROUTE_SKIP_KINDS = ("hunt",)    # goal kinds that follow a moving enemy: not a route, not judged
CELL_U = 256.0
COVERAGE_MIN = 0.45             # per team, TDM (real teams play lanes)
KILLS_PER_2MIN_ALIVE_MIN = 0.67       # 1 kill per 3 min alive (a quiet average player on a big map)
TEAM_KILL_SHARE = (0.35, 0.65)  # TDM / CTF
FFA_TOP_SHARE_MAX = 0.40
ACCURACY = (0.05, 0.40)         # hit_claims / shots, rifles (hits per shot; a kill takes several hits)
ACCURACY_MIN_SHOTS = 100        # a bot with fewer shots is not judged (CTF review)
SPY_BANDS = ((0.0, 150.0, 0.25, 0.60), (150.0, 600.0, 0.10, 0.40), (600.0, float("inf"), 0.02, 0.20))
                                # Spy Hunt hit rate by distance: (from u, to u, min, max) (BNT review)
SPY_BAND_MIN_SHOTS = 20         # a band with fewer room shots is not judged
SPY_POINT_BLANK_U = 20.0        # hits closer than this ...
SPY_POINT_BLANK_MAX_SHARE = 0.10  # ... at most this share of the hits (BNT review)
SPY_POINT_BLANK_MIN_HITS = 10
REACTION_MEDIAN = (0.3, 1.2)    # seconds, measured sight -> first shot
REACTION_MEDIAN_MIN_N = 20      # the measured median is judged only with this many samples (BNT review)
REACTION_INSTANT_S = 0.15       # "never instantly"
REACTION_MAX_S = 3.0
REACTION_OVER_MAX_SHARE = 0.03  # share of measured reactions allowed above REACTION_MAX_S (a lost sighting) ...
REACTION_OVER_MIN_SAMPLES = 3   # ... or this many samples (TDM_FFA, CTF reviews)
REACTION_OVER_MAX_SHARE_BNT = 0.10  # Spy Hunt: case-walking holders reacquire late (BNT review)
SND_PLANT_ROUNDS_MIN = 0.50     # team fights end rounds before a plant
SND_DEFUSE_AFTER_PLANT_MIN = 0.5  # ... of the plants a defender was alive SND_DEFUSE_ALIVE_S after (BOT-142: the
                                  # guards now kill half the defenders on their way in, which is the human outcome)
SND_DEFUSE_ALIVE_S = 10.0
SND_ATTACKER_WIN_MIN = 0.35     # share of planted rounds the attackers win (a defuse nobody contests is a tell)
SND_CONTEST_MIN = 0.50          # share of defuse starts with an attacker firing within SND_CONTEST_S
SND_CONTEST_S = 8.0             # counted only for defuses with an attacker alive (TMM_TSV review)
SND_BOMB_DEATH_MS = 50          # a `killed by None` this close to the detonation is a bomb death, not a world death
SND_DEFUSE_REACH_S = 25.0       # the site is reachable for a defender within this long ... (TMM_TSV review)
SND_RUN_SPEED_U = 219.0         # ... at this speed (distance / speed, no path)
SND_DEFUSE_REACH_MIN = 0.50     # a defuse starts in this share of the planted rounds a reaching defender was alive for
SND_DECIDED_MIN = 0.80          # rounds ended by bomb / defuse / elimination
SND_SPAWN_LEAVE_U = 512.0       # an attacker "left the spawn" when it got this far from its first round position
CTF_CAPTURE_EVERY_S = 360.0     # pub CTF with real defenders: 2-4 captures per 10 min
CTF_TAKE_EVERY_S = 180.0
CTF_BOTH_OUT_MAX_S = 120.0      # both flags off their bases with no capture or return: a stalemate
CTF_DROP_RESOLVE_S = 20.0
CTF_BALANCE_EVERY_S = 360.0     # each team takes the enemy flag at least once per this long ... (CTF review)
CTF_CAPTURE_MARGIN_MAX = 3      # ... and the capture margin stays within this
SPY_PICKUP_S = 30.0
SPY_STAGE_MAX_S = 240.0
SPY_STAGE2_TIMER_S = 180.0      # the stage 2 clock: an agent still alive after it ends the round by the timer
SPY_STAGE2_HUNTERS_MIN = 4      # in stage 2 at least this many hunters chase / guard / stay near the agent or its uplink ...
SPY_STAGE2_NEAR_U = 1500.0
SPY_STAGE2_SHARE_MIN = 0.70     # ... for this share of the stage (BNT review)
SPY_STAGE2_KINDS = ("spy_chase", "spy_guard")
SPY_AGENT_WIN_SHARE = (0.20, 0.60)  # pooled per mode, over the rounds that reached stage 2 (BNT review)
QRT_HUMAN_WIN_SHARE = (0.15, 0.40)  # POOLED per mode (QRT review); the 2009 numbers (infected 600 HP, host 1200, 19 per
                                    # rifle hit, one claw infects) give 8 players' humans about 1 round in 7 (BOT-150)
QRT_POST_OUTBREAK_S = (40.0, 120.0)  # median length of the round after the first infection (QRT review)
QRT_MUTUAL_DIST_U = 400.0       # humans' median mutual distance after the outbreak ...
QRT_MUTUAL_MIN_SHARE = 0.50     # ... at most this in at least this share of the samples (QRT review)
QRT_HUMAN_HUNT_MAX_SHARE = 0.05  # humans with a `hunt` goal after the outbreak, share of their samples (QRT review)
QRT_FIRST_CLAW_S = 3.0          # the first claw within this long of the outbreak ...
QRT_FIRST_CLAW_MAX_SHARE = 0.25  # ... in at most this share of the rounds (QRT review)
QRT_HUMAN_SHOTS_PER_S = 3.0     # humans fire at least this many shots per second in sight of an infected (QRT review)
QRT_SIGHT_MIN_S = 10.0          # ... judged with this many seconds in sight
QRT_HOST_KILL_MAX_SHARE = 0.25  # rounds won by killing the host with zero infections
TSV_ELIM_MIN = 0.80
CORPSE_KILL_MAX_SHARE = 0.05    # kills (seen by the bot) after which it still hit the body / kills seen
CORPSE_TAIL_P95_MAX_S = 1.2     # p95 of the firing at a body after its death was shown (a human stops within a second;
                                # the max was one outlier, TMM_TSV review)
TIMER_SLACK_S = 1.0

RESPAWN_MODES = ("TDM", "FFA", "CTF", "BNT")
TEAM_BALANCE_MODES = ("TDM", "CTF")

# ---------------------------------------------------------------------------------------------------------------
# the matrix (mode key, minutes, map indices); map names of the 2009 ROM
# ---------------------------------------------------------------------------------------------------------------
MAPS = {0: "Snow Valley", 1: "Gray Hammer", 2: "Junk Flea", 3: "Two Towers", 4: "Pump Jack", 5: "Brushwood",
        6: "Cold Seed", 7: "Sand Hog", 8: "Warhead", 9: "Waverider", 10: "Rattlesnake", 11: "Death Room",
        12: "Showdown", 14: "Power Surge", 15: "Overdose", 16: "Grave Digger", 17: "Roadkill"}
TDM_MAPS = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 15, 16, 17]
MATRIX = [
    ("TDM", 6, TDM_MAPS),
    ("TMM", 8, [0, 1, 7, 8, 10, 14]),
    ("FFA", 6, [0, 1, 2, 3, 4, 6, 7, 8, 9, 10, 12, 14, 15, 16]),
    ("CTF", 6, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 14, 16, 17]),
    ("BNT", 6, [0, 1, 3, 7, 8, 9, 14]),
    ("QRT", 8, [2, 3, 8, 12, 15]),
    ("TSV", 8, TDM_MAPS),
]
MODE_NAMES = {"TDM": "Elimination", "TMM": "Search and Destroy", "FFA": "One Man Army", "CTF": "Capture the Flag",
              "BNT": "Spy Hunt", "QRT": "Quarantine", "TSV": "Elimination Pro"}
MODE_ORDER = [m[0] for m in MATRIX]

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def slug(name):
    return re.sub(r"[^A-Za-z0-9]+", "", name)


# ===============================================================================================================
# run
# ===============================================================================================================
def cmd_run(a):
    rom = a.rom or os.environ.get("CA_ROM")
    if not rom:
        sys.exit("bot_eval run: no ROM (set CA_ROM or pass --rom)")
    exe = os.path.join(a.bin, "dc-netlab.exe" if os.name == "nt" else "dc-netlab")
    if not os.path.exists(exe):
        sys.exit(f"bot_eval run: {exe} not found (build dc-netlab release or pass --bin)")
    only = {k for k in a.only.split(",") if k}
    jobs = queue.Queue()
    for key, minutes, maps in MATRIX:
        if only and key not in only:
            continue
        for m in maps:
            jobs.put((key, a.minutes or minutes, m))
    os.makedirs(a.out, exist_ok=True)
    progress = os.path.join(a.out, "progress.txt")
    lock = threading.Lock()
    env = dict(os.environ, CA_ROM=rom)

    def worker(n):
        while True:
            try:
                key, minutes, m = jobs.get_nowait()
            except queue.Empty:
                return
            out = os.path.join(a.out, f"{key}_{m:02d}_{slug(MAPS[m])}")
            if os.path.exists(os.path.join(out, "report.json")):
                continue
            with lock:
                time.sleep(5 * n)  # stagger the starts a little
            cmd = [exe, "run", "--rom", rom, "--mix", f"good:{a.bots}", "--mode", key, "--map", str(m),
                   "--minutes", str(minutes), "--out", out]
            t0 = time.time()
            try:
                r = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=minutes * 60 + 400)
                code, tail = r.returncode, (r.stdout + r.stderr)[-400:].replace("\n", " | ")
            except subprocess.TimeoutExpired:
                code, tail = "timeout", ""
            line = f"{time.strftime('%H:%M:%S')} {key} {m:02d} {MAPS[m]} exit {code} {time.time() - t0:.0f}s {tail}\n"
            with lock:
                with open(progress, "a", encoding="utf-8") as f:
                    f.write(line)
                sys.stdout.write(line)
                sys.stdout.flush()

    threads = [threading.Thread(target=worker, args=(i,), daemon=True) for i in range(a.parallel)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    print("matrix done")
    if not a.no_score:
        score_dir(a.out, partial=False)


# ===============================================================================================================
# reading
# ===============================================================================================================
def read_events(path):
    out = []
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                o = json.loads(line)
            except ValueError:
                continue
            if isinstance(o, dict):
                out.append(o)
    return out


def dist3(a, b):
    return math.sqrt(sum((a[i] - b[i]) ** 2 for i in range(3)))


def dist_xz(a, b):
    return math.hypot(a[0] - b[0], a[2] - b[2])


LINE_RE = re.compile(r"^\s*(\d+)\s+(.*)$")
RE_SESSION_START = re.compile(r"^\s*\d+ dc-dserver: world \S+ \(\d+ bytes")  # first line of a game server session
RE_SESSION_HITS = re.compile(r"^dc-dserver: session hits: (\d+) claims, \d+ accepted.*?refused (.*)$")
RE_IN_WORLD = re.compile(r"^dc-dserver: client (\d+): in world, object (\d+)")
RE = {k: re.compile(v) for k, v in {
    "linked": r"^dc-dserver: client (\d+) linked from",
    "join": r"^@join (\d+) (\d+) \d+ (\d+) (\S+)",
    "spawned": r"^dc-dserver: client (\d+): spawned \(team (\d+)",
    "killed": r"^dc-dserver: client (\d+) killed by (None|Some\((\d+)\)).*?; score (\d+):(\d+)",
    "first_start": r"^dc-dserver: round started \(mode class (\d+), goal (\d+), (\d+) s\)",
    "warmup": r"^dc-dserver: warm-up: (\d+) s",
    "warmup_over": r"^dc-dserver: warm-up over",
    "start": r"^dc-dserver: round (\d+) started \(mode class (\d+), round wins (\d+):(\d+), (\d+) s\)",
    "over": r"^dc-dserver: round (\d+) over: win (\d+), score (\d+):(\d+), round wins (\d+):(\d+)(.*)$",
    "elim": r"^dc-dserver: round \d+: team\(s\) (.+?) without a living player",
    "forfeit": r"^dc-dserver: team (\d+) has had no players",
    "ffa_goal": r"^dc-dserver: ffa: .* reached the goal",
    "match_over": r"^dc-dserver: match over",
    "plant_start": r"^dc-dserver: bomb: client (\d+) starts to plant",
    "defuse_start": r"^dc-dserver: bomb: client (\d+) starts to defuse",
    "broken": r"^dc-dserver: bomb: client (\d+) (plant|defuse) broken off",
    "planted": r"^dc-dserver: bomb: planted at site (\S+) by client (\d+)",
    "defused": r"^dc-dserver: bomb: defused by client (\d+)",
    "exploded": r"^dc-dserver: bomb: exploded at site (\S+)",
    "bomb_cancel": r"^dc-dserver: bomb: the round ended .* bomb cancelled",
    "ctf_taken": r"^dc-dserver: ctf: team (\d+) flag taken by client (\d+)",
    "ctf_dropped": r"^dc-dserver: ctf: team (\d+) flag dropped by client (\d+)",
    "ctf_returned": r"^dc-dserver: ctf: team (\d+) flag returned by (client (\d+)|the timer)",
    "ctf_capture": r"^dc-dserver: ctf: client (\d+) captured the team (\d+) flag, captures (\d+):(\d+)",
    "spy_placed": r"^dc-dserver: spy: (\d+) intel placed, stage 1",
    "spy_seized": r"^dc-dserver: spy: client (\d+) seized intel \((\d+) of (\d+)\)",
    "spy_back": r"^dc-dserver: spy: (\d+ dropped intel back at the spawn points|stage 1 again)",
    "spy_drop": r"^dc-dserver: spy: client (\d+) dropped (\d+) intel",
    "spy_agent": r"^dc-dserver: spy: client (\d+) holds every intel: the agent, stage 2",
    "spy_uploads": r"^dc-dserver: spy: the agent \(client (\d+)\) uploads the intel",
    "spy_uploaded": r"^dc-dserver: spy: the agent \(client (\d+)\) uploaded the intel",
    "spy_done": r"^dc-dserver: spy: the agent \(client (\d+)\) completed the mission",
    "spy_fell": r"^dc-dserver: spy: the agent fell",
    "spy_agent_killed": r"^dc-dserver: spy: the agent \(client (\d+)\) killed by",
    "spy_survived": r"^dc-dserver: spy: the agent \(client (\d+)\) survived stage 2",
    "spy_gone": r"^dc-dserver: spy: the agent \(client (\d+)\) is gone",
    "infect_outbreak": r"^dc-dserver: infect: round (\d+): outbreak",
    "infect_client": r"^dc-dserver: infect: client (\d+) infected(?: by client (\d+))?",
}.items()}


def parse_dserver(path):
    """Everything the score needs from one game server log."""
    d = {"linked": {}, "nick": {}, "uid": {}, "team_at": {}, "join_team": {}, "kills": [], "rounds": [],
         "objective": [], "match_over": False, "refused": 0, "mode_class": None, "goal": None, "round_secs": 180, "lines": 0,
         "objects": {}}
    cur = None
    warmup_over_pending = False
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.readlines()
    # a retried room holds two sessions in one log (an aborted run, then the real one): score the last only
    starts = [i for i, ln in enumerate(lines) if RE_SESSION_START.match(ln)]
    if starts:
        lines = lines[starts[-1]:]
    d["lines"] = len(lines)
    last_ms = 0
    recent = []  # (ms, tag) markers that explain a round end

    def mark(ms, tag):
        recent.append((ms, tag))

    for raw in lines:
        m = LINE_RE.match(raw)
        if not m:
            continue
        ms, text = int(m.group(1)), m.group(2)
        last_ms = ms
        if "refused" in text:
            d["refused"] += 1
        rh = RE_SESSION_HITS.match(text)
        if rh:
            d["claims"] = int(rh.group(1))
            d["refused_kinds"] = {k: int(v) for k, v in re.findall(r"(\w+) (\d+)", rh.group(2))}
        iw = RE_IN_WORLD.match(text)
        if iw:
            d["objects"].setdefault(int(iw.group(1)), set()).add(int(iw.group(2)))
        g = None
        for key, rx in RE.items():
            g = rx.match(text)
            if g:
                break
        if not g:
            continue
        if key == "linked":
            d["linked"][int(g.group(1))] = ms
        elif key == "join":
            c = int(g.group(1))
            d["uid"][c] = int(g.group(2))
            d["join_team"][c] = int(g.group(3)) + 1  # 0-based on the @join line, 1-based like `spawned`
            d["nick"][c] = g.group(4)
        elif key == "spawned":
            d["team_at"].setdefault(int(g.group(1)), []).append((ms, int(g.group(2))))
            if cur is not None:
                cur["spawned"].setdefault(int(g.group(1)), int(g.group(2)))
        elif key == "killed":
            killer = int(g.group(3)) if g.group(3) is not None else None
            k = {"ms": ms, "victim": int(g.group(1)), "killer": killer,
                 "round": cur["no"] if cur else None, "warmup": bool(cur and cur["warmup"])}
            d["kills"].append(k)
            if cur is not None:
                cur["kills"].append(k)
        elif key in ("first_start", "start"):
            if key == "first_start":
                mc, goal, secs, no = int(g.group(1)), int(g.group(2)), int(g.group(3)), 1
                d["mode_class"], d["goal"], d["round_secs"] = mc, goal, secs
            else:
                no, secs = int(g.group(1)), int(g.group(5))
            cur = {"no": no, "start": ms, "end": None, "secs": secs, "warmup": False, "win": None, "score": None,
                   "round_wins": None, "how": None, "kills": [], "spawned": {}, "plant_starts": 0, "plants": 0,
                   "defuse_starts": 0, "defused": False, "exploded": False, "match_over": False}
            d["rounds"].append(cur)
            recent.clear()
        elif key == "warmup":
            if cur is not None:
                cur["warmup"] = True
        elif key == "warmup_over":
            warmup_over_pending = True
        elif key == "over":
            if cur is None:
                continue
            cur["end"] = ms
            cur["win"] = int(g.group(2))
            cur["score"] = (int(g.group(3)), int(g.group(4)))
            cur["round_wins"] = (int(g.group(5)), int(g.group(6)))
            cur["match_over"] = "match over" in g.group(7)
            if warmup_over_pending:
                cur["warmup"] = True
                warmup_over_pending = False
            cur["how"] = classify_round_end(cur, recent, d)
            cur = None
            recent.clear()
        elif key == "match_over":
            d["match_over"] = True
        else:
            ev = {"ms": ms, "kind": key, "args": g.groups(), "round": cur["no"] if cur else None}
            if key == "infect_client" and g.group(2) is not None:
                # an infection is the infector's kill and the victim's death (not a round-alive change: kept out of
                # the round's kills); the outbreak's first infection has no infector and is no kill
                d["kills"].append({"ms": ms, "victim": int(g.group(1)), "killer": int(g.group(2)),
                                   "round": cur["no"] if cur else None, "warmup": bool(cur and cur["warmup"]),
                                   "infection": True})
            d["objective"].append(ev)
            if key in ("elim", "forfeit", "ffa_goal", "defused", "exploded", "ctf_capture", "spy_done",
                       "spy_fell", "spy_uploaded", "spy_survived"):
                mark(ms, key)
            if cur is not None:
                if key == "plant_start":
                    cur["plant_starts"] += 1
                elif key == "planted":
                    cur["plants"] += 1
                elif key == "defuse_start":
                    cur["defuse_starts"] += 1
                elif key == "defused":
                    cur["defused"] = True
                elif key == "exploded":
                    cur["exploded"] = True
    if cur is not None:  # the run ended inside a round
        cur["end"] = last_ms
        cur["how"] = "cut"
    d["last_ms"] = last_ms
    boom = [e["ms"] for e in d["objective"] if e["kind"] == "exploded"]
    for k in d["kills"]:  # a death with no killer at the detonation is the bomb, not a fall
        if k["killer"] is None and any(abs(k["ms"] - b) <= SND_BOMB_DEATH_MS for b in boom):
            k["bomb"] = True
    # Spy Hunt and Quarantine put every player on one game team in `spawned`; the sides then come from `@join`
    spawn_teams = {tm for rows in d["team_at"].values() for _, tm in rows}
    d["team_flat"] = len(spawn_teams) <= 1 and len(set(d["join_team"].values())) > 1
    return d


def classify_round_end(r, recent, d):
    """How a round ended (see the module docstring, "round end")."""
    tags = [t for (ms, t) in recent if r["end"] - ms <= 1000]
    for tag, how in (("exploded", "bomb"), ("defused", "defuse"), ("elim", "elimination"), ("forfeit", "forfeit"),
                     ("ctf_capture", "capture"), ("spy_done", "mission"), ("spy_uploaded", "mission"),
                     ("spy_fell", "mission"), ("spy_survived", "mission"), ("ffa_goal", "goal")):
        if tag in tags:
            return how
    if r["warmup"]:
        return "warm-up"
    if (r["end"] - r["start"]) / 1000.0 >= r["secs"] - TIMER_SLACK_S:
        return "timer"
    goal = d.get("goal") or 0
    if goal and r["score"] and max(r["score"]) >= goal:
        return "goal"
    return "other"


def team_of(ds, client, ms):
    """The client's game team at server time ms (the latest `spawned` before it, else its first)."""
    if ds.get("team_flat"):
        return ds["join_team"].get(client)
    rows = ds["team_at"].get(client) or []
    team = rows[0][1] if rows else None
    for t, tm in rows:
        if t <= ms:
            team = tm
    return team


# ===============================================================================================================
# per-bot metrics (from the events)
# ===============================================================================================================
def running_at(ds, sms):
    """The running round (not a result pause) holding server time sms, or None."""
    for r in ds["rounds"]:
        if r["start"] <= sms <= (r["end"] if r["end"] is not None else ds["last_ms"]):
            return r
    return None


def life_segments(tracks, deaths_ms):
    """Split the track samples into life segments."""
    segs, cur = [], []
    di = sorted(deaths_ms)
    for s in tracks:
        if cur:
            gap = s["t"] - cur[-1]["t"]
            died_between = any(cur[-1]["ms"] < x <= s["ms"] for x in di)
            if gap > SEGMENT_GAP_S or died_between or s.get("_round") is not cur[-1].get("_round"):
                segs.append(cur)
                cur = []
        cur.append(s)
    if cur:
        segs.append(cur)
    return segs


def metric_stuck(segs):
    """Stuck episodes: a goal > STUCK_GOAL_U away, the body within STUCK_RADIUS_U of the start for >= STUCK_MIN_S."""
    eps = []
    for seg in segs:
        i = 0
        while i < len(seg):
            s0 = seg[i]
            j = i
            if s0.get("goal") and dist3(s0["pos"], s0["goal"]) > STUCK_GOAL_U:
                while (j + 1 < len(seg) and seg[j + 1].get("goal")
                       and dist3(seg[j + 1]["pos"], seg[j + 1]["goal"]) > STUCK_GOAL_U
                       and dist3(seg[j + 1]["pos"], s0["pos"]) <= STUCK_RADIUS_U):
                    j += 1
            dur = seg[j]["t"] - s0["t"]
            if j > i and dur >= STUCK_MIN_S:
                eps.append({"t": round(s0["t"], 1), "secs": round(dur, 1), "pos": [round(v) for v in s0["pos"]],
                            "goal": [round(v) for v in s0["goal"]], "kind": s0.get("kind")})
                i = j + 1
            else:
                i += 1
    return eps


def in_countdown(s):
    r = s.get("_round")
    return bool(r) and s.get("_sms") is not None and s["_sms"] - r["start"] < ROUND_COUNTDOWN_S * 1000


def metric_idle(segs, contacts_ms=(), plant_ms=None):
    """Idle episodes: no goal, no enemy in sight, the body within IDLE_RADIUS_U of the start for >= IDLE_MIN_S
    (>= IDLE_GUARD_MIN_S for a guard / post kind), outside a round's countdown. `contact`: an enemy was seen in the
    CONTACT_S before the episode (a hold with contact may be allowed by the rubric)."""
    eps = []
    cms = sorted(contacts_ms)
    for seg in segs:
        i = 0
        while i < len(seg):
            s0 = seg[i]
            j = i
            if s0.get("goal") is None and not s0.get("sight") and not in_countdown(s0):
                while (j + 1 < len(seg) and seg[j + 1].get("goal") is None and not seg[j + 1].get("sight")
                       and dist3(seg[j + 1]["pos"], s0["pos"]) <= IDLE_RADIUS_U):
                    j += 1
            dur = seg[j]["t"] - s0["t"]
            need = IDLE_GUARD_MIN_S if s0.get("kind") in IDLE_GUARD_KINDS else IDLE_MIN_S
            rno = s0["_round"]["no"] if s0.get("_round") else None
            if (str(s0.get("job") or "").startswith("Hold") and plant_ms is not None
                    and (rno not in plant_ms or seg[j].get("_sms", 0) < plant_ms[rno])):
                need = max(need, IDLE_HOLD_JOB_MIN_S)  # a Hold job holds an angle until the plant
            if j > i and dur >= need:
                t0 = s0["ms"]
                contact = any(t0 - CONTACT_S * 1000 <= c <= t0 for c in cms)
                eps.append({"t": round(s0["t"], 1), "secs": round(dur, 1), "pos": [round(v) for v in s0["pos"]],
                            "kind": s0.get("kind"), "wait": s0.get("wait"), "contact": contact})
                i = j + 1
            else:
                i += 1
    return eps


def metric_rooted(segs):
    """Rooted while engaged: the target in sight and the body within IDLE_RADIUS_U for >= ROOTED_MIN_S (a turret)."""
    eps = []
    for seg in segs:
        i = 0
        while i < len(seg):
            s0 = seg[i]
            j = i
            if s0.get("sight"):
                while (j + 1 < len(seg) and seg[j + 1].get("sight")
                       and dist3(seg[j + 1]["pos"], s0["pos"]) <= IDLE_RADIUS_U):
                    j += 1
            dur = seg[j]["t"] - s0["t"]
            if j > i and dur >= ROOTED_MIN_S:
                eps.append({"t": round(s0["t"], 1), "secs": round(dur, 1), "pos": [round(v) for v in s0["pos"]],
                            "tgt_dist": round(s0.get("tgt_dist") or 0)})
                i = j + 1
            else:
                i += 1
    return eps


def metric_pacing(segs, stuck):
    """Ledge pacing the 48 u stuck window misses: within PACE_CELL_U (flat) of a start for >= PACE_MIN_S while the
    goal stays > PACE_GOAL_U away; episodes that overlap a stuck episode are not counted twice."""
    eps = []
    for seg in segs:
        i = 0
        while i < len(seg):
            s0 = seg[i]
            j = i
            if s0.get("goal") and dist3(s0["pos"], s0["goal"]) > PACE_GOAL_U:
                while (j + 1 < len(seg) and seg[j + 1].get("goal")
                       and dist3(seg[j + 1]["pos"], seg[j + 1]["goal"]) > PACE_GOAL_U
                       and dist_xz(seg[j + 1]["pos"], s0["pos"]) <= PACE_CELL_U):
                    j += 1
            dur = seg[j]["t"] - s0["t"]
            if j > i and dur >= PACE_MIN_S:
                t0, t1 = s0["t"], seg[j]["t"]
                if not any(e["t"] <= t1 and e["t"] + e["secs"] >= t0 for e in stuck):
                    eps.append({"t": round(t0, 1), "secs": round(dur, 1), "pos": [round(v) for v in s0["pos"]],
                                "goal": [round(v) for v in s0["goal"]], "kind": "pacing"})
                i = j + 1
            else:
                i += 1
    return eps


def metric_pogo(jumps):
    """Pogo runs: POGO_N or more `stuck_jump` events within POGO_RADIUS_U of the run's first, gaps under POGO_GAP_S."""
    js = sorted(jumps, key=lambda e: e["t"])
    runs, i = [], 0
    while i < len(js):
        j = i
        while (j + 1 < len(js) and js[j + 1]["t"] - js[j]["t"] < POGO_GAP_S
               and dist3(js[j + 1]["pos"], js[i]["pos"]) <= POGO_RADIUS_U):
            j += 1
        if j - i + 1 >= POGO_N:
            runs.append({"t": round(js[i]["t"], 1), "n": j - i + 1, "secs": round(js[j]["t"] - js[i]["t"], 1),
                         "pos": [round(v) for v in js[i]["pos"]]})
            i = j + 1
        else:
            i += 1
    return runs


def metric_confined(segs):
    """Confined: CONFINED_MIN_S inside a CONFINED_BOX_U (x, z) box with the goal farther than CONFINED_GOAL_U."""
    def far(x):
        return bool(x.get("goal")) and dist3(x["pos"], x["goal"]) > CONFINED_GOAL_U

    eps = []
    for seg in segs:
        i = 0
        while i < len(seg):
            s0 = seg[i]
            j = i
            if far(s0):
                x0 = x1 = s0["pos"][0]
                z0 = z1 = s0["pos"][2]
                while j + 1 < len(seg) and far(seg[j + 1]):
                    p = seg[j + 1]["pos"]
                    if max(x1, p[0]) - min(x0, p[0]) > CONFINED_BOX_U or max(z1, p[2]) - min(z0, p[2]) > CONFINED_BOX_U:
                        break
                    x0, x1, z0, z1 = min(x0, p[0]), max(x1, p[0]), min(z0, p[2]), max(z1, p[2])
                    j += 1
            dur = seg[j]["t"] - s0["t"]
            if j > i and dur >= CONFINED_MIN_S:
                eps.append({"kind": "box", "t": round(s0["t"], 1), "secs": round(dur, 1),
                            "box": [round(x1 - x0), round(z1 - z0)], "pos": [round(v) for v in s0["pos"]],
                            "goal": [round(v) for v in s0["goal"]]})
                i = j + 1
            else:
                i += 1
    return eps


def metric_islands(evs):
    """The nav `islands` count growing by CONFINED_ISLANDS inside one life (a `died` starts a new one)."""
    out = []
    lo = hi = None
    done = False
    for e in evs:
        k = e.get("ev")
        if k == "died":
            lo = hi = None
            done = False
        elif k == "nav" and e.get("islands") is not None:
            v = e["islands"]
            lo = v if lo is None else min(lo, v)
            hi = v if hi is None else max(hi, v)
            if not done and hi - lo >= CONFINED_ISLANDS:
                out.append({"kind": "islands", "t": round(e.get("t") or 0, 1), "secs": 0, "growth": hi - lo})
                done = True
    return out


def band_of(d):
    for i, (lo, hi, _a, _b) in enumerate(SPY_BANDS):
        if lo <= d < hi:
            return i
    return len(SPY_BANDS) - 1


def metric_bands(evs):
    """Shots and hits by distance band. A hit's `fire` carries `dist`; a miss has none, so its distance is the
    origin to the last known position of the target the following `miss` event names."""
    miss_pid = {e["ms"]: e.get("pid") for e in evs if e.get("ev") == "miss" and e.get("ms") is not None}
    last = {}
    out = {"shots": [0] * len(SPY_BANDS), "hits": [0] * len(SPY_BANDS), "unknown": 0, "hits_total": 0, "hits_near": 0}
    for e in evs:
        k = e.get("ev")
        if k in ("target", "target_seen") and e.get("pid") is not None and e.get("pos"):
            last[e["pid"]] = e["pos"]
        elif k == "sight" and e.get("pid") is not None and e.get("target_pos"):
            last[e["pid"]] = e["target_pos"]
        elif k == "fire":
            hit = e.get("target_obj") is not None
            d = e.get("dist")
            if d is None and not hit:
                p, o = last.get(miss_pid.get(e.get("ms"))), e.get("origin")
                if p and o:
                    d = dist3(o, p)
            if hit:
                out["hits_total"] += 1
                if d is not None and d < SPY_POINT_BLANK_U:
                    out["hits_near"] += 1
            if d is None:
                out["unknown"] += 1
                continue
            i = band_of(d)
            out["shots"][i] += 1
            out["hits"][i] += 1 if hit else 0
    return out


def metric_goals(segs):
    """Goal episodes and the route verdict (ok within ROUTE_MAX_S / late / abandoned / unjudged far goals)."""
    res = {"ok": 0, "late": 0, "abandoned": 0, "far": 0, "moving": 0, "by_arrived": 0, "late_list": []}

    def close(ep):
        if ep is None:
            return
        if ep["kind"] in ROUTE_SKIP_KINDS:
            res["moving"] += 1
        elif ep["d0"] >= ROUTE_MAX_GOAL_U:
            res["far"] += 1
        elif ep["reach_t"] is not None and ep["reach_t"] - ep["t0"] <= ROUTE_MAX_S:
            res["ok"] += 1
        elif ep["reach_t"] is not None or ep["t1"] - ep["t0"] > ROUTE_MAX_S:
            res["late"] += 1
            res["late_list"].append({"t": round(ep["t0"], 1), "kind": ep["kind"], "d0": round(ep["d0"]),
                                     "secs": round((ep["reach_t"] or ep["t1"]) - ep["t0"], 1),
                                     "pos": [round(v) for v in ep["pos"]],
                                     "goal": [round(v) for v in ep["last_goal"]]})
        else:
            res["abandoned"] += 1

    for seg in segs:
        ep = None
        for s in seg:
            g = s.get("goal")
            if g is None:
                if ep is not None and s["t"] - ep["t1"] > ROUTE_GAP_S:
                    close(ep)
                    ep = None
                continue
            if ep is not None and (s.get("kind") != ep["kind"] or dist3(g, ep["last_goal"]) > ROUTE_RETARGET_U):
                close(ep)
                ep = None
            if ep is None:
                ep = {"t0": s["t"], "t1": s["t"], "kind": s.get("kind"), "d0": dist3(s["pos"], g), "reach_t": None}
            ep["t1"] = s["t"]
            ep["last_goal"] = g
            ep["pos"] = s["pos"]
            if s.get("arrived") or dist3(s["pos"], g) <= GOAL_REACH_U:
                ep["reach_t"] = s["t"]
                if s.get("arrived"):
                    res["by_arrived"] += 1
                close(ep)
                ep = None
        close(ep)
    return res


def cells_of(tracks):
    return {(math.floor(s["pos"][0] / CELL_U), math.floor(s["pos"][2] / CELL_U)) for s in tracks}


def metric_reaction(evs):
    """Planned delays (`reaction`) and measured sight -> first shot latencies (first sightings / re-acquires)."""
    planned = [e["delay_ms"] / 1000.0 for e in evs if e.get("ev") == "reaction" and e.get("delay_ms") is not None
               and (e.get("kind") == "new" if e.get("kind") else not e.get("reacquire"))]
    planned_re = [e["delay_ms"] / 1000.0 for e in evs if e.get("ev") == "reaction" and e.get("reacquire")
                  and e.get("delay_ms") is not None]
    first, again = [], []
    cur = None  # [pid, reaction ms, reacquire]
    for e in evs:
        k = e.get("ev")
        if k == "reaction" and e.get("ms") is not None:
            cur = [e.get("pid"), e["ms"], bool(e.get("reacquire"))]
        elif k == "target_unseen" and cur is not None and e.get("pid") == cur[0]:
            cur = None
        elif k == "fire" and cur is not None:
            (again if cur[2] else first).append((e["ms"] - cur[1]) / 1000.0)
            cur = None
    return planned, planned_re, first, again


def score_bot(nick, evs, client, ds):
    """All per-bot metrics of one bot."""
    off = ds["linked"].get(client, 0) if client is not None else 0
    tracks = []
    for e in evs:
        if e.get("ev") == "track" and e.get("pos") and e.get("ms") is not None:
            r = running_at(ds, e["ms"] + off)
            if r is None and ds["rounds"]:
                continue  # a result pause / between rounds: not "in a running round"
            e["_round"] = r
            e["_sms"] = e["ms"] + off
            tracks.append(e)
    fresh_t = [e["t"] for e in evs if e.get("ev") in ("spawn", "respawned") and e.get("t") is not None]
    for s in tracks:  # just spawned / respawned / the round just started: not judged for mate stacking
        r = s["_round"]
        s["_fresh"] = (any(0 <= s["t"] - x <= MATE_STACK_SPAWN_S for x in fresh_t)
                       or bool(r and s["_sms"] - r["start"] < MATE_STACK_SPAWN_S * 1000))
    plant_ms = {}
    for e in ds["objective"]:
        if e["kind"] == "planted":
            plant_ms.setdefault(e["round"], e["ms"])
    deaths_ms = [e["ms"] for e in evs if e.get("ev") == "died" and e.get("ms") is not None]
    contacts = [e["ms"] for e in evs if e.get("ev") in ("target_seen", "reaction") and e.get("ms") is not None]
    segs = life_segments(tracks, deaths_ms)
    stuck = metric_stuck(segs)
    stuck += metric_pacing(segs, stuck)
    stuck.sort(key=lambda x: x["t"])
    idle = metric_idle(segs, contacts, plant_ms)
    jumps = [e for e in evs if e.get("ev") == "stuck_jump" and e.get("pos") and e.get("t") is not None]
    pogo = metric_pogo(jumps)
    confined = metric_confined(segs) + metric_islands(evs)
    rooted = metric_rooted(segs)
    goals = metric_goals(segs)
    planned, planned_re, measured, measured_re = metric_reaction(evs)
    alive_s = len(tracks) * TRACK_PERIOD_S
    rep = next((e for e in reversed(evs) if e.get("ev") == "report"), None)
    sh = (rep or {}).get("shooter") or {}
    sniper = any(e.get("ev") == "weapon_select" and e.get("sniper") for e in evs)
    targets = sum(1 for e in evs if e.get("ev") == "target")
    team = next((e.get("team") for e in reversed(tracks) if e.get("team") is not None), None)
    idle_kinds = {}
    for ep in idle:
        idle_kinds[ep["kind"] or "-"] = round(idle_kinds.get(ep["kind"] or "-", 0) + ep["secs"], 1)
    return {
        "nick": nick, "client": client, "team": team, "alive_s": alive_s,
        "tracks": tracks,  # removed before writing
        "stuck": stuck, "stuck_n": len(stuck), "stuck_longest_s": max([s["secs"] for s in stuck] or [0]),
        "idle": idle, "idle_s": round(sum(s["secs"] for s in idle), 1), "idle_by_kind": idle_kinds,
        "rooted": rooted, "rooted_s": round(sum(s["secs"] for s in rooted), 1),
        "guard_share": round(sum(1 for s in tracks if s.get("kind") in IDLE_GUARD_KINDS) / len(tracks), 2)
        if tracks else 0.0,
        "fire_sms": [e["ms"] + off for e in evs if e.get("ev") == "fire" and e.get("ms") is not None],
        "goals": goals,
        "stuck_jumps": sum(1 for e in evs if e.get("ev") == "stuck_jump"),
        "stuck_jump_per_min": round(sum(1 for e in evs if e.get("ev") == "stuck_jump") / (alive_s / 60.0), 2)
        if alive_s else 0.0,
        "stuck_jumps_nonface": sum(1 for e in jumps if not e.get("face")),
        "stuck_jump_nonface_per_min": round(sum(1 for e in jumps if not e.get("face")) / (alive_s / 60.0), 2)
        if alive_s else 0.0,
        "pogo": pogo, "confined": confined, "bands": metric_bands(evs),
        "reaction_planned_s": planned, "reaction_reacquire_s": planned_re, "reaction_measured_s": measured,
        "reaction_reacquire_measured_s": measured_re,
        "shots": sh.get("shots"), "hit_claims": sh.get("hit_claims"), "kills_by_me": sh.get("kills_by_me"),
        "sniper": sniper, "targets": targets,
        "died_events": len(deaths_ms),
        "target_down_mine": sum(1 for e in evs if e.get("ev") == "target_down" and e.get("mine")),
        "end": (rep or {}).get("end"),
        "join_failed": sum(1 for e in evs if e.get("ev") == "join_failed"),
        "bomb_carrier": [e["ms"] + off for e in evs if e.get("ev") == "bomb_carrier" and e.get("me")],
        "squad": [(e["ms"] + off, e.get("job"), e.get("round")) for e in evs if e.get("ev") == "squad"
                  and e.get("ms") is not None],
    }


# ===============================================================================================================
# room
# ===============================================================================================================
def find_rooms(root, partial):
    """(room folder, scenario dict or None) for every room under root."""
    rooms = []

    def add(folder):
        rp = os.path.join(folder, "report.json")
        if os.path.exists(rp):
            try:
                rep = json.load(open(rp, encoding="utf-8"))
            except ValueError:
                rep = None
            scens = (rep or {}).get("scenarios") or [None]
            for sc in scens:
                rooms.append((folder, sc))
        elif partial and glob.glob(os.path.join(folder, "*", "*.events.jsonl")):
            rooms.append((folder, None))

    if os.path.exists(os.path.join(root, "report.json")):
        add(root)
    else:
        for sub in sorted(os.listdir(root)):
            p = os.path.join(root, sub)
            if os.path.isdir(p):
                add(p)
    return rooms


def scenario_files(folder, sc):
    """The scenario's events folder and its game server log (the log whose roster nicks match the bots)."""
    sdir = None
    if sc and sc.get("dir"):
        cand = sc["dir"] if os.path.isabs(sc["dir"]) else os.path.join(folder, sc["dir"])
        if os.path.isdir(cand):
            sdir = cand
    if sdir is None and sc and sc.get("name"):
        hits = glob.glob(os.path.join(folder, "*-" + sc["name"]))
        sdir = hits[0] if hits else None
    if sdir is None:
        hits = sorted({os.path.dirname(p) for p in glob.glob(os.path.join(folder, "*", "*.events.jsonl"))})
        sdir = hits[0] if hits else folder
    nicks = {os.path.basename(p)[:-len(".events.jsonl")] for p in glob.glob(os.path.join(sdir, "*.events.jsonl"))}
    best, best_n = None, -1
    for log in glob.glob(os.path.join(folder, "dserver", "*.log")):
        with open(log, encoding="utf-8", errors="replace") as f:
            head = f.read(200000)
        n = len(nicks & set(re.findall(r"roster: uid \d+ team \d+ nick (\S+)", head)))
        if n > best_n:
            best, best_n = log, n
    return sdir, nicks, best


def room_meta(folder, sc):
    key, mapn, minutes = None, None, None
    m = re.match(r"^([A-Z]+)_(\d\d)_", os.path.basename(os.path.normpath(folder)))
    if sc:
        key, mapn, minutes = sc.get("mode"), sc.get("map"), sc.get("minutes")
    if m:
        key = key or m.group(1)
        if mapn is None:
            mapn = MAPS.get(int(m.group(2)), m.group(2))
    return key, mapn, minutes


def score_room(folder, sc):
    key, mapname, minutes = room_meta(folder, sc)
    sdir, nicks, log = scenario_files(folder, sc)
    ds = parse_dserver(log) if log else parse_dserver_empty()
    client_of = {}
    evs_of = {}
    for nick in sorted(nicks):
        evs = read_events(os.path.join(sdir, nick + ".events.jsonl"))
        evs_of[nick] = evs
        own = next((e for e in evs if e.get("ev") == "roster" and e.get("own")), None)
        if own is not None:
            client_of[nick] = own.get("pid")
    for c, n in ds["nick"].items():
        client_of.setdefault(n, c)
    nick_of = {c: n for n, c in client_of.items() if c is not None}
    if minutes is None:
        tmax = max((e.get("t", 0) for evs in evs_of.values() for e in evs[-50:]), default=0)
        minutes = round(tmax / 60.0, 1)
    bots = [score_bot(n, evs_of[n], client_of.get(n), ds) for n in sorted(nicks)]
    netlab_bot = {rb.get("nick"): rb.get("bot") or {} for rb in (sc or {}).get("bots") or []}
    for b in bots:  # a bot without its end report (killed, crashed): netlab's per-bot numbers
        for f in ("shots", "hit_claims", "kills_by_me"):
            if b[f] is None:
                b[f] = netlab_bot.get(b["nick"], {}).get(f)
    by_client = {b["client"]: b for b in bots if b["client"] is not None}

    # kills / deaths from the server
    for b in bots:
        b.update(kills=0, deaths=0, world_deaths=0, team_kills=0, killed_by={})
    for k in ds["kills"]:
        if k["warmup"]:
            continue
        v = by_client.get(k["victim"])
        if v is not None:
            v["deaths"] += 1
            if k["killer"] is None and not k.get("bomb"):
                v["world_deaths"] += 1
            else:
                kn = nick_of.get(k["killer"], f"client {k['killer']}")
                v["killed_by"][kn] = v["killed_by"].get(kn, 0) + 1
        if k["killer"] is not None and k["killer"] != k["victim"]:
            kb = by_client.get(k["killer"])
            if kb is None:
                continue
            tk = team_of(ds, k["killer"], k["ms"])
            tv = team_of(ds, k["victim"], k["ms"])
            if key != "FFA" and not ds.get("team_flat") and not k.get("infection") and tk is not None and tk == tv:
                kb["team_kills"] += 1
            else:
                kb["kills"] += 1
    for b in bots:
        b["kd"] = round(b["kills"] / max(1, b["deaths"]), 2)
        if b["team"] is None and b["client"] is not None:
            b["team"] = team_of(ds, b["client"], ds["last_ms"])

    room = {"folder": folder, "mode": key, "map": mapname, "minutes": minutes, "bots": len(bots),
            "complete": sc is not None, "dserver_log": os.path.basename(log) if log else None,
            "rounds": [round_row(r) for r in ds["rounds"]], "match_over": ds["match_over"],
            "refused_lines": ds["refused"]}
    room["totals"] = room_totals(bots, ds, key)
    room["objective"] = objective_metrics(key, ds, bots)
    room["flow"] = flow_metrics(sdir, bots, ds, sc, evs_of)
    room["pool"] = pool_data(key, room, ds)
    room["rubric"] = rubric(key, minutes, bots, room, ds)
    room["verdict"] = "PASS" if all(l["ok"] for l in room["rubric"] if l["ok"] is not None) else "FAIL"
    for b in bots:
        b.pop("tracks", None)
        b.pop("bomb_carrier", None)
        b.pop("squad", None)
    room["per_bot"] = bots
    return room


def pool_data(key, room, ds):
    """What the summary pools over the rooms of a mode (see pooled_lines)."""
    o = room["objective"]
    if key in ("TMM", "TSV"):
        rw = next((r["round_wins"] for r in reversed(real_rounds(ds)) if r.get("round_wins")), None)
        return {"round_wins": list(rw) if rw else None}
    if key == "QRT":
        return {"rounds": o.get("rounds", 0), "human_wins": o.get("human_wins_valid", 0)}
    if key == "BNT":
        return {"agent_rounds": o.get("agent_rounds", 0), "agent_wins": o.get("agent_wins", 0)}
    return {}


def parse_dserver_empty():
    return {"linked": {}, "nick": {}, "uid": {}, "team_at": {}, "join_team": {}, "kills": [], "rounds": [],
            "objective": [], "match_over": False, "refused": 0, "mode_class": None, "goal": None, "round_secs": 180, "last_ms": 0,
            "objects": {},
            "lines": 0}


def round_row(r):
    return {"no": r["no"], "warmup": r["warmup"], "start_s": round(r["start"] / 1000.0, 1),
            "secs": round(((r["end"] or r["start"]) - r["start"]) / 1000.0, 1), "how": r["how"], "win": r["win"],
            "score": r["score"], "round_wins": r["round_wins"], "kills": len(r["kills"]),
            "plant_starts": r["plant_starts"], "plants": r["plants"], "defuse_starts": r["defuse_starts"]}


def real_rounds(ds):
    return [r for r in ds["rounds"] if not r["warmup"] and r["how"] not in ("cut",)]


def mate_stacking(bots):
    """Per bot: the share of its samples with a team mate within MATE_STACK_U (same server second), the samples just
    after a spawn / respawn / round start (MATE_STACK_SPAWN_S) left out."""
    import bisect
    by_team = {}
    for b in bots:
        if b["team"] is None:
            continue
        pts = sorted(((s["_sms"], s["pos"], bool(s.get("_fresh"))) for s in b["tracks"] if s.get("_sms") is not None),
                     key=lambda p: p[0])
        by_team.setdefault(b["team"], []).append((b["nick"], [p[0] for p in pts], [p[1] for p in pts],
                                                  [p[2] for p in pts]))
    out = {}
    for team, members in by_team.items():
        for nick, ts, ps, fs in members:
            n = hit = 0
            for t, p, fresh in zip(ts, ps, fs):
                if fresh:
                    continue
                n += 1
                near = False
                for other, ots, ops, _of in members:
                    if other == nick or not ots:
                        continue
                    k = bisect.bisect_left(ots, t)
                    for kk in (k - 1, k):
                        if 0 <= kk < len(ots) and abs(ots[kk] - t) <= 1000 and dist3(ops[kk], p) <= MATE_STACK_U:
                            near = True
                            break
                    if near:
                        break
                hit += near
            if n:
                out[nick] = round(hit / n, 3)
    return out


def room_totals(bots, ds, key):
    kills = [k for k in ds["kills"] if not k["warmup"] and not k.get("infection")]
    infections = sum(1 for k in ds["kills"] if not k["warmup"] and k.get("infection"))
    team_kills = {1: 0, 2: 0}
    for k in ds["kills"]:  # kill lines and infections
        if k["warmup"] or k["killer"] is None or k["killer"] == k["victim"]:
            continue
        tk, tv = team_of(ds, k["killer"], k["ms"]), team_of(ds, k["victim"], k["ms"])
        # one game team in `spawned` (Spy Hunt, Quarantine): the sides are only the lobby teams, no friendly fire
        if key == "FFA" or ds.get("team_flat") or tk != tv or k.get("infection"):
            if tk in team_kills:
                team_kills[tk] += 1
    kds = [b["kd"] for b in bots]
    cells_all = set()
    cells_team = {}
    for b in bots:
        c = cells_of(b["tracks"])
        cells_all |= c
        cells_team.setdefault(b["team"], set()).update(c)
    planned = [x for b in bots for x in b["reaction_planned_s"]]
    measured = [x for b in bots for x in b["reaction_measured_s"]]
    stack = mate_stacking(bots)
    return {
        "kills": sum(b["kills"] for b in bots), "server_kill_lines": len(kills), "infections": infections,
        "mate_stack": stack, "mate_stack_share_max": max([v for v in stack.values()] or [0.0]),
        "world_deaths": sum(1 for k in kills if k["killer"] is None and not k.get("bomb")),
        "bomb_deaths": sum(1 for k in kills if k["killer"] is None and k.get("bomb")),
        "team_kills": sum(b["team_kills"] for b in bots),
        "kills_by_team": team_kills,
        "kd_spread": round(max(kds) - min(kds), 2) if kds else 0.0,
        "stuck": sum(b["stuck_n"] for b in bots), "stuck_longest_s": max([b["stuck_longest_s"] for b in bots] or [0]),
        "idle_s": round(sum(b["idle_s"] for b in bots), 1),
        "stuck_jumps": sum(b["stuck_jumps"] for b in bots),
        "alive_min": round(sum(b["alive_s"] for b in bots) / 60.0, 1),
        "goals_ok": sum(b["goals"]["ok"] for b in bots), "goals_late": sum(b["goals"]["late"] for b in bots),
        "goals_abandoned": sum(b["goals"]["abandoned"] for b in bots),
        "goals_by_arrived": sum(b["goals"]["by_arrived"] for b in bots),
        "cells_all": len(cells_all),
        "coverage_by_team": {str(t): round(len(c) / len(cells_all), 2) for t, c in cells_team.items()
                             if t is not None and cells_all},
        "reaction_planned_median_s": round(statistics.median(planned), 3) if planned else None,
        "reaction_measured_median_s": round(statistics.median(measured), 3) if measured else None,
        "reaction_measured_n": len(measured),
        "reaction_measured_instant": sum(1 for x in measured if x < REACTION_INSTANT_S),
        "reaction_measured_over_max": sum(1 for x in measured if x > REACTION_MAX_S),
        "reaction_measured_max_s": round(max(measured), 2) if measured else None,
        "shots": sum(b["shots"] or 0 for b in bots), "hit_claims": sum(b["hit_claims"] or 0 for b in bots),
    }


# ---------------------------------------------------------------------------------------------------------------
# objectives
# ---------------------------------------------------------------------------------------------------------------
def objective_metrics(key, ds, bots):
    o = {"events": {}}
    for e in ds["objective"]:
        o["events"][e["kind"]] = o["events"].get(e["kind"], 0) + 1
    rounds = real_rounds(ds)
    o["rounds"] = len(rounds)
    o["round_ends"] = {}
    for r in rounds:
        how = r["how"] or "other"  # a round the classifier did not place has how None
        o["round_ends"][how] = o["round_ends"].get(how, 0) + 1
    if key == "TMM":
        o.update(snd_metrics(ds, rounds, bots))
    elif key == "CTF":
        o.update(ctf_metrics(ds))
    elif key == "BNT":
        o.update(spy_metrics(ds, bots))
    elif key == "TSV" or key == "QRT":
        o.update(elim_metrics(ds, rounds, bots, key))
    return o


def snd_metrics(ds, rounds, bots):
    """S&D: plant attempts, defuse after plant, how rounds end, attackers leave the spawn, defenders split."""
    plant_rounds = sum(1 for r in rounds if r["plant_starts"] > 0)
    planted = [r for r in rounds if r["plants"] > 0]
    defuse_tried = sum(1 for r in planted if r["defuse_starts"] > 0)
    decided = sum(1 for r in rounds if r["how"] in ("bomb", "defuse", "elimination"))
    left_spawn, split = [], []
    for r in rounds:
        end = r["end"] or ds["last_ms"]
        att_team = None
        for b in bots:
            if any(r["start"] <= t <= end for t in b["bomb_carrier"]):
                att_team = team_of(ds, b["client"], r["start"] + 1)
                break
        if att_team is None:
            continue
        far = []
        holds = set()
        for b in bots:
            if b["client"] is None:
                continue
            tm = team_of(ds, b["client"], r["start"] + 1)
            if tm == att_team:
                ts = [s for s in b["tracks"] if s.get("_round") is r]
                if ts:
                    far.append(max(dist3(s["pos"], ts[0]["pos"]) for s in ts))
            else:
                for (t, job, _rn) in b["squad"]:
                    if r["start"] <= t <= end and job and job.startswith("Hold("):
                        holds.add(job)
        left_spawn.append({"round": r["no"], "attackers_max_u": [round(x) for x in far],
                           "left": any(x >= SND_SPAWN_LEAVE_U for x in far)})
        split.append({"round": r["no"], "defender_holds": sorted(holds)})
    # post-plant: a defender alive SND_DEFUSE_ALIVE_S after the plant, who won the planted round, contested defuses
    ev = ds["objective"]
    by_client = {b["client"]: b for b in bots if b["client"] is not None}
    defuse_expected = 0
    defuse_when_alive = 0
    defuse_alive = 0
    reach_expected = 0
    reach_defused = 0
    att_wins = 0
    contested = 0
    defuse_starts = 0
    for r in planted:
        end = r["end"] or ds["last_ms"]
        plant = next((e for e in ev if e["kind"] == "planted" and r["start"] <= e["ms"] <= end), None)
        if plant is None:
            continue
        att_team = team_of(ds, int(plant["args"][1]), plant["ms"])
        defenders = [c for c, tm in r["spawned"].items() if tm != att_team]
        dead_by = {k["victim"] for k in r["kills"] if k["ms"] <= plant["ms"] + SND_DEFUSE_ALIVE_S * 1000}
        if any(c not in dead_by for c in defenders):
            defuse_expected += 1
            if r["defuse_starts"] > 0:
                defuse_when_alive += 1
        if r["how"] == "bomb" or (r["how"] == "elimination" and r.get("win") == att_team):
            att_wins += 1
        pp = nearest_pos(by_client.get(int(plant["args"][1])), plant["ms"])
        if pp is not None:  # a defender who could reach the site in SND_DEFUSE_REACH_S and was alive on arrival
            for c in defenders:
                dp = nearest_pos(by_client.get(c), plant["ms"])
                if dp is None:
                    continue
                secs = dist3(dp, pp) / SND_RUN_SPEED_U
                if secs <= SND_DEFUSE_REACH_S and not any(
                        k["victim"] == c and k["ms"] <= plant["ms"] + secs * 1000 for k in r["kills"]):
                    reach_expected += 1
                    reach_defused += 1 if r["defuse_starts"] > 0 else 0
                    break
        for e in ev:
            if e["kind"] == "defuse_start" and r["start"] <= e["ms"] <= end:
                defuse_starts += 1
                dead_att = {k["victim"] for k in r["kills"] if k["ms"] <= e["ms"]}
                if not any(c not in dead_att for c, tm in r["spawned"].items() if tm == att_team):
                    continue  # no attacker alive to contest it
                defuse_alive += 1
                if any(team_of(ds, b["client"], e["ms"]) == att_team
                       and any(e["ms"] <= f <= e["ms"] + SND_CONTEST_S * 1000 for f in b["fire_sms"])
                       for b in bots if b["client"] is not None):
                    contested += 1
    return {"plant_attempt_rounds": plant_rounds, "planted_rounds": len(planted),
            "defuse_after_plant_rounds": defuse_tried, "decided_rounds": decided,
            "defuse_expected_rounds": defuse_expected, "defuse_when_alive_rounds": defuse_when_alive,
            "attacker_wins_planted": att_wins, "defuse_starts": defuse_starts, "defuse_starts_alive": defuse_alive,
            "defuses_contested": contested, "defuse_reach_expected_rounds": reach_expected,
            "defuse_reach_rounds": reach_defused,
            "attackers_left_spawn": left_spawn, "defender_split": split}


def nearest_pos(b, ms, tol=4000):
    """The bot's track position nearest to server time ms (within tol), else None."""
    if b is None:
        return None
    best = None
    for s in b["tracks"]:
        if s.get("_sms") is not None and abs(s["_sms"] - ms) <= tol and (best is None or abs(s["_sms"] - ms) < abs(best["_sms"] - ms)):
            best = s
    return best["pos"] if best else None


def ctf_metrics(ds):
    """CTF: captures, drops and how fast each drop was resolved (taken again or returned)."""
    ev = ds["objective"]
    caps = [e for e in ev if e["kind"] == "ctf_capture"]
    drops = []
    for i, e in enumerate(ev):
        if e["kind"] != "ctf_dropped":
            continue
        team = e["args"][0]
        res = None
        for f in ev[i + 1:]:
            if f["kind"] in ("ctf_taken", "ctf_returned") and f["args"][0] == team:
                how = "taken" if f["kind"] == "ctf_taken" else ("timer" if f["args"][1] == "the timer" else "returned")
                res = {"how": how, "secs": round((f["ms"] - e["ms"]) / 1000.0, 1)}
                break
        if res is None and ds["last_ms"] - e["ms"] < CTF_DROP_RESOLVE_S * 1000:
            continue  # dropped when the bots left at the end: not judged
        drops.append({"t": round(e["ms"] / 1000.0, 1), "team": int(team), "resolved": res})
    carries = []
    for i, e in enumerate(ev):
        if e["kind"] != "ctf_taken":
            continue
        carrier = e["args"][1]
        for f in ev[i + 1:]:
            if (f["kind"] == "ctf_capture" and f["args"][0] == carrier
                    or f["kind"] == "ctf_dropped" and f["args"][1] == carrier):
                carries.append({"by": int(carrier), "end": "capture" if f["kind"] == "ctf_capture" else "dropped",
                                "secs": round((f["ms"] - e["ms"]) / 1000.0, 1)})
                break
    home = {"1": True, "2": True}
    both_out_since = None
    both_out_max = 0.0
    for e in ev:
        if e["kind"] == "ctf_taken":
            home[e["args"][0]] = False
        elif e["kind"] in ("ctf_returned",):
            home[e["args"][0]] = True
        elif e["kind"] == "ctf_capture":
            home[e["args"][1]] = True
        out = not home["1"] and not home["2"]
        if out and both_out_since is None:
            both_out_since = e["ms"]
        elif not out and both_out_since is not None:
            both_out_max = max(both_out_max, (e["ms"] - both_out_since) / 1000.0)
            both_out_since = None
    if both_out_since is not None:
        both_out_max = max(both_out_max, (ds["last_ms"] - both_out_since) / 1000.0)
    takes_by_flag = {"1": 0, "2": 0}
    for e in ev:
        if e["kind"] == "ctf_taken":
            takes_by_flag[e["args"][0]] = takes_by_flag.get(e["args"][0], 0) + 1
    margin = abs(int(caps[-1]["args"][2]) - int(caps[-1]["args"][3])) if caps else 0
    return {"captures": len(caps), "takes_by_flag": takes_by_flag, "capture_margin": margin, "drops": drops, "carries": carries, "both_out_max_s": round(both_out_max, 1),
            "taken": sum(1 for e in ev if e["kind"] == "ctf_taken"),
            "returned_by_client": sum(1 for e in ev if e["kind"] == "ctf_returned" and e["args"][1] != "the timer"),
            "returned_by_timer": sum(1 for e in ev if e["kind"] == "ctf_returned" and e["args"][1] == "the timer")}


def near_track(ts, tracks, ms, tol=1000):
    """The track sample whose server time is nearest ms (within tol); ts = the sorted sample times."""
    import bisect
    k = bisect.bisect_left(ts, ms)
    best = None
    for kk in (k - 1, k):
        if 0 <= kk < len(ts) and abs(ts[kk] - ms) <= tol and (best is None or abs(ts[kk] - ms) < abs(best[0] - ms)):
            best = (ts[kk], tracks[kk])
    return best[1] if best else None


def sorted_tracks(b):
    tr = sorted((s for s in b["tracks"] if s.get("_sms") is not None), key=lambda s: s["_sms"])
    return [s["_sms"] for s in tr], tr


def spy_metrics(ds, bots):
    """Spy Hunt: time from intel available to the next pickup, stage 1 lengths, how stage 2 ended, how many hunters
    work the agent in stage 2."""
    ev = ds["objective"]
    pickups, stages, stage2 = [], [], []
    avail = None
    stage_start = None
    agent_at = None
    agent_client = None
    for e in ev:
        k = e["kind"]
        if k in ("spy_placed", "spy_back", "spy_drop"):
            if avail is None:
                avail = e["ms"]
            if k in ("spy_placed", "spy_back"):
                stage_start = e["ms"] if stage_start is None or k == "spy_placed" or "again" in str(e["args"]) \
                    else stage_start
        elif k == "spy_seized" and avail is not None:
            pickups.append(round((e["ms"] - avail) / 1000.0, 1))
            avail = None
        elif k == "spy_agent":
            if stage_start is not None:
                stages.append(round((e["ms"] - stage_start) / 1000.0, 1))
            stage_start = None
            agent_at = e["ms"]
            agent_client = int(e["args"][0])
        elif k in ("spy_done", "spy_fell", "spy_agent_killed", "spy_survived", "spy_gone") and agent_at is not None:
            stage2.append({"end": k[4:], "secs": round((e["ms"] - agent_at) / 1000.0, 1), "client": agent_client,
                           "from_ms": agent_at, "to_ms": e["ms"]})
            agent_at = None
    if agent_at is not None:  # the log ended inside stage 2
        stage2.append({"end": "open", "secs": round((ds["last_ms"] - agent_at) / 1000.0, 1), "client": agent_client,
                       "from_ms": agent_at, "to_ms": ds["last_ms"]})
    open_stage = round((ds["last_ms"] - stage_start) / 1000.0, 1) if stage_start is not None else None
    timer_ends = sum(1 for x in stage2 if x["end"] == "survived" or (x["end"] == "open" and x["secs"] >= SPY_STAGE2_TIMER_S))
    judged = [x for x in stage2 if x["end"] != "open" or x["secs"] >= SPY_STAGE2_TIMER_S]
    agent_wins = sum(1 for x in judged if x["end"] in ("done", "survived", "open"))
    by_client = {b["client"]: b for b in bots if b["client"] is not None}
    hunt = {b["client"]: sorted_tracks(b) for b in bots if b["client"] is not None}
    samples = ok = 0
    for st in stage2:
        ag = by_client.get(st["client"])
        if ag is None:
            continue
        for s in ag["tracks"]:
            if s.get("_sms") is None or not st["from_ms"] <= s["_sms"] <= st["to_ms"]:
                continue
            samples += 1
            n = 0
            for c, (ts, tr) in hunt.items():
                if c == st["client"]:
                    continue
                h = near_track(ts, tr, s["_sms"])
                if h is None:
                    continue
                if (h.get("kind") in SPY_STAGE2_KINDS or dist3(h["pos"], s["pos"]) <= SPY_STAGE2_NEAR_U
                        or (s.get("goal") and dist3(h["pos"], s["goal"]) <= SPY_STAGE2_NEAR_U)):
                    n += 1
            ok += 1 if n >= SPY_STAGE2_HUNTERS_MIN else 0
    for x in stage2:
        x.pop("from_ms")
        x.pop("to_ms")
    return {"pickup_delays_s": pickups, "stage1_secs": stages, "stage1_open_s": open_stage, "stage2": stage2,
            "uploads": sum(1 for e in ev if e["kind"] == "spy_uploads"),
            "missions": sum(1 for e in ev if e["kind"] == "spy_done"),
            "stage2_timer_ends": timer_ends, "agent_rounds": len(judged), "agent_wins": agent_wins,
            "stage2_samples": samples, "stage2_hunters_ok": ok}


def qrt_metrics(ds, rounds, bots):
    """Quarantine after the outbreak (the first infection of a round): phase length, the humans' spread, their
    `hunt` goals, the first claw and their fire in sight of an infected."""
    ev = ds["objective"]
    post, claw_rounds, fast_claw = [], 0, 0
    hunt = hs = m_ok = m_n = shots = 0
    sight_s = 0.0
    for r in rounds:
        end = r["end"] or ds["last_ms"]
        inf = [e for e in ev if e["kind"] == "infect_client" and r["start"] <= e["ms"] <= end]
        if not inf:
            continue
        out_ms = inf[0]["ms"]
        post.append((end - out_ms) / 1000.0)
        claws = [e for e in inf if e["args"][1] is not None]
        if claws:
            claw_rounds += 1
            fast_claw += 1 if claws[0]["ms"] - out_ms <= QRT_FIRST_CLAW_S * 1000 else 0
        inf_at = {int(e["args"][0]): e["ms"] for e in inf}
        buckets = {}
        for b in bots:
            c = b["client"]
            if c is None:
                continue
            for s in b["tracks"]:
                if s.get("_round") is not r or s["_sms"] < out_ms or (c in inf_at and inf_at[c] <= s["_sms"]):
                    continue  # not this round, before the outbreak, or already infected: not a human sample
                hs += 1
                hunt += 1 if s.get("kind") == "hunt" else 0
                buckets.setdefault(s["_sms"] // 2000, {})[c] = s["pos"]
                if s.get("sight"):
                    sight_s += TRACK_PERIOD_S
                    shots += sum(1 for f in b["fire_sms"] if s["_sms"] <= f < s["_sms"] + TRACK_PERIOD_S * 1000)
        for pos in buckets.values():
            if len(pos) < 2:
                continue
            ps = list(pos.values())
            ds_ = [dist3(ps[i], ps[j]) for i in range(len(ps)) for j in range(i + 1, len(ps))]
            m_n += 1
            m_ok += 1 if statistics.median(ds_) <= QRT_MUTUAL_DIST_U else 0
    return {"post_outbreak_median_s": round(statistics.median(post), 1) if post else None,
            "claw_rounds": claw_rounds, "first_claw_fast": fast_claw,
            "human_samples": hs, "human_hunt_share": round(hunt / hs, 3) if hs else None,
            "mutual_samples": m_n, "mutual_share": round(m_ok / m_n, 3) if m_n else None,
            "human_sight_s": sight_s, "human_shots_in_sight": shots,
            "human_shots_per_s": round(shots / sight_s, 2) if sight_s else None}


def elim_metrics(ds, rounds, bots=None, key=None):
    """Round modes: the living players per team at each round end (no respawn inside a round)."""
    out = []
    for r in rounds:
        alive = {}
        for c, tm in r["spawned"].items():
            alive[tm] = alive.get(tm, 0) + 1
        for k in r["kills"]:
            tm = r["spawned"].get(k["victim"])
            if tm in alive:
                alive[tm] -= 1
        out.append({"round": r["no"], "how": r["how"], "alive_at_end": {str(t): n for t, n in sorted(alive.items())}})
    infected = sum(1 for e in ds["objective"] if e["kind"] == "infect_client")
    # Quarantine: humans (team 1) win by the timer or by killing every infected; a win with zero infections = host kill
    human_wins = [r for r in rounds if r.get("win") == 1]
    host_kills = timer_zero = 0
    for r in human_wins:
        end = r["end"] or ds["last_ms"]
        n_inf = sum(1 for e in ds["objective"] if e["kind"] == "infect_client" and r["start"] <= e["ms"] <= end)
        if n_inf == 0 and r["how"] != "timer":
            host_kills += 1
        elif n_inf == 0:
            timer_zero += 1  # a timer win with no outbreak: the infected never played (a nav defect), no human win
    lengths = sorted((r["end"] - r["start"]) / 1000.0 for r in rounds if r["end"] is not None)
    wins = rounds[-1]["round_wins"] if rounds and rounds[-1].get("round_wins") else None
    res = {"round_alive": out, "infections": infected, "human_wins": len(human_wins), "host_kill_wins": host_kills,
           "timer_zero_wins": timer_zero, "human_wins_valid": len(human_wins) - timer_zero,
           "round_median_s": round(statistics.median(lengths), 1) if lengths else None, "round_wins": wins}
    if key == "QRT" and bots is not None:
        res.update(qrt_metrics(ds, rounds, bots))
    return res


def corpse_metrics(evs_of, ds):
    """Firing at a body after its death was shown: for every `kill` event a bot received (any killer; `victim` is the
    client id) the `fire` events whose `target_obj` is that client's character object within 1.5 s after it, chained
    while the gaps stay under 1 s. A human stops within a second; the server's `victim_dead` refusals cannot tell
    this from several players firing at one target at the same time (claims in flight when the kill lands)."""
    client_of_obj = {o: c for c, objs in ds.get("objects", {}).items() for o in objs}
    kills_seen = 0
    corpse_kills = 0
    tails = []
    for evs in evs_of.values():
        fires = [e for e in evs if e.get("ev") == "fire" and e.get("target_obj") in client_of_obj
                 and e.get("t") is not None]
        for k in evs:
            if k.get("ev") != "kill" or k.get("t") is None or k.get("victim") is None:
                continue
            kills_seen += 1
            t0 = k["t"]
            last = t0
            tail = 0.0
            for x in fires:
                if x["t"] <= t0:
                    continue
                if x["t"] > t0 + 1.5 or x["t"] - last > 1.0:
                    break
                if client_of_obj[x["target_obj"]] != k["victim"]:
                    continue
                last = x["t"]
                tail = x["t"] - t0
            if tail > 0:
                corpse_kills += 1
                tails.append(tail)
    tails.sort()
    return {"kills_seen": kills_seen, "corpse_kills": corpse_kills,
            "corpse_tail_max_s": round(tails[-1], 2) if tails else 0.0,
            "corpse_tail_p95_s": round(tails[min(len(tails) - 1, int(len(tails) * 0.95))], 2) if tails else 0.0,
            "objects_known": bool(client_of_obj)}


def flow_metrics(sdir, bots, ds, sc, evs_of=None):
    panics = []
    for p in glob.glob(os.path.join(sdir, "*.out")):
        with open(p, encoding="utf-8", errors="replace") as f:
            if "panicked" in f.read():
                panics.append(os.path.basename(p))
    drops = [b["nick"] for b in bots if b["end"] and not str(b["end"]).startswith("quit")]
    if sc:
        for rb in sc.get("bots") or []:
            if rb.get("end") and not str(rb["end"]).startswith("quit") and rb.get("nick") not in drops:
                drops.append(rb.get("nick"))
    out = {"panics": panics, "drops": drops, "join_failed": sum(b["join_failed"] for b in bots),
           "refused_lines": ds["refused"], "claims": ds.get("claims"), "refused_kinds": ds.get("refused_kinds", {})}
    out.update(corpse_metrics(evs_of or {}, ds))
    return out


# ---------------------------------------------------------------------------------------------------------------
# the rubric
# ---------------------------------------------------------------------------------------------------------------
def line(group, name, ok, detail):
    return {"group": group, "line": name, "ok": ok, "detail": detail}


def rubric(key, minutes, bots, room, ds):
    """Every rubric line of the brief that applies to the mode; ok None = not applicable / no data."""
    t = room["totals"]
    o = room["objective"]
    per10 = max(1, math.ceil((minutes or 0) / 10.0))
    L = []
    # 1. mobility
    worst = max(bots, key=lambda b: b["stuck_n"], default=None)
    L.append(line("mobility", f"stuck episodes <= {STUCK_MAX_PER_BOT_PER_10MIN * per10} per bot",
                  all(b["stuck_n"] <= STUCK_MAX_PER_BOT_PER_10MIN * per10 for b in bots),
                  f"{t['stuck']} total, worst {worst['nick']} {worst['stuck_n']}" if worst else "no bots"))
    L.append(line("mobility", f"no stuck episode longer than {STUCK_LONGEST_MAX_S:.0f} s",
                  t["stuck_longest_s"] <= STUCK_LONGEST_MAX_S, f"longest {t['stuck_longest_s']} s"))
    kinds = {}
    for b in bots:
        for k, v in b["idle_by_kind"].items():
            kinds[k] = round(kinds.get(k, 0) + v, 1)
    judged_idle = 0.0
    allowed_n = 0
    for b in bots:
        allow = ROOTED_MAX_PER_BOT_PER_10MIN * per10
        for ep in sorted(b["idle"], key=lambda e: e["t"]):
            if allow and ep["secs"] < IDLE_HOLD_MAX_S and ep.get("contact"):
                allow -= 1
                allowed_n += 1
                continue
            judged_idle += ep["secs"]
    judged_idle = round(judged_idle, 1)
    L.append(line("mobility", f"idle (no goal, no enemy in sight, still >= {IDLE_MIN_S:.0f} s, a guard >= "
                              f"{IDLE_GUARD_MIN_S:.0f} s, one hold < {IDLE_HOLD_MAX_S:.0f} s with contact allowed per bot) "
                              f"<= {IDLE_MAX_S:.0f} s",
                  judged_idle <= IDLE_MAX_S, f"{judged_idle} s judged of {t['idle_s']} s in "
                  f"{sum(len(b['idle']) for b in bots)} episodes ({allowed_n} allowed) " + (str(kinds) if kinds else "")))
    rooted = [(b["nick"], len(b["rooted"]), b["rooted_s"]) for b in bots
              if len(b["rooted"]) > ROOTED_MAX_PER_BOT_PER_10MIN * per10]
    L.append(line("mobility", f"rooted while engaged (still >= {ROOTED_MIN_S:.0f} s with the target in sight) "
                              f"<= {ROOTED_MAX_PER_BOT_PER_10MIN} per bot per 10 min", not rooted,
                  f"{sum(len(b['rooted']) for b in bots)} episodes, {sum(b['rooted_s'] for b in bots):.0f} s"
                  + (f"; over: {rooted}" if rooted else "")))
    if key in RESPAWN_MODES:
        wmax = WORLD_DEATHS_ROOM_PER_6MIN * max(1, math.ceil((minutes or 6) / 6.0))
    else:
        wmax = WORLD_DEATHS_ROUND_ROOM
    L.append(line("mobility", f"world deaths <= {wmax} per room", t["world_deaths"] <= wmax,
                  f"{t['world_deaths']} total"))
    hoppers = [f"{b['nick']} {b['stuck_jump_nonface_per_min']}" for b in bots
               if b["stuck_jump_nonface_per_min"] > STUCK_JUMP_PER_MIN_MAX]
    L.append(line("mobility", f"non-face stuck jumps <= {STUCK_JUMP_PER_MIN_MAX:g} per bot per alive minute", not hoppers,
                  f"{sum(b['stuck_jumps_nonface'] for b in bots)} of {t['stuck_jumps']} stuck jumps are non-face"
                  + (f"; over: {', '.join(hoppers)}" if hoppers else "")))
    pogo_over, pogo_all = [], 0
    for b in bots:
        pogo_all += len(b["pogo"])
        allow = max(1, math.floor(b["alive_s"] / 60.0 / 5.0) * POGO_MAX_PER_BOT_PER_5MIN)
        if b["alive_s"] >= POGO_MIN_ALIVE_MIN * 60 and len(b["pogo"]) > allow:
            pogo_over.append(f"{b['nick']} {len(b['pogo'])} runs "
                             + ", ".join(f"t={r['t']} x{r['n']} at {r['pos']}" for r in b["pogo"][:3]))
    L.append(line("mobility", f"pogo runs ({POGO_N}+ stuck jumps within {POGO_RADIUS_U:.0f} u, gaps < {POGO_GAP_S:.0f} s) "
                              f"<= {POGO_MAX_PER_BOT_PER_5MIN} per bot per 5 alive min (bots with >= {POGO_MIN_ALIVE_MIN:g} min)",
                  not pogo_over, f"{pogo_all} runs" + (f"; over: {'; '.join(pogo_over)}" if pogo_over else "")))
    conf = [(b["nick"], e) for b in bots for e in b["confined"]]
    L.append(line("mobility", f"confined (>= {CONFINED_MIN_S:.0f} s in a {CONFINED_BOX_U:.0f} u box with the goal > "
                              f"{CONFINED_GOAL_U:.0f} u away, or nav islands +{CONFINED_ISLANDS} in a life) "
                              f"<= {CONFINED_MAX_PER_ROOM} per room",
                  len(conf) <= CONFINED_MAX_PER_ROOM,
                  f"{len(conf)} episodes" + ("; " + "; ".join(
                      (f"{n} t={e['t']}-{e['t'] + e['secs']:.0f} s box {e['box']} at {e['pos']} goal {e['goal']}"
                       if e["kind"] == "box" else f"{n} t={e['t']} islands +{e['growth']}") for n, e in conf[:6])
                   if conf else "")))
    judged = t["goals_ok"] + t["goals_late"]
    L.append(line("mobility", f"goals < {ROUTE_MAX_GOAL_U:.0f} u reached within {ROUTE_MAX_S:.0f} s "
                  f"(late <= {ROUTE_LATE_MAX_SHARE:.0%})",
                  (t["goals_late"] / judged <= ROUTE_LATE_MAX_SHARE) if judged else None,
                  f"{t['goals_ok']} ok, {t['goals_late']} late, {t['goals_abandoned']} abandoned "
                  f"({t['goals_by_arrived']} by `arrived`)"))
    if key == "TDM":
        cov = t["coverage_by_team"]
        L.append(line("mobility", f"each team covers >= {COVERAGE_MIN:.0%} of the visited cells",
                      all(v >= COVERAGE_MIN for v in cov.values()) if cov else None,
                      f"{t['cells_all']} cells, by team {cov}"))
    if key in ("TDM", "CTF"):
        st = t["mate_stack"]
        over = [f"{n} {v:.0%}" for n, v in st.items() if v > MATE_STACK_MAX_SHARE]
        L.append(line("mobility", f"a team mate within {MATE_STACK_U:.0f} u for <= {MATE_STACK_MAX_SHARE:.0%} of a bot's time",
                      not over if st else None, f"max {t['mate_stack_share_max']:.0%}" + (f"; over: {', '.join(over)}" if over else "")))
    # 2. combat
    if key in RESPAWN_MODES:
        guards = {b["nick"] for b in bots if key == "CTF" and b["guard_share"] >= 0.5}
        slow = [b["nick"] for b in bots if b["alive_s"] > 0 and b["nick"] not in guards
                and b["kills"] / (b["alive_s"] / 120.0) < KILLS_PER_2MIN_ALIVE_MIN]
        # Spy Hunt: the 2009 damage matrix (DSV-453) lets a hunter shoot only the holders, so kills are rare by design
        L.append(line("combat", f"each bot >= {KILLS_PER_2MIN_ALIVE_MIN:g} kill per 2 min alive (CTF guards exempt, not Spy Hunt)",
                      (not slow) if key != "BNT" else None, (f"below: {', '.join(slow)}" if slow else "all")
                      + (f"; guards {sorted(guards)}" if guards else "")))
        silent = [b["nick"] for b in bots if b["targets"] > 0 and not b["shots"]]
        L.append(line("combat", "no bot with 0 shots while it had a target", not silent,
                      f"{', '.join(silent)}" if silent else "none"))
        if key in TEAM_BALANCE_MODES:
            kt = t["kills_by_team"]
            tot = sum(kt.values())
            shares = {k: v / tot for k, v in kt.items()} if tot else {}
            L.append(line("combat", f"team kill share {TEAM_KILL_SHARE[0]:.0%}..{TEAM_KILL_SHARE[1]:.0%}",
                          all(TEAM_KILL_SHARE[0] <= s <= TEAM_KILL_SHARE[1] for s in shares.values()) if tot else None,
                          f"{kt}"))
        if key == "FFA":
            tot = sum(b["kills"] for b in bots)
            top = max(bots, key=lambda b: b["kills"], default=None)
            L.append(line("combat", f"no bot above {FFA_TOP_SHARE_MAX:.0%} of the kills",
                          (top["kills"] / tot <= FFA_TOP_SHARE_MAX) if tot and top else None,
                          f"top {top['nick']} {top['kills']} of {tot}" if top else "-"))
        if key == "BNT":
            sh = [sum(b["bands"]["shots"][i] for b in bots if not b["sniper"]) for i in range(len(SPY_BANDS))]
            hi = [sum(b["bands"]["hits"][i] for b in bots if not b["sniper"]) for i in range(len(SPY_BANDS))]
            judged_b = [i for i in range(len(SPY_BANDS)) if sh[i] >= SPY_BAND_MIN_SHOTS]
            bad = [i for i in judged_b if not SPY_BANDS[i][2] <= hi[i] / sh[i] <= SPY_BANDS[i][3]]
            names = ["<%g" % SPY_BANDS[0][1]] + [f"{SPY_BANDS[i][0]:g}-{SPY_BANDS[i][1]:g}" for i in range(1, len(SPY_BANDS) - 1)] \
                + [">%g" % SPY_BANDS[-1][0]]
            L.append(line("combat", "hit rate by distance " + ", ".join(
                f"{names[i]} u {SPY_BANDS[i][2]:.0%}-{SPY_BANDS[i][3]:.0%}" for i in range(len(SPY_BANDS)))
                          + f" (bands with >= {SPY_BAND_MIN_SHOTS} shots)",
                          (not bad) if judged_b else None,
                          ", ".join(f"{names[i]} u {hi[i]}/{sh[i]}" + (f" {hi[i] / sh[i]:.0%}" if sh[i] else "")
                                    for i in range(len(SPY_BANDS)))))
            ht = sum(b["bands"]["hits_total"] for b in bots if not b["sniper"])
            hn = sum(b["bands"]["hits_near"] for b in bots if not b["sniper"])
            L.append(line("combat", f"hits under {SPY_POINT_BLANK_U:.0f} u <= {SPY_POINT_BLANK_MAX_SHARE:.0%} of the hits",
                          (hn <= SPY_POINT_BLANK_MAX_SHARE * ht) if ht >= SPY_POINT_BLANK_MIN_HITS else None,
                          f"{hn} of {ht} hits"))
        else:
            acc = [(b["nick"], (b["hit_claims"] or 0) / b["shots"]) for b in bots
                   if not b["sniper"] and (b["shots"] or 0) >= ACCURACY_MIN_SHOTS]
            bad = [f"{n} {a:.1%}" for n, a in acc if not ACCURACY[0] <= a <= ACCURACY[1]]
            L.append(line("combat", f"accuracy hits/shots {ACCURACY[0]:.0%}..{ACCURACY[1]:.0%} (rifles, bots with >= "
                                    f"{ACCURACY_MIN_SHOTS} shots)",
                          (not bad) if acc else None, f"outside: {', '.join(bad)}" if bad else f"{len(acc)} judged"))
        med = t["reaction_measured_median_s"]
        n_m = t["reaction_measured_n"]
        over_share = REACTION_OVER_MAX_SHARE_BNT if key == "BNT" else REACTION_OVER_MAX_SHARE
        over_ok = t["reaction_measured_over_max"] <= max(REACTION_OVER_MIN_SAMPLES, over_share * n_m)
        L.append(line("combat", f"reaction sight->shot median {REACTION_MEDIAN[0]}..{REACTION_MEDIAN[1]} s (n >= "
                      f"{REACTION_MEDIAN_MIN_N}), none < {REACTION_INSTANT_S} s, > {REACTION_MAX_S:.0f} s at most "
                      f"{over_share:.0%} or {REACTION_OVER_MIN_SAMPLES} samples",
                      (REACTION_MEDIAN[0] <= med <= REACTION_MEDIAN[1] and t["reaction_measured_instant"] == 0 and over_ok)
                      if med is not None and n_m >= REACTION_MEDIAN_MIN_N else None,
                      f"median {med} s of {n_m}, instant {t['reaction_measured_instant']}, "
                      f"over {t['reaction_measured_over_max']} (max {t['reaction_measured_max_s']}); planned median "
                      f"{t['reaction_planned_median_s']} s"))
        flat = []
        judged_iqr = 0
        for b in bots:
            xs = sorted(b["reaction_planned_s"])
            if len(xs) >= REACTION_IQR_MIN_N:
                judged_iqr += 1
                iqr = xs[(3 * len(xs)) // 4] - xs[len(xs) // 4]
                if iqr < REACTION_IQR_MIN_S:
                    flat.append(f"{b['nick']} {iqr:.2f}")
        L.append(line("combat", f"a bot's planned reaction delays vary (kind new, IQR >= {REACTION_IQR_MIN_S:.2f} s over >= "
                                f"{REACTION_IQR_MIN_N})",
                      (not flat) if judged_iqr else None, f"{judged_iqr} judged" + (f"; flat: {', '.join(flat)}" if flat else "")))
    # 3. objective
    n = o.get("rounds", 0)
    if key == "TMM":
        L.append(line("objective", f"plant attempted in >= {SND_PLANT_ROUNDS_MIN:.0%} of rounds",
                      (o["plant_attempt_rounds"] / n >= SND_PLANT_ROUNDS_MIN) if n else None,
                      f"{o['plant_attempt_rounds']} of {n}"))
        L.append(line("objective", f"defuse attempted after >= {SND_DEFUSE_AFTER_PLANT_MIN:.0%} of the plants a defender was alive {SND_DEFUSE_ALIVE_S:.0f} s after",
                      (o["defuse_when_alive_rounds"] / o["defuse_expected_rounds"] >= SND_DEFUSE_AFTER_PLANT_MIN)
                      if o["defuse_expected_rounds"] else None,
                      f"{o['defuse_when_alive_rounds']} of {o['defuse_expected_rounds']} ({o['defuse_after_plant_rounds']} "
                      f"of {o['planted_rounds']} plants)"))
        L.append(line("objective", f"attackers win >= {SND_ATTACKER_WIN_MIN:.0%} of the planted rounds",
                      (o["attacker_wins_planted"] / o["planted_rounds"] >= SND_ATTACKER_WIN_MIN) if o["planted_rounds"] else None,
                      f"{o['attacker_wins_planted']} of {o['planted_rounds']}"))
        L.append(line("objective", f"a defuse starts in >= {SND_DEFUSE_REACH_MIN:.0%} of the planted rounds where a defender "
                                   f"could reach the site within {SND_DEFUSE_REACH_S:.0f} s ({SND_RUN_SPEED_U:.0f} u/s) alive",
                      (o["defuse_reach_rounds"] / o["defuse_reach_expected_rounds"] >= SND_DEFUSE_REACH_MIN)
                      if o["defuse_reach_expected_rounds"] else None,
                      f"{o['defuse_reach_rounds']} of {o['defuse_reach_expected_rounds']}"))
        L.append(line("objective", f"an attacker fires within {SND_CONTEST_S:.0f} s of >= {SND_CONTEST_MIN:.0%} of the defuse "
                                   f"starts with an attacker alive",
                      (o["defuses_contested"] / o["defuse_starts_alive"] >= SND_CONTEST_MIN) if o["defuse_starts_alive"] else None,
                      f"{o['defuses_contested']} of {o['defuse_starts_alive']} ({o['defuse_starts']} defuse starts)"))
        L.append(line("objective", f">= {SND_DECIDED_MIN:.0%} of rounds end by bomb / defuse / elimination",
                      (o["decided_rounds"] / n >= SND_DECIDED_MIN) if n else None,
                      f"{o['decided_rounds']} of {n}, ends {o['round_ends']}"))
        ls = o["attackers_left_spawn"]
        L.append(line("objective", f"attackers leave the spawn (one >= {SND_SPAWN_LEAVE_U:.0f} u) every round",
                      all(x["left"] for x in ls) if ls else None, f"{sum(x['left'] for x in ls)} of {len(ls)}"))
        sp = o["defender_split"]
        L.append(line("objective", "defenders split over >= 2 sites every round",
                      all(len(x["defender_holds"]) >= 2 for x in sp) if sp else None,
                      "; ".join(f"r{x['round']} {','.join(x['defender_holds']) or '-'}" for x in sp)))
    elif key == "CTF":
        need = max(1, math.floor((minutes or 0) * 60 / CTF_CAPTURE_EVERY_S))
        L.append(line("objective", f">= 1 capture per {CTF_CAPTURE_EVERY_S / 60:.0f} min ({need})",
                      o["captures"] >= need, f"{o['captures']} captures, {o['taken']} takes"))
        need_t = max(1, math.floor((minutes or 0) * 60 / CTF_TAKE_EVERY_S))
        L.append(line("objective", f">= 1 flag take per {CTF_TAKE_EVERY_S / 60:.0f} min ({need_t})",
                      o["taken"] >= need_t, f"{o['taken']} takes"))
        need_b = max(1, math.floor((minutes or 0) * 60 / CTF_BALANCE_EVERY_S))
        tk = o["takes_by_flag"]
        L.append(line("objective", f"each team takes the enemy flag >= {need_b} time(s) (1 per {CTF_BALANCE_EVERY_S / 60:.0f} min) "
                                   f"and the capture margin <= {CTF_CAPTURE_MARGIN_MAX}",
                      min(tk.get("1", 0), tk.get("2", 0)) >= need_b and o["capture_margin"] <= CTF_CAPTURE_MARGIN_MAX,
                      f"flag 1 taken {tk.get('1', 0)}, flag 2 taken {tk.get('2', 0)}, margin {o['capture_margin']}"))
        L.append(line("objective", f"both flags off their bases for <= {CTF_BOTH_OUT_MAX_S:.0f} s at a stretch",
                      o["both_out_max_s"] <= CTF_BOTH_OUT_MAX_S, f"longest {o['both_out_max_s']} s"))
        dr = o["drops"]
        late = [d for d in dr if not d["resolved"] or d["resolved"]["secs"] > CTF_DROP_RESOLVE_S]
        L.append(line("objective", f"a dropped flag is taken or returned within {CTF_DROP_RESOLVE_S:.0f} s",
                      (not late) if dr else None, f"{len(dr) - len(late)} of {len(dr)}"))
        L.append(line("objective", "defenders return their own flag (no return by the timer)",
                      o["returned_by_timer"] == 0 if (o["returned_by_timer"] + o["returned_by_client"]) else None,
                      f"{o['returned_by_client']} by a player, {o['returned_by_timer']} by the timer"))
    elif key == "BNT":
        pd = o["pickup_delays_s"]
        L.append(line("objective", f"intel picked up within {SPY_PICKUP_S:.0f} s of being available",
                      all(x <= SPY_PICKUP_S for x in pd) if pd else (False if o["events"].get("spy_placed") else None),
                      f"delays {pd}"))
        st = list(o["stage1_secs"]) + ([o["stage1_open_s"]] if o["stage1_open_s"] is not None
                                       and o["stage1_open_s"] > SPY_STAGE_MAX_S else [])
        L.append(line("objective", f"a stage completes within {SPY_STAGE_MAX_S / 60:.0f} min",
                      all(x <= SPY_STAGE_MAX_S for x in st) if st else None,
                      f"stage 1 {o['stage1_secs']} s, open {o['stage1_open_s']} s; stage 2 {o['stage2']}"))
        L.append(line("objective", "stage 2 ends by the upload or the agent's death, never by the stage timer",
                      (o["stage2_timer_ends"] == 0) if o["stage2"] else None,
                      f"{o['stage2_timer_ends']} timer ends of {len(o['stage2'])} stage 2"))
        L.append(line("objective", f">= {SPY_STAGE2_HUNTERS_MIN} hunters chase / guard / within {SPY_STAGE2_NEAR_U:.0f} u of the "
                                   f"agent or its uplink for >= {SPY_STAGE2_SHARE_MIN:.0%} of stage 2",
                      (o["stage2_hunters_ok"] / o["stage2_samples"] >= SPY_STAGE2_SHARE_MIN) if o["stage2_samples"] >= 5 else None,
                      f"{o['stage2_hunters_ok']} of {o['stage2_samples']} samples"))
    elif key == "QRT":
        hw = o["human_wins_valid"]
        L.append(line("objective", f"humans win {QRT_HUMAN_WIN_SHARE[0]:.0%}..{QRT_HUMAN_WIN_SHARE[1]:.0%} of the rounds "
                                   f"(pooled per mode in the summary; informational per room)", None,
                      f"{hw} of {n}, ends {o['round_ends']}, {o['infections']} infections"))
        L.append(line("objective", "no timer win with zero infections (the infected never played: a nav defect)",
                      o["timer_zero_wins"] == 0 if n else None, f"{o['timer_zero_wins']} of {n} rounds"))
        pm = o.get("post_outbreak_median_s")
        L.append(line("objective", f"median round after the outbreak {QRT_POST_OUTBREAK_S[0]:.0f}..{QRT_POST_OUTBREAK_S[1]:.0f} s "
                                   f"(countdown excluded)",
                      (QRT_POST_OUTBREAK_S[0] <= pm <= QRT_POST_OUTBREAK_S[1]) if pm is not None else None,
                      f"median {pm} s"))
        ms_ = o.get("mutual_share")
        L.append(line("objective", f"humans' median mutual distance <= {QRT_MUTUAL_DIST_U:.0f} u in >= "
                                   f"{QRT_MUTUAL_MIN_SHARE:.0%} of the samples after the outbreak",
                      (ms_ >= QRT_MUTUAL_MIN_SHARE) if o.get("mutual_samples", 0) >= 5 else None,
                      f"{ms_} of {o.get('mutual_samples')} samples"))
        hh = o.get("human_hunt_share")
        L.append(line("objective", f"humans with a `hunt` goal after the outbreak <= {QRT_HUMAN_HUNT_MAX_SHARE:.0%} of samples",
                      (hh <= QRT_HUMAN_HUNT_MAX_SHARE) if hh is not None else None,
                      f"{hh} of {o.get('human_samples')} samples"))
        L.append(line("objective", f"first claw within {QRT_FIRST_CLAW_S:.0f} s of the outbreak in <= "
                                   f"{QRT_FIRST_CLAW_MAX_SHARE:.0%} of the rounds",
                      (o["first_claw_fast"] <= QRT_FIRST_CLAW_MAX_SHARE * o["claw_rounds"]) if o.get("claw_rounds") else None,
                      f"{o.get('first_claw_fast')} of {o.get('claw_rounds')} rounds"))
        sp_ = o.get("human_shots_per_s")
        L.append(line("objective", f"humans fire >= {QRT_HUMAN_SHOTS_PER_S:g} shots/s in sight of an infected "
                                   f"(>= {QRT_SIGHT_MIN_S:.0f} s in sight)",
                      (sp_ >= QRT_HUMAN_SHOTS_PER_S) if o.get("human_sight_s", 0) >= QRT_SIGHT_MIN_S else None,
                      f"{sp_} shots/s over {o.get('human_sight_s')} s in sight"))
        L.append(line("objective", f"<= {QRT_HOST_KILL_MAX_SHARE:.0%} of the rounds won by killing the host with zero infections",
                      (o["host_kill_wins"] <= QRT_HOST_KILL_MAX_SHARE * n) if n else None,
                      f"{o['host_kill_wins']} host kills of {n} rounds"))
    elif key == "TSV":
        el = o["round_ends"].get("elimination", 0)
        L.append(line("objective", f">= {TSV_ELIM_MIN:.0%} of rounds end by elimination",
                      (el / n >= TSV_ELIM_MIN) if n else None, f"{el} of {n}, ends {o['round_ends']}"))
        never = [x["round"] for x in o["round_alive"] if x["how"] == "timer"
                 and sum(1 for v in x["alive_at_end"].values() if v >= 2) >= 2]
        L.append(line("objective", "no timer end with 2+ alive on both sides", not never if n else None,
                      f"rounds {never}" if never else "none"))
    if key in ("TMM", "TSV"):
        rw = next((r["round_wins"] for r in reversed(real_rounds(ds)) if r.get("round_wins")), None)
        L.append(line("objective", f"round wins {ROUND_WIN_POOLED[0]:.0%}..{ROUND_WIN_POOLED[1]:.0%} per side (pooled per mode "
                                   f"in the summary; informational per room)", None,
                      f"round wins {rw}" if rw else "no round wins"))
    # 4. flow
    f = room["flow"]
    L.append(line("flow", "no bot drop, no join_failed, no panic",
                  not f["drops"] and not f["join_failed"] and not f["panics"],
                  f"drops {f['drops']}, join_failed {f['join_failed']}, panics {f['panics']}"))
    dead = f["refused_kinds"].get("victim_dead", 0)
    claims = f.get("claims")
    ks = f.get("kills_seen", 0)
    ok = None
    if ks and f.get("objects_known"):
        ok = f["corpse_kills"] <= CORPSE_KILL_MAX_SHARE * ks and f["corpse_tail_p95_s"] <= CORPSE_TAIL_P95_MAX_S
    L.append(line("flow", f"firing at a body after its death was shown: <= {CORPSE_KILL_MAX_SHARE:.0%} of kills seen, "
                          f"tail p95 <= {CORPSE_TAIL_P95_MAX_S:g} s", ok,
                  f"{f.get('corpse_kills', 0)} of {ks} kills seen followed by hits on the body (longest tail "
                  f"{f.get('corpse_tail_max_s', 0)} s, p95 {f.get('corpse_tail_p95_s', 0)} s); server refused "
                  f"victim_dead {dead} of {claims} claims (several shooters on one target, not judged)"))
    return L


# ===============================================================================================================
# output
# ===============================================================================================================
def objective_summary(room):
    o = room["objective"]
    k = room["mode"]
    ends = ",".join(f"{a}:{b}" for a, b in sorted(o.get("round_ends", {}).items(), key=lambda kv: str(kv[0])))
    if k == "TMM":
        return f"{o['rounds']} rnd, plant-try {o['plant_attempt_rounds']}, planted {o['planted_rounds']}, " \
               f"defuse-try {o['defuse_after_plant_rounds']}; {ends}"
    if k == "CTF":
        return f"{o['captures']} cap, {o['taken']} take, {len(o['drops'])} drop"
    if k == "BNT":
        return f"pickups {o['pickup_delays_s']}, stage1 {o['stage1_secs']}, {o['missions']} missions"
    if k in ("QRT", "TSV"):
        return f"{o['rounds']} rnd; {ends}" + (f"; {o['infections']} infected" if k == "QRT" else "")
    return ends or "-"


def fmt2(x):
    return "-" if x is None else f"{x:.2f}"


def room_md(room):
    out = io.StringIO()
    w = out.write
    w(f"# Bot score: {room['mode']} ({MODE_NAMES.get(room['mode'], '?')}) on {room['map']}\n\n")
    t = room["totals"]
    w(f"- {room['bots']} bots, {room['minutes']} min{'' if room['complete'] else ' (partial: no report.json)'}, "
      f"verdict **{room['verdict']}**\n")
    w(f"- kills {t['kills']} (world deaths {t['world_deaths']}, team kills {t['team_kills']}"
      f"{', infections ' + str(t['infections']) if t.get('infections') else ''}), by team "
      f"{t['kills_by_team']}{'' if room['mode'] == 'BNT' else ', K/D spread ' + str(t['kd_spread'])}; shots {t['shots']}, hits claimed {t['hit_claims']}\n")
    w(f"- alive {t['alive_min']} bot-min, stuck {t['stuck']} (longest {t['stuck_longest_s']} s), idle {t['idle_s']} s, "
      f"stuck jumps {t['stuck_jumps']}, cells {t['cells_all']} {t['coverage_by_team']}\n")
    w(f"- objective: {objective_summary(room)}\n\n")
    w("## Rubric\n\n| group | line | result | numbers |\n|---|---|---|---|\n")
    for l in room["rubric"]:
        res = "n/a" if l["ok"] is None else ("PASS" if l["ok"] else "FAIL")
        w(f"| {l['group']} | {l['line']} | {res} | {l['detail']} |\n")
    w("\n## Bots\n\n| bot | client | team | alive s | kills | deaths | world | K/D | shots | hits | kills_by_me | "
      "stuck (longest s) | idle s | goals ok/late/aband | jumps/min | react planned / first / reacquire median s | "
      "killed by |\n|" + "---|" * 17 + "\n")
    for b in room["per_bot"]:
        rp = statistics.median(b["reaction_planned_s"]) if b["reaction_planned_s"] else None
        rm = statistics.median(b["reaction_measured_s"]) if b["reaction_measured_s"] else None
        rr = statistics.median(b["reaction_reacquire_measured_s"]) if b["reaction_reacquire_measured_s"] else None
        g = b["goals"]
        kb = ", ".join(f"{k} {v}" for k, v in sorted(b["killed_by"].items()))
        w(f"| {b['nick']}{' (sniper)' if b['sniper'] else ''} | {b['client']} | {b['team']} | {b['alive_s']:.0f} | "
          f"{b['kills']} | {b['deaths']} | {b['world_deaths']} | {b['kd']} | {b['shots']} | {b['hit_claims']} | "
          f"{b['kills_by_me']} | {b['stuck_n']} ({b['stuck_longest_s']}) | {b['idle_s']} | "
          f"{g['ok']}/{g['late']}/{g['abandoned']} | {b['stuck_jump_per_min']} | "
          f"{fmt2(rp)} / {fmt2(rm)} / {fmt2(rr)} | {kb} |\n")
    if room["rounds"]:
        w("\n## Rounds\n\n| round | start s | length s | end | win | score | round wins | kills | plant try | "
          "planted | defuse try |\n|" + "---|" * 11 + "\n")
        for r in room["rounds"]:
            w(f"| {r['no']}{' (warm-up)' if r['warmup'] else ''} | {r['start_s']} | {r['secs']} | {r['how']} | "
              f"{r['win']} | {r['score']} | {r['round_wins']} | {r['kills']} | {r['plant_starts']} | {r['plants']} | "
              f"{r['defuse_starts']} |\n")
    eps = [(b["nick"], e) for b in room["per_bot"] for e in b["stuck"]]
    if eps:
        w("\n## Stuck episodes\n\n| bot | t s | secs | pos | goal | kind |\n|---|---|---|---|---|---|\n")
        for n, e in eps:
            w(f"| {n} | {e['t']} | {e['secs']} | {e['pos']} | {e['goal']} | {e['kind']} |\n")
    lates = [(b["nick"], e) for b in room["per_bot"] for e in b["goals"]["late_list"]]
    if lates:
        w(f"\n## Goals not reached within {ROUTE_MAX_S:.0f} s\n\n| bot | t s | kind | first dist u | secs | "
          "last pos | last goal |\n|---|---|---|---|---|---|---|\n")
        for n, e in lates:
            w(f"| {n} | {e['t']} | {e['kind']} | {e['d0']} | {e['secs']} | {e['pos']} | {e['goal']} |\n")
    ids = [(b["nick"], e) for b in room["per_bot"] for e in b["idle"]]
    if ids:
        w("\n## Idle episodes\n\n| bot | t s | secs | pos | kind | wait |\n|---|---|---|---|---|---|\n")
        for n, e in ids:
            w(f"| {n} | {e['t']} | {e['secs']} | {e['pos']} | {e['kind']} | {e['wait']} |\n")
    return out.getvalue()


CSV_COLS = ["mode", "map", "bots", "minutes", "verdict", "kills", "world_deaths", "team_kills", "kd_spread", "stuck",
            "stuck_longest_s", "idle_s", "stuck_jumps", "goals_ok", "goals_late", "shots", "hit_claims",
            "reaction_measured_median_s", "objective", "failing", "folder"]


def summary_row(room):
    t = room["totals"]
    return {"mode": room["mode"], "map": room["map"], "bots": room["bots"], "minutes": room["minutes"],
            "verdict": room["verdict"], "kills": t["kills"], "world_deaths": t["world_deaths"],
            "team_kills": t["team_kills"], "kd_spread": None if room["mode"] == "BNT" else t["kd_spread"],
            "stuck": t["stuck"],
            "stuck_longest_s": t["stuck_longest_s"], "idle_s": t["idle_s"], "stuck_jumps": t["stuck_jumps"],
            "goals_ok": t["goals_ok"], "goals_late": t["goals_late"], "shots": t["shots"],
            "hit_claims": t["hit_claims"], "reaction_measured_median_s": t["reaction_measured_median_s"],
            "objective": objective_summary(room),
            "failing": "; ".join(f"{l['group']}: {l['line']}" for l in room["rubric"] if l["ok"] is False),
            "folder": os.path.basename(os.path.normpath(room["folder"])), "pool": room.get("pool") or {}}


def mode_rank(k):
    return MODE_ORDER.index(k) if k in MODE_ORDER else len(MODE_ORDER)


def pooled_lines(mode, rs):
    """The per-mode checks that pool the rooms (a single room is too few rounds): text lines with PASS / FAIL."""
    out = []

    def verdict(ok):
        return "n/a" if ok is None else ("PASS" if ok else "FAIL")

    if mode in ("TMM", "TSV"):
        rw = [r["pool"].get("round_wins") for r in rs if r["pool"].get("round_wins")]
        a, b = sum(x[0] for x in rw), sum(x[1] for x in rw)
        ok = (ROUND_WIN_POOLED[0] <= a / (a + b) <= ROUND_WIN_POOLED[1]) if a + b >= POOL_MIN_ROUNDS else None
        out.append(f"pooled round wins {ROUND_WIN_POOLED[0]:.0%}..{ROUND_WIN_POOLED[1]:.0%} per side: "
                   f"{verdict(ok)} ({a}:{b} over {len(rw)} rooms, >= {POOL_MIN_ROUNDS} rounds needed)")
    elif mode == "QRT":
        n = sum(r["pool"].get("rounds", 0) for r in rs)
        w = sum(r["pool"].get("human_wins", 0) for r in rs)
        ok = (QRT_HUMAN_WIN_SHARE[0] <= w / n <= QRT_HUMAN_WIN_SHARE[1]) if n >= POOL_MIN_ROUNDS else None
        out.append(f"pooled human wins {QRT_HUMAN_WIN_SHARE[0]:.0%}..{QRT_HUMAN_WIN_SHARE[1]:.0%}: {verdict(ok)} "
                   f"({w} of {n} rounds = {w / n:.0%})" if n else "pooled human wins: n/a (no rounds)")
    elif mode == "BNT":
        n = sum(r["pool"].get("agent_rounds", 0) for r in rs)
        w = sum(r["pool"].get("agent_wins", 0) for r in rs)
        ok = (SPY_AGENT_WIN_SHARE[0] <= w / n <= SPY_AGENT_WIN_SHARE[1]) if n >= 3 else None
        out.append(f"pooled agent wins {SPY_AGENT_WIN_SHARE[0]:.0%}..{SPY_AGENT_WIN_SHARE[1]:.0%} of the rounds that reached "
                   f"stage 2: {verdict(ok)} ({w} of {n})" if n else "pooled agent wins: n/a (no stage 2)")
    return out


def summary_md(rows):
    out = io.StringIO()
    w = out.write
    w("# Bot gameplay scores\n\n")
    w(f"{len(rows)} room(s), {sum(1 for r in rows if r['verdict'] == 'PASS')} PASS.\n\n")
    w("| mode | map | bots | min | kills | K/D spread | stuck | idle s | world deaths | objective | verdict | "
      "failing lines |\n|" + "---|" * 12 + "\n")
    for r in rows:
        w(f"| {r['mode']} | {r['map']} | {r['bots']} | {r['minutes']} | {r['kills']} | "
          f"{'-' if r['kd_spread'] is None else r['kd_spread']} | "
          f"{r['stuck']} | {r['idle_s']} | {r['world_deaths']} | {r['objective']} | {r['verdict']} | "
          f"{r['failing'] or '-'} |\n")
    w("\n## Per mode\n\n")
    for mode in sorted({r["mode"] for r in rows}, key=mode_rank):
        rs = [r for r in rows if r["mode"] == mode]
        fails = {}
        for r in rs:
            for f in filter(None, r["failing"].split("; ")):
                fails[f] = fails.get(f, 0) + 1
        top = ", ".join(f"{k} ({v})" for k, v in sorted(fails.items(), key=lambda x: -x[1])[:5])
        w(f"**{mode}** ({MODE_NAMES.get(mode, '?')}): {len(rs)} rooms, "
          f"{sum(1 for r in rs if r['verdict'] == 'PASS')} PASS; kills {sum(r['kills'] for r in rs)}, "
          f"stuck {sum(r['stuck'] for r in rs)}, idle {sum(r['idle_s'] for r in rs):.0f} s, world deaths "
          f"{sum(r['world_deaths'] for r in rs)}, stuck jumps {sum(r['stuck_jumps'] for r in rs)}. "
          f"Failing most: {top or 'none'}.\n\n")
        for text in pooled_lines(mode, rs):
            w(f"- {text}\n")
        w("\n")
    return out.getvalue()


def score_dir(root, partial, print_md=False, print_csv=False):
    rooms = find_rooms(root, partial)
    if not rooms:
        sys.exit(f"bot_eval score: no room under {root} (no report.json; --partial for running rooms)")
    rows = []
    for folder, sc in rooms:
        room = score_room(folder, sc)
        suffix = "" if sc is None or len(rooms) == 1 or sum(1 for f, _ in rooms if f == folder) == 1 \
            else "." + str(sc.get("name"))
        with open(os.path.join(folder, f"score{suffix}.json"), "w", encoding="utf-8") as f:
            json.dump(room, f, indent=1, default=str)
        with open(os.path.join(folder, f"score{suffix}.md"), "w", encoding="utf-8") as f:
            f.write(room_md(room))
        rows.append(summary_row(room))
        print(f"{room['mode']} {room['map']}: {room['verdict']}", file=sys.stderr)
    rows.sort(key=lambda r: (mode_rank(r["mode"]), str(r["map"])))
    md = summary_md(rows)
    with open(os.path.join(root, "summary.md"), "w", encoding="utf-8") as f:
        f.write(md)
    buf = io.StringIO()
    cw = csv.DictWriter(buf, fieldnames=CSV_COLS, lineterminator="\n", extrasaction="ignore")
    cw.writeheader()
    for r in rows:
        cw.writerow(r)
    with open(os.path.join(root, "scores.csv"), "w", encoding="utf-8", newline="") as f:
        f.write(buf.getvalue())
    if print_md:
        print(md)
    if print_csv:
        print(buf.getvalue())


def main():
    ap = argparse.ArgumentParser(description="bot gameplay matrix runner and scorer")
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run", help="play the mode x map matrix through dc-netlab")
    r.add_argument("--out", required=True)
    r.add_argument("--parallel", type=int, default=3)
    r.add_argument("--bots", type=int, default=8)
    r.add_argument("--only", default="", help="mode keys, comma-separated")
    r.add_argument("--minutes", type=float, default=0, help="override every room's minutes")
    r.add_argument("--bin", default=os.path.join(REPO, "target", "release"))
    r.add_argument("--rom", default=None, help="the client folder (default: CA_ROM)")
    r.add_argument("--no-score", action="store_true")
    s = sub.add_parser("score", help="score one netlab --out folder or a folder of many")
    s.add_argument("dir")
    s.add_argument("--md", action="store_true", help="also print summary.md")
    s.add_argument("--csv", action="store_true", help="also print scores.csv")
    s.add_argument("--partial", action="store_true", help="also score rooms without report.json")
    a = ap.parse_args()
    if a.cmd == "run":
        cmd_run(a)
    else:
        score_dir(a.dir, a.partial, a.md, a.csv)


if __name__ == "__main__":
    main()
