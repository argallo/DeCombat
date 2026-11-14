#!/usr/bin/env python3
"""engine_gate.py: the engine-boundary ratchet. Every crate in tools/engine_crates.txt must build without the emulator
crates: `cargo tree -p <crate> -e normal,build --target all` may not list dc-rom, dc-rez, dc-bute or dc-ui-data.

  python tools/engine_gate.py                 # check the list (exit 0 clean, 1 a crate pulls an emulator crate)
  python tools/engine_gate.py --crate dc-sim  # check these crates instead (to see what a cut still has to remove)
  python tools/engine_gate.py selftest        # the parser on canned cargo tree output (no cargo needed)

One cargo invocation per crate: cargo unifies features across the packages of one invocation, so a crate checked
together with an app would show the app's features. Dev-dependencies are not checked (ROM-gated tests may use the ROM).
On a hit it prints `cargo tree -i <emulator crate>` for that crate: the path that pulls it in. CARGO names the cargo
command (default `cargo`); exit 2 when it cannot be run.
"""
# Provenance: ORIGINAL
import argparse
import os
import re
import subprocess
import sys

EMULATOR = ("dc-rom", "dc-rez", "dc-bute", "dc-ui-data")
LIST = os.path.join(os.path.dirname(os.path.abspath(__file__)), "engine_crates.txt")
TREE = ["tree", "-e", "normal,build", "--target", "all"]


def read_list(path):
    with open(path, encoding="utf-8") as f:
        return [ln.strip() for ln in f if ln.strip() and not ln.lstrip().startswith("#")]


def emulator_hits(tree_out):
    """The emulator crates named in `cargo tree --prefix none --format {p}` output, in order, once each."""
    hits = []
    for line in tree_out.splitlines():
        m = re.match(r"^(dc-[a-z0-9-]+) v\d", line.strip())
        if m and m.group(1) in EMULATOR and m.group(1) not in hits:
            hits.append(m.group(1))
    return hits


class NoCargo(Exception):
    pass


def cargo(args):
    cmd = [os.environ.get("CARGO", "cargo")] + args
    try:
        r = subprocess.run(cmd, capture_output=True, text=True)
    except FileNotFoundError:
        raise NoCargo(cmd[0]) from None
    return r.returncode, r.stdout, r.stderr


def check(crates):
    bad = 0
    for c in crates:
        rc, out, err = cargo(TREE + ["-p", c, "--prefix", "none", "--format", "{p}"])
        if rc != 0:
            print(f"FAIL {c}: cargo tree failed: {err.strip().splitlines()[-1] if err.strip() else rc}")
            bad += 1
            continue
        hits = emulator_hits(out)
        if not hits:
            print(f"ok   {c}")
            continue
        bad += 1
        print(f"FAIL {c}: depends on {', '.join(hits)}")
        for h in hits:
            _, path, _ = cargo(TREE + ["-p", c, "-i", h])
            print("\n".join("     " + ln for ln in path.splitlines()))
    print(f"engine gate: {len(crates) - bad} of {len(crates)} crates free of {', '.join(EMULATOR)}")
    return 1 if bad else 0


def selftest():
    clean = "dc-wire v0.1.0 (C:/x/crates/dc-wire)\nproptest v1.5.0\n"
    assert emulator_hits(clean) == [], emulator_hits(clean)
    dirty = ("dc-world v0.1.0 (/x/crates/dc-world)\ndc-core v0.1.0 (/x/crates/dc-core)\n"
             "dc-rez v0.1.0 (/x/crates/dc-rez)\ndc-rom v0.1.0 (/x/crates/dc-rom)\ndc-rom v0.1.0 (/x/crates/dc-rom) (*)\n")
    assert emulator_hits(dirty) == ["dc-rez", "dc-rom"], emulator_hits(dirty)
    # a crate whose name only starts like an emulator crate is not one
    assert emulator_hits("dc-romlist v0.1.0 (/x)\ndc-bute-extra v0.1.0 (/x)\n") == []
    lst = read_list(LIST)
    assert lst and len(lst) == len(set(lst)), "engine_crates.txt: empty or a crate twice"
    assert not set(lst) & set(EMULATOR), "engine_crates.txt names an emulator crate"
    print("engine_gate.py selftest: ok")
    return 0


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if argv[:1] == ["selftest"]:
        return selftest()
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--crate", nargs="+", help="check these crates instead of tools/engine_crates.txt")
    a = ap.parse_args(argv)
    try:
        return check(a.crate or read_list(LIST))
    except NoCargo as e:
        print(f"engine_gate.py: cannot run {e} (not on PATH): set CARGO to the cargo executable, or run it through the "
              "environment wrapper that sets up the toolchain (on Windows: python cargo_env.py python tools/engine_gate.py)",
              file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
