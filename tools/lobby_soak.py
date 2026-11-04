#!/usr/bin/env python3
"""lobby_soak.py: how many dc-botclient logins can one lobby hold, with no game window and no ROM (docs/OPS.md "Load test").

    python tools/lobby_soak.py --bin-dir target/debug --bots 200 --seconds 240 [--per-second 2] [--join-timeout 45]

ORIGINAL harness of the DeCombat project (OPS-23, DC_DEFAULTS.md). Standard library only. It starts a dc-lobby-server on
loopback ports with a fresh database in a temp folder, then one dc-botclient process per bot. Each bot logs in over the
real auth and lobby ports, enters a channel, finds no room, gives up after --join-timeout and is started again, so the
run measures both held connections and a steady stream of logins. Afterwards: bots started, peak connections from the
lobby's /status, the lobby's CPU seconds and memory, and every line of its log and the bots' output that looks wrong.

All bots take channel 1, which holds 200 (--channel-max); the rest are refused and try again, which is the login stream.
What it does not measure: matches. No game server runs and nobody plays (a match needs the ROM). All clients share one
machine and loopback, which is exempt from the per-address cap (LG-326) and, without --public, skips the password
hash; every bot is a whole process, so this machine's process count is a limit before the lobby's is.
"""
import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request


def exe(bin_dir: pathlib.Path, name: str) -> str:
    for candidate in (bin_dir / name, bin_dir / f"{name}.exe"):
        if candidate.is_file():
            return str(candidate)
    sys.exit(f"no {name} in {bin_dir}")


def lobby_usage(pid: int) -> tuple[float, float]:
    """CPU seconds and resident megabytes of a process; (0, 0) when the platform's tool does not answer."""
    try:
        if os.name == "nt":
            out = subprocess.run(
                ["powershell", "-NoProfile", "-Command",
                 f"$p=Get-Process -Id {pid}; \"$($p.CPU) $($p.WorkingSet64)\""],
                capture_output=True, text=True, timeout=30).stdout.split()
            return float(out[0]), float(out[1]) / 1e6
        out = subprocess.run(["ps", "-o", "cputimes=,rss=", "-p", str(pid)],
                             capture_output=True, text=True, timeout=10).stdout.split()
        return float(out[0]), float(out[1]) / 1e3
    except (OSError, ValueError, IndexError, subprocess.SubprocessError):
        return 0.0, 0.0


def status(admin: int) -> dict:
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{admin}/status", timeout=5) as r:
            return json.load(r)
    except (OSError, ValueError):
        return {}


def wrong_lines(path: pathlib.Path) -> list[str]:
    words = ("panic", "error", "rolled back", "refused", "too many", "handshakes in progress", "failed")
    if not path.is_file():
        return []
    with path.open(encoding="utf-8", errors="replace") as f:
        return [l.rstrip() for l in f if any(w in l.lower() for w in words)]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--bin-dir", default="target/debug")
    ap.add_argument("--bots", type=int, default=100)
    ap.add_argument("--seconds", type=int, default=240)
    ap.add_argument("--per-second", type=float, default=2.0, help="new bot processes per second at most")
    ap.add_argument("--join-timeout", type=int, default=45)
    ap.add_argument("--port", type=int, default=21500)
    a = ap.parse_args()
    bins = pathlib.Path(a.bin_dir)
    lobby, bot = exe(bins, "dc-lobby-server"), exe(bins, "dc-botclient")
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="dc-soak-"))
    admin = a.port + 90
    log = tmp / "lobby.log"
    srv = subprocess.Popen(
        [lobby, "--db", str(tmp / "ca.db"), "--port", str(a.port), "--lobbyport", str(a.port + 2),
         "--clanport", str(a.port + 3), "--clanweb", "0", "--autochar", "--log", str(log),
         "--admin", f"127.0.0.1:{admin}"],
        stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
    deadline = time.time() + 20
    while not status(admin) and time.time() < deadline:
        time.sleep(0.2)
    if not status(admin):
        srv.kill()
        return f"the lobby did not start; {log}"
    procs: dict[int, subprocess.Popen] = {}
    outs: dict[int, pathlib.Path] = {}
    started = crashes = 0
    peak_sessions = 0
    peak_login_ms = 0.0
    begin = time.time()
    end = begin + a.seconds
    next_start = begin
    sample_at = begin
    rows = []

    def start(i: int) -> None:
        nonlocal started
        outs[i] = tmp / f"bot{i}.out"
        with outs[i].open("a", encoding="utf-8") as f:
            procs[i] = subprocess.Popen(
                [bot, "--lobby", f"127.0.0.1:{a.port}", "--name", f"Soak{i}", "--room", "any", "--no-rom",
                 "--join-timeout", str(a.join_timeout)],
                stdout=f, stderr=subprocess.STDOUT, cwd=tmp)
        started += 1

    try:
        while time.time() < end:
            now = time.time()
            for i in range(a.bots):
                if now < next_start:
                    break
                p = procs.get(i)
                if p is None or p.poll() is not None:
                    if p is not None and p.returncode not in (0, 1, 2):
                        crashes += 1
                    start(i)
                    next_start = max(next_start, now - 1) + 1 / a.per_second
                    now = time.time()
            if now >= sample_at:
                sample_at = now + 15
                st = status(admin)
                cpu, mem = lobby_usage(srv.pid)
                sessions = st.get("connections", st.get("sessions", 0))
                peak_sessions = max(peak_sessions, sessions if isinstance(sessions, int) else 0)
                rows.append((round(now - begin), sum(p.poll() is None for p in procs.values()), sessions, round(cpu, 1), round(mem)))
                print(f"t={rows[-1][0]:>4}s bot processes {rows[-1][1]:>4}  lobby connections {sessions}  cpu {cpu:.1f}s  rss {mem:.0f} MB", flush=True)
            time.sleep(0.05)
        final = status(admin)
    finally:
        for p in procs.values():
            if p.poll() is None:
                p.kill()
        cpu, mem = lobby_usage(srv.pid)
        srv.kill()
        srv.wait(timeout=20)
    wall = time.time() - begin
    lobby_bad = wrong_lines(log)
    bot_bad = [l for f in outs.values() for l in wrong_lines(f)]
    logins = 0
    if log.is_file():
        with log.open(encoding="utf-8", errors="replace") as f:
            logins = sum("lobby connection" in l for l in f)
    print(f"\nbots {a.bots}, bot starts {started}, unexpected bot exits {crashes}, wall {wall:.0f} s, machine CPUs {os.cpu_count()}")
    print(f"lobby: peak connections {peak_sessions}, lobby connections in its log {logins}, CPU {cpu:.1f} s "
          f"({100 * cpu / wall:.1f}% of one core), rss {mem:.0f} MB, /status keys {sorted(final)[:12]}")
    print(f"lobby log lines that look wrong: {len(lobby_bad)}")
    for l in lobby_bad[:8]:
        print("  ", l[:200])
    print(f"bot output lines that look wrong: {len(bot_bad)}")
    for l in sorted(set(x[:160] for x in bot_bad))[:8]:
        print("  ", l)
    shutil.rmtree(tmp, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
