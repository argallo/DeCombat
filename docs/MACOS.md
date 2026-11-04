# macOS port

**Status.** The client, lobby, game server and bot client build and run on macOS (Apple Silicon first; the package is
universal when the Intel target is built too). Players use the `DeCombat.app` zip from `tools/friend_pack_mac.sh`; it is
unsigned, so the first start needs "Open Anyway" in Privacy & Security. Open points are in the risk list (c). The rest of
this file is the port's working record.

## Mac session 1 (2026-10-29, Apple Silicon, macOS 26.6, Xcode 26, rustc 1.99.1)

- **Builds.** `cargo build` (debug and `--release`) of `dc-client`, `dc-lobby-server`, `dc-dserver` and
  `dc-botclient` is green out of the box, the `audio` feature included (cpal / coreaudio-sys build against the
  Xcode SDK; no code change needed). Debug build of everything: 30 s on an M-series with 14 cores.
- **Tests without a ROM.** `cargo test --workspace` on the Mac: 2434 passed, 5 failed, 1 ignored after the fixes below (2431 / 10 before them). The
  first run had ten failures, all present on master before this session's edits; five were portability faults
  and are fixed, five need the ROM:
  - fixed: `tests/support::pid_alive` looked for `/proc/<pid>` (none on macOS) -> `kill -0`; the lobby's
    "server exited (exit code: N)" log line came from `ExitStatus`'s Display, which says "exit status: N" on unix
    (`net::exit_text` now formats the code itself; two e2e tests and the stale-port test read that line); the
    relay test binds 127.0.0.2, which macOS does not configure on lo0 by default (skipped with a note unless
    `sudo ifconfig lo0 alias 127.0.0.2` added it); the two dc-hangdump crumb tests read thread names through the
    Windows `sys` and are Windows-only now, like the other hang-dump tests.
  - need the ROM (the Windows machine always has `CA_ROM` set, so they never ran without one): `menu_bar_screens`
    (3: the chat combos and F2/F3 come from the ROM layout) and `menu_options` (1: the interface click cue) in
    dc-client, and `remote_turn_net` (the game server streams a player's `yaw8` only with a character from the
    ROM's tables: `Game::character_for` returns `None` without one, so the walker never turns). Re-run with
    `CA_ROM` set once the ROM is on the Mac.
- **Fixed on the Mac (code), by risk number.** Rules in `DC_DEFAULTS.md` CLI-1284..1288, AUDIO-4, LG-126.
  - 1 mouse grab: `App::capture` tries the locked grab first on macOS (winit refuses the confined grab there).
  - 2 borderless look: on macOS the full-screen look is winit's simple fullscreen (`WindowExtMacOS::
    set_simple_fullscreen`; menu bar and Dock hidden, no new Space); taken while the window is still hidden at
    creation (`open_host`) and switched by `apply_look` (left before a windowed shaping, entered after a full-screen
    one). The `--size` test window and the windowed look stay plain windows.
  - 3 Retina: `default_windowed_size` makes the 1280x720 default of the windowed look a size in points on macOS
    (scale factor times pixels; 2560x1440 physical on a scale-2 panel), still limited to nine tenths of the monitor.
  - 5 audio rate: `AudioDevice::open` keeps the device's default configuration on macOS (no 44.1 kHz request).
  - 7 name case: `dc_rez::case_fold_lookup` finds `Game/<name>.rez` ignoring ASCII case when the exact name is
    missing (case-sensitive volumes). dc-rom's sentinels stay exact.
  - 8 Cmd+Q: the event loops open without winit's default menu (`window::new_event_loop`), Cmd+Q is handled as a
    key (`is_mac_quit`): the match window closes with exit reason `cmd+q`, the menu window quits.
  - (found on the Mac) the carried window hidden by winit at every turn's exit: `window::reshow_carried` (CLI-1288).
  - 9 profile path: `~/Library/Application Support/DeCombat` on macOS (`settings::platform_profile_dir`).
  - 12 lobby: without `--dserver-exe` the lobby starts the `dc-dserver` next to its own binary when it exists.
- **With the ROM** (the user put it at `rom/EU-ROM` inside the checkout, covered by the `/rom/` ignore rule;
  `export CA_ROM="$PWD/rom/EU-ROM"`; this Mac: Apple M4 Pro, 3024x1964 Retina panel, scale 2, macOS 26.6):
  - Headless render of SnowValley from the ROM on Metal: correct picture (terrain, trees, HUD, minimap), 2.7 ms
    per frame in release. `--save-every 20` with 60 frames wrote no PNG (not chased; without it every frame is written).
  - `tools/play_local_mac.sh` works after one fix (the ROM path is made absolute: the script `cd`s into
    `local_play/` before starting the processes). Headless loopback match (`DC_LOCAL_CLIENT_EXTRA="--duration 60
    --no-sound"`): lobby, bots, the lobby-started `dc-dserver`, the pilot through login / lobby / room / match on
    WARHEAD, hits accepted, clean quit, no leftover processes. Note: `--duration` makes the client headless; a real
    window needs `--leave-after S` (or nothing) instead. macOS has no `timeout` command.
  - **Real window, map mode** (`--map SnowValley --window-secs 10`): full-screen look = simple fullscreen over the
    whole panel (window 1512x982 points = 3024x1964 pixels at 0,0; menu bar and Dock hidden), 243 fps; `--windowed`
    = bordered window, 435 fps (Immediate presentation is honoured in a windowed Metal layer); cursor "locked +
    hidden" on capture, released on exit. Risks 1-3 and 5 confirmed fixed on screen; the `--windowed` size came
    from the saved resolution (90 % of the panel), so `default_windowed_size` was not exercised on screen.
  - **Real window, menu mode: one more macOS bug, fixed (CLI-1288).** The menu's window existed at full size but was
    `onscreen false` in the window server's list from the second screen on, and the match drew into it at exactly
    60 fps (the throttle for hidden layers; this was risk 4's "vsync-like" symptom, not Metal pacing). Cause: winit's
    macOS backend closes every window whenever an event-loop run exits (`notify_windows_of_exit`, winit issue
    4135), and the menu and the match each run `run_app_on_demand` once per turn with the one window carried
    across. Fix: `window::reshow_carried` in the menu's and the match's `resumed` when they take a carried window
    over. After it: window on screen from the login screen on, match 451 fps mean (median 1.9 ms).
  - Audio on the Mac: the device's default configuration is 44.1 kHz F32 on the MacBook Pro speakers; callbacks
    every 10.7 ms, latency 10.7 ms, no late callbacks over a 30 s match.
  - Screen captures: `screencapture` shows other apps' windows only with the Screen Recording permission for the
    terminal app; the window server's list (`CGWindowListCopyWindowInfo`, a 15-line Swift tool) needs none and was
    the proof above.
- **ROM-backed tests on the Mac** (`CA_ROM` set, full workspace): 2433 passed, 6 failed, 1 ignored. The five
  no-ROM failures above pass with the ROM. Of the six: `menu_charinfo` passed on an isolated rerun (a flake under
  the load of the window experiments); `menu_lobby::menu_logs_in_creates_a_room...` is the failure the Windows
  earlier notes list as pre-existing; the other four fail the same way with this session's edits stashed, so
  they are Mac-side and open (the Windows machine ran them with its ROM; whether they were green there is not
  recorded in this session): `audio_cues::flyby_and_team_callout_in_a_scripted_match` ("the throw relay's
  callout" never heard), `hit_nodes_rom::rom_the_client_names_head_and_torso_nodes...` (2 of 3 scripted shots
  fired), `lobby_nav::rom_lobby_navigation_by_the_real_widgets` (no profile box for "NavUser"),
  `scoreboard::loopback_result_screen_lists_every_player...` (no result screen). All four are scripted loopback
  matches or menu drives that miss one timed event; a common cause in the headless real-time path or the pilot's
  timing on macOS is the first thing to look for.
- **User's first play, bug 1 (fixed):** the arrow cursor stayed visible over the match until Escape opened the
  leave box and Cancel took the cursor again (reproduced with `screencapture -C`: arrow over the snow while the
  log said "locked + hidden"). winit's `set_cursor_visible(false)` on macOS only swaps the view's cursor rect,
  which AppKit applies on the next cursor update; a locked mouse never moves the cursor, so none came. Fix:
  `window::mac_cursor_hidden` hides the system cursor through `NSCursor::hide` on capture and unhides it on
  release; the AppKit call is `dc_render::system_cursor_hidden` in `macwin.rs` (dc-client forbids unsafe code), with
  a new macOS-only dependency of dc-render on `objc2-app-kit` (winit's version).
- **User's first play, bug 2 (fixed):** entering a match from the lobby showed no loading page; the window went to
  the desktop and came back as the match. Measured with the window-server list at 10 Hz: the window never left
  the screen, but its alpha went 1 -> 0 -> 1 over about 0.3 s at every turn hand-over (lobby -> loading page ->
  match -> room): macOS animates winit's close at loop exit as a fade-out and the re-show as a fade-in. Two
  changes (CLI-1288 amended): the ending turn re-shows the window in its own `exiting` (the same pass as the
  close), and the window's AppKit animations are off from creation (`dc_render::window_animations_off` through the
  raw AppKit handle, `NSWindowAnimationBehaviorNone`). After both the alpha trace is flat at 1 through the whole
  flow. (`macwin.rs` now holds both AppKit calls of dc-render; objc2-app-kit features NSWindow / NSView /
  NSResponder added, plus objc2 for the retained pointer.)
- **macOS friend package** (`tools/friend_pack_mac.sh`): `DeCombat.app`, a bundle whose executable is a shell
  launcher next to the universal client (arm64 + x86_64 through `lipo`, the bundle signed ad hoc). Double-click:
  the launcher finds the 2009 client folder next to the app (a folder with `Engine.exe` and `Game/`), asks for the
  account name once (`~/Library/Application Support/DeCombat/launcher-name.txt`), starts the game full screen
  with the server and the certificate fingerprint preset, logs to `~/Library/Logs/DeCombat`, and shows a box when
  the folder is missing or the game ends with an error. Tested through `open DeCombat.app` (LaunchServices, as a
  double-click) against the VPS. Not signed by Apple: a downloaded copy needs "Open Anyway" once (README). Intel
  Macs: untested on real hardware (the x86_64 slice is a cross-build from Apple Silicon with the same SDK).
- **One more Mac-side test failure after the master merge of 2026-10-30:** dc-botclient
  `tls::tests::the_store_is_written_through_a_temporary_file_and_a_failure_is_reported` expects a "could not save"
  warning when the store path runs through a plain file; on macOS the read before the save fails with ENOTDIR and
  `remember` reports "the saved server list is damaged" instead. Not touched here (master's TLS code; the Windows
  side owns it).
- **Not checked yet:** 6 (P3 colour: no side-by-side with Windows yet), 10 (no Mac stack sampler), 11 (device loss
  untested), 13 (key defaults in play), mouse-look feel (risk 1's acceleration) and Alt+Enter / Cmd+Q by hand: this
  session drove the client with the pilot and never pressed a key in the window. Those are the user's first play.
- **Toolchain on this Mac.** Xcode (full) at `/Applications/Xcode.app`; rustup installed to `~/.rustup` /
  `~/.cargo` with stable 1.99.1 (matches Windows), clippy and rustfmt; `source ~/.cargo/env` in a shell that was
  open before the install. Homebrew 7 is present but not needed.

## Mac session 1, continued (2026-10-31): heat and smoothness in play

- **Fans: the client drew 300..500 frames per second on a 120 Hz panel.** The match report of a VPS game read
  `Apple M4 Pro (Metal, IntegratedGpu) (Immediate), 104191 frames in 206 s = 505 fps`. Local WARHEAD match with 2
  bots, sampled once a second (`ps` CPU of dc-client): Immediate 290..300 fps at 55 % of a core; `--vsync` (Fifo)
  119.5 fps at 38 %, frame median 8.35 ms, p99 10.2 ms; `--fps-cap-match 120` with Immediate 119.6 fps at 36 % but
  the pacer's clock is not the panel's (a doubled or dropped frame as they drift). macOS now presents Fifo by default
  (CLI-1575, `window::vsync_default`); `--no-vsync` gives the old behaviour. The ioreg "Device Utilization %" rose
  with fewer frames (58 to 72..77 %), so it tracks GPU clock residency, not work; it is no power meter here.
- **"Laggier" after the VPS went to 30 Hz: the Mac is on Wi-Fi.** `route get` to the VPS goes over en0 (Wi-Fi 6E,
  -41 dBm); ping 20 probes: avg 85, max 190, stddev 35 ms against the Ethernet PC's 57..68. The 30 Hz tick cut the
  remote-player playout delay from 150 to 56 ms, and the Wi-Fi spikes then landed beyond it 30..100 times a minute
  per player (CLI-1331 extrapolate-then-correct hitches); 0 on loopback. CLI-1330 amended: the delay covers the
  largest lateness of the last 256 samples instead of the 95th percentile of 64 (docs/NET_LATENCY.md, last
  section). A cable is still the real fix for the Mac's own ping.
- **Checks for the next VPS match from this Mac:** `tail -3 ~/Library/Logs/DeCombat/dc-client.out` should say
  `(Fifo)` and about 120 fps; `python3 tools/net_report.py ~/Library/Logs/DeCombat/dc-client.net.jsonl` should show
  late/min near 0 and "late max" around the Wi-Fi spike size.

- **`/network` overlay confirmed on the Mac (2026-10-31).** `cargo test -p dc-client --test net_overlay` with
  `CA_ROM`: 3 of 3 pass (the ROM HUD frame with the overlay is drawn through Metal). In a real local match the step
  file typed `/network`, `/network 2` and `/network off` into the chat box (new: the `type` step and `key enter` /
  `escape` / `backspace` reach the match chat box, steps.rs) and the swapchain shots show the panel on, the player
  table, and nothing; no chat line went to the server. macOS key events from osascript were not an option: the
  terminal has no Accessibility permission ("osascript is not allowed to send keystrokes").
- **Oddity, test runs only:** with `--shot-dir` (the surface readable, COPY_SRC) the Fifo window ran at 493 fps,
  frame median 1.0 ms; without it 119.6 fps. Not chased: play never sets a shot folder.

- **Own-body jitter and doubled runners with clean playout numbers (2026-10-31, after Fifo).** The exit reports
  showed `frame 0.67..88 ms` under Fifo: the CPU frames alternate short and long around the 8.3 ms period
  (the swap chain blocks one frame, the next starts at once) while the panel shows them evenly. Everything
  simulated or interpolated at the CPU time then moves in uneven steps on an even display. CLI-1576: a frame clock
  predicts the presentation time (previous + period, bled to the wall clock) and the own sim, the remote playout,
  the effects and the sounds use it. The exit report now prints the raw short / long frame counts so the next
  match tells whether the panel is being missed.

- **Options on the Mac (2026-10-31).** The resolution row never changed a full-screen look (CLI-1280: it sizes the
  windowed look; the Mac first launch is full screen), which read as "the dropdown doesn't work". The Graphic tab's
  dropdown rows now open real lists (CLI-1597) and a resolution picked in full screen switches to a window of that
  size; a scaled full-screen render is still the perf audit's item 1. Frame Rate (CLI-1595) caps the pacer; under
  the Mac's Fifo default the panel's 120 Hz stays the ceiling, so 240 / 360 / Unlimited only matter with
  `--no-vsync`. Show FPS (CLI-1596) draws the rate top right in the match and the front end.

## MapKit app bundle (2026-11-01)

- **A second .app, `DeCombat MapKit.app`** (`tools/mapkit_pack_mac.sh`, DC_DEFAULTS.md MAP-73): the community map
  builder (docs/MAPKIT_PLAN.md). Unlike `DeCombat.app` its `CFBundleExecutable` is the GUI binary `dc-mapkit-gui`
  itself (no shell launcher: the editor finds the ROM through its own rule, MAP-54, and asks for the folder with a
  banner when none is found), with the console `dc-mapkit` and the three game binaries `dc-lobby-server`,
  `dc-dserver`, `dc-client` next to it in `Contents/MacOS`, so `dc-mapkit play` finds them beside its own exe.
  Bundle id `org.decombat.mapkit`, macOS 12 or later, Retina capable, icon from `assets/icon/mapkit_logo.png`
  through the same sips / iconutil pipeline, every binary universal when the x86_64 build exists (lipo, then an
  ad-hoc signature per binary and over the bundle, as for the friend app). Not signed by Apple either: "Open
  Anyway" once (the README in the zip). Packed locally from a debug build with `--bin-dir target/debug` (the
  console binary stands in as the executable with a warning while the GUI binary is not built); release.yml
  builds both Apple targets and packs on macos-latest.

## What was checked on Windows

- `DC_TARGET=mac python cargo_env.py cargo check --workspace --all-targets --exclude dc-client` and
  `DC_TARGET=mac python cargo_env.py cargo check -p dc-client --all-targets --no-default-features --features window`
  are green. `DC_TARGET=mac` (alias of `aarch64-apple-darwin`) is a check-only lane in `cargo_env.py`: the i686 host
  toolchain carries the Apple Silicon std, a stub C compiler stands in for the bundled SQLite build, nothing links.
- Not checkable here: the `audio` feature of dc-client (`dc-audio/device` = cpal). cpal's CoreAudio backend depends on
  `coreaudio-sys`, whose build script runs bindgen against the macOS SDK. On the Mac this needs the Xcode command line
  tools (they ship the SDK and libclang). A Windows experiment made `AudioDevice` not `Send` and dc-client still
  compiled, so cpal's macOS stream (which is not `Send`) does not break our code.
- The Windows-only code was already gated before this branch (CI builds ubuntu): `dc-hangdump` (no-op `sys` module
  on other platforms), `dc-wire/src/filetime.rs`, the lobby and bot `clock.rs`, `dc-render/src/perf.rs` (process
  memory reads `None`). This branch only made the hang-dump unit tests Windows-only.

## (a) What to pull onto the Mac

1. The git repository, branch `mac-check-r1` (or master once it is merged); the Rust workspace is the `rust/`
   folder of the repo. No Windows build output (`target/`, ``, the friend zip, `tools/msvc2022`,
   `tools/rust`) is needed. `cargo_env.py` is Windows-only: use plain `cargo` on the Mac.
2. Your own 2009 EU client folder: the ROM. Copy the whole folder (on the dev machine
   `extracted/app/client/eu-test`). `CA_ROM` (tests, lobby, dserver) and `--rom` (dc-client, dc-botclient) name the
   folder itself. The loader expects this layout, file names as in the client:
   - `Engine.exe` and `ltmsg.dll` at the top (`Engine.exe` holds the archive key, `dc-rom/src/rom.rs` `KEY_SOURCES`;
     both are sentinels of the build check);
   - `Game/` with `CShell.dll`, `CRes.dll`, `ClientFx.fxd` and every `*.rez` archive (`ATTRIBUTES.rez`, `WORLDS.rez`,
     `FONTS.rez`, ...), and the loose music in `Game/BGM/`;
   - `Profiles/` (the 2009 profile; read only, our own profile goes elsewhere, see risk 9).
   dc-client refuses a folder without `Game/` (`dc-client/src/lib.rs` `check_rom_dir`). The ROM is never written to.
3. Xcode command line tools (`xcode-select --install`): linker, SDK and libclang for `coreaudio-sys`.
4. rustup with the stable toolchain. `rust-toolchain.toml` pins `channel = "stable"` with clippy and rustfmt; the
   Windows machine builds with rustc / cargo 1.99.1 (2026-08-27). `rustup toolchain install 1.99.1` and
   `rustup override set 1.99.1` in `rust/` reproduce it exactly; plain `stable` is fine unless a build differs.

## (b) Commands

From `rust/`:

```sh
cargo build -p dc-client -p dc-lobby-server -p dc-dserver -p dc-botclient          # debug
cargo build --release -p dc-client -p dc-lobby-server -p dc-dserver -p dc-botclient # for playing
CA_ROM="$HOME/CA/eu" cargo test --workspace                                    # ROM tests run with CA_ROM set
```

Local play on loopback without the Windows launcher: `tools/play_local_mac.sh`, a transcription of the friend pack's
`play_local.bat` (HOSTING.md "Local-play package"). **Written on Windows and not yet run.**

```sh
tools/play_local_mac.sh rom/EU-ROM Player 3 TDM SnowValley      # the pilot creates and starts the room
DC_LOCAL_PILOT=0 tools/play_local_mac.sh rom/EU-ROM Player 3     # the real front end from the login screen on
```

It starts `dc-lobby-server` on 127.0.0.1 (auth port `DC_LOCAL_AUTH`, default 10000; lobby +2, clan +3, web +8080; UDP
game port `DC_LOCAL_GAME`, default 27888; database and logs under `local_play/`), passes `--dserver-exe` pointing
at the built `dc-dserver` (the lobby starts it when a match starts), waits with `nc -z` until the auth port listens,
starts the bots (`--shooter --hunt --skill 40 --matches 0 --room any`), then runs dc-client in the pilot mode of
the Windows launcher (`--menu lobby --menu-pilot --pilot-keep-result --room new --start-after <bots+1>`). On exit it stops
the bots and the lobby (and the lobby's dserver children with `pkill -P`). Binaries come from `target/release`
when built, else `target/debug` (`DC_BIN` overrides). `DC_LOCAL_CLIENT_EXTRA="--duration 60 --no-sound --out
/tmp/dc"` gives a headless client run.

## (c) macOS risk points, most likely first

Each: where, what may go wrong, the expected fix (as read on Windows), with the Mac session's status in front. Fixed on Windows: 14 and 15.

1. *(Mac session 1: locked grab first on macOS, CLI-1284; feel untested)* **Mouse look capture.** `crates/dc-client/src/window.rs:806` tries `CursorGrabMode::Confined` first; winit 0.30
   does not support Confined on macOS, so it falls back to `Locked` (expected to work). Relative motion comes from
   `DeviceEvent::MouseMotion` (`window.rs:2416`); on macOS those deltas are in points and carry the system's mouse
   acceleration, so sensitivity will feel different from Windows. Fix: try `Locked` first on macOS; if look feels
   wrong, scale the delta by the scale factor and document turning acceleration off (no raw-input API in winit).
2. *(Mac session 1: simple fullscreen on macOS, CLI-1284; untested on screen)* **Borderless look over the menu bar and the Dock.** `window.rs:506` (`host_attributes`) and `window.rs:1281`
   (`toggle_look`) cover the monitor with a borderless normal-level window; macOS keeps such a window below the menu
   bar, so the bottom or top of the frame may be cut or the Dock may cover it. Fix: on macOS use
   `set_fullscreen(Some(Fullscreen::Borderless(None)))` for the borderless look (or `set_simple_fullscreen` from
   winit's macOS extension trait), plain window for the bordered look.
3. *(Mac session 1: windowed default in points, CLI-1284)* **Retina / HiDPI.** All window sizes are physical pixels (`window.rs:537` `open_host` plans from the monitor's
   physical size; `window.rs:2359` re-requests the physical size on `ScaleFactorChanged`). On a Retina panel the
   borderless window renders at full native resolution (e.g. 3024x1964), which is correct but heavy, and the bordered
   look's `BORDERED_SIZE` 1280x720 (`window.rs:361`) becomes a quarter-area window (640x360 points). The menu maps
   physical cursor positions onto the 1024x768 virtual screen (`menu/window.rs:232`), which stays right. Fix:
   multiply `BORDERED_SIZE` by `window.scale_factor()` on macOS; optionally a render-scale option for Retina.
4. *(Mac session 1: not a pacing problem; the 60 fps seen was the hidden-window throttle, CLI-1288; 451 fps after)* **Frame pacing with Metal.** `window.rs:591` asks for `PresentMode::Immediate` and `window.rs:617` for frame latency 1;
   `pacer.rs` caps at 1000 fps in a match and `min(360, refresh)` in menus. On Metal the window server shows at
   most the display rate (60 or 120 Hz ProMotion) in a windowed (non-exclusive) layer, and with few drawables
   `get_current_texture` can block, so the profiler's frame times may look like vsync. Fix: measure first with
   `--profile`; if frames stall on the drawable, use latency 2 on macOS. The menu cap may get `None` for the refresh
   rate and fall back to 360 (harmless).
5. *(Mac session 1: device default rate on macOS, AUDIO-4)* **Audio device open.** `crates/dc-audio/src/device.rs:65-78` prefers 44.1 kHz when the device lists it; on macOS cpal
   then sets the device's nominal sample rate system-wide (Mac outputs default to 48 kHz), which changes other apps'
   audio and can fail while another app holds the device. Fix: on macOS keep the device's default rate (the mixer
   already takes any output rate, `set_output_rate`). The build needs the Xcode tools (see (a)).
6. *(open: compare shots with the ROM)* **Colour of the window surface.** `window.rs:586` picks `Bgra8Unorm` (no sRGB conversion, as on Windows). Metal
   offers it, but the CAMetalLayer then shows the pixels in the display's colour space (Display P3 on Mac panels),
   so colours may look more saturated than on Windows. Fix: set the layer's colour space to sRGB through the raw
   Metal layer (wgpu does not expose it) if the screenshots differ visibly.
7. *(Mac session 1: case-folded archive lookup, CLI-1287; this Mac's volume is case-insensitive)* **ROM file-name case.** The loader joins exact names: `crates/dc-rez/src/lib.rs:713` (`Game/<NAME>.rez`, the
   names come from code and tables, partly in other case than on disk, e.g. `GUNS_M_PV_Pistol.rez`),
   `crates/dc-rom/src/rom.rs:301` (`Engine.exe`, `Game/CShell.dll`), `crates/dc-ui-data/src/names.rs:32`. APFS is
   case-insensitive by default, so a normal Mac works. On a case-sensitive APFS volume (an option at format time;
   check with `diskutil info / | grep -i 'case'`) lookups that differ in case fail. Fix: a case-folding fallback
   like `music.rs:260` (`walk`, which already folds case) in dc-rez's archive open and dc-rom's sentinel paths; or
   keep the ROM on a case-insensitive volume.
8. *(Mac session 1: no default menu, Cmd+Q = our close, CLI-1285)* **Quit shortcut.** winit's default macOS menu gives Cmd+Q; it may end the process through the application's
   terminate path instead of `WindowEvent::CloseRequested` (`window.rs:2096`), skipping the client's own close (net
   leave, logs flush). Fix: check on the Mac; if so, install our own menu without the Quit item, or handle the
   exit in `ApplicationHandler::exiting`. Option is the Alt key (Alt+Enter toggles the look, Left Alt walks).
9. *(Mac session 1: Application Support, CLI-1286)* **Settings and log paths.** `crates/dc-client/src/settings.rs:1019` puts our profile in `DC_PROFILE_DIR`, else
   `%APPDATA%`, else `$XDG_CONFIG_HOME/decombat`, else `~/.config/decombat` (macOS lands on the last: works, not the
   usual `~/Library/Application Support/DeCombat`). Logs go where the flags say (the script uses `local_play/logs`).
   Fix: optional, add a macOS branch to `own_profile_dir`.
10. *(open, later)* **Hang reports.** `crates/dc-hangdump/src/lib.rs:447` is the non-Windows `sys`: no thread stacks, no minidump.
    `crates/dc-client/src/watchdog.rs:649` then writes stall dirs with `stall.json` and the breadcrumbs only
    ("no minidump: Windows only"). Nothing breaks. Fix (later): a macOS stack sampler, or rely on the crash reporter.
11. *(open, nothing needed)* **GPU device loss recovery.** `crates/dc-client/src/gpulost.rs` is backend-neutral (wgpu's device-lost callback
    and an unwind guard); the Windows trigger was a driver reset. Metal rarely reports device loss, so the path will
    be untested there. Fix: none needed; `DC_FAULT`-style simulation (`gpulost::simulate_loss`) can exercise it.
12. *(Mac session 1: sibling dc-dserver default, LG-126)* **Lobby starting the game server.** `crates/dc-lobby-server/src/game.rs:369` spawns the `--dserver-exe` path
    as given (portable; `.py` runs through Python). The default without the flag is the old
    path of the reference game server (`crates/dc-lobby-server/src/cli.rs:317`), so always pass `--dserver-exe`
    (the script does). Stale servers are stopped with `Child::kill` (portable). When the lobby itself is killed its
    dserver children keep running until their idle stop (`-idle`, 60 s); the script also kills them with `pkill -P`.
    Fix: optional, default to `dc-dserver` + the platform's executable suffix next to the lobby binary.
13. *(open: check in play)* **Keyboard layout keys.** Mac keyboards have no Insert / Pause and Fn changes F-keys; winit reports physical
    `KeyCode`s, so the defaults bind the same positions. Fix: only if a default binding has no Mac key.
14. *(fixed here)* Menu fonts by face name: `crates/dc-client/src/menu/sysfont.rs` looked only in the Windows font
    folders; it now also scans `/System/Library/Fonts`, `/System/Library/Fonts/Supplemental`, `/Library/Fonts` and
    `~/Library/Fonts` on macOS. A face macOS does not ship falls back to the menu's default font, as before.
15. *(fixed here)* `--adapter metal` for dc-client (`crates/dc-client/src/main.rs`), like worldview's flag; `auto`
    already picks Metal.


- Clipboard (CLI-1510..1512): menu text boxes copy / cut / paste with Cmd+C / X / V on macOS (Ctrl elsewhere) through `arboard` (window feature only); untested on a real Mac.
