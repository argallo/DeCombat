#!/usr/bin/env bash
# mapkit_pack_mac.sh: the macOS MapKit package (docs/MAPKIT_PLAN.md 6.6, DC_DEFAULTS.md MAP-73): a zip with
# "DeCombat MapKit.app" (the map builder's GUI as the bundle's executable, the console dc-mapkit and the three game
# binaries dc-lobby-server, dc-dserver, dc-client next to it inside Contents/MacOS so `dc-mapkit play` finds them
# beside its own exe) and a README with the first-run notes. The counterpart of tools/friend_pack_mac.sh; nothing
# of the game (ROM) and nothing of this machine goes in. First written on a Mac on 2026-11-01.
#
#   tools/mapkit_pack_mac.sh [--bin-dir DIR] [--x86-dir DIR] [--out DIR] [--name NAME]
#
# --bin-dir DIR  where the five binaries of this Mac's architecture are (default target/release, else
#                target/aarch64-apple-darwin/release; the debug folder works for a local test)
# --x86-dir DIR  the x86_64 build of the same binaries (default target/x86_64-apple-darwin/release when it exists;
#                `cargo build --release --target x86_64-apple-darwin -p dc-mapkit -p dc-lobby-server -p dc-dserver
#                -p dc-client --bins`); each binary present in both folders is joined with lipo into one universal
#                binary, so the zip runs on Apple Silicon and Intel Macs alike
# --out DIR      where the zip goes (default out_mapkit/, ignored by git)
# --name NAME    the zip's base name (default DeCombat-MapKit-mac-<commit>)
# The GUI binary dc-mapkit-gui is the bundle's executable; when the build has none yet (the console crate alone),
# the console dc-mapkit stands in with a warning, so the package can be tested before the GUI lands.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
BIN=""; X86=""; OUT="$HERE/out_mapkit"; NAME=""
while [ $# -gt 0 ]; do
  case "$1" in
    --bin-dir) BIN=$2; shift 2 ;;
    --x86-dir) X86=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --name) NAME=$2; shift 2 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
done
COMMIT=$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo unknown)
[ -n "$NAME" ] || NAME="DeCombat-MapKit-mac-$COMMIT"
if [ -z "$BIN" ]; then
  if [ -x "$HERE/target/release/dc-mapkit" ]; then BIN="$HERE/target/release"
  else BIN="$HERE/target/aarch64-apple-darwin/release"; fi
fi
[ -n "$X86" ] || X86="$HERE/target/x86_64-apple-darwin/release"
[ -x "$BIN/dc-mapkit" ] || { echo "no dc-mapkit in $BIN: cargo build --release -p dc-mapkit --bins first" >&2; exit 2; }

# DC-DEFAULT: MAP-73 (the five binaries of the package; the GUI one leads)
GUI="dc-mapkit-gui"
GAME="dc-lobby-server dc-dserver dc-client"
EXEC="$GUI"
if [ ! -x "$BIN/$GUI" ]; then
  echo "warning: no $GUI in $BIN; the console dc-mapkit is the bundle's executable" >&2
  EXEC="dc-mapkit"
  GUI=""
fi
for b in $GAME; do
  [ -x "$BIN/$b" ] || { echo "no $b in $BIN: cargo build --release -p ${b} first (play needs it next to the exe)" >&2; exit 2; }
done

STAGE="$(mktemp -d)/$NAME"
APP="$STAGE/DeCombat MapKit.app"
MACOS="$APP/Contents/MacOS"
mkdir -p "$MACOS" "$APP/Contents/Resources"
ARCHS="$(uname -m) only"
[ ! -x "$X86/dc-mapkit" ] || ARCHS="Apple Silicon and Intel (universal)"
for b in dc-mapkit $GUI $GAME; do
  if [ -x "$X86/$b" ]; then
    lipo -create "$BIN/$b" "$X86/$b" -output "$MACOS/$b"
  else
    [ "$ARCHS" = "$(uname -m) only" ] || echo "warning: $b has no x86_64 build in $X86; that one is $(uname -m) only" >&2
    cp "$BIN/$b" "$MACOS/$b"
  fi
  chmod 0755 "$MACOS/$b"
  # the linker's ad-hoc signature does not survive lipo on the arm64 slice: sign each binary ad hoc again
  codesign -s - -f "$MACOS/$b" 2>/dev/null || true
done

