#!/usr/bin/env python3
"""friend_pack_win.py: the Windows friend package (HOSTING.md "What a friend needs"), the counterpart of
tools/friend_pack_mac.sh: a zip with dc-client.exe, DeCombat.bat (double-click starts the game; the 2009 client
folder goes next to it; the account name is asked once) and a README, preset to one server. Standard library
only, so GitHub Actions' Windows runner and the Windows machine run it alike (.github/workflows/release.yml).

  python tools/friend_pack_win.py --client <dc-client.exe> --server HOST[:PORT] [--fingerprint SHA256]
                                  [--out DIR] [--name NAME] [--commit SHA] [--botclient <dc-botclient.exe>]
                                  [--update-dir DIR]

Nothing of the game (ROM) and nothing of the building machine goes in, except the --update-dir path.

pack.json next to the exe names the build ({"commit", "update_dir"}). With --update-dir the zip is also copied into
that folder and latest.json ({"commit", "zip"}) is written there last: a pack that names the folder offers "Update now"
when the folder holds another build (docs/RELEASES.md, "Friend packs that update themselves"). Building the next pack
with the same --update-dir is the whole publish step.
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
    ap.add_argument("--client", required=True, help="the built dc-client.exe")
    ap.add_argument("--botclient", default="", help="the built dc-botclient.exe: also writes <name>-bots.zip with it and bots.bat (docs/BOTS.md)")
    ap.add_argument("--server", required=True, help="lobby HOST[:PORT] (port 10000 without one)")
    ap.add_argument("--fingerprint", default="", help="the lobby's certificate fingerprint (pinned when given)")
    ap.add_argument("--out", default="out_friend", help="folder for the zip (default out_friend/)")
    ap.add_argument("--name", default="", help="zip base name (default DeCombat-win-<commit>)")
    ap.add_argument("--commit", default="", help="commit id for the name and the README (default: git)")
    ap.add_argument("--update-dir", default="", help="folder the pack looks into for newer builds; also gets this zip and latest.json")
    a = ap.parse_args()
    if not os.path.isfile(a.client):
        print(f"no such client: {a.client}", file=sys.stderr)
        return 2
    server = a.server if ":" in a.server else a.server + ":10000"
    host = server.rsplit(":", 1)[0]
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
    pack = json.dumps({"commit": commit, "update_dir": update_dir}, indent=2) + "\n"
    pin = f"--server-fingerprint {a.fingerprint}" if a.fingerprint else ""

    # Flow by goto, not by ( ) blocks: a ")" in a message ends a block and cmd then refuses the whole file, and %VAR%
    # inside a block holds its value from before the block.
    bat = f"""@echo off
rem DeCombat launcher (double-click). Put your 2009 Combat Arms client folder (Engine.exe + Game) next to this file.
rem The account name is asked once and kept in %APPDATA%\\DeCombat\\launcher-name.txt; the login screen takes the password.
setlocal EnableExtensions
set "SERVER={server}"
set "PIN={pin}"
set "HERE=%~dp0"
set "ROM="
for /d %%D in ("%HERE%*") do if exist "%%~fD\\Game\\" if exist "%%~fD\\Engine.exe" if not defined ROM set "ROM=%%~fD"
if defined ROM goto have_rom
echo Put your Combat Arms folder, the one with Engine.exe and Game, next to DeCombat.bat, then start it again.
pause
exit /b 2
:have_rom
set "CFG=%APPDATA%\\DeCombat"
if not exist "%CFG%" mkdir "%CFG%"
set "NAME="
if exist "%CFG%\\launcher-name.txt" set /p NAME=<"%CFG%\\launcher-name.txt"
if defined NAME goto have_name
set /p "NAME=Your DeCombat account name, the one you registered: "
if not defined NAME exit /b 2
>"%CFG%\\launcher-name.txt" echo %NAME%
:have_name
set "LOGS=%LOCALAPPDATA%\\DeCombat\\logs"
if not exist "%LOGS%" mkdir "%LOGS%"
cd /d "%HERE%"
rem diagnostic reports (docs/DIAGNOSTICS.md): the last run's output stays as dc-client.prev.out, stall and crash
rem files go to the log folder
if exist "%LOGS%\\dc-client.out" move /y "%LOGS%\\dc-client.out" "%LOGS%\\dc-client.prev.out" >nul
set "DC_WATCHDOG_DIR=%LOGS%"
"%HERE%dc-client.exe" --menu lobby --lobby "%SERVER%" --name "%NAME%" --rom "%ROM%" %PIN% --net-log "%LOGS%\\dc-client.net.jsonl" > "%LOGS%\\dc-client.out" 2>&1
set "RC=%errorlevel%"
if "%RC%"=="0" goto done
echo DeCombat ended with an error, code %RC%. The log is in %LOGS%\\dc-client.out; send it to the host.
pause
:done
endlocal & exit /b %RC%
"""
    fpline = (f"DeCombat.bat pins the server's certificate fingerprint ({a.fingerprint}): a changed server identity is refused."
              if a.fingerprint else
              "The game trusts the server's certificate at your first login and remembers it; a changed server identity is refused later.")
    readme = f"""# DeCombat for Windows: play on {host}

