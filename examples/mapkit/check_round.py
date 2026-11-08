#!/usr/bin/env python3
"""One check round of a ROM atlas (rom_atlas.lua): `dc-mapkit walk --reach` on every piece's region at three grids (16, 24, and
48 with the box shifted half a cell), each seeded at the piece's own spawns, doors open. One grid alone misses
places narrower than its cell or that fall between its samples; three rarely agree on a miss.

Writes <out>/<piece>_<grid>.out.json and <out>/<piece>_<grid>.json (the --nodes file: nodes, every deadly move and
every move into a trap) and prints a line per run. rails_from.py turns the folder into rails and left-out spawns.

Run: python3 check_round.py <dc-mapkit> <project dir> <run output.json> <out dir> [piece ids...]
"""
import json
import os
import re
import subprocess
import sys


def main():
    mapkit, project, gen_out, out = sys.argv[1:5]
    only = set(sys.argv[5:])
    os.makedirs(out, exist_ok=True)
    regions = {}
    for line in json.load(open(gen_out))["result"]["prints"]:
        m = re.match(r"region (\S+) play \S+ \S+ \S+ \S+ edge (\S+) (\S+) (\S+) (\S+)", line)
        if m:
            regions[m.group(1)] = [float(v) for v in m.groups()[1:]]
    total = {"traps": 0, "kill": 0, "outside": 0}
    for pid in sorted(regions, key=lambda s: int(s.split("_")[1])):
        if only and pid not in only:
            continue
        x0, z0, x1, z1 = regions[pid]
        for grid, step, box in (
            ("s16", 16, f"{x0},{z0},{x1},{z1}"),
            ("s24", 24, f"{x0},{z0},{x1},{z1}"),
            ("s48o", 48, f"{x0 + 24},{z0 + 24},{x1 + 24},{z1 + 24}"),
        ):
            base = os.path.join(out, f"{pid}_{grid}")
            cmd = [mapkit, "walk", project, "--reach", f"--box={box}", "--seed-piece", pid, "--step", str(step),
                   "--open-doors", "--nodes", base + ".json", "-o", base + ".png", "--json"]
            with open(base + ".out.json", "w") as f:
                subprocess.run(cmd, stdout=f, stderr=subprocess.DEVNULL, check=False)
            try:
                r = json.load(open(base + ".out.json"))["result"]
                d = json.load(open(base + ".json"))
            except (ValueError, FileNotFoundError, KeyError):
                print(f"{pid} {grid}: no result")
                continue
            kill = sum(1 for m in d.get("deadly", []) if m["cause"] == "kill_volume" and m["kind"] in ("walk", "run"))
            outside = sum(1 for s in r.get("spawns_outside", []) if (s.get("name") or "").startswith(pid + "~"))
            total["traps"] += r["traps"]["nodes"]
            total["kill"] += kill
            total["outside"] += outside
            print(f"{pid} {grid}: main {r['main']['nodes']} spawns {r['main']['spawns']} traps {r['traps']['nodes']} "
                  f"walk-into-kill {kill} spawns-outside {outside}", flush=True)
    print(f"total: trap nodes {total['traps']}, walk-into-kill moves {total['kill']}, spawns outside {total['outside']}")


if __name__ == "__main__":
    main()
