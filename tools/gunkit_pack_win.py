#!/usr/bin/env python3
"""gunkit_pack_win.py: the Windows GunKit package (DC_DEFAULTS.md VAR-8), the counterpart of tools/gunkit_pack_mac.sh: a zip with
the camo studio (dc-gunkit-gui.exe, the double-clickable window; dc-gunkit.exe, the console tool and MCP server) and a README with
the first-run notes. Standard library only, so GitHub Actions' Windows runner and the Windows machine run it alike.

  python tools/gunkit_pack_win.py --bin-dir <folder with the exes> [--out DIR] [--name NAME] [--commit SHA]

Nothing of the game (ROM) and nothing of the building machine goes in.
"""
import argparse
import os
import subprocess
import sys
import zipfile

EXES = ["dc-gunkit-gui.exe", "dc-gunkit.exe"]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--bin-dir", required=True, help="the folder holding the built exes (target/<triple>/release)")
    ap.add_argument("--out", default="out_gunkit", help="folder for the zip (default out_gunkit/)")
    ap.add_argument("--name", default="", help="zip base name (default DeCombat-GunKit-win-<commit>)")
    ap.add_argument("--commit", default="", help="commit id for the name and the README (default: git)")
    ap.add_argument("--examples", default="", help="folder of example packs to ship as examples/ next to the exes "
                    "(tools/gunkit_examples.py fills one; VAR-13)")
    a = ap.parse_args()
    examples = []
    if a.examples:
        for d in sorted(os.listdir(a.examples)):
            if os.path.isfile(os.path.join(a.examples, d, "manifest.json")):
                examples.append(d)
        if not examples:
            print(f"no pack folders (manifest.json) under {a.examples}", file=sys.stderr)
            return 2
    for e in EXES:
        if not os.path.isfile(os.path.join(a.bin_dir, e)):
            print(f"no {e} in {a.bin_dir}: cargo build --release -p dc-gunkit --bins first", file=sys.stderr)
            return 2
    commit = a.commit
    if not commit:
        try:
            commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True,
                                    check=True).stdout.strip()
        except Exception:
            commit = "unknown"
    name = a.name or f"DeCombat-GunKit-win-{commit}"

    readme = f"""# DeCombat GunKit for Windows

A small studio for designing camos on the game's own guns. Pick a gun, turn it with the mouse, and paint it from the window or
live from an AI agent. It draws with the game's renderer and paints with the game's recipe, so what you see is what players get.

## First run

1. Unzip this anywhere and keep the two exes together. Double-click **dc-gunkit-gui.exe**. The first time Windows SmartScreen may
   say the program is unrecognised: click "More info", then "Run anyway". There is nothing to install.
2. **Choose your Combat Arms folder** when it asks: your own client folder, the one that holds Engine.exe and a Game folder.
   Nothing of the game is in this package, and the folder is only read.
3. Pick a gun, click a pattern, and drag the gun to turn it. Drop your own PNG on the window to use it: save over the file and the
   gun updates by itself. "Open > Examples" lists the camos in the `examples` folder next to the exes{{EXAMPLES_NOTE}}.
4. **Save pack** writes a Vanilla++ pack to Documents\\DeCombat Camos and checks it.

## With an AI agent

    claude mcp add dc-gunkit -- "C:\\path\\to\\dc-gunkit.exe" mcp

The window's "Connect an agent" section shows this with the right path, ready to copy. The agent's changes appear on the gun as you
watch, and it opens the window if it is not open. The playbook is docs/GUNKIT_AGENTS.md in the repository.

Build {commit}, 64-bit Windows 10 or later.
"""
    os.makedirs(a.out, exist_ok=True)
    zpath = os.path.join(a.out, name + ".zip")
    if os.path.exists(zpath):
        os.remove(zpath)
    note = f" ({len(examples)} ship with this package)" if examples else " (none ship with this build: add your own packs there)"
    readme = readme.replace("{EXAMPLES_NOTE}", note)
    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as z:
        for e in EXES:
            z.write(os.path.join(a.bin_dir, e), f"{name}/{e}")
        z.writestr(f"{name}/README.md", readme)
        for d in examples:
            root = os.path.join(a.examples, d)
            for dirpath, _, files in os.walk(root):
                for f in sorted(files):
                    full = os.path.join(dirpath, f)
                    rel = os.path.relpath(full, a.examples).replace(os.sep, "/")
                    z.write(full, f"{name}/examples/{rel}")
    size = os.path.getsize(zpath) / 1e6
    print(f"wrote {zpath} ({size:.1f} MB); exes: {' '.join(EXES)}; examples: {len(examples)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
