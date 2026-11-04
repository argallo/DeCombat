# DeCombat on iPad

The client builds for iPadOS (target `aarch64-apple-ios`) and plays with a Bluetooth keyboard and mouse. It is built from
source, not distributed as a package. The desktop clients are untouched: everything iPad-specific is behind
`cfg(target_os = "ios")`.

## What is where

- `crates/dc-client/src/menu/window/ipad.rs`: the one UIKit event loop (`Shell`) that drives the front end, the
  loading page and every match as stages (CLI-1604). `src/ios/{entry,config,bridge}.rs`: the app entry
  (`dc_client::ios_main`), the config file, and the keyboard / mouse / touch bridge.
- `crates/dc-ios-hid`: Apple's GameController framework (GCKeyboard / GCMouse) behind a small C API in
  `src/hid.m`, the pointer lock (`prefersPointerLocked`), the USB-usage-to-KeyCode map (CLI-1621).
- `crates/dc-ios-app`: the static library the app links (`dc_ios_main`).
- `ios/`: the xcodegen spec (`project.yml`), `main.m` and `Info.plist` of the app wrapper; the generated
  `DeCombat.xcodeproj` and `build/ios/` are ignored by git.
- `tools/ios_run.sh`: cargo + xcodegen + xcodebuild + install (device or simulator), with the config and the ROM.

## Build and install

Needs Xcode (27.2 used), `brew install xcodegen`, `rustup target add aarch64-apple-ios` (and `aarch64-apple-ios-sim`
for the simulator), an Apple ID signed in to Xcode (Settings > Accounts), the iPad paired with the Mac, Developer Mode
on (Settings > Privacy & Security). The signing team is yours and stays out of the repository: Xcode's Accounts page
shows its id; put it in `~/.config/decombat/ios_team`, or set `DC_IOS_TEAM`, or pass `--team ID`.

    xcrun devicectl list devices                           # the iPad's UDID
    tools/ios_run.sh --device UDID --server HOST:10000 --fingerprint SHA256 --rom rom/EU-ROM --launch

`--rom` copies the 2009 client folder (about 3 GB) into the app's Documents once; later runs leave it out. A free
Apple ID's app expires after 7 days: run the script again. The simulator: `--simulator --device <sim UDID>`.

The app's Documents folder (Files app, or the Finder's file sharing) holds `decombat.toml`, `EU-ROM/` and `logs/`:

    server = "host:port"      # port 10000 when missing
    fingerprint = "AA:BB:..." # optional: the lobby's certificate pin
    name = "..."              # optional: fills the login ID
    rom = "EU-ROM"            # folder under Documents

## Playing

A Bluetooth keyboard and mouse paired with the iPad. The menu takes the mouse as a pointer (and touches); the match
locks the pointer (iPadOS grants the lock only when the app fills the screen and is in front: not in Stage Manager
or Split View). Keys are the desktop binds on a US layout; no key repeat yet.

## Open points

Unverified on hardware: the mouse Y sign and the scroll units from GameController, the pointer lock with winit's
view controller, key-up on disconnect. The surface is sized to the safe area (a slight stretch on Face ID iPads).
The system pointer and the ROM's drawn cursor both show in the menu. Logs: `Documents/logs` (net log, watchdog
reports; the watchdog never aborts on iOS).

## Releases

The GitHub Release carries an `.ipa`, unsigned unless the repository has the signing secrets; docs/RELEASES.md "iOS"
says what that means for a player. To put the app on your own iPad, `tools/ios_run.sh` above stays the shortest way.
