#!/usr/bin/env python3
"""mapkit_pack_win.py: the Windows MapKit package (docs/MAPKIT_PLAN.md 6.6, DC_DEFAULTS.md MAP-74), the counterpart
of tools/mapkit_pack_mac.sh: a zip with the five exes of the map builder (dc-mapkit-gui.exe, the double-clickable
editor; dc-mapkit.exe, the console tool; dc-lobby-server.exe, dc-dserver.exe and dc-client.exe, which
`dc-mapkit play` starts from the folder next to its own exe) and a README with the first-run notes. No .bat: the
GUI exe is the thing to double-click. Standard library only, so GitHub Actions' Windows runner and the Windows
machine run it alike (.github/workflows/release.yml).

  python tools/mapkit_pack_win.py --bin-dir <folder with the exes> [--out DIR] [--name NAME] [--commit SHA]

Nothing of the game (ROM) and nothing of the building machine goes in. A build without dc-mapkit-gui.exe (the
console crate alone) packs with a warning, so the packer can be tested before the GUI lands.
"""
import argparse
import os
import subprocess
import sys
import zipfile

# DC-DEFAULT: MAP-74 (the five exes of the package; the GUI one leads)
GUI = "dc-mapkit-gui.exe"
CONSOLE = "dc-mapkit.exe"
GAME = ["dc-lobby-server.exe", "dc-dserver.exe", "dc-client.exe"]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--bin-dir", required=True, help="the folder holding the built exes (target/<triple>/release)")
    ap.add_argument("--out", default="out_mapkit", help="folder for the zip (default out_mapkit/)")
    ap.add_argument("--name", default="", help="zip base name (default DeCombat-MapKit-win-<commit>)")
    ap.add_argument("--commit", default="", help="commit id for the name and the README (default: git)")
    a = ap.parse_args()
    exes = [CONSOLE] + GAME
    for e in exes:
        if not os.path.isfile(os.path.join(a.bin_dir, e)):
            print(f"no {e} in {a.bin_dir}: build it first (play needs it next to the exe)", file=sys.stderr)
            return 2
    has_gui = os.path.isfile(os.path.join(a.bin_dir, GUI))
    if has_gui:
        exes.insert(0, GUI)
    else:
        print(f"warning: no {GUI} in {a.bin_dir}; the package holds the console tool only", file=sys.stderr)
    commit = a.commit
    if not commit:
        try:
            commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True,
                                    check=True).stdout.strip()
        except Exception:
            commit = "unknown"
    name = a.name or f"DeCombat-MapKit-win-{commit}"
    start = (f"Double-click **{GUI}**." if has_gui else
             f"Run **{CONSOLE} gui** from a command prompt (this build has no separate GUI exe).")

    readme = f"""# DeCombat MapKit for Windows

The community map builder for DeCombat: build a map from a text project, look at it, export it as a mod and play
it on a local vanilla++ server (lobby + game server + client on this PC). The playbook for the tool and for AI
agents driving it is docs/MAPKIT_AGENTS.md in the repository; the GUI is the same tool with a window.

## First run

1. Unzip this anywhere. Keep the five exes together in one folder: the editor (dc-mapkit-gui.exe), the console
   tool (dc-mapkit.exe) and the three game programs that Play starts (dc-lobby-server.exe, dc-dserver.exe,
   dc-client.exe) are found next to each other.
2. {start} The first time Windows SmartScreen may say the program is unrecognised: click
   "More info", then "Run anyway". No installer, no runtime to install (the exes are self-contained).
3. **Choose the ROM folder** when the banner asks for it: your own 2009 Combat Arms client folder (the one that
   holds Engine.exe and a Game folder). Nothing of the game is included in this package and the folder is only
   read. The choice is remembered in %APPDATA%\\DeCombat\\mapkit.toml; the tool also takes `--rom DIR`, the
   `CA_ROM` variable, or a client folder placed next to the exes. Editing, validating and exporting work without
   a ROM; the ROM listings, the in-game views and Play need it.
4. New map > starter template, change it, Build, Export, Play. Play starts a modded lobby and game server on this
   PC and the client against them; close the client to stop everything. Logs land in the project's build\\play\\.

## The console tool and AI agents

dc-mapkit.exe is the same tool without a window (every verb takes `--json`; `dc-mapkit mcp` serves the verbs to
Claude Code and other MCP clients). Put this folder on your PATH or give the full path in the `.mcp.json` snippet
of docs/MAPKIT_AGENTS.md. The projects you make are plain folders (map.json, terrain.r16, assets\\) and belong in
your own git repository; the exported zip under build\\ is what a server hosts.

Problems: a message that the ROM folder has no Game folder means the folder picked is not the client folder
itself. An antivirus that quarantines an exe: restore it and add the folder to the exclusions (the exes are
unsigned builds of the open-source tool).
Build {commit}, 64-bit Windows 10 or later.
"""
    os.makedirs(a.out, exist_ok=True)
    zpath = os.path.join(a.out, name + ".zip")
    if os.path.exists(zpath):
        os.remove(zpath)
    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as z:
        for e in exes:
            z.write(os.path.join(a.bin_dir, e), f"{name}/{e}")
        z.writestr(f"{name}/README.md", readme)
    size = os.path.getsize(zpath) / 1e6
    print(f"wrote {zpath} ({size:.1f} MB); exes: {' '.join(exes)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
