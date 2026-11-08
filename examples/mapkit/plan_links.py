#!/usr/bin/env python3
"""Plans the corridors of a map made of ROM pieces (examples/mapkit/rom_atlas.lua) from each world's playable
floor, and the rails and left-out spawns from the walk check, and writes them into the generator.

Inputs:
  gen output   the JSON `dc-mapkit run . source/gen.lua --json` printed for the closed layout (LINKS empty): its
               `region <id> play x0 z0 x1 z1 edge x0 z0 x1 z1` lines give every map's play box and region.
  nodes dir    one `<piece id>.json` per map from `dc-mapkit walk . --reach --box <id> --seed-piece <id>
               --nodes <file>`: the floor every player of that map can reach and leave under its own rules.

For every two maps whose regions face each other across the gap (GAP, or up to MAX_GAP), it scans the shared stretch for a straight
corridor: on each side the playable node nearest the shared edge on the corridor's line, with a landing as wide as
the corridor at the same height, and keeps the corridor that reaches least far into the two maps (and climbs least).
The result replaces the block between `-- LINKS BEGIN` and `-- LINKS END` in the generator (`--gen`, default
source/gen.lua); `--rails reach.json` (and `--kill-rails main.json`, the whole map's `walk --reach --nodes` file) adds
a rail across every move into a trap (or into a kill volume) to RAILS, `--drop-spawns reach.json` the spawns the
walk check finds in a kill volume, on no floor, in a trap or on an island to DROPS. Rails and drops accumulate in
rails.json and drops.json beside the generator. Nothing of the ROM is read here: only the walk check's files.

Run: python3 plan_links.py <run output.json> <nodes dir> [--gen gen.lua] [--avoid rails.json] [--hazards dir,...]
[--max-len N [--keep a>b,...]]
[--rails reach.json [--kill-rails main.json]] [--drop-spawns reach.json] [--write]
`--max-len N` leaves out corridors longer than N (longest first) while both their maps keep two other corridors and
the maps stay joined; `--keep` names pairs kept whatever their length (bridges over water). `--avoid rails.json`
keeps every mouth clear of the rails the walk check placed (they stand at the edges of kill volumes and pockets);
the nodes files' own deaths (walks into kill volumes) keep mouths off such places from the start, and `--hazards
<round dir>[,...]` adds the check rounds' deaths (a pit a corridor's cut opened the way to).
"""
import json
import os
import re
import sys
from collections import defaultdict

GAP = 240
MAX_GAP = 900      # two regions further apart than GAP (the layout's packing leaves some) still get a corridor up to this
LINK_W = 240
LINE = 40          # a node on the corridor's line: this close across
LANDING = 96       # the landing reaches this far to each side of the line (the corridor's walls stand at 120)
LANDING_ALONG = 64
LANDING_DH = 40
MOUTH = 128        # the generator's mouth: it reaches this far into the map from the end
APPROACH_DOWN = 100   # the floor in front of a mouth may lie this far under the end (the generator ramps down to it)
APPROACH_UP = 30      # or this far over it (a step)
MAX_SLOPE = 0.6    # the corridor is one ramp from end to end: rise per unit (about 31 degrees; a player climbs 45)
EDGE_ROOM = 96     # an end stays this far inside its region (its cut starts 64 deeper)
GAP_FLOOR_MIN = -400   # across the gap a corridor stays this high: the ground under it can come down to -512 only
RAIL_LEN, RAIL_T, RAIL_H = 72, 12, 220
CORRIDOR_ROOM = 96     # two corridors keep this far apart (a wall of one never stands in the other's mouth)
MAIN_FLOOR = 0.0       # the generator puts each world's spawns' median height here
MAIN_WEIGHT = 1.5      # cost per unit an end lies above or below it (a canal far under the docks is a poor end)
MAIN_LIMIT = 400       # and no end further than this from it (the way back up may be thousands of units away)
C_STEP = 32
SECOND_LINK_MIN_EDGE = 4500   # a long shared stretch gets a second corridor this far from the first
SECOND_LINK_APART = 2500


