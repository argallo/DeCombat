#!/usr/bin/env bash
# ios_release.sh: the release .ipa, on a Mac with Xcode, xcodegen and the Rust target aarch64-apple-ios
# (.github/workflows/release.yml; docs/RELEASES.md "iOS"). tools/ios_run.sh stays the way to a paired iPad.
#
#   tools/ios_release.sh --label LABEL [--out DIR]
#
# All of DC_IOS_CERT_P12 (base64), DC_IOS_CERT_PASSWORD, DC_IOS_PROFILE (base64 of a .mobileprovision) and DC_IOS_TEAM
# set: a signed DeCombat-iOS-<label>.ipa, exported by the kind of the profile (development, ad hoc, enterprise or App
# Store). None set: DeCombat-iOS-<label>-unsigned.ipa, which sideloading tools sign with the user's own Apple ID. Some
# set and some not is a mistake and fails: a release must not turn unsigned by a typo.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
LABEL=""; OUT="$HERE/out_ios"
while [ $# -gt 0 ]; do
  case "$1" in
    --label) LABEL=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown option $1 (--help)" >&2; exit 2 ;;
  esac
done
[ -n "$LABEL" ] || { echo "--label is required" >&2; exit 2; }
SET=0
for v in DC_IOS_CERT_P12 DC_IOS_CERT_PASSWORD DC_IOS_PROFILE DC_IOS_TEAM; do [ -z "${!v:-}" ] || SET=$((SET + 1)); done
case $SET in
  0) SIGNED=0 ;;
  4) SIGNED=1 ;;
  *) echo "::error::only $SET of the 4 DC_IOS_* secrets are set: all or none"; exit 1 ;;
esac
TRIPLE=aarch64-apple-ios
LIBDIR="$HERE/target/$TRIPLE/release"
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
(cd "$HERE" && cargo build --locked --release --target "$TRIPLE" -p dc-ios-app)
[ -f "$LIBDIR/libdc_ios_app.a" ] || { echo "no $LIBDIR/libdc_ios_app.a" >&2; exit 1; }
(cd "$HERE/ios" && xcodegen generate --spec project.yml --quiet)
DERIVED="$HERE/build/ios"
PROJECT=(-project "$HERE/ios/DeCombat.xcodeproj" -scheme DeCombat -configuration Release -sdk iphoneos -destination "generic/platform=iOS")

if [ $SIGNED -eq 0 ]; then
  echo "::warning::no DC_IOS_* secrets: the release carries an unsigned .ipa, which installs only after a sideloading tool signs it (docs/RELEASES.md)"
  xcodebuild "${PROJECT[@]}" -derivedDataPath "$DERIVED" RUST_LIB_DIR="$LIBDIR" \
    CODE_SIGN_IDENTITY= CODE_SIGNING_REQUIRED=NO CODE_SIGNING_ALLOWED=NO build
  APP="$DERIVED/Build/Products/Release-iphoneos/DeCombat.app"
  [ -x "$APP/DeCombat" ] || { echo "no $APP/DeCombat" >&2; exit 1; }
  STAGE=$(mktemp -d)
  mkdir "$STAGE/Payload"
  cp -R "$APP" "$STAGE/Payload/"
  IPA="$OUT/DeCombat-iOS-$LABEL-unsigned.ipa"
  (cd "$STAGE" && zip -q -r -y "$IPA.new" Payload)
  mv -f "$IPA.new" "$IPA"
  rm -rf "${STAGE:?}"
  echo "wrote $IPA"
  exit 0
fi

WORK=$(mktemp -d)
KC="$WORK/signing.keychain-db"
KC_PASS=$(uuidgen)
PROFILE_DIR="$HOME/Library/MobileDevice/Provisioning Profiles"
UUID=""
cleanup() {
  security delete-keychain "$KC" 2>/dev/null || true
  if [ -n "$UUID" ]; then rm -f "${PROFILE_DIR:?}/${UUID:?}.mobileprovision"; fi
  rm -rf "${WORK:?}"
}
trap cleanup EXIT
security create-keychain -p "$KC_PASS" "$KC"
security set-keychain-settings -lut 3600 "$KC"
security unlock-keychain -p "$KC_PASS" "$KC"
printf '%s' "$DC_IOS_CERT_P12" | base64 -D > "$WORK/cert.p12"
security import "$WORK/cert.p12" -k "$KC" -P "$DC_IOS_CERT_PASSWORD" -T /usr/bin/codesign >/dev/null
rm -f "${WORK:?}/cert.p12"
security set-key-partition-list -S apple-tool:,apple: -s -k "$KC_PASS" "$KC" >/dev/null
# shellcheck disable=SC2046
security list-keychains -d user -s "$KC" $(security list-keychains -d user | tr -d '"')
printf '%s' "$DC_IOS_PROFILE" | base64 -D > "$WORK/p.mobileprovision"
security cms -D -i "$WORK/p.mobileprovision" > "$WORK/p.plist"
pb() { /usr/libexec/PlistBuddy -c "Print :$1" "$WORK/p.plist" 2>/dev/null; }
UUID=$(pb UUID || true); NAME=$(pb Name || true)
[ -n "$UUID" ] || { echo "::error::DC_IOS_PROFILE is not a provisioning profile"; exit 1; }
mkdir -p "$PROFILE_DIR"
cp "$WORK/p.mobileprovision" "$PROFILE_DIR/$UUID.mobileprovision"
if [ "$(pb ProvisionsAllDevices || true)" = true ]; then METHOD=enterprise; IDENTITY="Apple Distribution"
elif ! pb ProvisionedDevices >/dev/null; then METHOD=app-store-connect; IDENTITY="Apple Distribution"
elif [ "$(pb Entitlements:get-task-allow || true)" = true ]; then METHOD=debugging; IDENTITY="Apple Development"
else METHOD=release-testing; IDENTITY="Apple Distribution"; fi
echo "signing with profile '$NAME' ($METHOD)"
xcodebuild "${PROJECT[@]}" -derivedDataPath "$DERIVED" -archivePath "$WORK/DeCombat.xcarchive" RUST_LIB_DIR="$LIBDIR" \
  CODE_SIGN_STYLE=Manual DEVELOPMENT_TEAM="$DC_IOS_TEAM" CODE_SIGN_IDENTITY="$IDENTITY" PROVISIONING_PROFILE_SPECIFIER="$UUID" \
  archive
cat > "$WORK/export.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>method</key><string>$METHOD</string>
  <key>teamID</key><string>$DC_IOS_TEAM</string>
  <key>signingStyle</key><string>manual</string>
  <key>provisioningProfiles</key><dict><key>org.decombat.client</key><string>$UUID</string></dict>
</dict></plist>
PLIST
xcodebuild -exportArchive -archivePath "$WORK/DeCombat.xcarchive" -exportOptionsPlist "$WORK/export.plist" -exportPath "$WORK/export"
IPA="$OUT/DeCombat-iOS-$LABEL.ipa"
mv -f "$WORK"/export/*.ipa "$IPA"
echo "wrote $IPA"
