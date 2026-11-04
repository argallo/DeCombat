#!/usr/bin/env bash
# ios_run.sh: builds the iPad app and puts it on a paired iPad (or the simulator), with the config and, on request,
# the 2009 client folder in its Documents. Needs Xcode (xcodebuild, devicectl), xcodegen (brew) and the Rust target
# aarch64-apple-ios (aarch64-apple-ios-sim for --simulator). Signing: automatic provisioning with your team id, from
# --team, else $DC_IOS_TEAM, else the first line of ~/.config/decombat/ios_team (a free Apple ID's app expires after
# 7 days: run the script again).
#
#   tools/ios_run.sh --device UDID [--server HOST[:PORT]] [--fingerprint SHA256] [--name NICK] [--rom DIR]
#                    [--team ID] [--no-install] [--launch] [--simulator]
#
# --device      the iPad's UDID (`xcrun devicectl list devices`); with --simulator a simulator UDID (`xcrun simctl list`)
# --server      writes Documents/decombat.toml on the device with the lobby address (port 10000 without one)
# --fingerprint the lobby's certificate fingerprint for decombat.toml (optional; else trust on first login)
# --name        the player name for decombat.toml (optional; the login screen asks otherwise)
# --rom DIR     copies the 2009 client folder DIR to Documents/EU-ROM on the device (2.9 GB: minutes over USB; once)
# --team ID     the Apple team id for signing (Xcode > Settings > Accounts); not needed with --simulator
# --no-install  build only
# --launch      start the app after the install, streaming its console here
# --simulator   build for the iOS simulator and install there (no signing needed)
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
DEVICE=""; SERVER=""; FP=""; NICK=""; ROM=""; INSTALL=1; LAUNCH=0; SIM=0; TEAM="${DC_IOS_TEAM:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    --device) DEVICE="$2"; shift 2 ;;
    --server) SERVER="$2"; shift 2 ;;
    --fingerprint) FP="$2"; shift 2 ;;
    --name) NICK="$2"; shift 2 ;;
    --rom) ROM="$2"; shift 2 ;;
    --team) TEAM="$2"; shift 2 ;;
    --no-install) INSTALL=0; shift ;;
    --launch) LAUNCH=1; shift ;;
    --simulator) SIM=1; shift ;;
    -h|--help) sed -n 2,22p "$0"; exit 0 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done
if [ $SIM = 1 ]; then TRIPLE=aarch64-apple-ios-sim; SDK=iphonesimulator; else TRIPLE=aarch64-apple-ios; SDK=iphoneos; fi
BUNDLE=org.decombat.client
TEAM_FILE="$HOME/.config/decombat/ios_team"
if [ -z "$TEAM" ] && [ -f "$TEAM_FILE" ]; then TEAM="$(head -n 1 "$TEAM_FILE" | tr -d ' \r')"; fi
if [ $SIM = 0 ] && [ -z "$TEAM" ]; then
  echo "no signing team: pass --team ID, set DC_IOS_TEAM, or put the id in $TEAM_FILE (Xcode > Settings > Accounts shows it)" >&2
  exit 2
fi

echo "== cargo: the Rust static library for $TRIPLE"
( cd "$HERE" && cargo build --release --target "$TRIPLE" -p dc-ios-app )
LIBDIR="$HERE/target/$TRIPLE/release"
[ -f "$LIBDIR/libdc_ios_app.a" ] || { echo "no $LIBDIR/libdc_ios_app.a" >&2; exit 1; }

echo "== xcodegen + xcodebuild ($SDK)"
( cd "$HERE/ios" && xcodegen generate --spec project.yml --quiet )
DERIVED="$HERE/build/ios"
# a device build names the iPad itself: Xcode registers it with the team and puts it in the profile (a profile made
# for "generic iOS" does not carry the device and the install fails with "integrity could not be verified")
if [ $SIM = 1 ]; then DEST="generic/platform=iOS Simulator"; SIGN=(CODE_SIGN_IDENTITY= CODE_SIGNING_REQUIRED=NO CODE_SIGNING_ALLOWED=NO)
elif [ -n "$DEVICE" ]; then DEST="id=$DEVICE"; SIGN=(-allowProvisioningUpdates -allowProvisioningDeviceRegistration "DEVELOPMENT_TEAM=$TEAM")
else DEST="generic/platform=iOS"; SIGN=(-allowProvisioningUpdates "DEVELOPMENT_TEAM=$TEAM"); fi
xcodebuild -project "$HERE/ios/DeCombat.xcodeproj" -scheme DeCombat -configuration Release -sdk "$SDK" \
  -destination "$DEST" -derivedDataPath "$DERIVED" "${SIGN[@]}" \
  RUST_LIB_DIR="$LIBDIR" build 2>&1 | grep -E 'error:|Signing Identity|Provisioning Profile:|BUILD (SUCCEEDED|FAILED)' || true
if [ $SIM = 1 ]; then APP="$DERIVED/Build/Products/Release-iphonesimulator/DeCombat.app"; else APP="$DERIVED/Build/Products/Release-iphoneos/DeCombat.app"; fi
[ -d "$APP" ] || { echo "no app bundle at $APP (see the xcodebuild output above)" >&2; exit 1; }
echo "app: $APP"
[ $INSTALL = 1 ] || exit 0
[ -n "$DEVICE" ] || { echo "--device UDID is required to install (xcrun devicectl list devices)" >&2; exit 2; }

CONF="$(mktemp -d)/decombat.toml"
{
  [ -z "$SERVER" ] || echo "server = \"$SERVER\""
  [ -z "$FP" ] || echo "fingerprint = \"$FP\""
  [ -z "$NICK" ] || echo "name = \"$NICK\""
  echo 'rom = "EU-ROM"'
} > "$CONF"

if [ $SIM = 1 ]; then
  echo "== simulator install"
  xcrun simctl boot "$DEVICE" 2>/dev/null || true
  xcrun simctl install "$DEVICE" "$APP"
  DOCS="$(xcrun simctl get_app_container "$DEVICE" "$BUNDLE" data)/Documents"
  mkdir -p "$DOCS"
  [ -z "$SERVER" ] || cp "$CONF" "$DOCS/decombat.toml"
  if [ -n "$ROM" ]; then echo "== copying the ROM into the simulator's Documents"; rsync -a "$ROM/" "$DOCS/EU-ROM/"; fi
  [ $LAUNCH = 1 ] && xcrun simctl launch --console "$DEVICE" "$BUNDLE"
  exit 0
fi

echo "== device install ($DEVICE)"
xcrun devicectl device install app --device "$DEVICE" "$APP"
if [ -n "$SERVER" ]; then
  xcrun devicectl device copy to --device "$DEVICE" --source "$CONF" --destination Documents/decombat.toml \
    --domain-type appDataContainer --domain-identifier "$BUNDLE"
fi
if [ -n "$ROM" ]; then
  echo "== copying the ROM to the iPad's Documents/EU-ROM (2.9 GB; this takes a while)"
  xcrun devicectl device copy to --device "$DEVICE" --source "$ROM" --destination Documents/EU-ROM \
    --domain-type appDataContainer --domain-identifier "$BUNDLE"
fi
if [ $LAUNCH = 1 ]; then
  xcrun devicectl device process launch --device "$DEVICE" --console "$BUNDLE"
fi
