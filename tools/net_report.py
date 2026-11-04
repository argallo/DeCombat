#!/usr/bin/env python
"""net_report.py: per-player network quality of one match session, or the change between two sessions.

Tooling of the DeCombat project (net-tooling, LG-185 / DSV-411 / CLI-1331). Python 3, standard
library only; safe under `python -I`.

    python -I tools/net_report.py SESSION_DIR
    python -I tools/net_report.py --dserver DSERVER.log [--dserver ...] CLIENT.net.jsonl [...]
    python -I tools/net_report.py --compare A B        (A, B: session folders or log files)
    python -I tools/net_report.py --selftest

A session folder is searched for `*.net.jsonl` (dc-client `--net-log`; `*.net.menu.jsonl` is skipped) and for the
dserver logs (`dserver*.log`, also below `dserver/`). When the folder has a capture `manifest.json`, only dserver
logs written during the session are read (a capture folder collects the logs of earlier days too). `--bots` also
reads dc-botclient `*.events.jsonl`.

Printed as Markdown, one row per player:
  1. Server view, from the dserver's `client N: link 10 s: ...` lines (CLI-1331; the window form of DSV-412 is read
     when present): windows, round trip median / p95 / max, input jitter median / p95, input rate, longest input
     gap, resends, resend % (resends of the reliable blocks the server sent, from the link summary: the loss proxy
     of the server -> client direction), flow drops.
  2. Client view, from the client's `link_stats` events: windows, its round-trip estimate (`l_ms`) median / p95 /
     max, resends and resend % (client -> server), received duplicates (`rel_discarded`), flow drops, datagrams
     per second in / out; and from its `remote_interp` events (CLI-1331, how this client saw the OTHER players):
     holds per minute, late per minute, largest arrival lateness per window (median of the windows; q95 in logs before 2026-10-31), playout delay mean, and the drawn-motion smoothness (CLI-1331 amended 2026-10-31): reversed steps per minute, jumps per minute, the spread of the drawn speed over its mean (median of the windows). A client
     log older than CLI-1331 has no `remote_interp`: those columns print `-`.
  3. With --compare: per player the A and B values of the main columns and B - A.

Players are named by their nick (dserver `hello name`, the client's `bot` field); nothing else of the logs
(addresses, accounts) is printed.
"""
import argparse
import datetime
import glob
import json
import os
import re
import shutil
import sys
import tempfile

LINE_RE = re.compile(r'^\s*(\d+) dc-dserver: client (\d+): (.*)$')
HELLO_RE = re.compile(r'^hello name "([^"]*)"')
LINK_RE = re.compile(
    r'^link (\d+) s: rtt (\d+) ms(?: \(window mean (\d+), min (\d+), max (\d+), (\d+) samples\))?, '
    r'input ([\d.]+)/s \(jitter ([\d.]+) ms, longest gap (\d+) ms\), resends (\d+), flow drops (\d+), '
    r'datagrams in (\d+) out (\d+)')
SUMMARY_RE = re.compile(
    r'^link summary (\d+) s: rtt (?:mean (\d+) ms \(min (\d+), max (\d+)\)|n/a), input (\d+) updates '
    r'\(([\d.]+)/s, jitter ([\d.]+) ms, longest gap (\d+) ms\), resends (\d+) of (\d+) reliable blocks, '
    r'flow drops (\d+), datagrams in (\d+) out (\d+)')


def quantile(xs, q):
    """Nearest-rank quantile of a list (None when empty)."""
    if not xs:
        return None
    s = sorted(xs)
    i = min(len(s) - 1, max(0, int(q * (len(s) - 1) + 0.5)))
    return s[i]