# the app icon: assets/icon/mapkit_logo.png rendered into an .icns at every size the Finder uses (the same
# pipeline as the friend package)
ICON_PNG="$HERE/assets/icon/mapkit_logo.png"
ICON_KEY=""
if [ -f "$ICON_PNG" ] && command -v iconutil >/dev/null 2>&1; then
  ICONSET="$(mktemp -d)/mapkit.iconset"
  mkdir -p "$ICONSET"
  for sz in 16 32 128 256 512; do
    sips -z "$sz" "$sz" "$ICON_PNG" --out "$ICONSET/icon_${sz}x${sz}.png" >/dev/null
    dbl=$((sz * 2))
    sips -z "$dbl" "$dbl" "$ICON_PNG" --out "$ICONSET/icon_${sz}x${sz}@2x.png" >/dev/null
  done
  if iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/mapkit.icns" 2>/dev/null; then
    ICON_KEY="  <key>CFBundleIconFile</key><string>mapkit</string>"
  else
    echo "warning: iconutil failed; the app keeps the default icon" >&2
  fi
  rm -rf "$(dirname "$ICONSET")"
fi
# the GUI binary is the bundle's executable directly (no launcher script): LaunchServices starts it with no
# arguments, which is the editor on the last project or the welcome page
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>DeCombat MapKit</string>
  <key>CFBundleDisplayName</key><string>DeCombat MapKit</string>
  <key>CFBundleIdentifier</key><string>org.decombat.mapkit</string>
  <key>CFBundleVersion</key><string>$COMMIT</string>
  <key>CFBundleShortVersionString</key><string>0.1</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>$EXEC</string>
$ICON_KEY
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST
codesign -s - -f --deep "$APP" 2>/dev/null || true

cat > "$STAGE/README.md" <<README
# DeCombat MapKit for macOS

The community map builder for DeCombat: build a map from a text project, look at it, export it as a mod and play
it on a local vanilla++ server (lobby + game server + client on this Mac). The playbook for the tool and for AI
agents driving it is docs/MAPKIT_AGENTS.md in the repository; the GUI is the same tool with a window.

## First run

1. Unzip this anywhere (Applications, Desktop, a projects folder). Everything is inside **DeCombat MapKit.app**:
   the editor, the console tool and the three game binaries that \`play\` starts.
2. Double-click **DeCombat MapKit.app**. The first time macOS says it cannot verify the developer: open
   System Settings > Privacy & Security, scroll down, click **"Open Anyway"**, then start the app again.
   (Terminal alternative: \`xattr -dr com.apple.quarantine "DeCombat MapKit.app"\` once.)
3. **Choose the ROM folder** when the banner asks for it: your own 2009 Combat Arms client folder (the one that
   holds Engine.exe and a Game folder). Nothing of the game is included in this package and the folder is only
   read. The choice is remembered in ~/Library/Application Support/DeCombat/mapkit.toml; the tool also takes
   \`--rom DIR\`, the \`CA_ROM\` variable, or a client folder placed next to the app. Editing, validating and
   exporting work without a ROM; the ROM listings, the in-game views and \`play\` need it.
4. New map > starter template, change it, Build, Export, Play. \`Play\` starts a modded lobby and game server on
   this Mac and the client against them; close the client to stop everything. Logs land in the project's
   \`build/play/\`.

## The console tool and AI agents

The same binary without a window is \`"DeCombat MapKit.app/Contents/MacOS/dc-mapkit"\` (every verb takes
\`--json\`; \`dc-mapkit mcp\` serves the verbs to Claude Code and other MCP clients). Put that folder on your
\`PATH\` or give the full path in the \`.mcp.json\` snippet of docs/MAPKIT_AGENTS.md. The projects you make are
plain folders (map.json, terrain.r16, assets/) and belong in your own git repository; the exported zip under
\`build/\` is what a server hosts.

Problems: "DeCombat MapKit is damaged" or a refusal to open means the quarantine step above was skipped; a
message that the ROM folder has no Game folder means the folder picked is not the client folder itself.
Build $COMMIT, $ARCHS, macOS 12 or later.
README

mkdir -p "$OUT"
ZIP="$OUT/$NAME.zip"
rm -f "$ZIP"
(cd "$(dirname "$STAGE")" && zip -q -r "$ZIP" "$NAME")
rm -rf "$(dirname "$STAGE")"
echo "wrote $ZIP ($(du -h "$ZIP" | cut -f1)); $ARCHS; executable: $EXEC; binaries: dc-mapkit ${GUI:-(no gui)} $GAME"
