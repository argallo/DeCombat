# Render determinism contract (gate G0)

Status: frozen with gate G0 of `docs/RENDER_PERF_GATES.md` (branch `perf-g0`, 2026-10-26). Every later render
change (G2..G6) is judged against it: its exact-lane hashes stay unchanged, or the change carries an approved visual
review and a golden update made with the tool below.

Provenance: ORIGINAL (our own engineering contract; no 2009 behaviour claims).

## 1. What is hashed

An exact-lane frame hash covers the **intended texels only**:

| item | rule |
|---|---|
| source | the read-back of the offscreen colour target after the whole frame (sky, world, models, translucent world, view models, debug boxes, effects, HUD layer, debug panel) |
| format | RGBA8 unorm (`dc_render::OFFSCREEN_FORMAT`), 4 bytes per texel, R G B A |
| layout | rows top to bottom, texels left to right, **no row padding** (the 256-byte copy alignment is stripped by `dc_render::unpack_rows`) |
| alpha | forced to 255 by the read-back (the frame's alpha is not a contract) |
| gamma | the client gamma ramp is applied (the profile gamma of the ROM, else the default), as on screen |
| digest | SHA-256 of the ASCII header `dc-texels/1 rgba8 <width>x<height>\n` followed by the texels (`dc_render::texel_hash`, `FrameImage::texel_hash`) |

Width, height and format are part of the digest and of the manifest. A PNG written by any dc-client build
(`FrameImage::save_png`, `--headless-frames --out`) holds exactly these texels, so `tools/pixhash.py` computes the same
digest from the PNG (checked: the in-process hash and the PNG hash of the same frame are equal).

The HUD-only scenes also hash the CPU compositor's image (`HeadlessReport::last_hud`, RGBA8 with its own alpha) with
the same digest rule.

## 2. Lanes

### 2.1 Exact lane

Same inputs + same lane key ⇒ bit-identical hashes, in any process, under any CPU load, any thread scheduling, any
number of frames in flight.

- **Lane key** (`dc_client::exact::lane_key`): `backend | vendor:device | driver | driver_info | target triple | build
  profile | wgpu version`, e.g. `Vulkan|0x10de:0x2b85|NVIDIA|617.42|i686-pc-windows-msvc|debug opt-level 0|wgpu
  25.0.2`. The GPU side of a frame depends on adapter and driver; the CPU side (poses, particles, matrices) is f32
  arithmetic whose results may differ between targets and optimisation levels (libm calls, constant folding).
  Hashes of different lane keys are never compared in the exact lane.
- **Inputs** (`inputs_digest` of the manifest): the ROM bytes read, every WGSL source, the pixel-relevant settings of
  every pipeline and sampler created, and the scene's render settings.
- **How an exact-lane scene runs**:
  - offline headless frame loop (`dc_client::run_headless`) or dc-render directly, never a window, never the network;
  - explicit ticks: 50 Hz, one tick per 20 ms frame (`frame_us` = 20 000), frame `f` is tick `f` at `f * 20` ms; a
    fixed frame count; the session clock handed to HUD, effects, drops, animation trackers is `f * 20` ms;
  - scripted input (`ScriptedInput`, or a pan of fixed radians per tick), scripted camera (`FrameState::cam_shot`),
    scripted spawn (spawn 0), scripted remote players, drops, view weapon, HUD events and effect events from a
    scripted `WorldSource` keyed by the frame index;
  - `HeadlessOptions::hash_frames = true`: every rendered frame is read back and hashed, and models load **inline**
    whatever `DC_ASYNC_LOADS` says (a model's first frame never depends on a loader thread);
  - sound off (no audio callback), debug panel text from frame-derived values only.
- **What may vary without changing a hash** (each proven by the tests, section 6): the process, repeated runs in one
  process, 24 busy threads on the machine, profile mode (`DC_PROF`) with any number of frames in flight
  (`DC_PROF_INFLIGHT`, 0 and 3 tested) **provided `DC_PROF_HUDFORCE=1`**.
- **Profile HUD cadence is an input, not an invariance**: without `DC_PROF_HUDFORCE=1`, profile mode composes the HUD
  only when its items are due on the session clock (`dc_client::headless::hud_every_frame`, the window's policy),
  and HUD frames do differ (measured: 22 of 242 frames, in `hud_fallback` and `rom_hud`; deterministic, identical
  between runs and between frames-in-flight settings). The scene records it as `render_settings.hud_cadence`
  (absent: every frame, the exact lane's default), so every HUD scene's `inputs_digest` changes with it and its
  hashes are not compared with the every-frame goldens. Scenes without a HUD keep inputs and hashes.

### 2.2 Hardware-coverage lane

Covers what the exact lane cannot: other adapters and backends (DX12, Vulkan, Metal, WARP / llvmpipe), other drivers,
other targets (i686 / x86_64) and build profiles, and the window and network paths (wall-clock dt, network thread,
async loads). Its comparison is **per-channel tolerance**, not hashes:

- `python tools/pixhash.py --tolerance T [--max-fraction F] A B`: an image passes when no channel of any texel differs
  by more than `T`, or at most the fraction `F` of its texels does; the tool prints, per image, the differing texels,
  the largest channel difference and the first differing texel.
- Default bounds for a coverage comparison of exact-lane scenes across adapters: `T = 3`, `F = 0.001`. They are a
  starting point for G1, which measures the real spread; a wider bound is a written exception, never a silent
  rebaseline.
- A window or network run is coverage-lane only: it is checked for behaviour (counts, presence, A/B tolerances as the
  existing tests do), never by hash.

## 3. Scheduling-independent random streams

Every random stream that reaches pixels is seeded from what the thing **is**, never from a global sequence consumed in
arrival order:

| stream | seed |
|---|---|
| effect group (particles, anchors, model pieces, effect-model ids) | `FxRunner::stream_seed(kind 1, group name, position bits, rotation bits, start time, ordinal among identical starts of that instant)` from the scene seed `0x0dc0_ffee` (DC-DEFAULT CLI-430, staged) |
| particle | `group seed ^ key << 40 ^ emission << 20 ^ index` (analytic in age: frame-rate independent) |
| anchors (`fxkeys`) | `group seed ^ key << 32 ^ constant` |
| impact mark (bullet-hole scale, roll) | `stream_seed(kind 2, "", impact point, surface frame, time)` |
| effect-model draw order | `group seed.rotate_left(12) ^ key` (a `BTreeMap`, so deterministic) |

Before G0 the group seed was the n-th output of one splitmix chain: an unrelated effect started earlier (or a lost
packet) shifted every later effect's particles. Now two runs that start the same effect at the same place and time
draw the same particles whatever else happened.

Not render streams (left as they are, documented): the weapon spread cone (`arms`) mirrors the 2009 shot counter shared
with the server and is consumed per own shot, which in the exact lane is scripted (no own fire through `arms` in the
scenes); the sound picks (no pixels); the dedicated server's `-seed` (network lane only).

## 4. Deterministic resource initialisation and load order

- Every wgpu buffer and texture is zero-initialised before its first use (WebGPU rule; wgpu tracks initialisation and
  clears lazily), so nothing samples uninitialised memory. The one hidden case is a DTX mip level shorter than
  `w * h * 4` bytes, which `upload_image` skips: it samples as zeros, deterministically (G4 must keep or document it).
- Load order is fixed: worlds load synchronously, models inline in the exact lane, scene batches in bucket and block
  order, draw lists in `BTreeMap` id order or scene order; `HashMap`s on the render path are look-ups only.
- Lazy creations (model pipelines and samplers on first use of a key, effect pipelines and textures on the first
  effect frame, overlay atlas pages) happen at fixed frames in the exact lane and are pixel-neutral; they are listed in
  the manifest (`pipelines`, `samplers`). Removing them is G2 work, not G0.

## 5. Manifest

Emitted per scene by `dc_client::exact::Recording::manifest` (JSON):

| field | content |
|---|---|
| `contract`, `texel_contract`, `texel_layout` | `dc-render-determinism/1`, `dc-texels/1`, section 1 in words |
| `lane_key` | section 2.1 |
| `inputs_digest` | SHA-256 over `rom.digest` and `code_digest` |
| `inputs.rom` | `reads`, `digest` (SHA-256 over the sorted `key sha256` lines) and `entries`: per byte range read from the client folder, `key` (16 hex digits of the SHA-256 of the path relative to the folder, lower case, plus offset and size) and `sha256` of the bytes. Names never leave the process (CLEANROOM.md section 4). Source: `dc_rom::journal` (archive entries, override files, PE images, the profile file) |
| `inputs.shaders` | label and SHA-256 of every WGSL source compiled (`dc_render::shader_module`) |
| `inputs.pipelines` | label and SHA-256 of `dc_render::pipeline_settings` (entry points, vertex layouts, colour targets with blend and write mask, primitive, depth-stencil, multisample) of every render pipeline created |
| `inputs.samplers` | label and SHA-256 of every sampler descriptor's settings |
| `inputs.code_digest` | SHA-256 over shaders, pipelines, samplers and render settings |
| `render_settings` | width, height, format, MSAA (1), present mode (offscreen: none), gamma, fog (world settings object, on), field of view x / y, the `RenderSettings`, frame µs, tick Hz, frame count, HUD layout, scene facts |
| `software` | dc-client version, git commit (`-dirty` with tracked changes; `DC_GIT_COMMIT` overrides), rustc, target triple, build profile, wgpu version |
| `adapter` | name, vendor, device, device type, driver, driver info, backend |
| `os` | OS and version string |

The journals are process-global and off by default; off they cost one relaxed atomic load per ROM read and per GPU
object creation. The texel hash runs only with `hash_frames`. The mutation knob costs one `OnceLock` read per frame.

## 6. Tests

`crates/dc-client/tests/exact_lane.rs` (GPU-gated; ROM scenes need `CA_ROM` or the repo's ROM folder):

| scene | ROM | what |
|---|---|---|
| `synth_order` | no | dc-render alone: coplanar quads of one bucket (the later draw wins under LESSEQUAL), two cameras |
| `synth_frame_loop` | no | the whole headless frame loop on the synthetic world: scripted walk, three remote boxes, fallback HUD with names and radar, debug panel |
| `hud_fallback` | no | HUD-only compose (roster, health, kill feed, damage, ammo) with the fallback layout: GPU frames + CPU compositor image |
| `rom_world_views` | yes | every ROM map, spawn 0, two view directions |
| `rom_characters` | yes | three posed characters with weapons at tick-time animation, two drops, the view weapon, ROM HUD with names and radar |
| `rom_fx` | yes | own shots (muzzle flash, tracer, impact, mark), remote shot records, then a world camera on a breaking crate, a breaking drum and its blast |
| `rom_hud` | yes | `hud_fallback` with the ROM layout and font |

- `exact_lane_repeats_in_one_process`: all scenes three times in one process, identical hashes.
- `exact_lane_across_processes_load_inflight_and_mutations`: the test binary re-runs itself as a child
  (`exact_child_entry`, `DC_EXACT_CHILD_OUT`): child A; child B while 24 busy threads run; child C in profile mode with
  3 frames in flight (`DC_PROF`, `DC_PROF_INFLIGHT=3`, `DC_PROF_READBACK=1`, `DC_PROF_HUDFORCE=1`): identical hashes and
  identical `inputs_digest` per scene. Child D in profile mode without `DC_PROF_HUDFORCE`: `hud_cadence` is recorded
  exactly in the HUD scenes, whose `inputs_digest` changes; scenes without a HUD keep inputs and hashes; at least
  one `hud_fallback` frame differs (the documented cadence difference). Then `DC_DETERMINISM_MUTATE=arith` (the world clip transform's x row scaled by
  1 + 1/256) and `=order` (opaque world batches, model instances and the sorted translucent objects recorded in
  reverse): the hashes must change (the order scene, the frame-loop scene, the ROM world views). Finally the golden
  check of child A.
- `DC_EXACT_ADAPTER=dx12|vulkan|fallback` runs the scenes on another adapter (another lane); `DC_EXACT_PNG=<dir>` saves
  the hashed frames as local PNGs (ROM-derived, never committed).

## 7. Goldens

- File: `crates/dc-client/tests/golden/exact_lane.json`: per lane key, per scene, the frame hashes, `inputs_digest`,
  `rom_digest`, `code_digest` and the manifest without the per-file ROM rows. Hashes and manifests only: no ROM bytes,
  no ROM names.
- The test compares child A with the section of its lane. No section: reported, failing only with
  `DC_EXACT_REQUIRE_GOLDEN=1`. A scene whose `rom_digest` differs from the golden's (another ROM) is not compared.
  A mismatch says whether the code inputs changed (an intended change needs new goldens) or not (a pixel regression
  or nondeterminism).
- **Update**: only `python tools/exact_golden.py --update`, which runs the scenes in a fresh process, prints every
  differing scene and frame, then writes this lane's section. Never automatic, never from a test. A golden change is
  its own reviewed commit that says why the pixels changed (or cites the approved visual-change review of G2).
- `python tools/exact_golden.py` without `--update` is the check: exit 0 when identical.

## 8. Comparing builds

`tools/pixhash.py DIR_A DIR_B` compares the PNGs of two runs file by file (two branches' binaries on the same scripted
headless scene: `dc-client --headless-frames N --save-every K --size WxH --out DIR`, or a scripted probe test). Exact
comparison by default; `--tolerance` for the coverage lane.

## 9. Not covered by the exact lane (by design)

The window loop (wall-clock dt, uncapped frame pacing, Immediate present), the network lane (net thread, wall-clock
server and bots), async model loads, the hit-rig thread, the audio callback, and frames of a profile run without
read-back. They stay coverage-lane surfaces. A recorded `FrameState` stream replayed by tick would bring network
scenes into the exact lane (open item).

## Golden update 2026-10-26 (stage 7fe873e)

Lane `Vulkan|0x10de:0x2b85|NVIDIA|617.42|i686-pc-windows-msvc|debug opt-level 0|wgpu 25.0.2`. Goldens recorded on
perf-g0 (a810f1b, master 58d1894 + G0); stage 7fe873e adds master bc86308 (Wave AN), parity Wave 1, perf-g1 and
perf-hitch. Review: the DC_EXACT_PNG frames of a perf-g0 build and of the stage build, compared with
`tools/pixhash.py` (exact and `--tolerance 3`: 166 of 238 images differ, none within the tolerance), the differing
frame pairs looked at, and one stage probe run with `DC_SOLID_BODIES=0`. Frame counts are the golden's (the PNG frames
plus the CPU HUD image of the last frame). Unchanged: synth_order and rom_world_views (all maps). The rom_digest of the
ROM scenes changed because stage reads more ROM inputs (pinned name socket, voice keys, speed rows, board columns).

| Scene | Frames differing | Cause (branch / commit / file) | Visual change | Max channel delta |
|---|---|---|---|---|
| hud_fallback | 17 / 31 (frames 14-29 + CPU image) | w1-hudsound 71aa89d, `hud/state.rs` (damage arcs behind the Reality option, off by default) | the red damage-direction arc of the frame-14 hit is no longer drawn | 174 |
| rom_hud | 17 / 31 (frames 14-29 + CPU image) | w1-hudsound 71aa89d, `hud/state.rs` | the same, with the ROM arc sprite | 118 |
| rom_characters | 31 / 31 | client-viewmodel 7db69c0, `arms.rs` / `characters.rs` (2009 first-person eye offset); client-tags2 b339a80, `hud/marks.rs` / `hud/items.rs` / `headless.rs` (caption on the posed name socket, bottom-anchored, HUD composed after the characters); w1-hudsound 71aa89d (arc) | view weapon larger and further right/down on every frame; names move from above the box top down onto the characters' heads; the frame-14 arc is gone | 193 |
| rom_fx | 64 / 64 | client-viewmodel 7db69c0 | view weapon placement on every frame, and the first-person muzzle flash follows the moved muzzle socket; tracers, impacts, marks, crate/drum breaks and blast pixel-identical (all differences inside x >= 138, y >= 129) | 248 |
| synth_frame_loop | 41 / 41 | client-tags2 b339a80, `hud/items.rs` (caption anchored at its bottom, flag 0x20: fallback names 4 px higher, every frame); w1-hudsound 71aa89d (arc, frames 14-32); w1-bodies fb4c807, `player.rs` (solid remote bodies, SIM-13 on: from frame 28 the scripted walk is stopped by a remote box instead of passing into it, so the camera differs) | names 4 px up; arc gone; from frame 28 a different camera position (with `DC_SOLID_BODIES=0` frames 28-39 differ from perf-g0 only by the names and the arc) | 158 |

No difference was found that is not one of these intended changes; the perf-g1 / perf-hitch accounting wrappers
(`dc_render::perf::*`) and client-fps are pixel-neutral in every scene. Reviewer: visual-change review agent,
2026-10-26.

## Golden update 2026-10-27 (w2-fx, from stage aa295bc)

Lane `Vulkan|0x10de:0x2b85|NVIDIA|617.42|i686-pc-windows-msvc|debug opt-level 0|wgpu 25.0.2`. Change: parity audit
M21 / M20 / M22 / m50 (effect lights on the models, effect camera shake, looping placed effects, effect fog;
DC_DEFAULTS CLI-513..518). Review: `tools/exact_golden.py` before the update (only rom_fx differs: 53 of 64 frames;
hud_fallback, rom_characters, rom_hud, synth_frame_loop identical, only their code digests moved), then four
DC_EXACT_PNG child runs of the branch with each part switched off by a temporary local switch (removed before the
commit) compared with `tools/pixhash.py`: all three parts off = the old goldens bit for bit (no unintended change);
one part on at a time:

| Scene | Part | Frames differing | Visual change | Max channel delta |
|---|---|---|---|---|
| rom_fx | effect lights (CLI-514) | 30 / 64: 1-3, 5-7, 9-11, 13-15, 17-19, 21-23 (the own shots) and 27-38 (crate / drum break) | the first-person muzzle group's light key lights the view weapon for the three frames after each shot (about 6,600 pixels, the weapon only); the break groups' light keys light the effect models and the view weapon slightly (2,200-5,000 pixels) | 112 (shots), 19 (break) |
| rom_fx | camera shake (CLI-513 / 515) | 35 / 64: 24-58 | the world camera next to the drum blast is moved by the blast group's shake keys (the whole frame shifts, every frame of the shake) | 252 |
| rom_fx | effect fog (CLI-518) | 0 / 64 | none: every effect quad of the scene is nearer than the map's fog start | 0 |
| rom_fx | looping placed effects (CLI-516 / 517) | 0 / 64 | none: the scene's placed effects carry value 0 (play once) | 0 |

The union is the 53 frames of the update. No other difference. Reviewer: w2-fx worker, 2026-10-27 (frames 1 and 30
looked at side by side).

## Golden update 2026-10-28 (master 3a7e740: drift of 2026-10-27 plus VIS-20 fog)

Lane `Vulkan|0x10de:0x2b85|NVIDIA|617.42|i686-pc-windows-msvc|debug opt-level 0|wgpu 25.0.2`. The goldens of 2026-10-27
(c4b74c4) matched on c4b74c4 itself (all 7 scenes). Method: the check (`tools/exact_golden.py`) was run along the
first-parent history c4b74c4..master by bisection (per scene, one run per visited commit), then on the pre-fog master
c040f52, then on master; DC_EXACT_PNG frame runs of c4b74c4 and master were compared with `tools/pixhash.py` (185 of 238
images differ, none within tolerance 3) and one frame per scene was looked at. Each frame count below is the number of
golden frames that differ from the 2026-10-27 golden. Every difference traces to an intended change:

| Scene | Frames differing | Cause (merge / commit / file) | Visual change |
|---|---|---|---|
| synth_frame_loop | 41 / 41 | sim-fixes5 6b4b389 (merge 351d3fb), `dc-sim/src/player.rs` (SIM-22: the grounded step moves at the 2009 ground cap, so the scripted walk covers less ground) | the same walk ends a few pixels short: boxes and horizon shifted; HUD identical. Not the fog (this scene has no fog: 0 frames from VIS-20) |
| rom_hud | 21 / 31 | board-fixes6 (merge 50264f0, CLI-658: HUD text uses the menu's system face) | HUD captions drawn with the installed system face instead of the ROM font file; layout unchanged. Not the fog (0 frames from VIS-20) |
| rom_fx | 64 / 64 | arms-fixes b66caf9 (first-person flash key offsets turned by the parent model, ClientFx case 6: frames of the own shots); arms-fixes 6934d24 (CLI-575, view origin = row Pos + the playing clip's ROM offset: every frame); VIS-20 a71888e (fog from the world-settings object: 33 frames) | see below |
| rom_characters | 31 / 31 | arms-fixes 6934d24 (CLI-575: all frames); VIS-20 a71888e (30 frames: world fog on the room and the characters) | see below |
| rom_world_views | 31 / 42 | VIS-20 a71888e only (merge 431d1ef): fog from the settings object, sky fogged. `rom_digest` moved with the pre-fog master (more ROM reads), pixels did not (0 / 42 before the fog) | distant geometry and the open sky toned to the fog colour (a dark window shows its fogged sky instead of black) |
| hud_fallback, synth_order | 0 | none | none |

Finding for the scene author (not a game regression): the CLI-575 origin adds the clip offset only while a clip plays.
The ROM scenes `rom_characters` and `rom_fx` hand the view weapon over with `clip: None`, so since 6934d24 the rifle is
placed at the row `Pos` alone and falls outside the frame (the pictures before it show the rifle at the bottom right).
In play a clip always runs (the rifle's offset equals the old constant to 0.01, `view_model_clip_offset_rom`). The
goldens therefore no longer cover the view weapon's pixels; giving those scenes a playing clip is a follow-up with its
own golden update.

The full `exact_lane` test binary
(3 tests: repeat, across processes with load / in-flight / mutations, child) passes on the updated goldens.
