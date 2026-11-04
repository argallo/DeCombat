#!/usr/bin/env python
"""perf_stress.py: runs the G1 stress scenarios of docs/RENDER_PERF_GATES.md with the dc-client frame profiler.

Measurement harness of the DeCombat project (perf-g1). Python 3, standard library only.

    python tools/perf_stress.py --exe target/release/dc-client.exe --out DIR [--rom DIR] [--map NAME]
        [--scenarios all|bsp,characters,fx,hud,cold,match] [--frames N] [--size WxH] [--search-maps]
        [--label smoke|baseline] [--max-load PCT] [--features TEXT] [--adapter auto|dx12|vulkan]

Per scenario it runs `dc-client --headless-frames N --stress NAME --profile DIR/NAME_TARGET.json` (per-frame CSV
next to it through DC_PROF_FRAMES) and writes DIR/manifest.json: command line, exit code, wall time, the machine's
CPU load in the second before the run and during the run (the binary measures the latter itself), the build commit,
the target and the features. A run is classed `baseline` only with --label baseline AND both loads below --max-load
(default 10 %); everything else is `smoke` (the machine is shared: numbers under load are never a baseline).

--search-maps runs the visibility search (`--stress-search`: every spawn x 16 yaws x 3 pitches, ranked by world
triangles) on every ROM map, writes DIR/views.json and runs `bsp` on the map with the most triangles at its pinned
view. Without it `bsp` searches --map only. --map defaults to the first map of `dc-client --list`.

`cold` runs with no warm-up left out (DC_PROF_WARMUP=0): its first frames are the point. Everything the ROM yields
(PNGs are off in profile mode) stays outside the tree; give a DIR outside the checkout.
"""
import argparse
import ctypes
import json
import math
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
TREE = os.path.dirname(HERE)
SCENARIOS = ['bsp', 'characters', 'fx', 'hud', 'cold', 'match']
FRAMES = {'bsp': 600, 'characters': 600, 'fx': 600, 'hud': 600, 'cold': 300, 'match': 900}


def cpu_times():
    """(idle, busy) of the whole machine in 100 ns units, Windows only (None elsewhere)."""
    if os.name != 'nt':
        return None
    ft = [ctypes.c_ulonglong(0) for _ in range(3)]
    ok = ctypes.windll.kernel32.GetSystemTimes(*(ctypes.byref(f) for f in ft))
    if not ok:
        return None
    idle, kernel, user = (f.value for f in ft)
    return idle, kernel + user - idle


def machine_load(seconds=1.0):
    """The machine's CPU load (percent of all cores) over `seconds`; None off Windows."""
    a = cpu_times()
    time.sleep(seconds)
    b = cpu_times()
    if a is None or b is None:
        return None
    idle, busy = b[0] - a[0], b[1] - a[1]
    return round(100.0 * busy / max(1, idle + busy), 1)


def pe_target(exe):
    """i686 or x86_64 from the binary's PE header (machine field)."""
    try:
        with open(exe, 'rb') as f:
            head = f.read(4096)
        pe = int.from_bytes(head[0x3c:0x40], 'little')
        machine = int.from_bytes(head[pe + 4:pe + 6], 'little')
        return {0x14c: 'i686', 0x8664: 'x86_64'}.get(machine, 'machine%04x' % machine)
    except (OSError, ValueError):
        return 'unknown'


def git_commit():
    try:
        out = subprocess.run(['git', '-C', TREE, 'rev-parse', 'HEAD'], capture_output=True, text=True, timeout=30)
        dirty = subprocess.run(['git', '-C', TREE, 'status', '--porcelain', '--untracked-files=no'],
                               capture_output=True, text=True, timeout=30).stdout.strip()
        return out.stdout.strip() + ('+dirty' if dirty else '')
    except (OSError, subprocess.SubprocessError):
        return ''


def maps(exe, rom):
    out = subprocess.run([exe, '--rom', rom, '--list'], capture_output=True, text=True, timeout=120)
    return [ln.strip() for ln in out.stdout.splitlines() if ln.strip()]


def search(exe, rom, m, size, adapter):
    cmd = [exe, '--rom', rom, '--map', m, '--stress-search', '--size', size, '--adapter', adapter]
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
    for ln in out.stdout.splitlines():
        if ln.startswith('dc-client view search: '):
            return json.loads(ln[len('dc-client view search: '):])
    raise RuntimeError('no search result for %s: %s' % (m, out.stderr[-400:]))


