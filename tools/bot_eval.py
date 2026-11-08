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
(lines `<ms> dc-dserver: ...`), `report.json` (mode, map, minutes, per-bot shots) and the bots' `.out` files.

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
  stuck_jump/min  `stuck_jump` events per minute of alive time.
  reaction        (a) planned: the `reaction` event's `delay_ms` (the skill's delay, + a sniper's settle; first
                  sightings = `reacquire` false). (b) measured: per `reaction` event (the bot takes a seen enemy as
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
WORLD_DEATHS_MAX_PER_BOT_PER_10MIN = 1
ROUTE_RETARGET_U = 1500.0       # a goal jump this large between two samples starts a new goal episode
ROUTE_GAP_S = 6.0               # goal-less samples (waits) inside one goal episode
GOAL_REACH_U = 96.0
ROUTE_MAX_GOAL_U = 2000.0
ROUTE_MAX_S = 60.0
ROUTE_LATE_MAX_SHARE = 0.10     # share of judged goals allowed to be late
CELL_U = 256.0
COVERAGE_MIN = 0.60             # per team, TDM
KILLS_PER_2MIN_ALIVE_MIN = 0.67       # 1 kill per 3 min alive (a quiet average player on a big map)
TEAM_KILL_SHARE = (0.35, 0.65)  # TDM / CTF
FFA_TOP_SHARE_MAX = 0.40
ACCURACY = (0.05, 0.40)         # hit_claims / shots, rifles (hits per shot; a kill takes several hits)
ACCURACY_MIN_SHOTS = 20         # a bot with fewer shots is not judged
REACTION_MEDIAN = (0.3, 1.2)    # seconds, measured sight -> first shot
REACTION_INSTANT_S = 0.15       # "never instantly"
REACTION_MAX_S = 3.0
REACTION_OVER_MAX_SHARE = 0.02  # share of measured reactions allowed above REACTION_MAX_S (a lost sighting)
SND_PLANT_ROUNDS_MIN = 0.70
SND_DEFUSE_AFTER_PLANT_MIN = 1.0
SND_DECIDED_MIN = 0.80          # rounds ended by bomb / defuse / elimination
SND_SPAWN_LEAVE_U = 512.0       # an attacker "left the spawn" when it got this far from its first round position
CTF_CAPTURE_EVERY_S = 180.0
CTF_DROP_RESOLVE_S = 20.0
SPY_PICKUP_S = 30.0
SPY_STAGE_MAX_S = 240.0
QRT_BEFORE_TIMER_MIN = 0.50
TSV_ELIM_MIN = 0.80
REFUSAL_SPAM_MAX = 20           # server lines with "refused" per room
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
RE = {k: re.compile(v) for k, v in {
    "linked": r"^dc-dserver: client (\d+) linked from",
    "join": r"^@join (\d+) (\d+) \d+ \d+ (\S+)",
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
    "infect_client": r"^dc-dserver: infect: client (\d+) infected",
}.items()}


def parse_dserver(path):
    """Everything the score needs from one game server log."""
    d = {"linked": {}, "nick": {}, "uid": {}, "team_at": {}, "kills": [], "rounds": [], "objective": [],
         "match_over": False, "refused": 0, "mode_class": None, "goal": None, "round_secs": 180, "lines": 0}
    cur = None
    warmup_over_pending = False
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.readlines()
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
            d["nick"][c] = g.group(3)
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


def metric_idle(segs):
    """Idle episodes: no goal, the body within IDLE_RADIUS_U of the start for >= IDLE_MIN_S."""
    eps = []
    for seg in segs:
        i = 0
        while i < len(seg):
            s0 = seg[i]
            j = i
            if s0.get("goal") is None:
                while (j + 1 < len(seg) and seg[j + 1].get("goal") is None
                       and dist3(seg[j + 1]["pos"], s0["pos"]) <= IDLE_RADIUS_U):
                    j += 1
            dur = seg[j]["t"] - s0["t"]
            if j > i and dur >= IDLE_MIN_S:
                eps.append({"t": round(s0["t"], 1), "secs": round(dur, 1), "pos": [round(v) for v in s0["pos"]],
                            "kind": s0.get("kind"), "wait": s0.get("wait")})
                i = j + 1
            else:
                i += 1
    return eps


def metric_goals(segs):
    """Goal episodes and the route verdict (ok within ROUTE_MAX_S / late / abandoned / unjudged far goals)."""
    res = {"ok": 0, "late": 0, "abandoned": 0, "far": 0, "by_arrived": 0, "late_list": []}

    def close(ep):
        if ep is None:
            return
        if ep["d0"] >= ROUTE_MAX_GOAL_U:
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
    planned = [e["delay_ms"] / 1000.0 for e in evs if e.get("ev") == "reaction" and not e.get("reacquire")
               and e.get("delay_ms") is not None]
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
            tracks.append(e)
    deaths_ms = [e["ms"] for e in evs if e.get("ev") == "died" and e.get("ms") is not None]
    segs = life_segments(tracks, deaths_ms)
    stuck = metric_stuck(segs)
    idle = metric_idle(segs)
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
        "goals": goals,
        "stuck_jumps": sum(1 for e in evs if e.get("ev") == "stuck_jump"),
        "stuck_jump_per_min": round(sum(1 for e in evs if e.get("ev") == "stuck_jump") / (alive_s / 60.0), 2)
        if alive_s else 0.0,
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
            if k["killer"] is None:
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
            if key != "FFA" and tk is not None and tk == tv:
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
    room["flow"] = flow_metrics(sdir, bots, ds, sc)
    room["rubric"] = rubric(key, minutes, bots, room, ds)
    room["verdict"] = "PASS" if all(l["ok"] for l in room["rubric"] if l["ok"] is not None) else "FAIL"
    for b in bots:
        b.pop("tracks", None)
        b.pop("bomb_carrier", None)
        b.pop("squad", None)
    room["per_bot"] = bots
    return room


def parse_dserver_empty():
    return {"linked": {}, "nick": {}, "uid": {}, "team_at": {}, "kills": [], "rounds": [], "objective": [],
            "match_over": False, "refused": 0, "mode_class": None, "goal": None, "round_secs": 180, "last_ms": 0,
            "lines": 0}


def round_row(r):
    return {"no": r["no"], "warmup": r["warmup"], "start_s": round(r["start"] / 1000.0, 1),
            "secs": round(((r["end"] or r["start"]) - r["start"]) / 1000.0, 1), "how": r["how"], "win": r["win"],
            "score": r["score"], "round_wins": r["round_wins"], "kills": len(r["kills"]),
            "plant_starts": r["plant_starts"], "plants": r["plants"], "defuse_starts": r["defuse_starts"]}


def real_rounds(ds):
    return [r for r in ds["rounds"] if not r["warmup"] and r["how"] not in ("cut",)]


def room_totals(bots, ds, key):
    kills = [k for k in ds["kills"] if not k["warmup"]]
    team_kills = {1: 0, 2: 0}
    for k in kills:
        if k["killer"] is None or k["killer"] == k["victim"]:
            continue
        tk, tv = team_of(ds, k["killer"], k["ms"]), team_of(ds, k["victim"], k["ms"])
        if key == "FFA" or tk != tv:
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
    return {
        "kills": sum(b["kills"] for b in bots), "server_kill_lines": len(kills),
        "world_deaths": sum(1 for k in kills if k["killer"] is None),
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
        o["round_ends"][r["how"]] = o["round_ends"].get(r["how"], 0) + 1
    if key == "TMM":
        o.update(snd_metrics(ds, rounds, bots))
    elif key == "CTF":
        o.update(ctf_metrics(ds))
    elif key == "BNT":
        o.update(spy_metrics(ds))
    elif key == "TSV" or key == "QRT":
        o.update(elim_metrics(ds, rounds))
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
    return {"plant_attempt_rounds": plant_rounds, "planted_rounds": len(planted),
            "defuse_after_plant_rounds": defuse_tried, "decided_rounds": decided,
            "attackers_left_spawn": left_spawn, "defender_split": split}


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
    return {"captures": len(caps), "drops": drops, "carries": carries,
            "taken": sum(1 for e in ev if e["kind"] == "ctf_taken"),
            "returned_by_client": sum(1 for e in ev if e["kind"] == "ctf_returned" and e["args"][1] != "the timer"),
            "returned_by_timer": sum(1 for e in ev if e["kind"] == "ctf_returned" and e["args"][1] == "the timer")}


def spy_metrics(ds):
    """Spy Hunt: time from intel available to the next pickup, stage 1 lengths, how stage 2 ended."""
    ev = ds["objective"]
    pickups, stages, stage2 = [], [], []
    avail = None
    stage_start = None
    agent_at = None
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
        elif k in ("spy_done", "spy_fell", "spy_agent_killed", "spy_survived", "spy_gone") and agent_at is not None:
            stage2.append({"end": k[4:], "secs": round((e["ms"] - agent_at) / 1000.0, 1)})
            agent_at = None
    open_stage = round((ds["last_ms"] - stage_start) / 1000.0, 1) if stage_start is not None else None
    return {"pickup_delays_s": pickups, "stage1_secs": stages, "stage1_open_s": open_stage, "stage2": stage2,
            "uploads": sum(1 for e in ev if e["kind"] == "spy_uploads"),
            "missions": sum(1 for e in ev if e["kind"] == "spy_done")}


def elim_metrics(ds, rounds):
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
    return {"round_alive": out, "infections": infected}


def flow_metrics(sdir, bots, ds, sc):
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
    return {"panics": panics, "drops": drops, "join_failed": sum(b["join_failed"] for b in bots),
            "refused_lines": ds["refused"]}


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
    L.append(line("mobility", f"idle (no goal, still >= {IDLE_MIN_S:.0f} s) <= {IDLE_MAX_S:.0f} s",
                  t["idle_s"] <= IDLE_MAX_S, f"{t['idle_s']} s in {sum(len(b['idle']) for b in bots)} episodes "
                  + (str(kinds) if kinds else "")))
    wmax = WORLD_DEATHS_MAX_PER_BOT_PER_10MIN * per10
    L.append(line("mobility", f"world deaths <= {wmax} per bot", all(b["world_deaths"] <= wmax for b in bots),
                  f"{t['world_deaths']} total"))
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
    # 2. combat
    if key in RESPAWN_MODES:
        slow = [b["nick"] for b in bots if b["alive_s"] > 0
                and b["kills"] / (b["alive_s"] / 120.0) < KILLS_PER_2MIN_ALIVE_MIN]
        L.append(line("combat", f"each bot >= {KILLS_PER_2MIN_ALIVE_MIN:g} kill per 2 min alive", not slow,
                      f"below: {', '.join(slow)}" if slow else "all"))
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
        acc = [(b["nick"], (b["hit_claims"] or 0) / b["shots"]) for b in bots
               if not b["sniper"] and (b["shots"] or 0) >= ACCURACY_MIN_SHOTS]
        bad = [f"{n} {a:.1%}" for n, a in acc if not ACCURACY[0] <= a <= ACCURACY[1]]
        L.append(line("combat", f"accuracy hits/shots {ACCURACY[0]:.0%}..{ACCURACY[1]:.0%} (rifles)",
                      (not bad) if acc else None, f"outside: {', '.join(bad)}" if bad else f"{len(acc)} judged"))
        med = t["reaction_measured_median_s"]
        L.append(line("combat", f"reaction sight->shot median {REACTION_MEDIAN[0]}..{REACTION_MEDIAN[1]} s, "
                      f"none < {REACTION_INSTANT_S} s, <= {REACTION_OVER_MAX_SHARE:.0%} > {REACTION_MAX_S:.0f} s",
                      (REACTION_MEDIAN[0] <= med <= REACTION_MEDIAN[1] and t["reaction_measured_instant"] == 0
                       and t["reaction_measured_over_max"] <= REACTION_OVER_MAX_SHARE * t["reaction_measured_n"])
                      if med is not None else None,
                      f"median {med} s of {t['reaction_measured_n']}, instant {t['reaction_measured_instant']}, "
                      f"over {t['reaction_measured_over_max']} (max {t['reaction_measured_max_s']}); planned median "
                      f"{t['reaction_planned_median_s']} s"))
    # 3. objective
    n = o.get("rounds", 0)
    if key == "TMM":
        L.append(line("objective", f"plant attempted in >= {SND_PLANT_ROUNDS_MIN:.0%} of rounds",
                      (o["plant_attempt_rounds"] / n >= SND_PLANT_ROUNDS_MIN) if n else None,
                      f"{o['plant_attempt_rounds']} of {n}"))
        L.append(line("objective", "defuse attempted after every plant",
                      (o["defuse_after_plant_rounds"] / o["planted_rounds"] >= SND_DEFUSE_AFTER_PLANT_MIN)
                      if o["planted_rounds"] else None, f"{o['defuse_after_plant_rounds']} of {o['planted_rounds']}"))
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
    elif key == "QRT":
        before = sum(v for k, v in o["round_ends"].items() if k != "timer")
        L.append(line("objective", f">= {QRT_BEFORE_TIMER_MIN:.0%} of rounds end before the timer",
                      (before / n >= QRT_BEFORE_TIMER_MIN) if n else None,
                      f"{before} of {n}, ends {o['round_ends']}, {o['infections']} infections"))
    elif key == "TSV":
        el = o["round_ends"].get("elimination", 0)
        L.append(line("objective", f">= {TSV_ELIM_MIN:.0%} of rounds end by elimination",
                      (el / n >= TSV_ELIM_MIN) if n else None, f"{el} of {n}, ends {o['round_ends']}"))
        never = [x["round"] for x in o["round_alive"] if x["how"] == "timer"
                 and sum(1 for v in x["alive_at_end"].values() if v >= 2) >= 2]
        L.append(line("objective", "no timer end with 2+ alive on both sides", not never if n else None,
                      f"rounds {never}" if never else "none"))
    # 4. flow
    f = room["flow"]
    L.append(line("flow", "no bot drop, no join_failed, no panic",
                  not f["drops"] and not f["join_failed"] and not f["panics"],
                  f"drops {f['drops']}, join_failed {f['join_failed']}, panics {f['panics']}"))
    L.append(line("flow", f"server refusals <= {REFUSAL_SPAM_MAX}", f["refused_lines"] <= REFUSAL_SPAM_MAX,
                  f"{f['refused_lines']} lines"))
    return L


# ===============================================================================================================
# output
# ===============================================================================================================
def objective_summary(room):
    o = room["objective"]
    k = room["mode"]
    ends = ",".join(f"{a}:{b}" for a, b in sorted(o.get("round_ends", {}).items()))
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
    w(f"- kills {t['kills']} (world deaths {t['world_deaths']}, team kills {t['team_kills']}), by team "
      f"{t['kills_by_team']}, K/D spread {t['kd_spread']}; shots {t['shots']}, hits claimed {t['hit_claims']}\n")
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
            "team_kills": t["team_kills"], "kd_spread": t["kd_spread"], "stuck": t["stuck"],
            "stuck_longest_s": t["stuck_longest_s"], "idle_s": t["idle_s"], "stuck_jumps": t["stuck_jumps"],
            "goals_ok": t["goals_ok"], "goals_late": t["goals_late"], "shots": t["shots"],
            "hit_claims": t["hit_claims"], "reaction_measured_median_s": t["reaction_measured_median_s"],
            "objective": objective_summary(room),
            "failing": "; ".join(f"{l['group']}: {l['line']}" for l in room["rubric"] if l["ok"] is False),
            "folder": os.path.basename(os.path.normpath(room["folder"]))}


def mode_rank(k):
    return MODE_ORDER.index(k) if k in MODE_ORDER else len(MODE_ORDER)


def summary_md(rows):
    out = io.StringIO()
    w = out.write
    w("# Bot gameplay scores\n\n")
    w(f"{len(rows)} room(s), {sum(1 for r in rows if r['verdict'] == 'PASS')} PASS.\n\n")
    w("| mode | map | bots | min | kills | K/D spread | stuck | idle s | world deaths | objective | verdict | "
      "failing lines |\n|" + "---|" * 12 + "\n")
    for r in rows:
        w(f"| {r['mode']} | {r['map']} | {r['bots']} | {r['minutes']} | {r['kills']} | {r['kd_spread']} | "
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
    cw = csv.DictWriter(buf, fieldnames=CSV_COLS, lineterminator="\n")
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
