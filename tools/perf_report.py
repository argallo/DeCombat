#!/usr/bin/env python
"""perf_report.py: the RENDER_PERF_GATES.md budget ledger filled from dc-client profile reports.

Measurement tooling of the DeCombat project (perf-g1). Python 3, standard library only.

    python tools/perf_report.py DIR_OR_JSON [...] [--caps tools/perf_caps.json] [--markdown OUT.md]
        [--require-baseline]

Reads the JSON summaries of `dc-client --profile` (schema dc-prof/2; a folder is read with the manifest.json that
tools/perf_stress.py writes, for the CPU load before the run, the build commit and the smoke / baseline class) and
prints, as Markdown:
  1. one line per run: scenario, class, target, adapter, size, frames, CPU load before / during, frame p50 / p95 /
     p99 / max, GPU span p95 / p99, hitches;
  2. the ledger: for every row of the gates document its counters with unit, the value per scenario, the worst
     (high-water) value and the scenario it came from, the cap from the caps file and PASS / FAIL (a cap left null
     is not pinned: the row is reported only);
  3. GPU per-pass p95 / p99.
Exit 1 when a pinned cap fails, or with --require-baseline when any run is not a baseline run.
"""
import argparse
import glob
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CAPS_DEFAULT = os.path.join(HERE, 'perf_caps.json')
MB = 1024.0 * 1024.0

# (ledger row, bounded quantity as the gates document names it, [(counter path in the report's "ledger", unit,
# scale divisor, what it measures)])
LEDGER = [
    ('System RAM', 'Archive index, decoded-texture LRU, decompression scratch pool, streaming request queue', [
        ('system_ram.rss_peak_bytes', 'MB', MB, 'process working set, peak'),
        ('system_ram.decoded_image_pool_high_bytes', 'MB', MB, 'decoded-image caches (model loader + effect sprites)'),
        ('system_ram.loader_jobs_high', 'requests', 1, 'loader job queue, high-water'),
    ]),
    ('GPU static', 'BSP VB/IB, lightmap array, resident textures per level', [
        ('gpu_static.gpu_allocated_at_level_bytes', 'MB', MB, 'GPU memory held at level ready'),
        ('gpu_static.created_during_load_bytes', 'MB', MB, 'buffers + textures created by the load'),
        ('gpu_static.gpu_allocated_peak_bytes', 'MB', MB, 'GPU memory, peak of the run (sampled)'),
    ]),
    ('GPU dynamic/frame', 'Packed frame records, bone palettes, particle instances, HUD quads', [
        ('gpu_dynamic_frame.buffer_write_bytes_max', 'MB/frame', MB, 'write_buffer bytes, worst frame'),
        ('gpu_dynamic_frame.buffer_write_bytes_p95', 'MB/frame', MB, 'write_buffer bytes, p95'),
        ('gpu_dynamic_frame.bones_per_frame_max', 'bones', 1, 'bone matrices written, worst frame'),
        ('gpu_dynamic_frame.fx_quads_max', 'quads', 1, 'effect quads (particles, sprites, tracers)'),
        ('gpu_dynamic_frame.hud_quads_max', 'quads', 1, 'HUD / overlay quads'),
    ]),
    ('Staging in-flight', 'Recycled chunks outstanding, bytes awaiting fence', [
        ('staging_in_flight.bytes_max', 'MB', MB, 'upload bytes of the frames in flight (window inflight+1)'),
        ('staging_in_flight.ops_max', 'copies', 1, 'copy operations of the frames in flight'),
    ]),
    ('Upload traffic', 'Bytes copied/frame, copy ops/frame', [
        ('upload_traffic.bytes_p95', 'MB/frame', MB, 'upload bytes, p95'),
        ('upload_traffic.bytes_max', 'MB/frame', MB, 'upload bytes, worst frame'),
        ('upload_traffic.ops_p95', 'copies/frame', 1, 'copy operations, p95'),
        ('upload_traffic.ops_max', 'copies/frame', 1, 'copy operations, worst frame'),
    ]),
    ('CPU frame', 'Parse, decode jobs, pack, encode/submit', [
        ('cpu_frame.frame_ms_p95', 'ms', 1, 'frame time p95'),
        ('cpu_frame.frame_ms_p99', 'ms', 1, 'frame time p99'),
        ('cpu_frame.encode_submit_ms_p95', 'ms', 1, 'encode + submit p95'),
        ('cpu_frame.encode_submit_ms_p99', 'ms', 1, 'encode + submit p99'),
        ('cpu_frame.hot_loop_allocs_max', 'allocs/frame', 1, 'frame-thread heap allocations, worst frame'),
        ('cpu_frame.loader_jobs_high', 'jobs', 1, 'decode / load job queue, high-water'),
    ]),
    ('GPU frame', 'Per-pass timestamp time, total', [
        ('gpu_frame.span_ms_p95', 'ms', 1, 'first pass begin to last pass end, p95'),
        ('gpu_frame.span_ms_p99', 'ms', 1, 'first pass begin to last pass end, p99'),
    ]),
    ('Pipelines', 'Pipelines per level, load-time compile, runtime creates', [
        ('pipelines.created_at_load', 'pipelines', 1, 'pipelines created by the level load'),
        ('pipelines.load_ms', 'ms', 1, 'level load time (device, pipelines, world, tables)'),
        ('pipelines.runtime_creates', 'pipelines', 1, 'pipelines created after the level load'),
    ]),
    ('Draw/bind state', 'Draws, bind-group switches, dynamic offsets / frame', [
        ('draw_bind.draws_max', 'draws', 1, 'draws (world pass + client passes), worst frame'),
        ('draw_bind.bind_sets_max', 'binds', 1, 'bind-group sets, worst frame'),
        ('draw_bind.dynamic_offsets_max', 'offsets', 1, 'dynamic offsets (no layout declares any today)'),
    ]),
    ('Streaming', 'Time-to-resident, hitch attributable to streaming', [
        ('streaming.time_to_resident_frames_p95', 'frames', 1, 'model request to resident, p95'),
        ('streaming.time_to_resident_ms_p95', 'ms', 1, 'model request to resident, p95'),
        ('streaming.worst_frame_with_load_ms', 'ms', 1, 'worst frame that finished a model / texture load'),
    ]),
]


