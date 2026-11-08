#!/usr/bin/env python3
"""Adds rails and left-out spawns from a folder of `dc-mapkit walk --reach` runs to the generator.

Every `<name>.out.json` (the run's JSON) with its `<name>.json` (`--nodes`: nodes, every deadly move, every move
into a trap) in the folder counts:
- a rail across each move into a trap;
- a rail against each walk or run into a kill volume that dies next to its start, or that leaves a floor (down or up)
  whose next node that way is missing, joined along grid lines; a post where a level walk that slid along a wall died;
- a cheek along a corridor mouth's side where a walk slid past the corridor's wall, or left the mouth's floor, into a
  kill volume or a pocket it could not leave;
- the spawns the run finds in a kill volume, on no floor, in a trap or on an island that cannot reach its main
  component.
A rail that would stand in a corridor's walkable middle (at the corridor's height) is left out; one just inside its
side moves out to the side. Rails and drops accumulate in rails.json and drops.json beside the generator, and the
RAILS and DROPS blocks of the generator are rewritten from them.

`--exclude x0,z0,x1,z1` (repeatable) leaves out every rail whose centre lies in that box: a place left to be seen to by
hand, where rails only move the trap (a pocket reached only by long falls under a roof whose only way down passes it).

`--regions <run output.json>` (the generator's run, with its `region ... edge x0 z0 x1 z1` lines) leaves out every
rail whose centre lies in no region: a rail on ground between the maps (a cut-off floor's edge, a former mouth) guards
nothing.

`--edge-rails x0,z0,x1,z1` (repeatable) names a pocket reached only by long falls (rails midway hang in the air and
only move the trap): a long fall into it gets its rail on the roof edge it leaves instead.

Run: python3 rails_from.py <runs dir>[,<runs dir>...] [--gen source/gen.lua] [--exclude x0,z0,x1,z1 ...]
     [--regions run.json] [--edge-rails x0,z0,x1,z1 ...]
"""
import glob
import json
import math
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import plan_links as P  # noqa: E402


def trap_rails(dump, edge_boxes=()):
    """A rail across every move into a trap, from a nodes file's `entries`, midway between the two floors (tall: from
    under the lower floor to well over the higher). A rail on the edge a long fall leaves would also close that edge
    for the drops that lead out (a roof whose only way down it is becomes a trap), so a pocket reached only by long
    falls is left to be seen to by hand: `edge_boxes` (x0, z0, x1, z1) name such pockets, and a long fall into one
    gets its rail on the edge it leaves, at the floor it leaves from (the rail midway stands in the air there)."""
    entries = dump.get("entries", [])
    rails = P.rails_of({"result": {"traps": {"list": [{"entries": entries}]}}})
    step = dump["step"]
    edges = []
    for e in entries:
        (x, y, z), (tx, _, tz) = e["from"], e["to"]
        if e.get("fall", 0) <= 60 or not any(b[0] <= tx <= b[2] and b[1] <= tz <= b[3] for b in edge_boxes):
            continue
        dx, dz = tx - x, tz - z
        d = (dx * dx + dz * dz) ** 0.5
        if d < 1:
            continue
        sx = (dx > 0) - (dx < 0) if abs(dx) > 0.4 * d else 0
        sz = (dz > 0) - (dz < 0) if abs(dz) > 0.4 * d else 0
        edges.append({"from": e["from"], "at": e["from"], "dir": [sx, sz], "cause": "kill_volume", "kind": "run"})
    if edges:
        rails += P.kill_rails({"deadly": edges}, step)
    return rails


MOUTH = 128   # the generator's MOUTH (rom_atlas.lua): each corridor's mouth reaches this far into the map's playable floor
INTRUDE = 24  # a rail may stand this far inside a corridor's side (against a kill volume just beside a mouth)
RAMP_MAX = 240  # the generator's longest mouth ramp: a rail in front of a mouth within it would stand on the ramp


def corridors(gen_src):
    """The generator's LINKS as (along_x, c, lo, hi, floor): the corridor's line, its extent along it (mouths
    included) and its floor's height at a point along it."""
    return [(k["along_x"], k["c"], k["lo"], k["hi"], _floor(k)) for k in links_of(gen_src)]


def _floor(k):
    def h_at(t):
        f = min(1.0, max(0.0, (t - k["a_in"]) / ((k["b_in"] - k["a_in"]) or 1.0)))
        return k["ha"] + (k["hb"] - k["ha"]) * f
    return h_at


SPACE_UNDER, SPACE_OVER = 60, 320   # a corridor's space: from under its floor (the ramp and the slabs) to its walls' top


def _in_space(r, floor, a0, a1):
    """Whether a rail's height meets a corridor's space over the stretch a0..a1 along it (a rail far under a corridor
    that bridges a canal, or over it, stands in no one's way)."""
    hs = (floor(a0), floor(a1))
    return r["y1"] > min(hs) - SPACE_UNDER and r["y0"] < max(hs) + SPACE_OVER


