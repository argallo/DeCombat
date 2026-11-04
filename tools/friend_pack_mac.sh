#!/usr/bin/env bash
# friend_pack_mac.sh: the macOS friend package (HOSTING.md "What a friend needs"): a zip with DeCombat.app (the client
# in an app bundle: double-click starts the game, the 2009 client folder goes next to the app) and a README, preset
# to one server. Nothing of the game (ROM) and nothing of this machine
# (database, captures, paths) goes in. First written on a Mac on 2026-10-30 (docs/MACOS.md).
#
#   tools/friend_pack_mac.sh --server HOST[:PORT] [--fingerprint SHA256] [--out DIR] [--name NAME]
#
# --server      the lobby's address the launcher connects to (port 10000 without one)
# --fingerprint the lobby's certificate fingerprint (dc-lobby-server tls --db ...; the launcher pins it, CLI-1471);
#               without it the client trusts the server at the first login and remembers it
# --out DIR     where the zip goes (default out_friend/, ignored by git)
# --name NAME   the zip's base name (default DeCombat-mac-<commit>)
# The client is target/release/dc-client (this Mac's architecture); when target/x86_64-apple-darwin/release/dc-client
# exists too (cargo build --release --target x86_64-apple-darwin -p dc-client), the two are joined into one
# universal binary with lipo, so the zip runs on Apple Silicon and Intel Macs alike.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
SERVER=""; FP=""; OUT="$HERE/out_friend"; NAME=""
while [ $# -gt 0 ]; do
  case "$1" in
    --server) SERVER=$2; shift 2 ;;
    --fingerprint) FP=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --name) NAME=$2; shift 2 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
done
[ -n "$SERVER" ] || { echo "--server is required (--help)" >&2; exit 2; }
case "$SERVER" in *:*) ;; *) SERVER="$SERVER:10000" ;; esac
COMMIT=$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo unknown)
[ -n "$NAME" ] || NAME="DeCombat-mac-$COMMIT"
ARM="$HERE/target/release/dc-client"
X86="$HERE/target/x86_64-apple-darwin/release/dc-client"
[ -x "$ARM" ] || { echo "no client: cargo build --release -p dc-client first" >&2; exit 2; }

STAGE="$(mktemp -d)/$NAME"
mkdir -p "$STAGE"
if [ -x "$X86" ]; then
  lipo -create "$ARM" "$X86" -output "$STAGE/dc-client"
  ARCHS="Apple Silicon and Intel (universal)"
else
  cp "$ARM" "$STAGE/dc-client"
  ARCHS="$(uname -m) only"
fi
chmod 0755 "$STAGE/dc-client"
# the linker's ad-hoc signature does not survive lipo on the arm64 slice: sign the result ad hoc again
codesign -s - -f "$STAGE/dc-client" 2>/dev/null || true

PIN=""; [ -z "$FP" ] || PIN="--server-fingerprint $FP"
HOST="${SERVER%:*}"

# ---- DeCombat.app: double-click starts the game; the 2009 client folder sits next to the app
APP="$STAGE/DeCombat.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
mv "$STAGE/dc-client" "$APP/Contents/MacOS/dc-client"
# the app icon: assets/icon/decombat_logo.png (the user's DC logo, 1024 x 1024 with transparent corners) rendered
# into an .icns at every size the Finder uses (user request 2026-10-31)
ICON_PNG="$HERE/assets/icon/decombat_logo.png"
ICON_KEY=""
if [ -f "$ICON_PNG" ] && command -v iconutil >/dev/null 2>&1; then
  ICONSET="$(mktemp -d)/DeCombat.iconset"
  mkdir -p "$ICONSET"
  for sz in 16 32 128 256 512; do
    sips -z "$sz" "$sz" "$ICON_PNG" --out "$ICONSET/icon_${sz}x${sz}.png" >/dev/null
    dbl=$((sz * 2))
    sips -z "$dbl" "$dbl" "$ICON_PNG" --out "$ICONSET/icon_${sz}x${sz}@2x.png" >/dev/null
  done
  if iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/DeCombat.icns" 2>/dev/null; then
    ICON_KEY="  <key>CFBundleIconFile</key><string>DeCombat</string>"
  else
    echo "warning: iconutil failed; the app keeps the default icon" >&2
  fi
  rm -rf "$(dirname "$ICONSET")"