def run_one(a, scenario, m, view, out_dir, target):
    base = '%s_%s' % (scenario, target)
    js, csv = os.path.join(out_dir, base + '.json'), os.path.join(out_dir, base + '.csv')
    frames = a.frames or FRAMES[scenario]
    cmd = [a.exe, '--rom', a.rom, '--map', m, '--headless-frames', str(frames), '--size', a.size,
           '--stress', scenario, '--profile', js, '--out', os.path.join(out_dir, 'png'), '--adapter', a.adapter]
    if view is not None:
        cmd += ['--stress-view', '%d,%.4f,%.4f' % view]
    env = dict(os.environ)
    env['DC_PROF_FRAMES'] = csv
    env['DC_PROF_WARMUP'] = '0' if scenario == 'cold' else str(a.warmup)
    env['DC_PROF_SCENARIO'] = scenario
    pre = machine_load(1.0)
    t = time.time()
    p = subprocess.run(cmd, capture_output=True, text=True, env=env, timeout=a.timeout)
    wall = time.time() - t
    with open(os.path.join(out_dir, base + '.out'), 'w', encoding='utf-8') as f:
        f.write(p.stdout)
        f.write(p.stderr)
    run_load = None
    try:
        with open(js, encoding='utf-8') as f:
            run_load = (json.load(f).get('cpu') or {}).get('machine_load_pct')
    except (OSError, ValueError):
        pass
    quiet = pre is not None and run_load is not None and pre < a.max_load and run_load < a.max_load
    entry = {'scenario': scenario, 'map': m, 'view': view, 'json': js, 'csv': csv, 'cmd': cmd, 'exit': p.returncode,
             'wall_s': round(wall, 2), 'cpu_load_before_pct': pre, 'cpu_load_during_pct': run_load,
             'class': 'baseline' if (a.label == 'baseline' and quiet) else 'smoke'}
    print('%-11s exit %d  %6.1f s  load before %s %%, during %s %%  -> %s (%s)' % (
        scenario, p.returncode, wall, pre, run_load, js, entry['class']), flush=True)
    return entry


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--exe', required=True, help='dc-client binary (release build)')
    ap.add_argument('--out', required=True, help='output folder (outside the tree)')
    ap.add_argument('--rom', default=os.environ.get('CA_ROM'), help='ROM folder (default CA_ROM)')
    ap.add_argument('--map', default=None, help='map of the scenarios (default: the first of --list)')
    ap.add_argument('--scenarios', default='all')
    ap.add_argument('--frames', type=int, default=None, help='frames per scenario (default per scenario)')
    ap.add_argument('--warmup', type=int, default=30, help='warm-up frames left out (cold: always 0)')
    ap.add_argument('--size', default='2560x1440')
    ap.add_argument('--adapter', default='auto', choices=('auto', 'dx12', 'vulkan', 'fallback'))
    ap.add_argument('--search-maps', action='store_true', help='visibility search on every map for bsp')
    ap.add_argument('--label', default='smoke', choices=('smoke', 'baseline'))
    ap.add_argument('--max-load', type=float, default=10.0, help='CPU load (%%) above which a run is smoke')
    ap.add_argument('--features', default='', help='cargo features of the build (recorded only)')
    ap.add_argument('--timeout', type=float, default=1800.0)
    a = ap.parse_args(argv)
    if not a.rom:
        ap.error('--rom or CA_ROM is required')
    a.exe = os.path.abspath(a.exe)
    a.out = os.path.abspath(a.out)
    os.makedirs(a.out, exist_ok=True)
    names = SCENARIOS if a.scenarios == 'all' else [s.strip() for s in a.scenarios.split(',') if s.strip()]
    bad = [s for s in names if s not in SCENARIOS]
    if bad:
        ap.error('unknown scenario(s) %s' % ', '.join(bad))
    all_maps = maps(a.exe, a.rom)
    if not all_maps:
        sys.exit('no ROM maps listed by %s' % a.exe)
    m = a.map or all_maps[0]
    manifest = {'schema': 'dc-perf-run/1', 'commit': git_commit(), 'exe': a.exe, 'features': a.features,
                'size': a.size, 'adapter': a.adapter, 'label': a.label, 'max_load_pct': a.max_load,
                'started': time.strftime('%Y-%m-%d %H:%M:%S'), 'runs': []}
    bsp_map, bsp_view = m, None
    if 'bsp' in names and a.search_maps:
        views = []
        for mm in all_maps:
            r = search(a.exe, a.rom, mm, a.size, a.adapter)
            views.append(r)
            top = r['top'][0] if r['top'] else {}
            print('search %-16s %5d views, worst %s world triangles, %s draws' % (
                mm, r['candidates'], top.get('world_tris'), top.get('draws')), flush=True)
        with open(os.path.join(a.out, 'views.json'), 'w', encoding='utf-8') as f:
            json.dump(views, f, indent=1)
        worst = max((v for v in views if v['top']), key=lambda v: (v['top'][0]['world_tris'], v['top'][0]['draws']))
        bsp_map = worst['map']
        t = worst['top'][0]
        bsp_view = (t['spawn'], math.degrees(t['yaw']), math.degrees(t['pitch']))
        manifest['bsp_pinned'] = {'map': bsp_map, 'spawn': t['spawn'], 'yaw_deg': bsp_view[1],
                                  'pitch_deg': bsp_view[2], 'world_tris': t['world_tris'], 'draws': t['draws']}
    target = pe_target(a.exe)
    for s in names:
        mm, view = (bsp_map, bsp_view) if s == 'bsp' else (m, None)
        manifest['runs'].append(run_one(a, s, mm, view, a.out, target))
    manifest['target'] = target
    path = os.path.join(a.out, 'manifest.json')
    with open(path, 'w', encoding='utf-8') as f:
        json.dump(manifest, f, indent=1)
    print('manifest', path)
    return 0 if all(r['exit'] == 0 for r in manifest['runs']) else 1


if __name__ == '__main__':
    sys.exit(main())