1. Register at https://{host}:18080/register (your name, an email address, a password you use nowhere else; if the
   page asks for an invite code, the server's host gives you one). The browser warns about the server's own certificate: continue,
   that is expected.
2. Unzip this. DeCombat runs on your own copy of the original Combat Arms EU client files (2009). We don't provide
   them or link to them. Point DeCombat at the folder of your existing install: put that folder (the one with
   Engine.exe and a Game folder) into the unzipped folder, next to DeCombat.bat. Nothing of the game is included
   and your folder is never changed.
3. Double-click **DeCombat.bat** (see "First start" below if Windows warns you).
4. The first start asks for your account name (kept in %APPDATA%\\DeCombat\\launcher-name.txt; delete that file to
   be asked again). The game opens full screen on the login screen with the name filled in: type your password,
   Enter. Pick the server and channel, create or join a room. Alt+Enter switches to a window; in a match Escape
   frees the mouse and Escape again leaves. Forgot the password or the login ID: the buttons on the login screen
   mail you a code.

{fpline}

## First start: Windows may warn you

DeCombat is a new program and its exe is not code-signed, so Windows is careful with it.

- **SmartScreen** says "Windows protected your PC": click "More info", then "Run anyway".
- **Windows Defender** may flag the exe as `Trojan:Win32/Wacatac.B!ml`. That is a known false positive for new
  unsigned programs (the same happens to other open-source game projects). The build has been submitted to
  Microsoft. If you want to be sure the file is the one we published, check it as below.
- **Check your download.** Every release has a `SHA256SUMS` file on the release page
  (https://github.com/argallo/DeCombat/releases). In PowerShell, in the folder of the zip:
  `Get-FileHash .\\{name}.zip` and compare the hash with the line for this zip in `SHA256SUMS`.

## Beta: feedback and resets

This is a beta: accounts and progress may be reset before the full launch.

- Discord: https://discord.gg/hNWcXpTST
- Mail: contact@decombat.com
- In the game: the **Report a Bug** form in the menu, or type `/report` in the chat.

Problems: "Put your Combat Arms folder ..." means the folder is not next to DeCombat.bat or has no Game inside.
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
{("set " + chr(34) + "DC_SERVER_FINGERPRINT=" + a.fingerprint + chr(34)) if a.fingerprint else "rem no pinned server certificate"}
"%BOT%" swarm --server "{server}" --rom "%ROM%" --count %COUNT% %FOLLOW% %3 %4 %5 %6 %7 %8 %9
if errorlevel 1 pause
endlocal
"""
    if a.botclient:
        readme += f"""
## Bots

Bots are a separate download, {name}-bots.zip, so the game itself stays a single small program. Unzip it into the
same place as this one: its files land next to DeCombat.bat. Then `bots.bat 3 YourNick` starts three bots on your PC
that sit in whatever room you sit in on {host}; `bots.bat stop` ends them (at most 7, Ctrl-C works too). They earn
nothing, you are paid as usual. More in docs/BOTS.md of the source.
"""
    os.makedirs(a.out, exist_ok=True)
    zpath = os.path.join(a.out, name + ".zip")
    if os.path.exists(zpath):
        os.remove(zpath)
    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as z:
        z.write(a.client, f"{name}/dc-client.exe")
        z.writestr(f"{name}/DeCombat.bat", bat.replace("\n", "\r\n"))
        z.writestr(f"{name}/README.md", readme)
        z.writestr(f"{name}/pack.json", pack)
    size = os.path.getsize(zpath) / 1e6
    print(f"wrote {zpath} ({size:.1f} MB); server {server}; pin: {a.fingerprint or 'none (trust on first use)'}")
    if update_dir:
        # the zip first, latest.json last (replaced in one step): a pack never reads a latest.json whose zip is not there
        os.makedirs(update_dir, exist_ok=True)
        target = os.path.join(update_dir, name + ".zip")
        if os.path.abspath(zpath) != os.path.abspath(target):
            shutil.copyfile(zpath, target)
        tmp = os.path.join(update_dir, "latest.json.part")
        with open(tmp, "w", encoding="utf-8", newline="\n") as f:
            json.dump({"commit": commit, "zip": name + ".zip"}, f, indent=2)
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
