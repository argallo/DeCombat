#!/usr/bin/env bash
# mac_notarise.sh: signs the app inside a package zip with a Developer ID and notarises it, replacing the zip. Optional:
# with no DC_MAC_* secret set it warns and leaves the ad hoc signed zip as it is (docs/RELEASES.md "Signing").
#
#   bash tools/mac_notarise.sh ZIP...
#
# DC_MAC_CERT_P12 (base64 of a Developer ID Application .p12), DC_MAC_CERT_PASSWORD, DC_MAC_SIGN_IDENTITY ("Developer
# ID Application: ..."), DC_MAC_NOTARY_APPLE_ID, DC_MAC_NOTARY_PASSWORD (an app-specific password), DC_MAC_NOTARY_TEAM.
# Some set and some not is a mistake and fails.
set -euo pipefail
VARS="DC_MAC_CERT_P12 DC_MAC_CERT_PASSWORD DC_MAC_SIGN_IDENTITY DC_MAC_NOTARY_APPLE_ID DC_MAC_NOTARY_PASSWORD DC_MAC_NOTARY_TEAM"
SET=0; TOTAL=0
for v in $VARS; do TOTAL=$((TOTAL + 1)); [ -z "${!v:-}" ] || SET=$((SET + 1)); done
if [ $SET -eq 0 ]; then
  echo "::warning::no DC_MAC_* secrets: the macOS zips are ad hoc signed and not notarised, so Gatekeeper asks the player to allow them (docs/RELEASES.md)"
  exit 0
fi
[ $SET -eq $TOTAL ] || { echo "::error::only $SET of the $TOTAL DC_MAC_* secrets are set: $VARS"; exit 1; }

WORK=$(mktemp -d)
KC="$WORK/signing.keychain-db"
KC_PASS=$(uuidgen)
trap 'security delete-keychain "$KC" 2>/dev/null || true; rm -rf "${WORK:?}"' EXIT
security create-keychain -p "$KC_PASS" "$KC"
security set-keychain-settings -lut 3600 "$KC"
security unlock-keychain -p "$KC_PASS" "$KC"
printf '%s' "$DC_MAC_CERT_P12" | base64 -D > "$WORK/cert.p12"
security import "$WORK/cert.p12" -k "$KC" -P "$DC_MAC_CERT_PASSWORD" -T /usr/bin/codesign >/dev/null
rm -f "${WORK:?}/cert.p12"
security set-key-partition-list -S apple-tool:,apple: -s -k "$KC_PASS" "$KC" >/dev/null
# shellcheck disable=SC2046
security list-keychains -d user -s "$KC" $(security list-keychains -d user | tr -d '"')

for zip in "$@"; do
  x="$WORK/x"
  rm -rf "${WORK:?}/x"; mkdir -p "$x"
  ditto -x -k "$zip" "$x"
  app=$(find "$x" -maxdepth 2 -name '*.app' -type d | head -n 1)
  [ -n "$app" ] || { echo "::error::no .app in $zip"; exit 1; }
  while IFS= read -r f; do
    if file -b "$f" | grep -q 'Mach-O'; then
      codesign --force --options runtime --timestamp --keychain "$KC" -s "$DC_MAC_SIGN_IDENTITY" "$f"
    fi
  done < <(find "$app/Contents/MacOS" -type f)
  codesign --force --options runtime --timestamp --keychain "$KC" -s "$DC_MAC_SIGN_IDENTITY" "$app"
  ditto -c -k --keepParent "$app" "$WORK/submit.zip"
  xcrun notarytool submit "$WORK/submit.zip" --apple-id "$DC_MAC_NOTARY_APPLE_ID" --password "$DC_MAC_NOTARY_PASSWORD" \
    --team-id "$DC_MAC_NOTARY_TEAM" --wait
  xcrun stapler staple "$app"
  top=$(cd "$x" && ls)
  abs=$(cd "$(dirname "$zip")" && pwd)/$(basename "$zip")
  (cd "$x" && ditto -c -k --keepParent "$top" "$abs.new")
  mv -f "$abs.new" "$abs"
  echo "notarised $zip"
done