def get(d, path):
    for k in path.split('.'):
        if not isinstance(d, dict):
            return None
        d = d.get(k)
    return d


def load_runs(paths):
    runs = []
    for p in paths:
        manifest = {}
        files = []
        if os.path.isdir(p):
            mp = os.path.join(p, 'manifest.json')
            if os.path.exists(mp):
                with open(mp, encoding='utf-8') as f:
                    manifest = json.load(f)
                files = [r['json'] for r in manifest.get('runs', []) if os.path.exists(r['json'])]
            else:
                files = sorted(glob.glob(os.path.join(p, '*.json')))
        else:
            files = [p]
        by_json = {os.path.normcase(os.path.abspath(r['json'])): r for r in manifest.get('runs', [])}
        for f in files:
            try:
                with open(f, encoding='utf-8') as fh:
                    v = json.load(fh)
            except (OSError, ValueError):
                continue
            if v.get('schema') != 'dc-prof/2':
                continue
            meta = by_json.get(os.path.normcase(os.path.abspath(f)), {})
            runs.append({'path': f, 'report': v, 'meta': meta, 'manifest': manifest})
    return runs


def fmt(x, unit_scale=1):
    if x is None:
        return '-'
    try:
        x = float(x) / unit_scale
    except (TypeError, ValueError):
        return str(x)
    if x == int(x) and abs(x) < 1e9:
        return '%d' % x
    return '%.3f' % x if abs(x) < 100 else '%.1f' % x