fi
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>DeCombat</string>
  <key>CFBundleDisplayName</key><string>DeCombat</string>
  <key>CFBundleIdentifier</key><string>org.decombat.client</string>
  <key>CFBundleVersion</key><string>$COMMIT</string>
  <key>CFBundleShortVersionString</key><string>0.1</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>DeCombat</string>
$ICON_KEY
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.games</string>
</dict>
</plist>
PLIST
cat > "$APP/Contents/MacOS/DeCombat" <<LAUNCH
#!/bin/bash
# DeCombat launcher (double-click DeCombat.app). The 2009 Combat Arms client folder (Engine.exe + Game/) must be
# next to the app; the game's login screen takes the name and password. Logs: ~/Library/Logs/DeCombat.
SERVER="$SERVER"
PIN="$PIN"
HERE="\$(cd "\$(dirname "\$0")/../.." && pwd)"         # the .app
BASE="\$(cd "\$HERE/.." && pwd)"                        # the folder the app sits in
say() { /usr/bin/osascript -e "display dialog \\"\$1\\" with title \\"DeCombat\\" buttons {\\"OK\\"} default button 1 with icon stop" >/dev/null 2>&1; }
CFG="\$HOME/Library/Application Support/DeCombat"
LOGS="\$HOME/Library/Logs/DeCombat"
mkdir -p "\$CFG" "\$LOGS"
log() { printf '%s %s\\n' "\$(date '+%Y-%m-%d %H:%M:%S')" "\$1" >> "\$LOGS/launcher.log"; }
log "start: app \$HERE"
# a Combat Arms folder: Game/ with the archives, and the key in Engine.exe or Game/CShell.dll
is_rom() { [ -d "\$1/Game" ] && { [ -f "\$1/Engine.exe" ] || [ -f "\$1/Game/CShell.dll" ]; }; }
find_rom_in() {
  for d in "\$1"/*/ "\$1"/*/*/; do
    [ -d "\$d" ] || continue
    case "\$d" in *.app/*) continue ;; esac
    if is_rom "\${d%/}"; then printf '%s' "\${d%/}"; return 0; fi
  done
  return 1
}
ROM=""
# 1. the folder remembered from an earlier start
if [ -f "\$CFG/rom-path.txt" ]; then
  r="\$(head -n 1 "\$CFG/rom-path.txt")"
  if [ -n "\$r" ] && is_rom "\$r"; then ROM="\$r"; fi
fi
# 2. next to the app (macOS may run a downloaded app from a hidden copy, so the usual places are tried too)
[ -n "\$ROM" ] || ROM="\$(find_rom_in "\$BASE" || true)"
[ -n "\$ROM" ] || log "not beside the app (\$BASE); looking in the usual folders"
if [ -z "\$ROM" ]; then
  for base in "\$HOME/Downloads" "\$HOME/Desktop" "\$HOME/Applications" "\$HOME/Documents" "\$HOME"; do
    [ -d "\$base" ] || continue
    for d in "\$base"/*/; do
      [ -d "\$d" ] || continue
      if [ -d "\${d}DeCombat.app" ]; then
        ROM="\$(find_rom_in "\${d%/}" || true)"
        [ -n "\$ROM" ] && break 2
      fi
    done
  done
fi
# 3. ask, and remember the answer
if [ -z "\$ROM" ]; then
  log "not found: asking"
  picked="\$(/usr/bin/osascript -e 'POSIX path of (choose folder with prompt "Pick your Combat Arms folder (the one with Engine.exe and a Game folder)")' 2>/dev/null)"
  picked="\${picked%/}"
  if [ -z "\$picked" ]; then exit 2; fi
  if ! is_rom "\$picked"; then
    say "That folder has no Game folder with the game archives inside. Pick the 2009 Combat Arms client folder itself (it holds Engine.exe and Game)."
    exit 2
  fi
  ROM="\$picked"
fi
printf '%s\\n' "\$ROM" > "\$CFG/rom-path.txt"
log "rom: \$ROM"
# the account name: asked once, kept next to the game's own settings; the login screen shows it (editable)
CFG="\$HOME/Library/Application Support/DeCombat"
mkdir -p "\$CFG"
NAME="\$(cat "\$CFG/launcher-name.txt" 2>/dev/null | tr -d '\\r\\n')"
if [ -z "\$NAME" ]; then
  NAME="\$(/usr/bin/osascript -e 'text returned of (display dialog "Your DeCombat account name (the one you registered):" default answer "" with title "DeCombat" buttons {"Cancel", "OK"} default button 2)' 2>/dev/null | tr -d '\\r\\n')"
  [ -n "\$NAME" ] || exit 2
  printf '%s\\n' "\$NAME" > "\$CFG/launcher-name.txt"
fi
xattr -d com.apple.quarantine "\$HERE/Contents/MacOS/dc-client" 2>/dev/null
# the client's working folder is the log folder: its crash-report folder (made at every start) lands there, not
# beside the app
cd "\$LOGS" || exit 2
# diagnostic reports (docs/DIAGNOSTICS.md): the last run's output stays as dc-client.prev.out
[ -f "\$LOGS/dc-client.out" ] && mv -f "\$LOGS/dc-client.out" "\$LOGS/dc-client.prev.out"
# shellcheck disable=SC2086
"\$HERE/Contents/MacOS/dc-client" --menu lobby --lobby "\$SERVER" --name "\$NAME" --rom "\$ROM" \$PIN \\
  --net-log "\$LOGS/dc-client.net.jsonl" > "\$LOGS/dc-client.out" 2>&1
rc=\$?
if [ \$rc -ne 0 ]; then
  say "DeCombat ended with an error (code \$rc). The log is in ~/Library/Logs/DeCombat/dc-client.out; send it to the host."
fi
exit \$rc
LAUNCH
chmod 0755 "$APP/Contents/MacOS/DeCombat"
codesign -s - -f --deep "$APP" 2>/dev/null || true

FPLINE="The app pins the server's certificate fingerprint ($FP): a changed server identity is refused."
[ -n "$FP" ] || FPLINE="The game trusts the server's certificate at your first login and remembers it; a changed server identity is refused later."
cat > "$STAGE/README.md" <<README
# DeCombat for macOS: play on $HOST

1. Register at https://$HOST:18080/register (your name, a password you use nowhere else; if the page asks for an
   invite code, the server's host gives you one). The browser warns about the server's own certificate: continue, that is expected.
2. Unzip this. DeCombat runs on your own copy of the original Combat Arms EU client files (2009). We don't provide
   them or link to them. Point DeCombat at the folder of your existing install: put that folder (the one with
   Engine.exe and a Game folder) into the unzipped folder, next to DeCombat.app, or pick it when the app asks.
   Nothing of the game is included and your folder is never changed.
3. Double-click **DeCombat.app**. The first time macOS says it cannot verify the developer (the app is not
   notarised yet): open System Settings > Privacy & Security, scroll down, click "Open Anyway", then start the
   app again. (Terminal alternative: \`xattr -dr com.apple.quarantine "<the unzipped folder>"\` once.)
   If the app cannot see the folder next to it (macOS runs a freshly downloaded app from a hidden copy), it
   asks you to pick your Combat Arms folder once and remembers it
   (~/Library/Application Support/DeCombat/rom-path.txt; delete that file to be asked again). macOS may also ask
   whether DeCombat may access your Downloads or Desktop folder: allow it. The app's own log is
   ~/Library/Logs/DeCombat/launcher.log.
4. The first start asks for your account name (kept in ~/Library/Application Support/DeCombat/launcher-name.txt;
   delete that file to be asked again). The game opens full screen on the login screen with the name filled in:
   type your password, Enter. Pick the server and
   channel, create or join a room. Alt+Enter switches to a window, Cmd+Q quits; in a match Escape frees the mouse
   and Escape again leaves.

$FPLINE

## Check your download

Every release has a \`SHA256SUMS\` file on the release page (https://github.com/argallo/DeCombat/releases). In
Terminal, in the folder of the zip: \`shasum -a 256 $NAME.zip\` and compare the hash with the line for this zip in
\`SHA256SUMS\`.

## Beta: feedback and resets

This is a beta: accounts and progress may be reset before the full launch.

- Discord: https://discord.gg/hNWcXpTST
- Mail: contact@decombat.com
- In the game: the **Report a Bug** form in the menu, or type \`/report\` in the chat.

Problems: a box saying the folder has no Game folder means the folder you picked is not the client folder
itself (the one that holds Engine.exe and Game). "This server does not offer an encrypted login" or a message about the server's identity: the
host updated the server; ask for a new package. Logs for the host: ~/Library/Logs/DeCombat (no password in them).
Client build $COMMIT, $ARCHS, macOS 12 or later.
README

# BOT-102: the local bots, when the bot client is built next to the client
BOT_ARM="$HERE/target/release/dc-botclient"
BOT_X86="$HERE/target/x86_64-apple-darwin/release/dc-botclient"
if [ -x "$BOT_ARM" ]; then
  if [ -x "$BOT_X86" ]; then
    lipo -create "$BOT_ARM" "$BOT_X86" -output "$STAGE/dc-botclient"
  else
    cp "$BOT_ARM" "$STAGE/dc-botclient"
  fi
  chmod 0755 "$STAGE/dc-botclient"
  codesign -s - -f "$STAGE/dc-botclient" 2>/dev/null || true
  cat > "$STAGE/bots.command" <<BOTS
#!/bin/bash
# Local bots that play with you on $HOST. Usage: ./bots.command [count] [your nick], ./bots.command stop | list.
# Anything after those goes to dc-botclient swarm (docs/BOTS.md). Ctrl-C ends them.
BASE="\$(cd "\$(dirname "\$0")" && pwd)"
BOT="\$BASE/dc-botclient"
case "\${1:-}" in stop|list) exec "\$BOT" swarm "\$1" ;; esac
is_rom() { [ -d "\$1/Game" ] && { [ -f "\$1/Engine.exe" ] || [ -f "\$1/Game/CShell.dll" ]; }; }
ROM=""
SAVED="\$HOME/Library/Application Support/DeCombat/rom-path.txt"
if [ -f "\$SAVED" ]; then r="\$(head -n 1 "\$SAVED")"; if is_rom "\$r"; then ROM="\$r"; fi; fi
if [ -z "\$ROM" ]; then
  for d in "\$BASE"/*/; do if is_rom "\${d%/}"; then ROM="\${d%/}"; break; fi; done
fi
if [ -z "\$ROM" ]; then echo "Put your Combat Arms folder next to bots.command, then start it again."; exit 2; fi
COUNT="\${1:-3}"; [ \$# -gt 0 ] && shift
FOLLOW=(); if [ \$# -gt 0 ]; then FOLLOW=(--follow "\$1"); shift; fi
[ -n "$FP" ] && export DC_SERVER_FINGERPRINT="$FP"
exec "\$BOT" swarm --server "$SERVER" --rom "\$ROM" --count "\$COUNT" "\${FOLLOW[@]+"\${FOLLOW[@]}"}" "\$@"
BOTS
  chmod 0755 "$STAGE/bots.command"
  cat >> "$STAGE/README.md" <<README

## Bots

\`./bots.command 3 YourNick\` in Terminal starts three bots on your Mac that sit in whatever room you sit in on $HOST;
\`./bots.command stop\` ends them (at most 7, Ctrl-C works too). They earn nothing, you are paid as usual.
More in docs/BOTS.md of the source.
README
fi

mkdir -p "$OUT"
ZIP="$OUT/$NAME.zip"
rm -f "$ZIP"
(cd "$(dirname "$STAGE")" && zip -q -r "$ZIP" "$NAME")
rm -rf "$(dirname "$STAGE")"
echo "wrote $ZIP ($(du -h "$ZIP" | cut -f1)); client: $ARCHS; server $SERVER; pin: ${FP:-none (trust on first use)}"