def links_of(gen_src):
    out = []
    for m in re.finditer(r"along_x = (true|false), c = (\S+), a_in = (\S+), ha = (\S+), b_in = (\S+), hb = (\S+) ",
                         gen_src):
        c, a_in, ha, b_in, hb = (float(m.group(i)) for i in range(2, 7))
        out.append({"along_x": m.group(1) == "true", "c": c, "a_in": a_in, "ha": ha, "b_in": b_in, "hb": hb,
                    "lo": min(a_in, b_in) - MOUTH - RAMP_MAX, "hi": max(a_in, b_in) + MOUTH + RAMP_MAX})
    return out


CHEEK_PAST = 128  # a cheek reaches this far past its mouth's end, over the map's own floor (a river bank or a pit
                  # diagonally in front of a mouth took walks that left it at a slant)
CHEEK_NEAR = 80   # a death this far outside a mouth's side (or INTRUDE inside it) asks for a cheek on that side
CHEEK_DH = 120    # from a floor this close to the mouth's height


def cheeks(dump, links):
    """A mouth's sides stand open between the corridor's walls and the map's floor (64 units), and the cut that
    opened it can take away the world's own invisible wall there: a walk along the corridor's wall then slides out
    past the wall's end into a kill volume beside the mouth, far from where it started (so kill_rails leaves it out).
    Every walk or run into a kill volume that ends beside a mouth puts a cheek on that side: a rail along the side
    line, from the wall's end to just past the mouth's end, INTRUDE inside the side line. The other side, and the
    mouth's front, stay open."""
    half = P.LINK_W / 2
    out = {}
    # a move into a trap from a mouth's floor counts like a death there: a pocket beside the mouth
    moves = [m for m in dump.get("deadly", []) if m["cause"] == "kill_volume" and m["kind"] in ("walk", "run")]
    moves += [{"from": e["from"], "at": e["to"]} for e in dump.get("entries", []) if e.get("kind") in ("walk", "run", "jump")]
    for m in moves:
        ax, _, az = m["at"]
        fx, _, fz = m["from"]
        for i, k in enumerate(links):
            along, cross = (ax, az) if k["along_x"] else (az, ax)
            f_along, f_cross = (fx, fz) if k["along_x"] else (fz, fx)
            off, f_off = cross - k["c"], f_cross - k["c"]
            # the death ends beside the mouth's side, or it starts on the mouth's floor and ends past a side
            beside = half - INTRUDE <= abs(off) <= half + CHEEK_NEAR
            out_of = abs(f_off) <= half + 48 and abs(off) >= half - INTRUDE
            if not beside and not out_of:
                continue
            side = 1 if off > 0 else -1
            for end, t_in, h, out_dir in (("a", k["a_in"], k["ha"], -1), ("b", k["b_in"], k["hb"], 1)):
                wall_end, mouth_end = t_in + out_dir * 64, t_in + out_dir * (MOUTH + CHEEK_PAST)
                lo, hi = min(wall_end, mouth_end), max(wall_end, mouth_end)
                where = along if beside else f_along
                if not lo - 8 <= where <= hi or abs(m["from"][1] - h) > CHEEK_DH:
                    continue
                key = (i, end, side)
                if key in out:
                    continue
                at = k["c"] + side * (half - INTRUDE + P.RAIL_T / 2 + 1)
                mid = (lo + hi) / 2
                x, z = (mid, at) if k["along_x"] else (at, mid)
                out[key] = {"x": round(x, 1), "z": round(z, 1), "y0": round(h - 60, 1), "y1": round(h + P.RAIL_H, 1),
                            "yaw": 90.0 if k["along_x"] else 0.0, "len": round(hi - lo, 1), "t": P.RAIL_T}
    return list(out.values())


def extent(r):
    """A rail's box on the ground (x0, z0, x1, z1): gen.lua makes it t across and len along, turned by yaw."""
    yaw = math.radians(r.get("yaw", 0.0))
    t, ln = r.get("t", P.RAIL_T), r.get("len", P.RAIL_LEN)
    hx = abs(math.cos(yaw)) * t / 2 + abs(math.sin(yaw)) * ln / 2
    hz = abs(math.sin(yaw)) * t / 2 + abs(math.cos(yaw)) * ln / 2
    return r["x"] - hx, r["z"] - hz, r["x"] + hx, r["z"] + hz


def in_corridor(r, corrs):
    """Whether a rail stands in a corridor's or a mouth's walkable middle (it would block the way through); one along
    a side, or beyond the walls, stays."""
    x0, z0, x1, z1 = extent(r)
    inner = P.LINK_W / 2 - INTRUDE
    for along_x, c, lo, hi, floor in corrs:
        a0, a1, c0, c1 = (x0, x1, z0, z1) if along_x else (z0, z1, x0, x1)
        if a1 > lo and a0 < hi and c1 > c - inner and c0 < c + inner and _in_space(r, floor, max(a0, lo), min(a1, hi)):
            return True
    return False