def median(xs):
    """Median (the mean of the two middle values of an even count; None when empty)."""
    if not xs:
        return None
    s = sorted(xs)
    n = len(s)
    return s[n // 2] if n % 2 else (s[n // 2 - 1] + s[n // 2]) / 2.0


def fmt(v, digits=0):
    if v is None:
        return '-'
    if isinstance(v, float):
        return ('%.' + str(digits) + 'f') % v
    return str(v)


# ------------------------------------------------------------------------------------------------ server logs

def read_dserver(path, players):
    """Adds the link windows of one dserver log to `players` (nick -> dict). Returns the number of link lines."""
    names = {}
    n = 0
    tag = os.path.basename(path)
    with open(path, encoding='utf-8', errors='replace') as f:
        for raw in f:
            m = LINE_RE.match(raw.rstrip('\r\n'))
            if not m:
                continue
            client, rest = m.group(2), m.group(3)
            h = HELLO_RE.match(rest)
            if h:
                names[client] = h.group(1)
                continue
            if not rest.startswith('link '):
                continue
            nick = names.get(client) or ('client %s (%s)' % (client, tag))
            p = players.setdefault(nick, new_server())
            lm = LINK_RE.match(rest)
            if lm:
                n += 1
                g = lm.groups()
                rtt = float(g[1])
                p['windows'] += 1
                p['rtt'].append(float(g[2]) if g[2] is not None else rtt)
                p['rtt_max'].append(float(g[4]) if g[4] is not None else rtt)
                p['input'].append(float(g[6]))
                p['jitter'].append(float(g[7]))
                p['gap'] = max(p['gap'], int(g[8]))
                p['resends'] += int(g[9])
                p['flow_drops'] += int(g[10])
                p['dg_in'] += int(g[11])
                p['dg_out'] += int(g[12])
                continue
            sm = SUMMARY_RE.match(rest)
            if sm:
                g = sm.groups()
                p['summary_resends'] += int(g[8])
                p['rel_blocks'] += int(g[9])
    return n


def new_server():
    return {'windows': 0, 'rtt': [], 'rtt_max': [], 'input': [], 'jitter': [], 'gap': 0, 'resends': 0,
            'flow_drops': 0, 'dg_in': 0, 'dg_out': 0, 'summary_resends': 0, 'rel_blocks': 0}


def server_row(p):
    rel = p['rel_blocks']
    return {
        'windows': p['windows'],
        'rtt_med': median(p['rtt']),
        'rtt_p95': quantile(p['rtt'], 0.95),
        'rtt_max': max(p['rtt_max']) if p['rtt_max'] else None,
        'jit_med': median(p['jitter']),
        'jit_p95': quantile(p['jitter'], 0.95),
        'input_med': median(p['input']),
        'gap_max': p['gap'] if p['windows'] else None,
        'resends': p['resends'],
        'resend_pct': (100.0 * p['summary_resends'] / rel) if rel else None,
        'flow_drops': p['flow_drops'],
    }


# ------------------------------------------------------------------------------------------------ client logs

COUNTERS = ('resends', 'rel_blocks_out', 'rel_discarded', 'flow_drops', 'in', 'out', 'ms')


def read_client(path, players):
    """Adds one dc-client (or dc-botclient) event log to `players` (viewer nick -> dict). Returns the viewer."""
    viewer = None
    st = new_client()
    last = {}
    with open(path, encoding='utf-8', errors='replace') as f:
        for raw in f:
            raw = raw.strip()
            if not raw.startswith('{'):
                continue
            ev_kind = '"link_stats"' in raw or '"remote_interp"' in raw
            if not ev_kind and viewer is not None:
                continue
            try:
                ev = json.loads(raw)
            except ValueError:
                continue
            if viewer is None and isinstance(ev.get('bot'), str):
                viewer = ev['bot']
            kind = ev.get('ev')
            if kind == 'link_stats':
                st['windows'] += 1
                if isinstance(ev.get('l_ms'), (int, float)):
                    st['l_ms'].append(float(ev['l_ms']))
                # cumulative counters of the link: a value below the previous one is a new link (counted from 0)
                for k in COUNTERS:
                    v = ev.get(k)
                    if not isinstance(v, (int, float)):
                        continue
                    prev = last.get(k)
                    st['sum'][k] = st['sum'].get(k, 0) + (v - prev if prev is not None and v >= prev else v)
                    last[k] = v
            elif kind == 'remote_interp':
                w = float(ev.get('window_ms') or 0)
                if w <= 0:
                    continue
                st['interp_windows'] += 1
                st['interp_ms'] += w
                st['holds'] += float(ev.get('holds_per_min') or 0) * w
                st['late'] += float(ev.get('late_per_min') or 0) * w
                jq = ev.get('late_max_ms', ev.get('jitter_q95_ms'))  # late_max_ms since 2026-10-31 (CLI-1330 amended)
                if isinstance(jq, (int, float)):
                    st['jq95'].append(float(jq))
                st['back'] += float(ev.get('backsteps_per_min') or 0) * w
                st['jump'] += float(ev.get('jumps_per_min') or 0) * w
                if isinstance(ev.get('speed_cv'), (int, float)) and (ev.get('moving_frames') or 0) >= 30:
                    st['speed_cv'].append(float(ev['speed_cv']))
                frames = float(ev.get('frames') or 0)
                st['delay'] += float(ev.get('delay_mean_ms') or 0) * frames
                st['frames'] += frames
                st['stale'] += int(ev.get('stale') or 0)
    viewer = viewer or os.path.basename(path)
    p = players.setdefault(viewer, new_client())
    merge_client(p, st)
    return viewer


def new_client():
    return {'windows': 0, 'l_ms': [], 'sum': {}, 'interp_windows': 0, 'interp_ms': 0.0, 'holds': 0.0,
            'late': 0.0, 'jq95': [], 'delay': 0.0, 'frames': 0.0, 'stale': 0, 'back': 0.0, 'jump': 0.0,
            'speed_cv': []}


def merge_client(p, st):
    for k in ('windows', 'interp_windows', 'interp_ms', 'holds', 'late', 'delay', 'frames', 'stale', 'back', 'jump'):
        p[k] += st[k]
    p['l_ms'] += st['l_ms']
    p['jq95'] += st['jq95']
    p['speed_cv'] += st['speed_cv']
    for k, v in st['sum'].items():
        p['sum'][k] = p['sum'].get(k, 0) + v


def client_row(p):
    s = p['sum']
    secs = s.get('ms', 0) / 1000.0
    im = p['interp_ms']
    return {
        'windows': p['windows'],
        'l_med': median(p['l_ms']),
        'l_p95': quantile(p['l_ms'], 0.95),
        'l_max': max(p['l_ms']) if p['l_ms'] else None,
        'resends': s.get('resends') if p['windows'] else None,
        'resend_pct': (100.0 * s['resends'] / s['rel_blocks_out']) if s.get('rel_blocks_out') else None,
        'dup': s.get('rel_discarded') if p['windows'] else None,
        'flow_drops': s.get('flow_drops') if p['windows'] else None,
        'in_rate': (s.get('in', 0) / secs) if secs > 0 else None,
        'out_rate': (s.get('out', 0) / secs) if secs > 0 else None,
        'interp_windows': p['interp_windows'],
        'holds_min': (p['holds'] / im) if im else None,
        'late_min': (p['late'] / im) if im else None,
        'jq95': median(p['jq95']),
        'jq95_max': max(p['jq95']) if p['jq95'] else None,
        'delay_mean': (p['delay'] / p['frames']) if p['frames'] else None,
        'stale': p['stale'] if p['interp_windows'] else None,
        'back_min': (p['back'] / im) if im else None,
        'jump_min': (p['jump'] / im) if im else None,
        'speed_cv': median(p['speed_cv']),
    }


# ------------------------------------------------------------------------------------------------ collection

def session_window(folder):
    """(start, end) epoch seconds of a capture session from its manifest, or None."""
    m = os.path.join(folder, 'manifest.json')
    if not os.path.isfile(m):
        return None
    try:
        with open(m, encoding='utf-8') as f:
            man = json.load(f)
        start = man.get('started_utc')
        end = man.get('ended_utc')
        if not start:
            return None
        t0 = datetime.datetime.fromisoformat(start).timestamp()
        t1 = datetime.datetime.fromisoformat(end).timestamp() if end else t0 + float(man.get('duration_s') or 0)
        return t0, t1
    except (ValueError, OSError, TypeError):
        return None


def find_inputs(folder, bots=False):
    clients = [p for p in glob.glob(os.path.join(folder, '**', '*.net.jsonl'), recursive=True)
               if not p.endswith('.net.menu.jsonl')]
    if bots:
        clients += glob.glob(os.path.join(folder, '**', '*.events.jsonl'), recursive=True)
    servers = sorted(set(glob.glob(os.path.join(folder, '**', 'dserver*.log'), recursive=True)
                         + glob.glob(os.path.join(folder, '**', 'dserver', '*.log'), recursive=True)))
    win = session_window(folder)
    skipped = 0
    if win:
        keep = []
        for s in servers:
            mt = os.path.getmtime(s)
            if win[0] - 120 <= mt <= win[1] + 600:
                keep.append(s)
            else:
                skipped += 1
        servers = keep
    return sorted(clients), servers, skipped


def collect(dservers, clients):
    sp, cp, notes = {}, {}, []
    for d in dservers:
        n = read_dserver(d, sp)
        if n == 0:
            notes.append('%s: no link lines (a dserver older than CLI-1331)' % os.path.basename(d))
    for c in clients:
        v = read_client(c, cp)
        if cp[v]['interp_windows'] == 0:
            notes.append('%s (%s): no remote_interp events (a client older than CLI-1331)'
                         % (os.path.basename(c), v))
    srv = {k: server_row(v) for k, v in sp.items() if v['windows'] or v['rel_blocks']}
    cli = {k: client_row(v) for k, v in cp.items()}
    return srv, cli, notes


def collect_arg(path, bots):
    """A session folder or one log file (`.log` = dserver, else a client event log)."""
    if os.path.isdir(path):
        c, s, skipped = find_inputs(path, bots)
        srv, cli, notes = collect(s, c)
        if skipped:
            notes.append('%d dserver log(s) outside the session window skipped' % skipped)
        return srv, cli, notes
    if path.endswith('.log'):
        return collect([path], [])
    return collect([], [path])


# ------------------------------------------------------------------------------------------------ output

SERVER_COLS = [('windows', 'windows', 0), ('rtt_med', 'rtt med', 0), ('rtt_p95', 'rtt p95', 0),
               ('rtt_max', 'rtt max', 0), ('jit_med', 'jitter med', 1), ('jit_p95', 'jitter p95', 1),
               ('input_med', 'input/s', 1), ('gap_max', 'longest gap', 0), ('resends', 'resends', 0),
               ('resend_pct', 'resend %', 2), ('flow_drops', 'flow drops', 0)]
CLIENT_COLS = [('windows', 'windows', 0), ('l_med', 'l_ms med', 0), ('l_p95', 'l_ms p95', 0),
               ('l_max', 'l_ms max', 0), ('resends', 'resends', 0), ('resend_pct', 'resend %', 2),
               ('dup', 'dup in', 0), ('flow_drops', 'flow drops', 0), ('in_rate', 'dg in/s', 1),
               ('out_rate', 'dg out/s', 1), ('interp_windows', 'interp win', 0), ('holds_min', 'holds/min', 1),
               ('late_min', 'late/min', 1), ('jq95', 'late max', 1), ('jq95_max', 'late max hi', 1),
               ('delay_mean', 'delay mean', 1), ('stale', 'stale', 0), ('back_min', 'back/min', 1),
               ('jump_min', 'jump/min', 1), ('speed_cv', 'speed cv', 3)]
COMPARE = [('server', 'rtt_med', 'srv rtt med', 0), ('server', 'rtt_p95', 'srv rtt p95', 0),
           ('server', 'jit_p95', 'srv jitter p95', 1), ('server', 'resend_pct', 'srv resend %', 2),
           ('client', 'l_med', 'cl l_ms med', 0), ('client', 'l_p95', 'cl l_ms p95', 0),
           ('client', 'resend_pct', 'cl resend %', 2), ('client', 'holds_min', 'holds/min', 1),
           ('client', 'late_min', 'late/min', 1), ('client', 'jq95', 'late max', 1),
           ('client', 'delay_mean', 'delay mean', 1)]


def table(title, rows, cols, out):
    out.append('## ' + title)
    out.append('')
    if not rows:
        out.append('(no data)')
        out.append('')
        return
    out.append('| player | ' + ' | '.join(c[1] for c in cols) + ' |')
    out.append('|---|' + '---:|' * len(cols))
    for name in sorted(rows, key=str.lower):
        r = rows[name]
        out.append('| ' + name + ' | ' + ' | '.join(fmt(r.get(k), d) for k, _, d in cols) + ' |')
    out.append('')


def report(srv, cli, notes, title='Network report'):
    out = ['# ' + title, '']
    table('Server view (dserver link lines, per 10 s window; ms)', srv, SERVER_COLS, out)
    table('Client view (dc-client link_stats every 5 s, remote_interp every 10 s; ms)', cli, CLIENT_COLS, out)
    for n in notes:
        out.append('- ' + n)
    return '\n'.join(out).rstrip() + '\n'


def compare(a, b):
    out = ['# Network comparison (B - A)', '']
    names = sorted(set(a[0]) | set(a[1]) | set(b[0]) | set(b[1]), key=str.lower)
    out.append('| player | metric | A | B | B - A |')
    out.append('|---|---|---:|---:|---:|')
    for name in names:
        for side, key, label, d in COMPARE:
            i = 0 if side == 'server' else 1
            va = a[i].get(name, {}).get(key)
            vb = b[i].get(name, {}).get(key)
            if va is None and vb is None:
                continue
            delta = (vb - va) if (va is not None and vb is not None) else None
            sign = '+' if delta is not None and delta > 0 else ''
            out.append('| %s | %s | %s | %s | %s |' % (name, label, fmt(va, d), fmt(vb, d),
                                                         (sign + fmt(delta, d)) if delta is not None else '-'))
    for tag, n in (('A', a[2]), ('B', b[2])):
        for x in n:
            out.append('- %s: %s' % (tag, x))
    return '\n'.join(out).rstrip() + '\n'


# ------------------------------------------------------------------------------------------------ self test

FIXTURE_DSERVER = '''\
       98 dc-dserver: world Worlds\\X (1 bytes)
     5035 dc-dserver: client 0: hello name "alpha" (second string 5 bytes)
     5297 dc-dserver: client 1: hello name "beta" (second string 4 bytes)
    15040 dc-dserver: client 0: link 10 s: rtt 2 ms, input 30.0/s (jitter 0.5 ms, longest gap 40 ms), resends 0, flow drops 0, datagrams in 300 out 600
    15300 dc-dserver: client 1: link 10 s: rtt 110 ms (window mean 104, min 80, max 300, 12 samples), input 29.0/s (jitter 12.5 ms, longest gap 420 ms), resends 3, flow drops 1, datagrams in 290 out 600
    25300 dc-dserver: client 1: link 10 s: rtt 130 ms (window mean 126, min 90, max 1044, 11 samples), input 28.0/s (jitter 20.0 ms, longest gap 200 ms), resends 1, flow drops 0, datagrams in 280 out 600
    26000 dc-dserver: client 1: link summary 21 s: rtt mean 118 ms (min 80, max 1044), input 600 updates (28.5/s, jitter 20.0 ms, longest gap 420 ms), resends 4 of 200 reliable blocks, flow drops 1, datagrams in 570 out 1200
'''


def fixture_client(name, interp):
    lines = []
    for k in range(4):
        lines.append({'bot': name, 'ev': 'link_stats', 'l_ms': [100, 120, 110, 400][k], 'resends': k,
                      'rel_blocks_out': 50 * (k + 1), 'rel_discarded': k * 2, 'flow_drops': 0,
                      'in': 300 * (k + 1), 'out': 150 * (k + 1), 'ms': 5000 * (k + 1)})
    if interp:
        lines.append({'bot': name, 'ev': 'remote_interp', 'name': 'alpha', 'window_ms': 10000, 'samples': 300,
                      'stale': 1, 'jitter_q95_ms': 20.0, 'delay_mean_ms': 80.0, 'frames': 600,
                      'late_per_min': 6.0, 'holds_per_min': 0.0})
        lines.append({'bot': name, 'ev': 'remote_interp', 'name': 'gamma', 'window_ms': 10000, 'samples': 300,
                      'stale': 0, 'jitter_q95_ms': 40.0, 'delay_mean_ms': 100.0, 'frames': 200,
                      'late_per_min': 12.0, 'holds_per_min': 3.0})
    return '\n'.join(json.dumps(x) for x in lines) + '\n{"bad json\n'


def selftest():
    tmp = tempfile.mkdtemp(prefix='net_report_')
    try:
        a = os.path.join(tmp, 'a', 'logs')
        b = os.path.join(tmp, 'b', 'logs')
        os.makedirs(a)
        os.makedirs(b)
        with open(os.path.join(a, 'dserver_g1.log'), 'w', encoding='utf-8') as f:
            f.write(FIXTURE_DSERVER)
        with open(os.path.join(a, 'dc-client.net.jsonl'), 'w', encoding='utf-8') as f:
            f.write(fixture_client('beta', False))
        with open(os.path.join(a, 'dc-client.net.menu.jsonl'), 'w', encoding='utf-8') as f:
            f.write('{"ev":"tcp_in"}\n')
        with open(os.path.join(b, 'dc-client.net.jsonl'), 'w', encoding='utf-8') as f:
            f.write(fixture_client('beta', True))
        srv, cli, notes = collect_arg(os.path.join(tmp, 'a'), False)
        s = srv['beta']
        assert (s['windows'], s['rtt_med'], s['rtt_max'], s['resends'], s['flow_drops']) == (2, 115.0, 1044.0, 4, 1), s
        assert abs(s['resend_pct'] - 2.0) < 1e-9 and s['jit_p95'] == 20.0 and s['gap_max'] == 420, s
        al = srv['alpha']
        assert (al['windows'], al['rtt_med'], al['rtt_max'], al['resend_pct']) == (1, 2.0, 2.0, None), al
        c = cli['beta']
        assert (c['windows'], c['l_med'], c['l_max'], c['resends'], c['dup']) == (4, 115.0, 400.0, 3, 6), c
        assert abs(c['resend_pct'] - 1.5) < 1e-9 and abs(c['in_rate'] - 60.0) < 1e-9, c
        assert c['holds_min'] is None and c['interp_windows'] == 0, c
        assert any('no remote_interp' in n for n in notes), notes
        text = report(srv, cli, notes)
        assert '| beta | 2 | 115 | 126 | 1044 |' in text, text
        _, cli_b, _ = collect_arg(os.path.join(tmp, 'b'), False)
        cb = cli_b['beta']
        assert (cb['holds_min'], cb['late_min'], cb['jq95'], cb['stale']) == (1.5, 9.0, 30.0, 1), cb
        assert abs(cb['delay_mean'] - 85.0) < 1e-9, cb
        cmp = compare((srv, cli, notes), (srv, cli_b, []))
        assert '| beta | holds/min | - | 1.5 | - |' in cmp, cmp
        assert '| beta | cl l_ms med | 115 | 115 | 0 |' in cmp, cmp
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print('selftest ok')
    return 0


# ------------------------------------------------------------------------------------------------ main

def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('paths', nargs='*', help='a session folder, or client event logs (*.net.jsonl)')
    ap.add_argument('--dserver', action='append', default=[], help='a dserver log (repeatable)')
    ap.add_argument('--compare', nargs=2, metavar=('A', 'B'), help='two session folders or log files')
    ap.add_argument('--bots', action='store_true', help='also read dc-botclient *.events.jsonl in a folder')
    ap.add_argument('--selftest', action='store_true', help='run the built-in fixture test')
    a = ap.parse_args(argv)
    if a.selftest:
        return selftest()
    if a.compare:
        sys.stdout.write(compare(collect_arg(a.compare[0], a.bots), collect_arg(a.compare[1], a.bots)))
        return 0
    if len(a.paths) == 1 and os.path.isdir(a.paths[0]) and not a.dserver:
        srv, cli, notes = collect_arg(a.paths[0], a.bots)
    elif a.paths or a.dserver:
        for p in a.paths:
            if os.path.isdir(p):
                ap.error('give one session folder alone, or log files')
        srv, cli, notes = collect(a.dserver, a.paths)
    else:
        ap.error('nothing to read')
    sys.stdout.write(report(srv, cli, notes))
    return 0


if __name__ == '__main__':
    sys.exit(main())