def name(r):
    v = r['report']
    lane = '-window' if str(v.get('note', '')).startswith('window') else ''
    return '%s%s/%s' % (v.get('scenario') or 'run', lane, {'x86': 'i686'}.get(v.get('target'), v.get('target')))


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('paths', nargs='+')
    ap.add_argument('--caps', default=CAPS_DEFAULT)
    ap.add_argument('--markdown', default=None, help='also write the report to this file')
    ap.add_argument('--require-baseline', action='store_true')
    a = ap.parse_args(argv)
    runs = load_runs(a.paths)
    if not runs:
        sys.exit('no dc-prof/2 reports found in %s' % ', '.join(a.paths))
    caps = {}
    if os.path.exists(a.caps):
        with open(a.caps, encoding='utf-8') as f:
            caps = json.load(f).get('caps', {})
    out = []
    w = out.append
    w('# Render performance ledger (perf_report.py)')
    w('')
    commits = sorted({r['manifest'].get('commit') or r['report'].get('build_commit') or '?' for r in runs})
    w('Build commit(s): %s. Caps: %s.' % (', '.join(commits), a.caps if os.path.exists(a.caps) else 'none'))
    w('')
    w('| run | class | adapter | size | frames | CPU load before / during % | frame p50 / p95 / p99 / max ms '
      '| GPU span p95 / p99 ms | hitches >5 ms |')
    w('|---|---|---|---|---|---|---|---|---|')
    not_baseline = 0
    for r in runs:
        v, m = r['report'], r['meta']
        fm, g = v.get('frame_ms', {}), get(v, 'gpu.span_ms') or {}
        cls = m.get('class', 'smoke')
        not_baseline += cls != 'baseline'
        w('| %s | %s | %s | %s | %s | %s / %s | %s / %s / %s / %s | %s / %s | %s |' % (
            name(r), cls.upper(), v.get('adapter', '?'), 'x'.join(str(s) for s in v.get('size', [])),
            v.get('frames'), m.get('cpu_load_before_pct', '-'), get(v, 'cpu.machine_load_pct'),
            fmt(fm.get('p50')), fmt(fm.get('p95')), fmt(fm.get('p99')), fmt(fm.get('max')),
            fmt(g.get('p95')), fmt(g.get('p99')), get(v, 'all_frames.over_hitch')))
    w('')
    w('## Ledger (high-water per counter; cap blank = not pinned)')
    w('')
    heads = [name(r) for r in runs]
    w('| row | counter | unit | %s | worst (from) | cap | verdict |' % ' | '.join(heads))
    w('|---|---|---|%s---|---|---|' % ('---|' * len(heads)))
    failed = 0
    for row, quantity, counters in LEDGER:
        for k, (path, unit, scale, what) in enumerate(counters):
            vals = [get(r['report'].get('ledger', {}), path) for r in runs]
            nums = [(float(x) / scale, i) for i, x in enumerate(vals) if isinstance(x, (int, float))]
            worst = max(nums) if nums else None
            cap = (caps.get(path) or {}).get('cap') if isinstance(caps.get(path), dict) else caps.get(path)
            if cap is None or worst is None:
                verdict = 'report only' if cap is None else 'no data'
            else:
                ok = worst[0] <= float(cap)
                verdict = 'PASS' if ok else 'FAIL'
                failed += not ok
            w('| %s | `%s` (%s) | %s | %s | %s | %s | %s |' % (
                row if k == 0 else '', path, what, unit, ' | '.join(fmt(x, scale) for x in vals),
                '-' if worst is None else '%s (%s)' % (fmt(worst[0]), heads[worst[1]]),
                '' if cap is None else fmt(cap), verdict))
    w('')
    w('## GPU passes (p95 / p99 ms)')
    w('')
    passes = sorted({p for r in runs for p in (get(r['report'], 'gpu.passes') or {})})
    w('| run | %s |' % ' | '.join(passes))
    w('|---|%s' % ('---|' * len(passes)))
    for r in runs:
        ps = get(r['report'], 'gpu.passes') or {}
        w('| %s | %s |' % (name(r), ' | '.join(
            '%s / %s' % (fmt(ps[p].get('p95')), fmt(ps[p].get('p99'))) if p in ps else '-' for p in passes)))
    w('')
    w('## Load phase and runtime creates')
    w('')
    w('| run | load ms | io ms | parse ms | decode ms | pack ms | upload MB | pipelines | runtime creates |')
    w('|---|---|---|---|---|---|---|---|---|')
    for r in runs:
        ld = r['report'].get('load') or {}
        rc = get(r['report'], 'violations.runtime_creates') or []
        w('| %s | %s | %s | %s | %s | %s | %s | %s | %s |' % (
            name(r), fmt(ld.get('ms')), fmt(ld.get('io_ms')), fmt(ld.get('parse_ms')), fmt(ld.get('decode_ms')),
            fmt(ld.get('pack_ms')), fmt(ld.get('upload_bytes'), MB), ld.get('pipeline_creates'),
            '%d: %s' % (len(rc), ', '.join(sorted(set(rc)))) if rc else '0'))
    if not_baseline:
        w('')
        w('%d of %d runs are SMOKE (not a baseline: label smoke, or the machine was loaded).' % (
            not_baseline, len(runs)))
    text = '\n'.join(out)
    print(text)
    if a.markdown:
        with open(a.markdown, 'w', encoding='utf-8') as f:
            f.write(text + '\n')
    if failed:
        print('\n%d pinned cap(s) FAILED' % failed, file=sys.stderr)
        return 1
    if a.require_baseline and not_baseline:
        print('\n--require-baseline: %d run(s) are not baseline runs' % not_baseline, file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