def far_ok(dump, corrs):
    """Which deaths far from their start still get a rail at the start: a walk that goes down (a fall, a slide down
    a slope too steep to stand on) or up (a face that carries the body up onto a block) from a floor whose next node
    that way is missing left the floor right there, and the edge is on that side of its grid square. A death at the
    start's height slid along a wall and died somewhere else (at_posts); one that starts in a corridor is a mouth's
    (cheeks)."""
    step = dump["step"]
    cols = {}
    for x, y, z in dump.get("nodes", []):
        cols.setdefault((round(x * 2), round(z * 2)), []).append(y)

    def ok(m):
        (x, y, z), (dx, dz) = m["from"], m["dir"]
        if abs(m["at"][1] - BODY_Y - y) <= LEVEL_DY or in_corridor_space(m["from"], corrs):
            return False
        ys = cols.get((round((x + dx * step) * 2), round((z + dz * step) * 2)), [])
        return not any(abs(v - y) <= STEP_DY for v in ys)

    return ok


BODY_Y = 50   # a deadly move's `at` is this far over the floor the body stood on
STEP_DY = 48  # a next node this much higher or lower still carries the walk on (a stair, a kerb), not an edge
LEVEL_DY = 30  # a death within this of the start's height is level: a slide along a wall
POST = 40


def at_posts(dump, corrs):
    """A post where a level walk far from its start died (it slid along a wall into a kill volume's edge): the walls
    that funnel such walks lead them to one place, and a post there stops every one of them. Never in a corridor."""
    step = dump["step"]
    out = {}
    for m in dump.get("deadly", []):
        if m["cause"] != "kill_volume" or m["kind"] not in ("walk", "run"):
            continue
        (fx, fy, fz), (ax, ay, az) = m["from"], m["at"]
        if (fx - ax) ** 2 + (fz - az) ** 2 <= (2.5 * step) ** 2 or abs(ay - BODY_Y - fy) > LEVEL_DY:
            continue
        if in_corridor_space(m["from"], corrs):
            continue
        key = (round(ax / 8), round(az / 8))
        floor = ay - BODY_Y
        out.setdefault(key, {"x": round(ax, 1), "z": round(az, 1), "y0": round(floor - 10, 1),
                             "y1": round(floor + P.RAIL_H, 1), "yaw": 0.0, "len": POST, "t": POST})
    return list(out.values())


def in_corridor_space(pt, corrs):
    """Whether a point stands in a corridor or its mouth: between its side lines, at its height."""
    x, y, z = pt
    for along_x, c, lo, hi, floor in corrs:
        a, cr = (x, z) if along_x else (z, x)
        if lo <= a <= hi and abs(cr - c) <= P.LINK_W / 2 and floor(a) - SPACE_UNDER <= y <= floor(a) + SPACE_OVER:
            return True
    return False


def fit_to_sides(r, corrs):
    """The cut that opens a corridor or its mouth can leave a world's kill volume right at the corridor's side line,
    and the kill rail on the side of a grid square beside it then stands a little inside the corridor. Such a rail
    (one running along the corridor, or a post, in the corridor's outer half) is split where it leaves the corridor
    and its part inside moves out to INTRUDE inside the side line: still short of the volume, which the cut stopped
    at the side line. Returns the pieces; in_corridor still leaves out what stands in the middle."""
    out = [r]
    for corr in corrs:
        out = [q for p in out for q in _fit_one(p, *corr)]
    return out


def _fit_one(r, along_x, c, lo, hi, floor):
    inner = P.LINK_W / 2 - INTRUDE
    post = r.get("len") == r.get("t")
    yaw = r.get("yaw", 0.0) % 180
    along = post or (abs(yaw - 90) < 1 if along_x else abs(yaw) < 1)
    x0, z0, x1, z1 = extent(r)
    a0, a1, c0, c1 = (x0, x1, z0, z1) if along_x else (z0, z1, x0, x1)
    if not along or a1 <= lo or a0 >= hi or c1 <= c - inner or c0 >= c + inner:
        return [r]
    if not _in_space(r, floor, max(a0, lo), min(a1, hi)):
        return [r]
    cross = (c0 + c1) / 2
    if abs(cross - c) < P.LINK_W / 4:
        return [r]
    moved = c + (1 if cross > c else -1) * (inner + (c1 - c0) / 2)

    def piece(s0, s1, at):
        q = dict(r) if post else dict(r, len=round(s1 - s0, 1))
        mid = (s0 + s1) / 2
        q["x"], q["z"] = (round(mid, 1), round(at, 1)) if along_x else (round(at, 1), round(mid, 1))
        return q

    if post:
        return [piece(a0, a1, moved)]
    pieces = [piece(max(a0, lo), min(a1, hi), moved)]
    if a0 < lo:
        pieces.append(piece(a0, lo, cross))
    if a1 > hi:
        pieces.append(piece(hi, a1, cross))
    return pieces


