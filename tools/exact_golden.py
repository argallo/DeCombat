#!/usr/bin/env python3
"""Exact-lane golden hashes of the render determinism contract (docs/RENDER_DETERMINISM_CONTRACT.md, gate G0).

Runs the exact-lane scenes once in a fresh process (the child entry of crates/dc-client/tests/exact_lane.rs), compares
their frame hashes with this machine's lane section of crates/dc-client/tests/golden/exact_lane.json and prints every
difference. The golden file is written ONLY with --update, after the diff has been printed: a golden change is an
explicit, reviewed commit, never a side effect of a test run.

Usage (from the worktree root):
    python tools/exact_golden.py               # run, compare, print the diff; exit 1 when anything differs
    python tools/exact_golden.py --update      # run, print the diff, then write this lane's section
    python tools/exact_golden.py --from RUN.json [--update]   # use the output of an earlier child run

DC_TARGET (i686 default, x86_64) picks the build through cargo_env.py; CA_ROM / the repo's ROM folder decides
whether the ROM scenes run. The golden file holds hashes and manifests only: no ROM bytes, no ROM names.
"""
# Provenance: ORIGINAL
import argparse
import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
GOLDEN = os.path.join(ROOT, 'crates', 'dc-client', 'tests', 'golden', 'exact_lane.json')
CONTRACT = 'dc-render-determinism/1'


def run_child(out_path):
    env = dict(os.environ)
    env['DC_EXACT_CHILD_OUT'] = out_path
    for k in ('DC_DETERMINISM_MUTATE', 'DC_ASYNC_LOADS', 'DC_PROF', 'DC_EXACT_PNG'):
        env.pop(k, None)
    cmd = [sys.executable, os.path.join(ROOT, 'cargo_env.py'), 'cargo', 'test', '-p', 'dc-client',
           '--test', 'exact_lane', 'exact_child_entry', '--', '--exact', '--test-threads=1']
    print('running:', ' '.join(cmd[1:]), flush=True)
    r = subprocess.run(cmd, cwd=ROOT, env=env)
    if r.returncode != 0:
        sys.exit('exact_golden: the exact-lane run failed (%d)' % r.returncode)


def section_of(run):
    """The golden form of a run: per scene the digests, the frame hashes and the manifest without per-file rows."""
    scenes = {}
    lane = None
    recorded = None
    for name, s in sorted(run.items()):
        m = s['manifest']
        lane = lane or m.get('lane_key')
        if recorded is None:
            recorded = {k: m.get(k) for k in ('software', 'adapter', 'os')}
        inputs = dict(m['inputs'])
        rom = dict(inputs['rom'])
        rom.pop('entries', None)
        inputs['rom'] = rom
        scenes[name] = {
            'inputs_digest': m['inputs_digest'],
            'rom_digest': m['inputs']['rom']['digest'],
            'code_digest': m['inputs']['code_digest'],
            'frames': s['frames'],
            'manifest': {
                'contract': m['contract'],
                'texel_contract': m['texel_contract'],
                'texel_layout': m['texel_layout'],
                'inputs': inputs,
                'render_settings': m['render_settings'],
            },
        }
    return lane, {'recorded_with': recorded, 'scenes': scenes}


def diff(old, new):
    """Lines describing what changed between two lane sections (empty: identical hashes)."""
    out = []
    old_s = (old or {}).get('scenes', {})
    new_s = new['scenes']
    for name in sorted(set(old_s) | set(new_s)):
        if name not in old_s:
            out.append('+ scene %s (%d frames)' % (name, len(new_s[name]['frames'])))
            continue
        if name not in new_s:
            out.append('- scene %s (not run now: no ROM or skipped)' % name)
            continue
        a, b = old_s[name], new_s[name]
        notes = []
        for key in ('rom_digest', 'code_digest'):
            if a.get(key) != b.get(key):
                notes.append('%s %s -> %s' % (key, str(a.get(key))[:12], str(b.get(key))[:12]))
        fa, fb = a['frames'], b['frames']
        changed = [(x, y) for x, y in zip(fa, fb) if x != y]
        if len(fa) != len(fb):
            notes.append('frames %d -> %d' % (len(fa), len(fb)))
        if changed or notes:
            out.append('~ scene %s: %d of %d frame hashes differ%s' % (
                name, len(changed), min(len(fa), len(fb)), ('; ' + '; '.join(notes)) if notes else ''))
            for x, y in changed[:12]:
                out.append('    frame %s: %s -> %s' % (x.get('frame'), str(x.get('sha256'))[:16],
                                                      str(y.get('sha256'))[:16]))
            if len(changed) > 12:
                out.append('    ... %d more' % (len(changed) - 12))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--update', action='store_true', help='write this lane\'s section after printing the diff')
    ap.add_argument('--from', dest='src', help='a child run output (JSON) instead of a new run')
    a = ap.parse_args()
    if a.src:
        run_path = a.src
    else:
        fd, run_path = tempfile.mkstemp(prefix='dc_exact_', suffix='.json')
        os.close(fd)
        run_child(run_path)
    with open(run_path, encoding='utf-8') as f:
        run = json.load(f)
    lane, section = section_of(run)
    if not lane:
        sys.exit('exact_golden: the run has no lane key (no adapter?)')
    golden = {'contract': CONTRACT, 'lanes': {}}
    if os.path.exists(GOLDEN):
        with open(GOLDEN, encoding='utf-8') as f:
            golden = json.load(f)
    old = golden.get('lanes', {}).get(lane)
    print('lane: %s' % lane)
    lines = diff(old, section) if old else ['(no golden section for this lane yet)']
    for l in lines:
        print(l)
    if not lines:
        print('identical: %d scenes, %d frame hashes' % (
            len(section['scenes']), sum(len(s['frames']) for s in section['scenes'].values())))
    if a.update:
        # scenes of the old section that did not run now (no ROM here) are kept
        merged = dict(old or {})
        scenes = dict((old or {}).get('scenes', {}))
        scenes.update(section['scenes'])
        merged['scenes'] = dict(sorted(scenes.items()))
        merged['recorded_with'] = section['recorded_with']
        golden['contract'] = CONTRACT
        golden.setdefault('lanes', {})[lane] = merged
        golden['note'] = ('Exact-lane golden hashes (docs/RENDER_DETERMINISM_CONTRACT.md). Written only by '
                          'tools/exact_golden.py --update; hashes and manifests only, no ROM data or names.')
        os.makedirs(os.path.dirname(GOLDEN), exist_ok=True)
        with open(GOLDEN, 'w', encoding='utf-8', newline='\n') as f:
            json.dump(golden, f, indent=1, sort_keys=True)
            f.write('\n')
        print('written: %s (lane %s)' % (os.path.relpath(GOLDEN, ROOT), lane))
        return 0
    return 1 if (lines and old) or not old else 0


if __name__ == '__main__':
    sys.exit(main())