def regions_of(gen):
    out = {}
    for line in gen["result"]["prints"]:
        m = re.match(r"region (\S+) play (\S+) (\S+) (\S+) (\S+) edge (\S+) (\S+) (\S+) (\S+)", line)
        if m:
            v = [float(x) for x in m.groups()[1:]]
            out[m.group(1)] = {"play": v[0:4], "edge": v[4:8]}
    return out


def kill_boxes(path):
    """The kill-node areas of the reach run beside a nodes file (`<name>.out.json`): (x0, z0, x1, z1, y0, y1)."""
    out = []
    try:
        r = json.load(open(path[:-len(".json")] + ".out.json"))["result"]
    except (FileNotFoundError, KeyError, ValueError):
        return out
    for a in r.get("kill_nodes", {}).get("list", []):
        out.append((a["min"][0], a["min"][1], a["max"][0], a["max"][1], a["floor"][0] - 30, a["floor"][1] + 30))
    return out


def load_nodes(path):
    """The reach run's nodes, less those inside its kill-node areas (floor inside a kill volume is no floor to end a
    corridor on, nor to walk onto one from)."""
    d = json.load(open(path))
    boxes = kill_boxes(path)
    nodes = [n for n in d["nodes"]
             if not any(x0 <= n[0] <= x1 and z0 <= n[2] <= z1 and y0 <= n[1] <= y1 for x0, z0, x1, z1, y0, y1 in boxes)]
    if len(nodes) != len(d["nodes"]):
        print(f"{os.path.basename(path)}: {len(d['nodes']) - len(nodes)} nodes inside kill volumes left out")
    return d["step"], nodes


def hazards(path):
    """Where the reach run's walks into kill volumes died (any kind): a mouth never opens onto such a place."""
    d = json.load(open(path))
    return [m["at"] for m in d.get("deadly", []) if m.get("cause") == "kill_volume"]


