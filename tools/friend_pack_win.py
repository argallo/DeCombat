#!/usr/bin/env python3
"""friend_pack_win.py: the Windows friend package (HOSTING.md "What a friend needs"), the counterpart of
tools/friend_pack_mac.sh: a zip with DeCombat.exe (the built dc-client.exe under the name a player starts), a README
and pack.json, preset to one server. A double-click on DeCombat.exe finds the 2009 client folder next to it and opens
the login screen of that server (CLI-1906). Standard library only, so GitHub Actions' Windows runner and the Windows
machine run it alike (.github/workflows/release.yml).

  python tools/friend_pack_win.py --client <dc-client.exe> --server HOST[:PORT] [--fingerprint SHA256]
                                  [--out DIR] [--name NAME] [--commit SHA] [--botclient <dc-botclient.exe>]
                                  [--update-dir DIR]

Nothing of the game (ROM) and nothing of the building machine goes in, except the --update-dir path.

pack.json next to the exe names the build and the server ({"commit", "update_dir", "server", "fingerprint"}). With
--update-dir an update zip is also written into that folder and latest.json ({"commit", "zip"}) there last: a pack
that names the folder offers "Update now" when the folder holds another build (docs/RELEASES.md, "Friend packs that
update themselves"). Building the next pack with the same --update-dir is the whole publish step. The update zip is
the player's zip plus the exe once more as dc-client.exe: a pack from before DeCombat.exe takes only a zip with that
name, and the new build removes it (and the old DeCombat.bat) after the swap.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import zipfile


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--client", required=True, help="the built dc-client.exe (packed as DeCombat.exe)")
    ap.add_argument("--botclient", default="", help="the built dc-botclient.exe: also writes <name>-bots.zip with it and bots.bat (docs/BOTS.md)")
    ap.add_argument("--server", required=True, help="lobby HOST[:PORT] (port 10000 without one)")
    ap.add_argument("--fingerprint", default="", help="the lobby's certificate fingerprint (pinned when given)")
    ap.add_argument("--out", default="out_friend", help="folder for the zip (default out_friend/)")
    ap.add_argument("--name", default="", help="zip base name (default DeCombat-win-<commit>)")
    ap.add_argument("--commit", default="", help="commit id for the name and the README (default: git)")
    ap.add_argument("--update-dir", default="", help="folder the pack looks into for newer builds; also gets the update zip and latest.json")
    a = ap.parse_args()
    if not os.path.isfile(a.client):
        print(f"no such client: {a.client}", file=sys.stderr)
        return 2
    server = a.server if ":" in a.server else a.server + ":10000"
    host = server.rsplit(":", 1)[0]
    fingerprint = a.fingerprint.strip()
    commit = a.commit
    if not commit:
        try:
            commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True,
                                    check=True).stdout.strip()
        except Exception:
            commit = "unknown"
    name = a.name or f"DeCombat-win-{commit}"
    update_dir = os.path.abspath(a.update_dir) if a.update_dir else ""
    hex_commit = 4 <= len(commit) <= 40 and all(c in "0123456789abcdefABCDEF" for c in commit)
    if update_dir and not hex_commit:
        print(f"--update-dir needs a commit id of hex digits, got {commit!r}", file=sys.stderr)
        return 2
    pack = json.dumps({"commit": commit, "update_dir": update_dir, "server": server, "fingerprint": fingerprint},
                      indent=2) + "\n"

    fpline = (f"The game pins the server's certificate fingerprint ({fingerprint}): a changed server identity is refused."
              if fingerprint else
              "The game trusts the server's certificate at your first login and remembers it; a changed server identity is refused later.")
    readme = f"""# DeCombat for Windows: play on {host}

1. Register at https://{host}:18080/register (your name, an email address, a password you use nowhere else; if the
   page asks for an invite code, the server's host gives you one). The browser warns about the server's own certificate: continue,
   that is expected.
2. Unzip this. DeCombat runs on your own copy of the original Combat Arms EU client files (2009). We don't provide
   them or link to them. Point DeCombat at the folder of your existing install: put that folder (the one with
   Engine.exe and a Game folder) into the unzipped folder, next to DeCombat.exe. Nothing of the game is included
   and your folder is never changed.
3. Double-click **DeCombat.exe** (see "First start" below if Windows warns you).
4. The game opens full screen on the login screen: type your account name (the one you registered) and your
   password, Enter. Pick the server and channel, create or join a room. Alt+Enter switches to a window; in a match
   Escape frees the mouse and Escape again leaves. Forgot the password or the login ID: the buttons on the login
   screen mail you a code.

{fpline}

## First start: Windows may warn you

DeCombat is a new program and its exe is not code-signed, so Windows is careful with it.

- **SmartScreen** says "Windows protected your PC": click "More info", then "Run anyway".
- **Windows Defender** may flag the exe as `Trojan:Win32/Wacatac.B!ml`. That is a known false positive for new
  unsigned programs (the same happens to other open-source game projects). You can report the file to Microsoft as
  a false positive at https://www.microsoft.com/en-us/wdsi/filesubmission. To be sure the file is the one we
  published, check it as below.
