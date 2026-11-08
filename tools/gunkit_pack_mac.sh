#!/usr/bin/env bash
# gunkit_pack_mac.sh: the macOS GunKit package (DC_DEFAULTS.md VAR-8): a zip with "DeCombat GunKit.app" (the camo studio's window as
# the bundle's executable, the console dc-gunkit next to it inside Contents/MacOS so an agent's MCP server can open the window) and
# a README with the first-run notes. Nothing of the game (ROM) and nothing of this machine goes in.
#
#   tools/gunkit_pack_mac.sh [--bin-dir DIR] [--x86-dir DIR] [--out DIR] [--name NAME]
#
# --bin-dir DIR  where the two binaries of this Mac's architecture are (default target/release, else
#                target/aarch64-apple-darwin/release; the debug folder works for a local test)
# --x86-dir DIR  the x86_64 build of the same binaries (default target/x86_64-apple-darwin/release when it exists); each binary
#                present in both folders is joined with lipo into one universal binary
# --out DIR      where the zip goes (default out_gunkit/, ignored by git)
# --name NAME    the zip's base name (default DeCombat-GunKit-mac-<commit>)
# --examples DIR example packs to ship as examples/ next to the binaries (tools/gunkit_examples.py fills one; VAR-13)
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
BIN=""; X86=""; OUT="$HERE/out_gunkit"; NAME=""; EXAMPLES=""
while [ $# -gt 0 ]; do
  case "$1" in
    --bin-dir) BIN=$2; shift 2 ;;
    --x86-dir) X86=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --name) NAME=$2; shift 2 ;;
    --examples) EXAMPLES=$2; shift 2 ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
done
COMMIT=$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo unknown)
[ -n "$NAME" ] || NAME="DeCombat-GunKit-mac-$COMMIT"
if [ -z "$BIN" ]; then
  if [ -x "$HERE/target/release/dc-gunkit" ]; then BIN="$HERE/target/release"
  else BIN="$HERE/target/aarch64-apple-darwin/release"; fi
fi
[ -n "$X86" ] || X86="$HERE/target/x86_64-apple-darwin/release"
for b in dc-gunkit dc-gunkit-gui; do
  [ -x "$BIN/$b" ] || { echo "no $b in $BIN: cargo build --release -p dc-gunkit --bins first" >&2; exit 2; }
done

STAGE="$(mktemp -d)/$NAME"
APP="$STAGE/DeCombat GunKit.app"
MACOS="$APP/Contents/MacOS"
mkdir -p "$MACOS" "$APP/Contents/Resources"
ARCHS="$(uname -m) only"
[ ! -x "$X86/dc-gunkit" ] || ARCHS="Apple Silicon and Intel (universal)"
for b in dc-gunkit dc-gunkit-gui; do
  if [ -x "$X86/$b" ]; then
    lipo -create "$BIN/$b" "$X86/$b" -output "$MACOS/$b"
  else
    cp "$BIN/$b" "$MACOS/$b"
  fi
  chmod 0755 "$MACOS/$b"
  # the linker's ad-hoc signature does not survive lipo on the arm64 slice: sign each binary ad hoc again
  codesign -s - -f "$MACOS/$b" 2>/dev/null || true
done

N_EXAMPLES=0
if [ -n "$EXAMPLES" ]; then
  for d in "$EXAMPLES"/*/; do
    [ -f "$d/manifest.json" ] || continue
    mkdir -p "$MACOS/examples"
    cp -R "$d" "$MACOS/examples/"
    N_EXAMPLES=$((N_EXAMPLES + 1))
  done
  [ "$N_EXAMPLES" -gt 0 ] || { echo "no pack folders (manifest.json) under $EXAMPLES" >&2; exit 2; }
fi

ICON_PNG="$HERE/assets/icon/decombat_logo.png"
ICON_KEY=""
if [ -f "$ICON_PNG" ] && command -v iconutil >/dev/null 2>&1; then
  ICONSET="$(mktemp -d)/gunkit.iconset"
  mkdir -p "$ICONSET"
  for sz in 16 32 128 256 512; do
    sips -z "$sz" "$sz" "$ICON_PNG" --out "$ICONSET/icon_${sz}x${sz}.png" >/dev/null
    dbl=$((sz * 2))
    sips -z "$dbl" "$dbl" "$ICON_PNG" --out "$ICONSET/icon_${sz}x${sz}@2x.png" >/dev/null
  done
  if iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/gunkit.icns" 2>/dev/null; then
    ICON_KEY="  <key>CFBundleIconFile</key><string>gunkit</string>"
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
  <key>CFBundleName</key><string>DeCombat GunKit</string>
  <key>CFBundleDisplayName</key><string>DeCombat GunKit</string>
  <key>CFBundleIdentifier</key><string>org.decombat.gunkit</string>
  <key>CFBundleVersion</key><string>$COMMIT</string>
  <key>CFBundleShortVersionString</key><string>0.1</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>dc-gunkit-gui</string>
$ICON_KEY
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST
codesign -s - -f --deep "$APP" 2>/dev/null || true

cat > "$STAGE/README.md" <<README
# DeCombat GunKit for macOS

A small studio for designing camos on the game's own guns. Pick a gun, turn it with the mouse, and paint it from the window or
live from an AI agent. It draws with the game's renderer and paints with the game's recipe, so what you see is what players get.

## First run

1. Unzip this anywhere and double-click **DeCombat GunKit.app**. The first time macOS says it cannot verify the developer: open
   System Settings > Privacy & Security, scroll down, click **"Open Anyway"**, then start the app again.
   (Terminal alternative: \`xattr -dr com.apple.quarantine "DeCombat GunKit.app"\` once.)
2. **Choose your Combat Arms folder** when it asks: your own client folder, the one that holds Engine.exe and a Game folder.
   Nothing of the game is in this package, and the folder is only read.
3. Pick a gun, click a pattern, and drag the gun to turn it. Drop your own PNG on the window to use it: save over the file and the
   gun updates by itself.
4. **Save pack** writes a Vanilla++ pack to Documents/DeCombat Camos and checks it.

## With an AI agent

\`\`\`
claude mcp add dc-gunkit -- "/path/to/DeCombat GunKit.app/Contents/MacOS/dc-gunkit" mcp
\`\`\`

The window's "Connect an agent" section shows this with the right path, ready to copy. The agent's changes appear on the gun as
you watch, and it opens the window if it is not open. The playbook is docs/GUNKIT_AGENTS.md in the repository.

Build $COMMIT, $ARCHS, macOS 12 or later.
README

mkdir -p "$OUT"
ZIP="$OUT/$NAME.zip"
rm -f "$ZIP"
(cd "$(dirname "$STAGE")" && zip -q -r "$ZIP" "$NAME")
rm -rf "$(dirname "$STAGE")"
echo "wrote $ZIP ($(du -h "$ZIP" | cut -f1)); $ARCHS; binaries: dc-gunkit-gui dc-gunkit"