class Side:
    """One map's nodes seen along a corridor axis: bucketed by the across coordinate."""

    def __init__(self, nodes, along_x):
        self.by = defaultdict(list)
        for x, y, z in nodes:
            along, across = (x, z) if along_x else (z, x)
            self.by[int(across // 16)].append((along, y, across))

    def near_line(self, c, half):
        lo, hi = int((c - half) // 16), int((c + half) // 16)
        for k in range(lo, hi + 1):
            for n in self.by.get(k, ()):
                if abs(n[2] - c) <= half:
                    yield n

    def approach(self, c, t_in, facing_plus, h):
        """Whether the floor in front of the mouth (MOUTH to MOUTH + 96 further into the map from t_in, within
        LANDING across) lies where a player walks onto the end: at most APPROACH_DOWN under it (a ramp) or
        APPROACH_UP over it. An end on a platform over the floor in front fails."""
        lo, hi = (t_in - MOUTH - 96, t_in - MOUTH) if facing_plus else (t_in + MOUTH, t_in + MOUTH + 96)
        for a, y, _ in self.near_line(c, LANDING):
            if lo <= a <= hi and -APPROACH_DOWN <= y - h <= APPROACH_UP:
                return True
        return False

    def entry(self, c, facing_plus, limit):
        """The node on line c nearest the facing side (max along when the other map is on the plus side) but no
        nearer than `limit`, with a landing at the same height on both sides of the line; None when there is none."""
        line = sorted(
            (n for n in self.near_line(c, LINE) if (n[0] <= limit if facing_plus else n[0] >= limit)),
            key=lambda n: -n[0] if facing_plus else n[0],
        )
        for along, y, _ in line[:60]:
            ok = True
            for off in (-LANDING, LANDING):
                if not any(
                    abs(n[0] - along) <= LANDING_ALONG and abs(n[1] - y) <= LANDING_DH
                    for n in self.near_line(c + off, LINE)
                ):
                    ok = False
                    break
            if ok:
                return along, y
        return None


def footprint(along_x, c, a_in, b_in):
    """The x/z box a corridor takes, its walls and mouths included, grown by CORRIDOR_ROOM."""
    half = LINK_W / 2 + 32 + CORRIDOR_ROOM
    a0, a1 = a_in - 64 - CORRIDOR_ROOM, b_in + 64 + CORRIDOR_ROOM
    return (a0, c - half, a1, c + half) if along_x else (c - half, a0, c + half, a1)


def overlaps(p, q):
    return p[0] < q[2] and q[0] < p[2] and p[1] < q[3] and q[1] < p[3]


AVOID_IN = 100    # a rail this close to the corridor's line, in the mouth or on the way onto it, moves the mouth


def mouth_box(along_x, c, t_in, facing_plus):
    """The ground a mouth takes: the corridor's inside, from the floor in front of the mouth to the start of the walls
    (an end is only as good as the way onto it)."""
    lo, hi = (t_in - MOUTH - 96, t_in + 64) if facing_plus else (t_in - 64, t_in + MOUTH + 96)
    return (lo, c - AVOID_IN, hi, c + AVOID_IN) if along_x else (c - AVOID_IN, lo, c + AVOID_IN, hi)


HAZARD_AHEAD = 200   # a death into a kill volume this far in front of a mouth (a pit, a river bank) moves the mouth


def hazard_box(along_x, c, t_in, facing_plus):
    """The ground in front of a mouth and the mouth itself, the corridor's walls' width across."""
    half = LINK_W / 2 + 32
    lo, hi = (t_in - MOUTH - HAZARD_AHEAD, t_in + 64) if facing_plus else (t_in - 64, t_in + MOUTH + HAZARD_AHEAD)
    return (lo, c - half, hi, c + half) if along_x else (c - half, lo, c + half, hi)


def rail_in(box, rails, h):
    """A rail standing in the box at the floor's height (one far under or over it, a bridge's or a roof's, is no
    obstacle)."""
    x0, z0, x1, z1 = box
    return any(x0 <= r["x"] <= x1 and z0 <= r["z"] <= z1 and r["y0"] <= h + 120 and r["y1"] >= h - 40 for r in rails)


def death_in(box, points, h):
    """A walk into a kill volume died in the box near the floor's height (from above it or up to a fall below)."""
    x0, z0, x1, z1 = box
    return any(x0 <= p[0] <= x1 and z0 <= p[2] <= z1 and h - 400 <= p[1] <= h + 120 for p in points)


def plan(regions, nodes_by_piece, avoid=(), hazards_by_piece=None, hazards_all=()):
    """Every pair of facing regions gets its candidate corridors (sorted by cost); then the pairs are placed most
    constrained first (fewest candidates), each taking its cheapest candidate clear of the corridors already placed,
    so a pair with little room is not crowded out by a neighbour that had many places to choose from. A mouth is
    never put where `avoid` (the rails the walk check placed: the edges of kill volumes and pockets) has a rail."""
    ids = sorted(regions, key=lambda s: int(s.split("_")[1]))
    sides = {}
    pairs = []
    for a in ids:
        for b in ids:
            if a == b or a not in nodes_by_piece or b not in nodes_by_piece:
                continue
            ra, rb = regions[a], regions[b]
            # b east of a (along x) or north of a (along z)
            for along_x, ax_edge, ac in ((True, 0, 1), (False, 1, 0)):
                if not GAP - 4 < rb["edge"][ax_edge] - ra["edge"][ax_edge + 2] < MAX_GAP:
                    continue
                lo = max(ra["play"][ac], rb["play"][ac])
                hi = min(ra["play"][ac + 2], rb["play"][ac + 2])
                if hi - lo < LINK_W:
                    continue
                key_a, key_b = (a, along_x), (b, along_x)
                for key in (key_a, key_b):
                    if key not in sides:
                        sides[key] = Side(nodes_by_piece[key[0]], along_x)
                sa, sb = sides[key_a], sides[key_b]
                a_side, b_side = ra["play"][ax_edge + 2], rb["play"][ax_edge]
                a_edge, b_edge = ra["edge"][ax_edge + 2], rb["edge"][ax_edge]
                found = []
                c = lo + LINK_W / 2
                while c <= hi - LINK_W / 2:
                    ea = sa.entry(c, True, a_edge - EDGE_ROOM)
                    eb = sb.entry(c, False, b_edge + EDGE_ROOM) if ea else None
                    if ea and eb and abs(ea[1] - MAIN_FLOOR) > MAIN_LIMIT:
                        ea = None
                    if ea and eb and abs(eb[1] - MAIN_FLOOR) > MAIN_LIMIT:
                        eb = None
                    if ea and eb and not sa.approach(c, ea[0], True, ea[1]):
                        ea = None
                    if ea and eb and not sb.approach(c, eb[0], False, eb[1]):
                        eb = None
                    if ea and eb and avoid and (rail_in(mouth_box(along_x, c, ea[0], True), avoid, ea[1])
                                                or rail_in(mouth_box(along_x, c, eb[0], False), avoid, eb[1])):
                        ea = None
                    if ea and eb and hazards_by_piece and (
                            death_in(hazard_box(along_x, c, ea[0], True), hazards_by_piece.get(a, ()), ea[1])
                            or death_in(hazard_box(along_x, c, eb[0], False), hazards_by_piece.get(b, ()), eb[1])):
                        ea = None
                    if ea and eb and hazards_all and (
                            death_in(hazard_box(along_x, c, ea[0], True), hazards_all, ea[1])
                            or death_in(hazard_box(along_x, c, eb[0], False), hazards_all, eb[1])):
                        ea = None
                    if ea and eb:
                        length = eb[0] - ea[0]

                        def h_at(t):
                            f = min(1.0, max(0.0, (t - ea[0]) / length))
                            return ea[1] + (eb[1] - ea[1]) * f

                        low_in_gap = min(h_at(a_edge), h_at(b_edge))
                        if abs(eb[1] - ea[1]) <= length * MAX_SLOPE and low_in_gap >= GAP_FLOOR_MIN:
                            da, db = max(0.0, a_side - ea[0]), max(0.0, eb[0] - b_side)
                            off_main = abs(ea[1] - MAIN_FLOOR) + abs(eb[1] - MAIN_FLOOR)
                            found.append((da + db + 1.5 * abs(eb[1] - ea[1]) + MAIN_WEIGHT * off_main, c, ea, eb))
                    c += C_STEP
                if not found:
                    print(f"no corridor between {a} and {b}")
                    continue
                found.sort()
                pairs.append({"a": a, "b": b, "along_x": along_x, "found": found, "hi_lo": hi - lo})
    pairs.sort(key=lambda p: (len(p["found"]), p["found"][0][0]))
    links, placed = [], []
    for p in pairs:
        a, b, along_x, found = p["a"], p["b"], p["along_x"], p["found"]
        clear = [f for f in found
                 if not any(overlaps(footprint(along_x, f[1], f[2][0], f[3][0]), q) for q in placed)]
        if not clear:
            print(f"no corridor between {a} and {b} clear of the others")
            continue
        chosen = [clear[0]]
        if p["hi_lo"] >= SECOND_LINK_MIN_EDGE:
            for f in clear[1:]:
                if abs(f[1] - clear[0][1]) >= SECOND_LINK_APART and f[0] <= 1.5 * clear[0][0] + 600:
                    chosen.append(f)
                    break
        for f in chosen:
            placed.append(footprint(along_x, f[1], f[2][0], f[3][0]))
        for cost, c, ea, eb in chosen:
            links.append({
                "a": a, "b": b, "along_x": along_x, "c": round(c, 1),
                "a_in": round(ea[0], 1), "ha": round(ea[1], 1),
                "b_in": round(eb[0], 1), "hb": round(eb[1], 1),
                "cost": round(cost),
            })
    links.sort(key=lambda l: (int(l["a"].split("_")[1]), int(l["b"].split("_")[1]), l["c"]))
    return links


def prune_long(links, max_len, keep):
    """Leaves out, longest first, every corridor longer than max_len whose two maps each keep two other corridors
    and whose loss keeps the maps joined; `keep` names pairs ("a>b") that stay whatever their length (bridges over
    water that are wanted as such)."""
    links = list(links)
    while True:
        per = defaultdict(int)
        for l in links:
            per[l["a"]] += 1
            per[l["b"]] += 1
        ids = sorted(per)
        cands = [l for l in links if l["b_in"] - l["a_in"] > max_len and f"{l['a']}>{l['b']}" not in keep
                 and per[l["a"]] >= 3 and per[l["b"]] >= 3]
        cands.sort(key=lambda l: -(l["b_in"] - l["a_in"]))
        dropped = False
        for l in cands:
            rest = [m for m in links if m is not l]
            if len(joined(ids, rest)) == 1:
                print(f"left out {l['a']} -> {l['b']} c {l['c']:.0f}: {l['b_in'] - l['a_in']:.0f} long, both maps keep other doors")
                links = rest
                dropped = True
                break
        if not dropped:
            return links


def joined(ids, links):
    parent = {i: i for i in ids}

    def find(i):
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    for l in links:
        parent[find(l["a"])] = find(l["b"])
    groups = defaultdict(list)
    for i in ids:
        groups[find(i)].append(i)
    return list(groups.values())


def rails_of(reach):
    """A rail (an invisible wall for players) across every move into a trap: midway between the two floors, square
    to the move, from just under the higher floor to well over it."""
    import math
    rails = []
    for area in reach["result"]["traps"]["list"]:
        for m in area.get("entries", []):
            (x1, y1, z1), (x2, y2, z2) = m["from"], m["to"]
            dx, dz = x2 - x1, z2 - z1
            d = math.hypot(dx, dz)
            if d < 1:
                continue
            top = max(y1, y2)
            rails.append({
                "x": round((x1 + x2) / 2, 1), "z": round((z1 + z2) / 2, 1),
                "y0": round(min(y1, y2) - 10, 1), "y1": round(top + RAIL_H, 1),
                "yaw": round(math.degrees(math.atan2(-dz / d, dx / d)), 2),
                "len": RAIL_LEN if d <= 70 else RAIL_LEN + 24,
            })
    return rails


def kill_rails(dump, step, far_ok=None):
    """Rails against walking into a kill volume: for every node from which a walk or a run in some direction dies
    in one, a rail on that side of its grid square (or a post at the corner for a diagonal whose two sides stay
    open); the sides on one grid line at about one height are joined into long rails. A jump into one is the
    player's choice and keeps its way."""
    half = step / 2
    dirs = {}
    for m in dump["deadly"]:
        if m["cause"] != "kill_volume" or m["kind"] not in ("walk", "run"):
            continue
        # only a death next to the start: a move that slid a long way (along a corridor's wall, say) died somewhere
        # else, and the moves from the floor beside that place carry its rail
        # (far_ok may still take one: a fall or a slide down from a floor whose next node that way is missing)
        (fx, _, fz), (ax, _, az) = m["from"], m["at"]
        if (fx - ax) ** 2 + (fz - az) ** 2 > (2.5 * step) ** 2 and not (far_ok and far_ok(m)):
            continue
        dirs.setdefault(tuple(m["from"]), set()).add(tuple(m["dir"]))
    xs, zs, posts = defaultdict(list), defaultdict(list), []
    for (x, y, z), ds in dirs.items():
        band = round(y / 48)
        for dx, dz in ds:
            if dx and dz:
                if (dx, 0) not in ds or (0, dz) not in ds:
                    posts.append((x + dx * half, z + dz * half, y))
                continue
            if dx:
                xs[(x + dx * half, band)].append((z, y))
            else:
                zs[(z + dz * half, band)].append((x, y))
    rails = []

    def runs(items):
        items.sort()
        run = [items[0]]
        for it in items[1:]:
            if it[0] - run[-1][0] <= step + 1:
                run.append(it)
            else:
                yield run
                run = [it]
        yield run

    for (line, _), items in list(xs.items()) + [(k, v) for k, v in zs.items()]:
        pass
    for table, yaw in ((xs, 0.0), (zs, 90.0)):
        for (line, _), items in table.items():
            for run in runs(items):
                lo, hi = run[0][0] - step, run[-1][0] + step
                ys = [y for _, y in run]
                mid = (lo + hi) / 2
                x, z = (line, mid) if yaw == 0.0 else (mid, line)
                rails.append({"x": round(x, 1), "z": round(z, 1), "y0": round(min(ys) - 10, 1),
                              "y1": round(max(ys) + RAIL_H, 1), "yaw": yaw, "len": round(hi - lo, 1), "t": RAIL_T})
    for x, z, y in posts:
        rails.append({"x": round(x, 1), "z": round(z, 1), "y0": round(y - 10, 1), "y1": round(y + RAIL_H, 1),
                      "yaw": 0.0, "len": 40, "t": 40})
    return rails


def rails_table(rails):
    rows = [
        "  {{ x = {x}, z = {z}, y0 = {y0}, y1 = {y1}, yaw = {yaw}, len = {len}, t = {t} }},".format(
            t=r.get("t", RAIL_T), **{k: v for k, v in r.items() if k != "t"})
        for r in rails
    ]
    return "local RAILS = {\n" + "\n".join(rows) + "\n}"


def drops_of(reach):
    """The ROM spawns to leave out (MAP-125): those the reach report finds inside a kill volume, on no floor, in a
    trap, or on an island that cannot reach the main component; by piece, in the map's coordinates."""
    out = defaultdict(list)
    for sp in reach["result"].get("spawns_outside", []):
        piece = sp.get("piece")
        name = sp.get("name") or ""
        if not piece or not name.startswith(piece + "~"):
            continue  # the map's own spawns are the generator's to place
        bad = sp["class"] in ("deadly", "trap", "no_node") or (sp["class"] == "island" and not sp.get("reaches_main"))
        if bad:
            out[piece].append([round(v, 1) for v in sp["pos"]])
    return dict(out)


def drops_table(drops):
    rows = []
    for pid in sorted(drops, key=lambda s: int(s.split("_")[1])):
        pts = ", ".join("{{ {}, {}, {} }}".format(*p) for p in drops[pid])
        rows.append(f"  {pid} = {{ {pts} }},")
    return "local DROPS = {\n" + "\n".join(rows) + "\n}"


def lua_table(links):
    rows = []
    for l in links:
        rows.append(
            "  {{ a = \"{a}\", b = \"{b}\", along_x = {ax}, c = {c}, a_in = {a_in}, ha = {ha}, b_in = {b_in}, hb = {hb} }},"
            .format(ax="true" if l["along_x"] else "false", **l)
        )
    return "local LINKS = {\n" + "\n".join(rows) + "\n}"


def main():
    gen_path = sys.argv[sys.argv.index("--gen") + 1] if "--gen" in sys.argv else "source/gen.lua"
    gen = json.load(open(sys.argv[1]))
    nodes_dir = sys.argv[2]
    regions = regions_of(gen)
    nodes_by_piece, hazards_by_piece = {}, {}
    for pid in regions:
        try:
            _, nodes = load_nodes(f"{nodes_dir}/{pid}.json")
            nodes_by_piece[pid] = nodes
            hazards_by_piece[pid] = hazards(f"{nodes_dir}/{pid}.json")
        except FileNotFoundError:
            print(f"no nodes for {pid}")
    avoid = json.load(open(sys.argv[sys.argv.index("--avoid") + 1])) if "--avoid" in sys.argv else ()
    hazards_all = []
    if "--hazards" in sys.argv:
        # the check rounds' nodes dumps: where walks died after the corridors were cut (a pit the cut opened the
        # way to is no place for a mouth either)
        import glob
        for d in sys.argv[sys.argv.index("--hazards") + 1].split(","):
            for f in glob.glob(os.path.join(d, "*.json")):
                if f.endswith(".out.json"):
                    continue
                hazards_all += hazards(f)
        print(f"{len(hazards_all)} deaths from the check rounds kept clear of")
    links = plan(regions, nodes_by_piece, avoid, hazards_by_piece, hazards_all)
    if "--max-len" in sys.argv:
        keep = set(sys.argv[sys.argv.index("--keep") + 1].split(",")) if "--keep" in sys.argv else set()
        links = prune_long(links, float(sys.argv[sys.argv.index("--max-len") + 1]), keep)
    per = defaultdict(int)
    for l in links:
        per[l["a"]] += 1
        per[l["b"]] += 1
        print(f"{l['a']:>9} -> {l['b']:<9} {'x' if l['along_x'] else 'z'} c {l['c']:8.0f}  "
              f"in {l['a_in']:8.0f}/{l['b_in']:8.0f}  floor {l['ha']:6.0f}/{l['hb']:6.0f}  cost {l['cost']}")
    groups = joined(list(regions), links)
    print(f"{len(links)} corridors; {len(groups)} group(s): " + " | ".join(",".join(sorted(g)) for g in groups))
    for pid in sorted(regions, key=lambda s: int(s.split('_')[1])):
        print(f"  {pid}: {per[pid]} corridors")
    rails = []
    if "--rails" in sys.argv:
        # the rails accumulate: each reach run adds the moves into whatever traps are left (source/rails.json)
        store = os.path.join(os.path.dirname(gen_path), "rails.json")
        try:
            rails = json.load(open(store))
        except FileNotFoundError:
            rails = []
        seen = {(r["x"], r["z"], r["yaw"], r.get("len"), r.get("t")) for r in rails}
        reach = json.load(open(sys.argv[sys.argv.index("--rails") + 1]))
        new = rails_of(reach)
        if "--kill-rails" in sys.argv:
            dump = json.load(open(sys.argv[sys.argv.index("--kill-rails") + 1]))
            new += kill_rails(dump, dump["step"])
        added = [r for r in new if (r["x"], r["z"], r["yaw"], r.get("len"), r.get("t")) not in seen]
        rails += added
        json.dump(rails, open(store, "w"), indent=0)
        print(f"{len(added)} rails added, {len(rails)} in all")
    if "--write" in sys.argv:
        path = gen_path
        src = open(path).read()
        new = re.sub(r"-- LINKS BEGIN\n.*?\n-- LINKS END", "-- LINKS BEGIN\n" + lua_table(links) + "\n-- LINKS END",
                     src, count=1, flags=re.S)
        if "--rails" in sys.argv:
            new = re.sub(r"-- RAILS BEGIN\n.*?\n-- RAILS END", "-- RAILS BEGIN\n" + rails_table(rails) + "\n-- RAILS END",
                         new, count=1, flags=re.S)
        if "--drop-spawns" in sys.argv:
            # the drops accumulate too (source/drops.json): a spawn left out once stays out
            store = os.path.join(os.path.dirname(gen_path), "drops.json")
            try:
                drops = json.load(open(store))
            except FileNotFoundError:
                drops = {}
            got = drops_of(json.load(open(sys.argv[sys.argv.index("--drop-spawns") + 1])))
            added = 0
            for pid, pts in got.items():
                have = drops.setdefault(pid, [])
                for pt in pts:
                    if pt not in have:
                        have.append(pt)
                        added += 1
            json.dump(drops, open(store, "w"), indent=0)
            print(f"{added} spawns left out, {sum(len(v) for v in drops.values())} in all")
            new = re.sub(r"-- DROPS BEGIN\n.*?\n-- DROPS END", "-- DROPS BEGIN\n" + drops_table(drops) + "\n-- DROPS END",
                         new, count=1, flags=re.S)
        open(path, "w").write(new)
        print("written into", path)


if __name__ == "__main__":
    main()