- **Check your download.** Every release has a `SHA256SUMS` file on the release page
  (https://github.com/argallo/DeCombat/releases). In PowerShell, in the folder of the zip:
  `Get-FileHash .\\{name}.zip` and compare the hash with the line for this zip in `SHA256SUMS`.

## If the game crashes at start or in the menus

Most often it is the graphics driver, so update your GPU driver first (NVIDIA, AMD or Intel, from their website).
The game uses DirectX 12 by default. To try Vulkan instead, set Options, Graphic, Renderer to Vulkan (it applies at
the next start), or start the game with `--adapter vulkan`: make a shortcut to DeCombat.exe (right-click it, "Show
more options", "Send to", "Desktop (create shortcut)"), right-click the shortcut, "Properties", and add
` --adapter vulkan` at the very end of the Target box, after the closing quote. If the game crashes on Vulkan, go
back to DirectX 12 the same way (Renderer row, or ` --adapter dx12` on the shortcut).

## Beta: feedback and resets

This is a beta: accounts and progress may be reset before the full launch.

- Discord: https://discord.gg/hNWcXpTST
- Mail: contact@decombat.com
- In the game: the **Report a Bug** form in the menu, or type `/report` in the chat.

Problems: "Put your Combat Arms folder ..." means the folder is not next to DeCombat.exe or has no Game inside.
"This server does not offer an encrypted login" or a message about the server's identity: the host updated the
server; ask for a new package. Logs for the host: %LOCALAPPDATA%\\DeCombat\\logs (no password in them).
Client build {commit}, 64-bit Windows 10 or later.
"""
    bots_bat = f"""@echo off
rem Local bots that play with you on {host}. Usage: bots.bat [count] [your nick], bots.bat stop, bots.bat list.
rem Anything after those goes to dc-botclient swarm (docs/BOTS.md). Ctrl-C ends them.
setlocal EnableExtensions
set "HERE=%~dp0"
set "BOT=%HERE%dc-botclient.exe"
if /i "%~1"=="stop" ( "%BOT%" swarm stop & exit /b )
if /i "%~1"=="list" ( "%BOT%" swarm list & pause & exit /b )
set "ROM="
for /d %%D in ("%HERE%*") do (
  if exist "%%~fD\\Game\\" if exist "%%~fD\\Engine.exe" if not defined ROM set "ROM=%%~fD"
)
if not defined ROM (
  echo Put your Combat Arms folder next to bots.bat, then start it again.
  pause
  exit /b 2
)
set "COUNT=3"
if not "%~1"=="" set "COUNT=%~1"
set "FOLLOW="
if not "%~2"=="" set "FOLLOW=--follow %~2"
{("set " + chr(34) + "DC_SERVER_FINGERPRINT=" + fingerprint + chr(34)) if fingerprint else "rem no pinned server certificate"}
"%BOT%" swarm --server "{server}" --rom "%ROM%" --count %COUNT% %FOLLOW% %3 %4 %5 %6 %7 %8 %9
if errorlevel 1 pause
endlocal
"""
    if a.botclient:
        readme += f"""
## Bots

Bots are a separate download, {name}-bots.zip, so the game itself stays a single small program. Unzip it into the
same place as this one: its files land next to DeCombat.exe. Then `bots.bat 3 YourNick` starts three bots on your PC
that sit in whatever room you sit in on {host}; `bots.bat stop` ends them (at most 7, Ctrl-C works too). They earn
nothing, you are paid as usual. More in docs/BOTS.md of the source.
"""

    def write_game_zip(path: str, bridge: bool) -> None:
        if os.path.exists(path):
            os.remove(path)
        with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
            z.write(a.client, f"{name}/DeCombat.exe")
            if bridge:
                z.write(a.client, f"{name}/dc-client.exe")
            z.writestr(f"{name}/README.md", readme)
            z.writestr(f"{name}/pack.json", pack)

    os.makedirs(a.out, exist_ok=True)
    zpath = os.path.join(a.out, name + ".zip")
    write_game_zip(zpath, bridge=False)
    size = os.path.getsize(zpath) / 1e6
    print(f"wrote {zpath} ({size:.1f} MB); server {server}; pin: {fingerprint or 'none (trust on first use)'}")
    if update_dir:
        # the zip first, latest.json last (replaced in one step): a pack never reads a latest.json whose zip is not there
        os.makedirs(update_dir, exist_ok=True)
        uname = name + "-update.zip"
        target = os.path.join(update_dir, uname)
        tmp_zip = target + ".part"
        write_game_zip(tmp_zip, bridge=True)
        os.replace(tmp_zip, target)
        tmp = os.path.join(update_dir, "latest.json.part")
        with open(tmp, "w", encoding="utf-8", newline="\n") as f:
            json.dump({"commit": commit, "zip": uname}, f, indent=2)
            f.write("\n")
        os.replace(tmp, os.path.join(update_dir, "latest.json"))
        print(f"published {target} as the latest build of {update_dir}")
    # the bots go in a zip of their own: a program that starts several network clients is what antivirus heuristics
    # dislike, and the game should not share its fate. Same folder name, so both unzip into one folder.
    if a.botclient:
        bpath = os.path.join(a.out, name + "-bots.zip")
        if os.path.exists(bpath):
            os.remove(bpath)
        with zipfile.ZipFile(bpath, "w", zipfile.ZIP_DEFLATED) as z:
            z.write(a.botclient, f"{name}/dc-botclient.exe")
            z.writestr(f"{name}/bots.bat", bots_bat.replace("\n", "\r\n"))
        print(f"wrote {bpath} ({os.path.getsize(bpath) / 1e6:.1f} MB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