def main():
    runs_list = sys.argv[1].split(",")
    gen_path = sys.argv[sys.argv.index("--gen") + 1] if "--gen" in sys.argv else "source/gen.lua"
    here = os.path.dirname(gen_path)
    rails_store, drops_store = os.path.join(here, "rails.json"), os.path.join(here, "drops.json")
    rails = json.load(open(rails_store)) if os.path.exists(rails_store) else []
    excludes = [tuple(float(v) for v in sys.argv[i + 1].split(",")) for i, a in enumerate(sys.argv) if a == "--exclude"]
    edge_boxes = [tuple(float(v) for v in sys.argv[i + 1].split(",")) for i, a in enumerate(sys.argv) if a == "--edge-rails"]
    regions = []
    if "--regions" in sys.argv:
        for l in json.load(open(sys.argv[sys.argv.index("--regions") + 1]))["result"]["prints"]:
            m = re.match(r"region \S+ play \S+ \S+ \S+ \S+ edge (\S+) (\S+) (\S+) (\S+)", l)
            if m:
                regions.append(tuple(float(v) for v in m.groups()))

    def excluded(r):
        if regions and not any(b[0] <= r["x"] <= b[2] and b[1] <= r["z"] <= b[3] for b in regions):
            return True
        return any(b[0] <= r["x"] <= b[2] and b[1] <= r["z"] <= b[3] for b in excludes)

    links = links_of(open(gen_path).read())
    boxes = corridors(open(gen_path).read())
    before = len(rails)
    rails = [r for r in rails if not in_corridor(r, boxes)]
    if before != len(rails):
        print(f"{before - len(rails)} stored rails inside corridors left out")
    drops = json.load(open(drops_store)) if os.path.exists(drops_store) else {}
    seen = {(r["x"], r["z"], r["yaw"], r.get("len"), r.get("t")) for r in rails}
    added_rails = added_drops = added_cheeks = 0
    traps = kills = 0
    for out in sorted(f for runs in runs_list for f in glob.glob(os.path.join(runs, "*.out.json"))):
        nodes = out[: -len(".out.json")] + ".json"
        report = json.load(open(out))
        if not os.path.exists(nodes) or not report.get("ok"):
            continue
        dump = json.load(open(nodes))
        traps += len(dump.get("entries", []))
        kills += sum(1 for m in dump.get("deadly", []) if m["cause"] == "kill_volume")
        kill = [q for r in P.kill_rails(dump, dump["step"], far_ok(dump, boxes)) + at_posts(dump, boxes)
                for q in fit_to_sides(r, boxes)]
        for r in cheeks(dump, links):
            key = (r["x"], r["z"], r["yaw"], r.get("len"), r.get("t"))
            if key not in seen:
                seen.add(key)
                rails.append(r)
                added_rails += 1
                added_cheeks += 1
        for r in trap_rails(dump, edge_boxes) + kill:
            key = (r["x"], r["z"], r["yaw"], r.get("len"), r.get("t"))
            if in_corridor(r, boxes):
                continue
            if key not in seen:
                seen.add(key)
                rails.append(r)
                added_rails += 1
        for pid, pts in P.drops_of(report).items():
            have = drops.setdefault(pid, [])
            for pt in pts:
                if pt not in have:
                    have.append(pt)
                    added_drops += 1
    if excludes or regions:
        before = len(rails)
        rails = [r for r in rails if not excluded(r)]
        print(f"{before - len(rails)} rails outside every region or in excluded boxes left out")
    json.dump(rails, open(rails_store, "w"), indent=0)
    json.dump(drops, open(drops_store, "w"), indent=0)
    src = open(gen_path).read()
    src = re.sub(r"-- RAILS BEGIN\n.*?\n-- RAILS END", "-- RAILS BEGIN\n" + P.rails_table(rails) + "\n-- RAILS END",
                 src, count=1, flags=re.S)
    src = re.sub(r"-- DROPS BEGIN\n.*?\n-- DROPS END", "-- DROPS BEGIN\n" + P.drops_table(drops) + "\n-- DROPS END",
                 src, count=1, flags=re.S)
    open(gen_path, "w").write(src)
    print(f"trap entries {traps}, kill-volume moves {kills}; rails +{added_rails} (cheeks {added_cheeks}; {len(rails)} in all), "
          f"spawns left out +{added_drops} ({sum(len(v) for v in drops.values())} in all)")


if __name__ == "__main__":
    main()
