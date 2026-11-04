# RENDER_2009: renderer behaviour of the 2009 Engine.exe, for a wgpu reimplementation

Scope: what the renderer inside the 2009 EU-test `Engine.exe` does each frame and with which device state, written as
behaviour our wgpu renderer must reproduce: device and swap-chain setup, frame order, camera maths, world, model,
sprite/effect and 2D paths, render/texture-stage/sampler state per pass, lighting inputs, texture upload, and the
programmable shaders shipped in the ROM. It does not cover file formats already specified (`FMT_DTX`, `FMT_DAT85`,
`FMT_LTB`), the game-side (CShell.dll) decisions that fill the scene description, or ClientFx.fxd internals beyond the
engine entry points they call.
Evidence sources: decompiler output of the 2009 Engine.exe (`Engine.exe/*.c`, function table
`Engine.exe.funcs.orig.tsv`), read-only capstone disassembly of the ROM `Engine.exe` (own scratch tools in
`staging/render-2009/`), read-only ROM census through `evidence/rezls.py` and the DAT85 scratch
reader, public Direct3D 9 / D3DX 9 documentation for the meaning of API enums and interface method order.
Status: DRAFT. Folds: render-view addendum (Appendix Z, spec-fold-2); render-models addendum M1-M10 (Appendix M, spec-fold-5, 2026-10-25); render-styles addendum S1-S6
(Appendix S, spec-fold-6, 2026-10-25).

## 0. Conventions

- `engine+0xRVA`: 2009 EU-test `Engine.exe`, image base 0x00460000 (VA = 0x460000 + RVA). Data addresses are given as
  VA (`VA 0x6d3e84`) because they are absolute in the code.
- "slot N" of a COM object is the N-th method of its public interface (0 = QueryInterface). Device calls were identified
  as calls through the device pointer kept at VA 0x7e4ce0 (`dev_pp`: pointer to the object whose first field is the
  `IDirect3DDevice9` pointer). Enum names (`D3DRS_*`, `D3DTOP_*`, `D3DFMT_*` ...) are PUBLIC Direct3D 9 names, cited
  from the public DirectX 9 documentation; they are API facts, not engine identifiers.
- Engine and ROM strings are never quoted. A string is referenced as `h:<sha256[:16]>/<length>` of its bytes (without
  the NUL); the coined name in backticks is ours. Our renderer resolves them at run time from the user's ROM where it
  needs to (for example when it loads the shipped `.fx` files).
- Tags: `engine+0x...` = read in decompiler output or disassembly; PUBLIC = public API documentation; OBSERVED = measured over
  the ROM files; UNPROVEN = plausible reading that the code did not settle; DeCombat = our decision for the port.
- "Linear-sweep decode" means the state values were read from the machine code by our scratch decoder (`stdec.py`),
  which follows pushes and stack stores but not branches; values that depend on a branch are given as alternatives.

## 1. Renderer architecture

### 1.1 Linkage

The renderer is linked into `Engine.exe`; there is no separate renderer module. Imports (read from the PE import
table, OBSERVED): one entry point of `d3d9.dll` (the Direct3D 9 factory, called once with SDK version 32 at
engine+0x15d540) and 15 entry points of `d3dx9_35.dll`: effect creation and effect pool, shader assembler, surface
load-from-memory and save-to-file, texture-from-file-in-memory, declaration vertex size, matrix multiply / transpose /
look-at / left-handed perspective (plain and field-of-view form) / left-handed orthographic, vector transform and
project (PUBLIC: D3DX 9 function set). A wgpu port replaces all of them; only the effect files of section 11 need a
real replacement (rewritten as WGSL).

### 1.2 Device object and state cache

- VA 0x7e4ce0 (`dev_pp`) points to a 0x1dbc-byte wrapper (constructed at engine+0xfea10) whose first field is the
  device interface; almost all device calls in the engine go through it.
- The wrapper caches 210 render states, 8 x 33 texture-stage states and the software-vertex-processing flag
  (engine+0xfea40 reads the current values for the entries enabled in two byte tables at VA 0x6d4e40 (210 entries) and
  VA 0x6d4f14 (33 entries)). Index space: 0..0xd1 render states, 0xd2..0x1d9 stage states (`stage = (i-0xd2)/33`,
  `type = (i-0xd2)%33`), 0x1da the vertex-processing flag. Deferred changes are flushed before draws by engine+0xfeb20;
  engine+0xfec00 flushes one index. Most passes, however, call the device directly.
- Pass code follows one discipline throughout: read the old value (`Get*State`), set the new one, draw, restore in
  reverse order. About 940 compiler-generated unwind handlers in engine+0x1f0000..0x202000 restore render states on
  exceptions (they consist of single `SetRenderState(saved)` calls). DeCombat: in wgpu every pass is a self-contained
  pipeline plus bind groups; the save/restore discipline means no pass relies on state left by another pass, except
  where section 5 says so (the shared per-frame state of 3.3).

### 1.3 Renderer function table

The engine fills a table of function pointers in a static struct at VA 0x6e0fd8 (filled by engine+0xfe7b0, called
from engine+0x8a9b4 / 0x8ae2a). Offsets are from the struct start; names are ours.

| off | engine RVA | our name | behaviour (evidence: the RVA) |
|---|---|---|---|
| 0x50 | 0xfe2c0 | `r_init` | create the device for a requested mode (section 2) |
| 0x54 | 0xfdd90 | `r_term` | release everything |
| 0x5c | 0xf2760 | `r_list_msaa` | list multisample types 2..16 supported for both back-buffer and depth format |
| 0x70 | 0x100e50 | `r_pick_tex_format` | DTX format + flags to device format (FMT_DTX section 5) |
| 0x7c | 0x100f30 | `r_create_surface_tex` | 2D surface texture (section 12) |
| 0x90 | 0xf67c0 | `r_clear` | clear colour/depth (3.2) |
| 0x94 | 0xf1c80 | `r_begin_3d` | begin scene (3.1) |
| 0x98 | 0xf0a80 | `r_end_3d` | end scene |
| 0x9c | 0xf0b20 | `r_in_3d` | scene-open flag (VA 0x7e4e60) |
| 0xa0 / 0xa4 | 0x176cf0 / 0x176e90 | `r_begin_2d` / `r_end_2d` | optimized 2D mode (section 12) |
| 0xac | 0x176f70 | `r_set_2d_blend` | 2D blend mode 0..6 (section 12) |
| 0xbc | 0xf83c0 | `r_render_scene` | draw one scene description (3.3) |
| 0xc4 | 0x175540 | `r_present` | present + frame-latency wait (3.4) |
| 0xfc | 0x1763b0 | `r_render_to_file` | begin, clear, render scene, read back, write an image file |
| 0x100 | 0x102640 | `r_apply_settings` | per-frame fog and filter state from the settings table (3.3) |
| 0x108 | 0x175010 | `r_blit_backbuffer` | copy a surface to the back buffer (UpdateSurface) |
| 0x114 | 0xfd990 | `r_load_world` | world render data (FMT_DAT85 section 10) |
| 0x138..0x140 | 0x166780 / 0x1667a0 / 0x1667c0 | `r_glow_*` | screen-glow texture remap table and two glow textures (section 9) |

Other slots (0x58..0x8c, 0xa8..0xb8, 0xc0..0xf8, 0x104..0x134) are texture/surface helpers and debug hooks; several
are not defined as functions in the decompiler table (section 15).

## 2. Device creation, present parameters, reset

### 2.1 Adapter and mode list (engine+0x15c9a0, engine+0x159620)

- Every adapter is queried for its identifier and display mode; modes are enumerated for four adapter formats taken
  from a table at VA 0x6d50ec: X8R8G8B8, A2R10G10B10, X1R5G5B5, R5G6B5 (PUBLIC D3DFORMAT values 22, 35, 24, 23).
  Only modes with width >= 800 and height >= 600 that also pass a further check (engine+0x157a80, UNPROVEN) are kept,
  de-duplicated by (width, height, format).
- Mode pick for a request (width, height, bits): exact width and height; windowed requires the desktop format;
  full screen with 16 bits accepts R5G6B5 or X1R5G5B5, with 24 or 32 bits X8R8G8B8 or R8G8B8. Non-hardware devices are
  accepted only when `use_ref_device` (section 13) is set.
- Windowed flag: the engine console value `h:eff9f08c79ee9925/8` parsed as an integer equal to 1 (engine+0xfe2c0).

DeCombat: wgpu has no exclusive mode; we always present to a window surface and treat "full screen" as a borderless
window of the requested size. Back-buffer format: `Bgra8Unorm` (the 32-bit choice); 16-bit modes are not reproduced.

### 2.2 Present parameters (engine+0xf15e0)

| field (PUBLIC D3DPRESENT_PARAMETERS order) | value |
|---|---|
| back buffer width / height / format | from the picked mode |
| back buffer count | `back_buffer_count` (default 2); forced to 1 when `force_copy_swap` is set |
| multisample type | 0, or (when `accept_msaa` is set) `msaa_samples` lowered until the device accepts it (engine+0xf0560, IDirect3D9 slot 11 per step) |
| multisample quality | 0 |
| swap effect | DISCARD (1); COPY (3) when `force_copy_swap` is set and MSAA is off |
| device window / windowed | engine window handle (VA 0x7e6a14) / windowed flag (VA 0x7e6a10) |
| auto depth-stencil | on |
| depth format | chooser below with (`depth_bits` default 32, `stencil_bits` default 0) |
| flags | LOCKABLE_BACKBUFFER (1) without MSAA, 0 with MSAA |
| refresh rate | 0 (default) |
| presentation interval | IMMEDIATE (0x80000000): never waits for vertical blank |

When `fullscreen_by_mode_change` is set and the window is not windowed, the device is created windowed and the display
mode is changed through the OS instead (engine+0xf0f30); gamma then goes through the OS ramp (2.5).

Depth format chooser (engine+0xf0220), each step tested with IDirect3D9 slot 10 (usage DEPTHSTENCIL, surface):

| request (bits, stencil) | try | on failure retry as |
|---|---|---|
| (16, 0) | D16 | none (fails) |
| (16, 1 or 8) | D15S1 | (16, 4) |
| (16, 4) | D15S1 | (16, 0) |
| (24, 0) | D24X8 | (16, 8) |
| (24, 1 or 8) | D24S8 | (24, 4) |
| (24, 4) | D24X4S4 | (24, 0) |
| (32, 0) | D32 | (24, 8) |
| (32, 1 or 8) | D24S8 | (32, 4) |
| (32, 4) | D24X4S4 | (24, 8) |

With the defaults (32, 0) the device ends up with D32, else D24S8, D24X4S4, D24X8, D15S1, D16 in that order.
DeCombat: `Depth32Float` (or `Depth24PlusStencil8` when a stencil pass is enabled, see 7.6); reversed-Z is not used because
the engine depends on the [0,1] window depth split of section 4.3.

### 2.3 Device creation (engine+0xf20d0)

1. Behaviour flags: the enumerated device's hardware transform-and-light capability (PUBLIC caps bit 0x10000 of
   DevCaps) gives 0x84 (MIXED + MULTITHREADED), else 0x24 (SOFTWARE + MULTITHREADED).
2. On failure the engine prints a fallback warning, re-picks device type and formats (engine+0x15d720) and tries once
   more.
3. Afterwards: device caps are copied to VA 0x7e4d00; an EVENT query (type 8) is created for 3.4; render states
   MULTISAMPLEANTIALIAS = TRUE and MULTISAMPLEMASK = 0xffffffff; the default state of 2.4 is applied; a log line
   reports the reference rasterizer when no hardware device exists.
4. Every draw path that may use software vertex processing calls `SetSoftwareVertexProcessing(!hw_tnl)` first, with
   `hw_tnl` read from the device caps copy (byte VA 0x7e4d1e bit 0 = DevCaps bit 16; engine+0x10fb70 and others).

### 2.4 Default state (engine+0xf16e0, linear-sweep decode)

Applied after creation, after reset and at the start of each scene (engine+0xfd610):

| state | value |
|---|---|
| fog | table fog LINEAR and vertex fog NONE when the table-fog path is enabled (byte at manager+0x151), else vertex fog LINEAR and table fog NONE |
| CULLMODE | NONE |
| ZENABLE / ZWRITEENABLE / ZFUNC | TRUE / TRUE / LESSEQUAL |
| SLOPESCALEDEPTHBIAS / DEPTHBIAS | 0.0 / 0.0 (values loaded from a zeroed register; the second is UNPROVEN) |
| SPECULARENABLE / SHADEMODE / DITHERENABLE | FALSE / GOURAUD / TRUE |
| FILLMODE | SOLID, WIREFRAME when `wireframe` is set |
| ALPHABLENDENABLE / ALPHATESTENABLE / ALPHAFUNC / ALPHAREF | FALSE / FALSE / GREATEREQUAL / 0 |
| SRCBLEND / DESTBLEND | SRCALPHA / INVSRCALPHA |
| COLORVERTEX, VERTEXBLEND, INDEXEDVERTEXBLENDENABLE, NORMALIZENORMALS, LIGHTING | all 0 |
| CLIPPING | TRUE |
| material sources (ambient, diffuse, emissive, specular) | MATERIAL (0) |
| stage 0 | TEXCOORDINDEX 0, TEXTURETRANSFORMFLAGS DISABLE, address WRAP/WRAP, filters from 3.3 |
| stage 0 colour / alpha | MODULATE(TEXTURE, DIFFUSE) / MODULATE(TEXTURE, DIFFUSE) |
| stage 1 | DISABLE / DISABLE (arguments TEXTURE, CURRENT) |
| material | every colour component and the power = 1.0 |

Model passes do not keep this CULLMODE: each render-style pass sets its own (7.3; `FMT_RENDERSTYLE_2009.md` 4.2), CCW in 47 of the 69 ROM passes
(OBSERVED, `style_census`); NONE applies only to draws without a style (spec-fold-6, addendum S1).

### 2.5 Reset, lost device, gamma

- Reset (engine+0xf2330): rebuild the present parameters, re-check MSAA, call Reset. On failure poll the cooperative
  level every 100 ms (up to 21 polls) while it reports device-lost (PUBLIC D3DERR_DEVICELOST 0x88760868), reset
  again, then adjust the parameters (engine+0xf12c0) for a third try. Success re-creates device resources
  (engine+0xf05f0) and forgets the cached viewport (manager fields +0x168..+0x174).
- Gamma (engine+0xf7060 at each scene, engine+0xf6f10): when `gamma_r/g/b` change, a 256-entry ramp per channel is
  written: `ramp[i] = trunc(65535 * (i/255) ^ (1/gamma))` (constants 1/255 and 65535 read in the code). It goes to the
  device ramp, or to the OS ramp when `fullscreen_by_mode_change` is set. DeCombat: wgpu has no gamma ramp; apply the same
  curve in the final composite pass.

### 2.6 Vertical sync and frame rate

- No vsync: the presentation interval is always IMMEDIATE (2.2). The setting `vsync_on_flip` (`h:22867a734ee7df71/11`,
  default 1) is registered but no code reads its value (no reference to VA 0x6d3df4 in .text). DeCombat: present with
  `Immediate` (or `Mailbox`) by default, `Fifo` as an option.
- Frame cap: an engine console value `frame_cap_hz` (`h:3f0f6dec81fa3395/6`, record at VA 0x6cb870 in the engine's
  general console table, float value at VA 0x6df8ac, zero-initialised) is read only by a limiter body
  at engine+0x17380: if non-zero, clamp to [1, 200] (written back), compute the target frame time `1/cap`, compare
  with the elapsed time from a performance-counter tick, then sleep 1 ms at a time (at most 500 times) until the
  remainder has passed. No call, jump or data reference to engine+0x17380 exists in the image (byte search for rel32
  calls/jumps and for the absolute address), and decompiler did not create a function there: whether the cap is active in
  2009 is UNPROVEN, most likely dead code. DeCombat: implement the same clamp and sleep loop as an option.
- Frame-latency limiter (engine+0xf1d70, after every present): when `prevent_frame_buffering` (default 1) is on, issue
  the EVENT query and spin (`Sleep(0)`) until its data is available with the flush flag, giving up after 1000 ms; a
  debug setting prints the wait. DeCombat: equivalent to one frame in flight (wgpu `desired_maximum_frame_latency = 1` or a
  wait on the previous submission).

## 3. Frame sequence

The game module drives the frame through the table of 1.3 (engine-side wrappers, for example engine+0x15220 for the
present slot and engine+0x188c0 for the scene slot). The order the game uses (begin, clear, one or more scenes, 2D,
end, present) is the observable contract; the exact game-side call order is UNPROVEN here (CShell is out of scope).

### 3.1 Begin / end (engine+0xf1c80, engine+0xf0a80)

`r_begin_3d`: under the renderer lock, BeginScene once (flag at VA 0x7e4e60), flush cache indices 0x1da (vertex
processing) and 0x89 (LIGHTING), unbind any vertex shader, set FVF = XYZ. `r_end_3d`: EndScene when open. Stream
source 0 and the index buffer are unbound by engine+0xf1c10 when draw paths finish.

### 3.2 Clear (engine+0xf67c0)

Arguments: optional rectangle (NULL or full screen = whole target), flags (bit0 colour, bit1 depth; depth also clears
stencil when the depth format has stencil, byte at VA 0x7e4e32), colour (ARGB), depth 1.0, stencil 0. The viewport is
temporarily set to the full screen with depth range [0.1, 1.0] (see 4.3) and restored afterwards; the cleared rectangle
is recorded for the dirty-rectangle present (3.4). Render-target clears (engine+0x1002e0) clear colour + depth +
stencil to black / 1.0 / 0, retrying without stencil on failure.

### 3.3 One scene (`r_render_scene`, engine+0xf83c0)

Refused (error log) while optimized 2D mode is active (VA 0x7e7ae0). Otherwise:

1. Gamma check (2.5).
2. Scene setup (engine+0xfd610): apply per-frame settings (engine+0x102640, below), remember the scene description
   (VA 0x7e59d8), bump the frame counter of mode-1 scenes, reset per-frame lists, apply the default state (2.4), set the
   viewport and build the camera (section 4). When a render target is bound, its size replaces the scene rectangle.
3. FILLMODE = WIREFRAME when `wireframe` is set, else SOLID (through the state cache).
4. Main draw (engine+0xf7e70):
   1. Upload WORLD, VIEW and PROJECTION from the camera context (engine+0xf6970 transposes the engine's column-vector
      matrices into Direct3D's row-vector layout).
   2. Run the per-frame hook of every render-object type (engine+0x167ad0 over the 12-entry table at VA 0x6d546c, 0x1c
      bytes per type: frame hook, draw function, a flag set for types that can be translucent, a per-type list
      function; field meanings beyond the first two are our reading).
   3. Scene mode 2 (object list only, used for previews): draw just the listed objects (engine+0xf6770) with no world;
      otherwise the world pass (section 5) which includes sky, world sections and the visibility collection of objects.
   4. Object queue (engine+0x169a70, section 7.1): solid objects, model shadows, then translucent objects sorted back to
      front, then objects that ignore depth.
   5. Screen glow when `screen_glow_on` (section 9): render the glow scene into a small texture, blur, add to the frame.
   6. Colour-scale overlay (engine+0xf6b20): when any component of the scene's tint (scene +0x28, three floats) is
      outside [0.99, 1.01], draw a full-viewport quad with vertex colour `tint * 127.5` (per channel), alpha 255,
      untextured, fog off, Z off, blend SRC = DESTCOLOR, DST = SRCCOLOR (result `dst * tint`).
   7. Additive overlay (engine+0xf7a80): when `light_add_poly` (default 1.0) is non-zero and any component of the
      scene's additive colour (scene +0x34) is >= 0.001, draw the same quad with colour `add * 255`, blend ONE / ONE.
5. FILLMODE = SOLID; optional debug statistics (engine+0xf75b0..0xf79d0, console text only).

Per-frame settings (engine+0x102640 tail at engine+0x16b770): fog is forced off unless the device can fog (PUBLIC
raster caps: table fog with W or Z fog, or vertex fog with W or Z fog); fog off when near == far; then FOGCOLOR =
`0xff000000 | r<<16 | g<<8 | b` from `fog_r/g/b` (0..255), FOGSTART = `fog_near`, FOGEND = `fog_far`, FOGENABLE =
`fog_on`, DITHERENABLE = `dither`. For samplers 0..3: MAXANISOTROPY = `anisotropy` (0 means 1); MIN and MAG filter =
ANISOTROPIC when the device supports it for that filter and `anisotropy` is non-zero, else LINEAR when `bilinear`, else
POINT; MIPFILTER = LINEAR when `trilinear`, else POINT. Fog in this engine is always linear with distances in world
units; the game sets the fog values per map through these settings.

Scene description fields read by the renderer (offsets in the struct passed to the scene slot; filled at
engine+0x188c0):

| off | type | meaning |
|---|---|---|
| 0x00 | i32 | mode: 2 = object list only; 1 = scene with an external frame counter at +0x24 |
| 0x28 | 3 x f32 | colour-scale tint (3.3 step 4.6) |
| 0x34 | 3 x f32 | additive colour (step 4.7) |
| 0x48 | 12 x u32 | block copied from the client state (engine+0x188c0 source +0x9cc); meaning UNPROVEN |
| 0x78 / 0x7c | ptr / i32 | sky objects (up to 30, resolved from 16-bit object ids) and their count (5.3) |
| 0x80..0x8c | 4 x i32 | viewport left, top, right, bottom in pixels (full screen when the camera says so) |
| 0x90 / 0x94 | f32 | horizontal / vertical field of view, radians |
| 0x98 | 3 x f32 | camera position |
| 0xa4 | 4 x f32 | camera rotation quaternion |
| 0xb4 / 0xb8 | ptr / i32 | object list for mode 2 |

The two field-of-view values are independent: the engine never derives one from the other or from the viewport
aspect, so the game's choice of the pair defines the aspect behaviour (4.1).

### 3.4 Present (engine+0x175540)

Flags bit 2 selects a dirty-rectangle present (engine+0x169dc0: a region header with the rectangles recorded by clears
and 2D blits since the last present, bounded by the screen size); otherwise a plain full present. Then the
frame-latency wait of 2.6 and the per-present bookkeeping (engine+0x169e40). DeCombat: always present the whole frame.

## 4. Camera and projection

### 4.1 Main camera (engine+0xfd610 -> engine+0xfd4e0 -> engine+0xfb8c0)

Inputs: viewport rectangle (l, t, r, b) in pixels, `fov_x`, `fov_y` (radians), position `p`, rotation quaternion `q`
(scene description, 3.3). Constants: near plane `n = 7.0` (immediate operand at engine+0xfd610), far plane `f` =
`far_z` (default 12000.0), clamped to [3.0, 500000.0] by engine+0xfb8c0.

1. `tx = tan(fov_x / 2)`, `ty = tan(fov_y / 2)` (computed as `cos(pi/2 - a/2) / sin(pi/2 - a/2)`).
2. View: rotation from `q` (engine+0x5f6d0), transposed, times translation by `-p` (left-handed, +Z forward, +Y
   up, +X right; positions in world units). OBSERVED (Appendix Z R7): the euler property of the first player spawn of
   each world is `(0, yaw, 0)` (yaw in radians, e.g. -pi/2, pi), consistent with x = pitch, y = yaw, z = roll; with the
   quaternion-to-matrix routine above, yaw `a` maps +Z to `(sin a, 0, cos a)`. The composition order inside
   engine+0x5b6c0 is UNPROVEN.
3. Projection (engine column-vector form, before the transpose of 3.3): `x' = x / tx`, `y' = y / ty`,
   `z' = f/(f-n) * z - n*f/(f-n)`, `w' = z`. The builder supports an off-centre shear `(-cx/d, -cy/d)` but the scene path
   passes `cx = cy = 0`, `d = 1`.
4. Screen transform kept for culling and sprites: `sx = x_ndc * (w/2 - 0.0001) + (l + w/2)`,
   `sy = -y_ndc * (h/2 - 0.0001) + (t + h/2)` with `w = r - l`, `h = b - t`; the builder receives `r - 0.1` and
   `b - 0.1` as the right and bottom edges and rounds all four edges to integers.
5. The eight frustum corners (`(+-n*tx, +-n*ty, n)` and `(+-f*tx, +-f*ty, f)`) are transformed to world space and
   stored for culling (camera context +0x23c..+0x29b).

The camera context (VA 0x7e5a08, 0x370+ bytes) holds: +0x1c rectangle (4 f32), +0x2c width, +0x30 height, +0x34
near, +0x38 far, +0x3c camera matrix, +0x7c view, +0xbc projection, +0x13c screen transform, +0x17c view-projection-
screen, +0x1bc world (identity), +0x350 eye position (also the translucent sort origin, 7.1).

DeCombat: build the same matrices with a [0,1] depth convention (wgpu clip space matches Direct3D for depth and NDC y up).
Pass `fov_x` and `fov_y` straight through; do not derive one from the other.

### 4.2 Player-view ("really close") camera (engine+0x1b4870, restore at engine+0x1b46b0)

Used for objects flagged as attached to the eye (first-person weapon and hands). Grades re-derived by the render-playerview worker (folded by spec-fold-7):
**CONFIRMED** = the instruction read in our own capstone disassembly of the user's Engine.exe (base 0x460000; tag DISASM) **and** the data value read from the
same file; the decompiler output of engine+0x1b4870 is incomplete (it drops the identity matrix's diagonal and the viewport fields). Device slots are PUBLIC
(`IDirect3DDevice9`: 0xb0 SetTransform, 0xb4 GetTransform, 0xbc SetViewport, 0xc0 GetViewport; transform 2 = VIEW, 3 = PROJECTION).

- Save the current viewport, PROJECTION and VIEW; set the same rectangle with depth range MinZ 0.0, MaxZ 0.1. CONFIRMED (DISASM engine+0x1b4870..0x1b493b: MinZ
  by `xorps`, MaxZ = the f32 at VA 0x675b78 = 0.1).
- PROJECTION = left-handed field-of-view perspective (PUBLIC D3DX formula: `ys = 1/tan(fy/2)`, `xs = ys/aspect`,
  `z' = zf/(zf-zn) z - zn*zf/(zf-zn)`, `w' = z`) with `fy = pv_fov_deg * pi/180` (`pv_fov_deg` default 50.0),
  `aspect = width / height` of the camera context (VA 0x7e5a34 / VA 0x7e5a38), `zn = pv_near` (0.1), `zf = pv_far`
  (7.0). CONFIRMED: the call `D3DXMatrixPerspectiveFovLH(out, fy, aspect, zn, zf)` at engine+0x1b4981 with its arguments in push order; `fy` = the f32 at
  VA 0x6d3a04 (50.0) times the f32 at VA 0x676914 (pi/180) (engine+0x1b4967 / 0x1b496f); `aspect` = VA 0x7e5a34 / VA 0x7e5a38 (engine+0x1b494c / 0x1b4954);
  `zn` = the f32 at VA 0x6d39d4 (0.1), `zf` = the f32 at VA 0x6d39ec (7.0) (engine+0x1b4941 / 0x1b4946).
- `pv_fov_deg`, `pv_near` and `pv_far` are **console-variable records** (stored default and current value equal in the ROM: 50.0, 0.1, 7.0; CONFIRMED by the
  ROM read of VA 0x6d39c4..0x6d3a1c); a run-time override is therefore possible. The fourth record next to them (VA 0x6d3a1c, 1.2) is not read by this function.
- VIEW = identity: such objects are positioned in eye space by the game. CONFIRMED: PROJECTION and VIEW are saved (GetTransform 3 / 2), PROJECTION set, and
  VIEW set to a full 4 x 4 identity built from the f32 1.0 at VA 0x672988 (engine+0x1b4996 .. 0x1b49db).
- Afterwards restore the saved viewport and both matrices: CONFIRMED (engine+0x1b46b0, decompiler output).
- Switch per piece: the model piece pass (engine+0x1b94e0, 7.2) calls engine+0x1b4870 when the piece record's flag byte (+0x14 of the 0x2c-byte records) changes
  and engine+0x1b46b0 at the end. PLAUSIBLE (decompiler output); the order of the piece queue it walks is not traced (REN-12 (proposed) draws eye-attached models last).
- Placement: the game module positions the first-person model; CShell registers a console command (cshell+0x1a9400) whose handler cshell+0x1a52e0 gets or sets a
  three-float position on the current player-view object (methods +0xbc / +0xc0). The default position is not traced: OPEN (REN-15, CLI-28 revised, proposed).

DeCombat (D, REN-12 .. REN-16 (proposed); `dc-render` `encode_frame_with_view` / `render_frame_offscreen_with_view`, merged; `dc-client` draws its own weapon
through it since the render-playerview patch was applied; TESTED: `tests/playerview_render.rs` (a view model in the lower half wins against a world wall 8 units ahead;
eye-relative, escapes the world near plane; an empty list draws byte-identical images; lighting equal to the world pass for a model both passes can draw), `camera.rs`
unit tests (the D3DX form, near / far -> 0 / 1), ROM-gated `tests/rom_playerview.rs` (297 first-person rows; the rifle row at 640 x 480 changes 35,425 pixels, all in the lower
half); not compared with the 2009 client): own projection from `PLAYER_VIEW_DEFAULT` (50, 0.1, 7.0; user-overridable, REN-16), aspect = target width / height, VIEW =
identity (instance position / rotation in eye space, +Z forward), viewport depth [0.0, 0.1], no depth clear, drawn at the end of the world pass after the solid models
and the sorted translucent world objects (REN-12), lit like a world model with the light set built at the eye (REN-13), LOD by eye distance (REN-14), default model
origin at eye space (0, -4, 0) (REN-15, a viewer default; the knife and grenade idle poses at time 0 fall off screen with it: per-weapon placement is open).

The settings `pv_aspect` (1.2) and `pv_env_scale` / `pv_env_clamp` have no reader in the decompiler output or by address search;
`pv_env_velocity` is read by the model env-map path (engine+0x1b74a0).

### 4.3 Depth range split

World and every normal object draw with viewport depth [0.1, 1.0]; really-close objects with [0.0, 0.1] (4.2; the 0.1 is CONFIRMED by the render-playerview
DISASM of engine+0x1b4870, VA 0x675b78). The
player-view pass therefore always wins the depth test against the world without clearing depth. Clears use [0.1, 1.0]
too but clear the whole buffer to 1.0. DeCombat: wgpu viewports carry `min_depth` / `max_depth`; reproduce both ranges
exactly. Implemented for the player view as `PLAYER_VIEW_DEPTH_RANGE` [0.0, 0.1] with no depth clear (D, REN-12); TESTED (`camera.rs`: a view model 3 units ahead
in [0, 0.1] sorts before world geometry 8 units ahead; with one shared [0, 1] range the world would win, 0.98 vs 0.21). World geometry nearer than the main camera's
near plane (7 units, 4.1) is clipped and drawn by neither pass, so the split decides only beyond it.

### 4.4 Sky camera (engine+0x193ea0)

Built per frame when sky portals are visible (5.3): near 0.01, far `sky_far_z` (default 10000.0), rectangle = screen
bounds of the visible sky portals, same rotation as the main camera, translation taken from a second position in the
camera context (+0x35c..+0x364), and a uniform world scale `sky_scale` (default 1.0) applied to the sky geometry.
That position is the **eye in sky space** (EVIDENCE engine+0x193ea0, engine+0xfb8c0; Appendix M, M1): per axis
`sky_box_min + (sky_box_max - sky_box_min) * t` with `t = (eye - world_min) / (world_max - world_min)` for the game scene
(mode 1) and `t = 0.5` otherwise; `world_min` / `world_max` are the world extents (FMT_DAT85 4 items 3-4) and the sky box is
floats 6..11 of the 12-word scene block, which `S2C_SKY_TABLE` fills (UDP_SESSION_2009_A 4.8). What the server puts there is now known (REFERENCE + ROM on two
worlds, `UDP_SESSION_2009_B.md` 8.5; capval-rust3 3.2, folded by spec-fold-8): floats 6..11 = `pos - dims x inner`, `pos + dims x inner` of one sky world-model object
(its sky dims and inner fractions; this confirms M1) and floats 0..5 = `pos - dims`, `pos + dims`, the outer sky box. The client's use of floats 0..5 and the main
context's +0x368 argument stay UNPROVEN. DeCombat's server sends that block (DSV-45, PENDING), so the zero-size-box fallback below applies only without a table. DeCombat: without a server block the sky box is a zero-size box at the centre of the
sky world models' geometry (no sky parallax; drawn without seams on 21 of 21 worlds, not compared with the 2009 client).

## 5. World rendering

### 5.1 World pass order (engine+0x16b620 -> engine+0x10fb70)

1. When a world is loaded and `draw_world` (default 2) is non-zero: collect visible objects from the world tree
   (engine+0x1579f0) and the always-visible objects (engine+0x16b580: render-object groups enabled, object visible bit
   0x1 at +0x84, not hidden bit 0x20 at +0x88).
2. Render-block visibility (engine+0x10fb70): a stack walk over the render-block tree of FMT_DAT85 10.2 (child mask
   and child indices), testing each block box against the frustum of 4.1 (engine+0x10ecb0) and against the block's
   occluder polygons (engine+0x108100). Debug settings replace the walk: draw all blocks, only the camera's block, or
   the frustum blocks without occluders.
3. Sky (5.3): the screen bounds of the sky portals of visible blocks are accumulated (engine+0x17cdc0); when the
   bounds are wider and taller than 0.9 pixel the sky pass runs before any world section.
4. State for the world sections: texture coordinate index of stages 0..2 = 0, 1, 2 (saved and restored), CULLMODE =
   CCW (counter-clockwise faces are culled: front faces wind clockwise on screen), software vertex processing as in
   2.3, ALPHAFUNC = GREATER.
5. When `draw_world_blocks` (default 1): every visible block refreshes its geometry and light-group lightmaps if dirty
   (engine+0x17cd90, 6.2) and queues its sections into shader buckets (engine+0x17e110); then the 23 buckets are
   flushed in index order 0..22 (vtable slot 8 of each bucket object, engine+0x10fb70 loop over block +0x88).
6. World models (moving BSP objects) are drawn later by the object queue (7.1) with the same section renderer: WORLD
   = object transform (engine+0x1a9610), all blocks of the model's nested render world (FMT_DAT85 10.1), WORLD reset
   to identity. Translucent world models use the alpha state of 7.1, additive ones SRC = SRCALPHA, DST = ONE with
   FOGCOLOR black (engine+0x1a9790). OBSERVED (Appendix Z R5): nested render worlds are stored in world coordinates
   (every block of every nested render world, 1,913 render models, lies inside the BSP box of the same-named model; 0
   blocks outside), so the object transform is the identity until the object moves. Which world models the main view
   shows is decided by object properties (the game turns them into engine flags): visibility flag `8411f5abb4c710b1/7`
   false on 1,183 of the 1,913 world-model objects (all 1,008 kill volumes, 47 ladders, 20 hazard volumes, 108
   others); `7e6fefff0f0aeb6c/6` or `f5e282252cf55a76/11` true on 3. Blend-mode text property `f6a07c0a7faf40de/9`:
   opaque value `dc937b59892604f5/4` on 463, translucent `fa5668c88f92faf4/11` on 202, additive `765fc5369dc160b0/8` on
   145; a real alpha property `b1a96dd646bccaa2/5` takes 0, 0.3, 0.5, 0.7, 0.75 or 1. 710 kill-volume models carry an
   editor texture that exists in no archive; drawing them would show large untextured boxes, which the visibility flag
   explains.

DeCombat: wgpu `FrontFace::Cw`, `Face::Back` culling for world sections; the coordinate system stays left-handed.
OBSERVED (Appendix Z R8): this reproduces the expected picture on all 21 worlds and in the synthetic test (a quad wound
clockwise on screen is drawn, the counter-clockwise one is culled).

### 5.2 Section kinds, shader buckets and passes

The u8 `shader_kind` of a section (FMT_DAT85 10.2) is mapped at load (switch at engine+0x181be4, jump table VA
0x5e20e4) to an internal id stored in the section (+0xc):

| file kind | internal id | load check | ROM sections | ROM triangles | role (our reading of the passes below) |
|---|---|---|---|---|---|
| 1 | 0 | none | 16,593 | 866,837 | vertex-lit: texture x vertex colour |
| 2 | 14 | none | 6,274 | 555,882 | lightmap: carries the lightmap payload, drawn opaque first |
| 4 | 15 | first texture required | 22,233 | 544,586 | lightmapped texture: multiplied over the lightmap |
| 8 | 10 | both textures required | 611 | 16,633 | two-texture blend by vertex alpha, vertex-lit |
| 9 | 20 | both textures required | 1,094 | 11,296 | two-texture blend multiplied over the lightmap |
| 0, 3, 5, 6, 7, other | 24 | - | 0 | 0 | never drawn (bucket selector returns none) |

OBSERVED over all 21 worlds: every section with a lightmap payload is kind 2 (6,274 of 6,274), and the triangle count
of the kind-2 sections equals the sum of kinds 4 and 9 (555,882 = 544,586 + 11,296): the lightmap sections duplicate
the geometry of the lightmapped texture sections.

Bucket selection (engine+0x136780; the decompiler table ends that function after 0x44 bytes, the switch bodies and
the lazy constructors continue in undefined code up to about engine+0x136fa0, section 15). Let `mode` be the DTX
command mode of the section's first texture (FMT_DTX 7: 1 detail, 2 env, 3 env-alpha, 4 env-bump,
5 env-bump-no-fallback, 6 dot3-bump, 7 dot3-env-bump, 8 effect, 0 none) and `hi` = DTX flag 0x1 of that texture
(renderer record bit 0x01, FMT_DTX 5; our reading: the alpha channel is a self-illumination mask, see the recipes):

- id 24: none. id 10: bucket 10. id 20: bucket 20. id 14: bucket 14, but none when `draw_flat` is on or `lightmaps`
  is off.
- `draw_flat` on: ids 0 and 15 -> bucket 0 (flat).
- id 15 with `lightmaps` on: none when `lightmaps_only` or when the section has no texture; mode 1 -> 16 if
  `detail_textures` else 15; mode 2 -> 17 if `env_maps` else 15; mode 3 -> 15; mode 4 -> 18 if `env_bump` else (17 if
  `env_maps` else 15); mode 5 -> 19 if `env_bump` else 15; mode 6 -> 21 if `dot3_bump` else 15; mode 7 -> 22 if
  `dot3_env_bump` else (17 if `env_maps` else 15); any other mode -> 15.
- id 0, or id 15 with `lightmaps` off: no texture -> 0; mode 1 with `detail_textures` -> 8 if `hi` else 2; mode 2
  with `env_maps` -> 9 if `hi` else 3; mode 3 with `env_maps` -> 9 if `hi` else 4; mode 4 -> with `env_bump` 9 if `hi`
  else 5, without it 9 if `hi` else 3 (no `env_maps` check on this path); mode 5 with `env_bump` -> 9 if `hi` else 6;
  mode 6 with `dot3_bump` -> 7 if `hi` else 11; mode 7 -> with `dot3_env_bump` 9 if `hi` else 12, without it 9 if
  `hi` else 3; mode 8 -> 7 if `hi` else 13; every case not listed -> 7 if `hi` else 1.

Buckets are created lazily (engine+0x136780 second half) and cached per world in the block-list object at +0x88.
Each bucket class derives from one of 16 vertex-format base classes (vtables VA 0x6881e8 and 0x6882b0..0x6889b0, step
0x80); the base flush (vtable slot 8) sets the stream format and walks the queued batches calling slot 19 (pass
setup), 20 (per batch: bind textures, ALPHATESTENABLE from the batch key), 21 (texture-script transforms, 10.4), the
indexed draw, 22 (extra draw for translucent batches) and 23 (pass restore).

Vertex formats (PUBLIC FVF bits; "3D" = three-component texture coordinate set):

| base | FVF | stride | attributes | used by buckets |
|---|---|---|---|---|
| 0 | 0x42 | 16 | position, colour | 0 |
| 1 | 0x10142 | 28 | position, colour, uv0 (3D) | 1, 7 |
| 2 | 0x50242 | 40 | position, colour, uv0, uv1 (3D, 3D) | 2, 8 |
| 3 | 0x10152 | 40 | position, normal, colour, uv0 (3D) | 3, 4, 9 |
| 4 | 0x50252 | 52 | position, normal, colour, uv0, uv1 | 5, 6 |
| 5 | 0x50242 | 40 | position, colour, uv0, uv1 | 10 |
| 6 / 7 | 0x10152 / 0x50252 | 40 / 52 | as 3 / 4 | 11 / 12 |
| 8 / 9 | 0x10102 | 24 | position, uv0 (3D) | 14 / 15 |
| 10 | 0x50202 | 36 | position, uv0, uv1 | 16 |
| 11 | 0x10112 | 36 | position, normal, uv0 | 17 |
| 12 | 0x50212 | 48 | position, normal, uv0, uv1 | 18, 19 |
| 13 | 0x50242 | 40 | position, colour, uv0, uv1 | 20 |
| 14 / 15 | 0x10112 / 0x50212 | 36 / 48 | as 11 / 12 | 21 / 22 |

The vertices are converted from the 68-byte file vertex (FMT_DAT85 10.2), whose layout is OBSERVED as: position
(3 f32 at 0), uv0 (2 f32 at 12), uv1 (2 f32 at 20), packed colour (u32 ARGB at 28, alpha 0xff in 97% of sampled
vertices), normal (3 f32 at 32, unit), two more unit vectors (3 f32 at 44 and 56; tangent frame for the bump
buckets). Which file field feeds the third texture component is UNPROVEN.

UV usage per section kind (OBSERVED over the 21 worlds by the dc-render census, Appendix Z R1): for kind-2 (lightmap)
vertices uv0 (file offset 12) is the LIGHTMAP coordinate, range [0.00195, 0.99805] = [0.5/256, 1 - 0.5/256] (texel
centres of the largest lightmap side), and uv1 (offset 20) is 0 in every sampled vertex. uv1 is zero on kinds 1, 2 and 4
and non-zero only on kinds 8 and 9, where it is the coordinate set of the second texture (consistent with TEXCOORDINDEX
stage 1 = 1 in 5.1 step 4). Kind-2 sections do not share vertices with kinds 4/9: in 0 of the lightmapped blocks does a
kind-2 triangle use the same vertex triple as a kind-4/9 triangle (the duplicated geometry above is duplicated vertices
too); positions are equal, so the multiply pass passes LESSEQUAL. Section triangle ranges: the `section_triangles` of a
block's sections sum to the block's `n_triangles` in every block, sections take consecutive ranges in file order
(FMT_DAT85 10.2). Every kind-2 section names one first texture (one name hash for all 6,274) that resolves in no
archive; the lightmap pass does not bind it. 3 distinct section texture names end in `.spr` (FMT_SPR_LTC_FX 2.1); their
first frame resolves to a DTX. Verified on screen in the DeCombat renderer (wgpu) only; not yet compared with the 2009
client.

Pass recipes (slot 19 of each class, linear-sweep decode with the branches resolved from the machine code). Notation:
`T0..T3` texture of stage 0..3, `D` vertex colour, `F` texture factor, `x2` = MODULATE2X when `saturate` (default 1)
else MODULATE; `fb` = `full_bright`; `F_lm` = `lightmap_tfactor` when non-zero else 0xffffffff. All recipes keep the
world default state (Z test LESSEQUAL with write, alpha test GREATER with the texture's reference, 10.3).

| bucket | class vtable (VA) | evidence | colour | alpha | blend |
|---|---|---|---|---|---|
| 0 | 0x688eb0 | engine+0x196fd0 | `D + D` (ADD) when `saturate`, else `D`; no texture | `D` | none |
| 1 | 0x688a30 | engine+0x197360 | `T0 * D` (MODULATE4X if `fb`, else x2); then `* F` on stage 1 when `lightmap_tfactor` is set | `T0 * D` | none |
| 2 | 0x688ab0 | engine+0x1979c0 | `T0`, then detail `T1` combined ADDSIGNED (`detail_add`, default 1) or MODULATE, then `* D` (x2) | `T0 * D` | none |
| 3 / 4 | 0x688b30 | engine+0x1983b0 | `T0`, env `T1` ADDSIGNED (`env_add`, default 1) or MODULATE; bucket 4: MODULATEALPHA_ADDCOLOR (`T0 + T0.a * T1`); then `* D` (x2) | `T0`, then `D` | none |
| 5 / 6 | 0x688bb0 | engine+0x198c40 | bump-env: stage 0 BUMPENVMAP on the bump texture, stage 1 env `T1`, stage 2 base added or modulated (`env_add`), stage 3 `* D` (x2) | `T2 * D` | none |
| 7 | 0x688f30 | engine+0x1a4950 | `C = T0 * D` (x2), then `lerp(C, T1, C.a)` (BLENDCURRENTALPHA, `T1` on uv0) | `T0 * D` | none |
| 8 | 0x688fb0 | engine+0x1a5400 | as 2, then `lerp(C, T3, T3.a)` (BLENDTEXTUREALPHA, `T3` on uv0) | as 2 | none |
| 9 | 0x689030 | engine+0x1a6100 | as 8 with env instead of detail | as 8 | none |
| 10 | 0x688c30 | engine+0x1995f0 | `lerp(T1, T0, a)` with `a = D.a * T0.a`, then `* D` (x2) | `D.a * T0.a` | none |
| 11 | 0x688cb0 | engine+0x199d10 | DOTPRODUCT3(`T0` normal map, `F` = light vector), MODULATE2X or MODULATE (`dot3_saturate`) with `T1` on uv0, `* D` | dot3 then `* T1` | none |
| 12 | 0x688d30 | engine+0x19a5d0 | as 11, then env `T2` ADD or MODULATE (`env_add`), `* D` | adds `T2.a` | none |
| 13 | 0x68a5d8 | engine+0x1a1b40, 0x19aee0 | effect sections: the effect registered under the DTX effect id; stage-0 texture bound as h:6da5c734d1710125/8, the four DTX effect textures as h:e8f018424eea625e/8, h:d7a8e1d7ae38e0bd/8, h:0625e34cbf24fa92/8, h:28c41b4b0475dbe3/8; game callback, then every pass drawn (no ROM texture uses the effect command) | - | - |
| 14 | 0x6890b0 | engine+0x184c00 | `T0 * F_lm` (MODULATE2X if `fb` else MODULATE), `T0` = section lightmap, address CLAMP | `T0` | none (writes depth) |
| 15 | 0x689130 | engine+0x1850e0 | `T0 * F_lm` (MODULATE2X if `fb` else MODULATE) | `T0` | SRC = DESTCOLOR, DST = SRCCOLOR if `saturate` else ZERO; FOGCOLOR = 0x808080 if `saturate` else 0xffffff |
| 16 | 0x6891b0 | engine+0x1857d0 | `T0`, detail `T1` (uv1) ADDSIGNED or MODULATE | `T0` | as 15 |
| 17 | 0x689230 | engine+0x185fc0 | `T0`, env `T1` ADDSIGNED or MODULATE | `T0` | as 15 |
| 18 / 19 | 0x6892b0 | engine+0x186910 | bump-env as 5 without the vertex colour stage | `T2` | as 15 |
| 20 | 0x689330 | engine+0x1872c0 | as 10 without the vertex colour stage | as 10 | as 15 |
| 21 | 0x688db0 | engine+0x187980 | DOT3 as 11 without the vertex colour stage | as 11 | as 15 |
| 22 | 0x688e30 | engine+0x1882a0 | DOT3 + env as 12 without the vertex colour stage | as 12 | as 15 |

Reading of the `hi` buckets 7, 8, 9: the extra stage restores the unlit texture where its alpha is high, which is a
self-illumination mask carried in the alpha channel (181 ROM files have DTX flag 0x1); which texture the engine binds
to that extra stage is UNPROVEN (the base texture is the consistent choice).

Reading of the lightmap model: bucket 14 writes `lightmap * F_lm` opaquely; the lightmapped texture buckets then
multiply the framebuffer by their texture (`dst = dst * src`, or `2 * dst * src` with `saturate`), with the fog colour
set to the neutral value of that multiply so fog applied in both passes does not double. Translucent batches (slot 22,
for example engine+0x195020 and engine+0x1852c0) are drawn again with SRCALPHA / INVSRCALPHA blending and stage-0
colour SELECTARG1.

DeCombat (wgpu): one pipeline per (bucket, blend, alpha-test) with the combiner written as WGSL from the table; the
lightmap pass and the multiply pass can be fused into a single pass sampling both textures (`base * lightmap * 2`),
which is equivalent when the fog colour rule above is reproduced; keep the two-pass form available for differential
screenshots.

### 5.3 Sky (engine+0x193ea0, objects at engine+0x1b4d50)

With `draw_sky` (default 1) and at least one sky object in the scene description: build the sky camera (4.4), upload
its transforms, draw the sky objects with Z test and Z write off, fog range `sky_fog_near` / `sky_fog_far` (defaults
0 / 2000), stage 1 disabled: first sky world geometry (section renderer), then sky poly grids and sky sprites with the
translucent state of 7.1; restore the main camera; then clear depth only (3.2 with flags = depth) so the world draws
over the sky. The sky is drawn only into the screen bounds of the visible sky portals.
Details (EVIDENCE engine+0x1b4d50, Appendix M, M2): only sky objects with visible bit 0x1; world models through the opaque
state when alpha is 255 and object flags (+0x88) & 0x42 are clear, else the translucent state (additive with bit 0x2).
OBSERVED: 24 sky world models over the 21 worlds (1 or 2 per world), all 119 of their sections vertex-lit (kind 1), 6 sky
portals per world.
**CORRECTION 2026-10-26 (client-hudgaps):** the clouds and the sunset glare are painted by these sky-box world models; nothing else is missing from the sky pass. Which sky models draw is chosen by the ids of `S2C_SKY_TABLE`
(resolved to objects, visible bit), and the sky eye sits in the table block's view box (floats 6..11): `UDP_SESSION_2009_A.md` 4.8.

### 5.4 Dynamic lights on the world (bucket slot 15, for example engine+0x19c7b0)

For each dynamic light touching a block, the bucket redraws the affected triangles: FVF with a 3D first coordinate set,
fog off with FOGCOLOR black, Z write off, ZFUNC EQUAL, blend SRC = DESTCOLOR, DST from `saturate` (as bucket 15),
stage 0 = SELECTARG1(attenuation texture), vertex colour carrying the light colour. A square attenuation texture of
`dynlight_texture_size` (default 256) and a second "Z" texture plus an index buffer are created by engine+0x18cf40 /
engine+0x18abc0 (their failure messages name a Z texture); the light position and radius become texture-coordinate
transforms (TEXTURE0 / TEXTURE1, engine+0x18d090 and engine+0x18d0b5, built with the D3DX transpose). This is the
two-texture (XY map times Z map) attenuation scheme; the falloff curve baked into the textures is UNPROVEN (16).
The "Z" texture is read (DISASM engine+0x18aca0..0x18ad2a, made by engine+0x18abc0; addendum S6): 128 x 1, texel 0 = 0xffffffff, texel `i >= 1` = grey
`trunc(255 x max(0, (i - 1) / 126 x 1.2 - 0.2))` with alpha 0xff (constants VA 0x68a2e8 = 1.2, VA 0x676b70 = 0.2, VA 0x673cc8 = 255.0). The XY map (engine+0x18cf40,
which calls engine+0x18c720 and engine+0x18cd80) and the coordinate transforms (engine+0x18d090) are not decoded, so the falloff stays UNPROVEN. DeCombat: not implemented
(no dynamic light sources in the viewer).
Whether a light touches a block is decided per block box and light sphere; `dynlight_objects` / `dynlight_world`
(default 1 / 1) gate the object and world parts in the light object's draw hook (engine+0x1af280), and
`dynlight_backfacing` (default 0) is read by every base-class indexed draw (vtable slot 18, for example
engine+0x112170).

## 6. Lighting model

### 6.1 Static lightmaps (world)

- Lightmaps live only in kind-2 sections (5.2). Payload (FMT_DAT85 10.2 item 7) = run-length packed RGB triples
  (engine+0x4fbb0): repeat until the input is consumed: control byte `c`, `run = (c & 0x7f) + 1`; with `c & 0x80` one
  3-byte texel repeated `run` times, else `run` literal 3-byte texels. Output is `w * h * 3` bytes into a 0x30000-byte
  scratch buffer (256 x 256 maximum). This is the same packing as the light grid (FMT_DAT85 8). OBSERVED: all 6,274
  payloads decode to exactly `w * h * 3` bytes and all `w`, `h` are powers of two (16..256).
- Upload (engine+0x18a250): a one-level managed texture of `next_pow2(w) x next_pow2(h)` (engine+0x183f10) in the
  lightmap device format (VA 0x7e5e40, chosen by `full_colour_lightmaps`, engine+0x101240), filled by the pixel
  converter from a 24-bit source with channel masks R 0xff0000, G 0x00ff00, B 0x0000ff (engine+0x183f40), i.e. bytes
  B, G, R in memory (UNPROVEN until compared on screen).
- Lightmap texture coordinates: the FIRST uv pair (uv0, file offset 12) of the kind-2 vertices, in the texel-centre
  range [0.5/256, 1 - 0.5/256] (OBSERVED, 5.2 and Appendix Z R1; this corrects the earlier reading that took the second
  pair, which is zero on kind 2). Bucket 14 samples stage 0 with it and CLAMP. Not yet compared on screen with the 2009
  client.

### 6.2 Light groups (switchable baked lights)

Render light groups (FMT_DAT85 10.3) add to the lightmaps at run time (engine+0x17b6d0): for every lightmap section
of a block and every light group with patches for that section whose colour (group colour times intensity, truncated
per channel) is not black: decode the section's base lightmap (6.1) once, then for each patch rectangle (x, y, w, h,
data) add `colour_c * value` to each texel channel with saturation at 255, where `value` comes from the patch bytes
with a run escape (a byte 0xff followed by a count repeats the previous added value that many texels). Then re-upload
(engine+0x184bc0). The scaling of `value` against the colour is UNPROVEN (the multiply happens before a float to int
conversion that the decompiler hides). OBSERVED over the 21 worlds (Appendix Z R4): each light group has exactly one slot
per section of its block; patches occur only in slots of kind-2 sections; every patch rectangle `(x, y, w, h)` (word
order as in FMT_DAT85 10.3) lies inside its section's lightmap; the escape rule yields exactly `w * h` values in all
4,364 patches (2 worlds); group colours are three floats in [0, 1] (for example (1.0, 0.967, 0.846)). The DeCombat
renderer applies `texel_c += trunc(255 * colour_c * intensity) * value / 255` with saturation and intensity 1 for every
group; the division by 255 is our reading only (open item 4), and with it the one dark world becomes a plausibly lit
interior. Light-group grids (FMT_DAT85 10.5) are the matching contribution to the light
grid used by models (6.3), traced since spec-fold-6 (addendum S3; EVIDENCE engine+0x4ebd0 sampler -> engine+0x4e6a0 group add -> engine+0x4e370 one cell, grid load
engine+0x4f6a0):

- Each light-group grid keeps its colour (initially the file colour; the game switches a group by changing it), its box in main-grid cells (offset .. offset + dims) and one
  byte per cell, x fastest, then y, then z.
- Grids with a colour channel below 1/255 are kept at the end of the list; the add loop stops at the first of them.
- After the sampler has gathered the eight corner samples of the main grid, each corner gets, for every grid whose box holds the corner cell `(i0 + dx, i0y + dy, i0z + dz)`
  (clamped base index, +0 / +1, without the main grid's edge rule), `colour_c x byte`; then the trilinear blend and the clamp to 0..255 of 6.3.
- OBSERVED: 4 grids on 2 worlds (2 each), 25 lit cells; one grid has a zero green channel and so adds nothing.
- DeCombat (REN-9 proposed; dc-render `AmbientGrid::with_groups`): every group on at intensity 1, the same state as the lightmap side; worldview `--light-groups K` scales
  both.

### 6.3 Models: ambient from the light grid plus up to eight hardware lights

Per model (engine+0x1be550 and engine+0x1be930):

- AMBIENT render state = the light-grid colour at the model position (engine+0x33b70 -> engine+0x4ebd0 over the grid
  of FMT_DAT85 8: cell = `(p - origin) * (1 / cell_size)` per axis, truncated and clamped to [-1, dim - 1], then a
  trilinear blend with the fractions of the unclamped position, the step to the neighbour 0 at the last index or below 0,
  each channel clamped to 0..255; packed as 0xff RGB with the **first stored byte as red**; grey 0x80 without a grid; DISASM
  engine+0x4ef00..0x4f168, Appendix M, M5), plus the accumulated ambient of the light set below.
- Dynamic lights from the visible-light list (VA 0x7e57c8, count VA 0x7e57c0) whose sphere overlaps the model's
  bounding sphere, then static lights found through the world light tree, each as a fixed-function POINT light
  (engine+0x1be740): diffuse = specular = light colour / 255 (alpha 1), ambient 0, position = light position, range =
  radius squared (as coded), falloff 1, attenuation (1, 0, 19 / radius^2), i.e. intensity `1 / (1 + 19 d^2 / r^2)`.
  At most 8 lights are enabled; the rest are disabled.
- The light list is built by engine+0x1b21b0 with `light_models` (1), `model_light_limit` (4), `model_sun` (1),
  `model_sun_variance` (0.05), `model_ambient` (1), `model_light_skip_root` (0); `model_saturation` (1.0) is applied
  in engine+0x1b16e0. LIGHTING is turned on for model pieces (engine+0x1b94e0) and per render-style pass (7.3).
- The set itself (EVIDENCE engine+0x1b21b0, 0x1b3ca0, 0x1ba460 and their helpers; Appendix M, M3): capacity
  min(`model_light_limit`, 8); `light_models` off or object flag 0x200: ambient white, no lights; otherwise the ambient
  starts at 0 and takes the grid sample, the sun (a directional record scaled by a visibility factor, 6.3.1),
  every dynamic light without flag 0x20 and the static lights of the world light tree; records are kept sorted by
  (1 - f) x their strongest channel at the position (f = convert-to-ambient fraction), a record that does not fit adds its
  whole contribution to the ambient. Falloff per record kind: 1 / (c0 + c1 d + c2 d^2), 1 - d / r, or (1 - d^2 / r^2)^2,
  zero outside the radius; spots scaled by the cone. The static records come from the world's light objects (three
  classes and their properties, M4; OBSERVED: 695 records over the ROM, every one with the quartic falloff).

DeCombat: evaluate the same lighting in the model vertex shader (ambient + N point lights with that attenuation); the
D3D fixed-function lighting equations are PUBLIC (Direct3D 9 "Mathematics of Lighting").

### 6.3.1 The global (sun) light (folded from the render-styles addendum S2 / S2.1, spec-fold-6)

- **Client side (EVIDENCE):** `S2C_DIR_LIGHT` (id 22, reader engine+0xa31f0, `UDP_SESSION_2009_B.md` 8.4) hands `dir` to the world interface method +0x144, `color` to
  +0x14c and `extra` to +0x154. The model light builder (engine+0x1b21b0) reads them back from the world interface data at +0x148..+0x150 (direction), +0x154..+0x15c
  (colour) and +0x160 (convert-to-ambient fraction): when `model_sun` is on and the colour is not black it adds one directional record of colour x the sun visibility
  factor and inserts it with the fraction `extra`, the share that goes to the ambient (the same fraction the light records of 6.3 carry). So `extra` = **convert-to-ambient**.
- **Visibility factor (EVIDENCE engine+0x1b1cc0 / engine+0x1b1ba0):** a ray from the model position along the sun offset is traced against the world; the factor is 1 when the
  surface hit has the sky flag (surface +4 bit 4), else 0, with a bisection over `model_sun_variance` (0.05) between two test points for soft edges; it is re-evaluated when the
  model moved more than `model_sun_variance` + 0.001. DeCombat (REN-7 proposed): factor 1 for every model (no ray test; `render.sun_visibility`).
- **Server side (EVIDENCE engine+0x9c030; engine+0x97000 / engine+0x97020 = server interface slots +0x1c4 / +0x1c8, filled by engine+0x9b1f0):** the game module registers
  "the global light object" (host +0x7b4); registering a new one sets option bit 0x200 on every client, which makes engine+0x8d6c0 send `S2C_DIR_LIGHT`: `dir` = the rotated +Z of
  the object's rotation, `color` = the light colour (+0x10..+0x18) x two scales (+0x1c, +0x20), `extra` = +0x24 (the object's server light data through the server interface table
  registered under `bd0fb80c9303b9b7/17`).
- **Which world object (OBSERVED, PLAUSIBLE):** the 2009 server's game module is not in the client ROM, so its choice is not visible. Exactly one world class carries the message's
  inputs and nothing else of a light: class `7cea843ab1383e94/14`, one object on 19 of the 21 worlds (none on the other two), properties {name, position, euler rotation
  `57b5e2fc1bba18bb/8`, an int, colour `9a072808beba8a7a/10` (0..255), bright scale `1ae3f3a79e685b27/11`, object bright scale `639a93a207df88a6/17`, convert-to-ambient
  `dd6d73b963113d14/16`}; the client's own light scan (engine+0xcea90) ignores the class. All 19 point down (dir.y -0.50 .. -0.96), colour x scales 22 .. 314, fractions
  0 .. 0.5. PLAUSIBLE: this is the global light.
- **DeCombat client (REN-11 proposed; dc-render `global_light`):** without a server message (offline viewer, or a server that sends none) dir = the euler rotation (the same
  composition as the other light objects, order UNPROVEN, 16 item 19) applied to +Z, colour = colour x bright scale x object bright scale (a missing scale reads 1), extra =
  convert-to-ambient (missing: 0); worldview uses it as the default sun (`--no-sun` / `--sun` override).
- **DeCombat server (D, DSV-40 proposed, `DC_DEFAULTS.md` PENDING; former working name DSV-next-dirlight; assigned to the dserver-light worker):** at world load take the first
  object of that class (hash match); none: send no message (the client's colour stays black and it adds no sun; PLAUSIBLE, the 2009 initial value at +0x154 is zero-initialised
  world data). Else send `[22][dir: 3 x f32][color: 3 x f32][extra: f32]` reliably to each client once its world is loaded (with the other world-bound tables,
  `UDP_SESSION_2009_A.md` 5) and to every client when the light changes: `dir = q * (0, 0, 1)` for the object's euler rotation `q` (the writer's formula
  `(2(wy + zx), 2(zy - wx), 1 - 2(y^2 + x^2))`, B 8.4), `color` = colour (0..255 per channel, like the static light records, which the client divides by 255 when it uploads lights,
  6.3) x bright scale x object bright scale, `extra` = convert-to-ambient (0 when missing).
- **REFERENCE-CONFIRMED (spec-fold-6, 2026-10-25):** the reference server sent one `S2C_DIR_LIGHT` per link, reliable, during the join (tdm2 seq 271 / 742 / 17353, tdm4 seq 278 / 807;
  232 bits each), the real client accepted it, and its seven floats are **exactly** the seven that the rule above (dc-render `global_light`) computes from the captures' world
  `0ce83218ed0d3fa3` (every difference 0.0; compared at run time by a scratch program, values not reproduced). So the reference stack took the same world object, the same rotation
  composition, the 0..255 colour units and the convert-to-ambient fraction; the class choice is therefore REFERENCE-CONFIRMED for the reference server, and the 2009 server's own
  choice stays unknown.

- **DSV-40 implemented** (D, proposed, details staged by dserver-light; `dc-dserver` `worldlight.rs`, merged at master 7fbad44; TESTED `session_sim`, ROM-gated test over the
  21 worlds and a binary run on a ROM world): the object is found at world load (19 of the 21 ROM worlds carry it, 2 none: no message); its direction is a unit vector
  pointing down on all 19; the highest colour channel after the two scales is 314.29, sent **unclamped** (whether the client clamps is open, item 33); sent once per client
  after the preload messages and the optional sky table, before the view object (`UDP_SESSION_2009_A.md` 5.7, 9.2 S8b); `-globallight world|off`. No re-send: this server never
  changes the light.

### 6.4 Sprites and particles near lights

`dynlight_sprites` (default 1, engine+0x1af530) lets dynamic lights tint sprites; particles can be lit (LIGHTING on in
engine+0x1bb310) using the same ambient-plus-point-lights setup as 6.3.

## 7. Models and the object queue

### 7.1 Object queue order (engine+0x169a70)

Visible objects are kept in per-type lists of a singleton (engine+0x16aa00; list = base, count at base+4). Order:

1. Model pre-pass (engine+0x1b29b0): reads `draw_player_view`, `draw_shadow_casting_lights`, `all_model_shadows`,
   `shadow_proj_on` and decides player-view drawing and which models cast projected shadows (details UNPROVEN).
2. Solid world models (list +0x9c, 5.1 step 6).
3. Solid poly grids and the first translucent poly-grid list (+0x108, +0x12c; engine+0x1aefd0).
4. Solid volume effects (+0x1bc), solid canvases (+0x204).
5. Model shadows (engine+0x1749c0, engine+0x174480; 7.5).
6. Solid models (+0x0c; engine+0x1a9570), optionally after the stencil mark of 7.6.
7. Translucent state on (engine+0xf65f0 with alpha mode): stage 0 MODULATE(TEXTURE, DIFFUSE) for colour and alpha,
   stage 1 off, ALPHABLENDENABLE on, ZWRITEENABLE off, SRC = SRCALPHA / DST = INVSRCALPHA (additive variant ONE / ONE).
8. Translucent poly grids (+0x12c, +0x150) drawn directly; then particles (+0x198), late poly grids, line systems
   (+0x174), translucent world models (+0xc0), translucent models (+0x30), translucent canvases (+0x228),
   translucent sprites (+0x54) and translucent volume effects (+0x1e0) are pushed into one priority queue.
9. The queue is drawn (engine+0x1699a0): key = squared distance from the eye (camera context +0x350) to the object
   position (+0xc8), or from the origin for eye-attached objects (flag 0x40 at +0x84) (engine+0x1691f0); `sort_translucent`
   (default 1) off draws in submission order. Model entries save and restore ALPHABLENDENABLE around their draw.
   Direction: back to front (UNPROVEN on the comparator, the heap code is at engine+0x169480).
10. Sprites that ignore depth (+0x78) with ZENABLE off (engine+0x1b14d0).
11. Translucent state off (engine+0xf6730): ALPHABLENDENABLE off, ZWRITEENABLE on.

Translucent and additive world models (EVIDENCE engine+0x1a9790, Appendix M, M9): additive ones (+0x88 bit 0x2) set
SRCBLEND SRCALPHA, DESTBLEND ONE and FOGCOLOR 0 around the normal section renderer; translucent ones draw under the state of
step 7. OBSERVED: 339 visible blended world models (194 translucent, 145 additive).

Object blend helper used by sprites, particles, poly grids and canvases (engine+0x1abc80): object flags at +0x84 bit
0x80 turn fog off for that object; flags at +0x88: bit 0x2 additive (ONE / ONE, FOGCOLOR black), bit 0x4 multiply
(ZERO / SRCCOLOR, FOGCOLOR white), otherwise SRCALPHA / INVSRCALPHA with the scene fog colour.

### 7.2 Model pieces (engine+0x1b94e0)

- Per model pass: LIGHTING on, NORMALIZENORMALS off, DEPTHBIAS saved at the start and restored at the end (with
  WORLD reset to identity and the default state re-applied); WORLD = model transform per model; eye-attached models
  switch to the player-view camera (4.2) and back (per piece, by the piece record's flag byte +0x14; PLAUSIBLE, 4.2). The code that changes DEPTHBIAS in between was not located
  (UNPROVEN: a render-style or per-piece value; `zbias_style_passes` has no reader).
- Each piece is drawn with its render style (7.3) or, when the piece references an effect, through the effect path:
  for stages 0..3 the engine binds the piece texture to the effect parameter whose name is built from the format
  string `h:913720f4d1743ff3/9` and the stage number (the stage-0 result is `h:6da5c734d1710125/8`), calls the game
  module's callback (method +0x88 of the interface object at VA 0x7eeb28) with the engine effect object, the render
  object, the piece and the current camera, then runs every pass (ID3DXEffect Begin / BeginPass / EndPass / End,
  PUBLIC method order). NORMALIZENORMALS is toggled per piece from a piece flag.
- `texture_models` (1) off draws untextured; `wireframe_models` forces WIREFRAME in the render-style pass.

### 7.3 Render styles (RS.rez `.ltb` render-style files)

A render-style pass (engine+0x1a88f0) sets: LIGHTING (with a material from the style, +0x1094) and SPECULARENABLE;
ALPHATESTENABLE with ALPHAFUNC; ZFUNC; FILLMODE; ALPHABLENDENABLE with SRCBLEND / DESTBLEND; ZENABLE and ZWRITEENABLE;
CULLMODE; TEXTUREFACTOR; per stage COLOROP / ALPHAOP, colour and alpha arguments, TEXCOORDINDEX (incl. generated
coordinates) and U/V addressing. The file stores the engine's own enum numbers; they are translated through tables
in .bss (VA 0x7e8318 colour op, 0x7e835c alpha op, 0x7e8388 / 0x7e8398 arguments, 0x7e83a8 fill, 0x7e83b0 cull,
0x7e83c4 / 0x7e83f0 blend, 0x7e841c compare, 0x7e8474 address) filled once by engine+0x1b50d0. The file format, the per-pass and
per-stage state, the translation tables and the ROM census are `FMT_RENDERSTYLE_2009.md` (spec-fold-6, addendum S1): render-style object vtable VA 0x689870, loader slot 12
engine+0x161bb0 (DISASM), pass setup engine+0x1a88f0, texture binding engine+0x1a8240, default style engine+0x162890 (used for empty or unloaded slots), piece lookup
engine+0x1b9390. RS.rez holds 61 render-style files of 637..1,723 bytes plus one zero-length entry; all 61 re-encode byte for byte with that reader (OBSERVED).
`multipass_light_match` (0) is read by the pass code; `zbias_style_passes` (1) has no reader.
DeCombat (since render-styles, master 5610c8b): models draw with their own styles (`FMT_RENDERSTYLE_2009.md` 10; REN-1..REN-6 proposed). The former state of
Appendix M, M10 (no culling, stage 0 MODULATE) is **superseded**: the default character's skin pieces use MODULATE2X with an emissive material, so the brightness gap of
the plain state is closed by the data (`FMT_RENDERSTYLE_2009.md` 9; item 24 closed). The culling of a model pass is the style's (2.4 note).

### 7.4 Skinning

- Non-indexed blending (engine+0x15fae0): per bone set up to 4 bone indices (0xff ends the list); WORLDMATRIX(k) =
  bone matrix; VERTEXBLEND = 1WEIGHTS / 2WEIGHTS / 3WEIGHTS for 2 / 3 / 4 bones, DISABLE for 1 bone, or 0WEIGHTS
  (0x100) for 1 bone when `use_zero_weights_for_disable` is set.
- Indexed blending (engine+0x15f450, engine+0x160a60): matrix palette range written to WORLDMATRIX(i) (optionally
  remapped), INDEXEDVERTEXBLENDENABLE on, VERTEXBLEND from the weights per vertex, software vertex processing when the
  device lacks enough palette entries (`force_sw_vertex` forces it).
- Vertex-shader skinning exists for effect pieces (engine+0xf31b0 sets shader and declaration; engine+0xf2c60 uploads
  96 vec4 constants from a constant block).
- D3D semantics (PUBLIC): with `n` blend weights stored per vertex the last weight is `1 - sum`; positions and normals
  are blended in world space.

DeCombat: one WGSL skinning path with a bone palette and four weights per vertex reproduces all three.

Animation data and the three trackers (Appendix M, M6-M8; OBSERVED over 1,461 ROM models with clips, DeCombat readings):
rotation keys are unit quaternions stored (x, y, z, w) and tracks are parent-relative (frame 0 of the best clip equals the
bind pose: median errors 0.0025 rad and 0.0000 units); positions interpolate linearly and rotations by the shorter-arc slerp
between keys at the marker times. A character's main tracker applies with weight 1 on every node, then tracker 1 with blend
set 0 and tracker 2 with blend set 1 (`UDP_SESSION_2009_B.md` 3.5.1, REFERENCE pairing); per node `pose = mix(pose, clip, w)`.
For the 60-node bodies blend set 1 is almost empty (sum 0.6) while set 2 holds the legs, so the fixed pairing leaves their
legs unanimated by tracker 2: whether the client pairs sets per body or by name is UNPROVEN (item 23). The face model follows
the body's pose node by node name.

### 7.5 Projected model shadows (`shadow_proj_*`, engine+0x16fa10..0x174480, 0x18a450)

- Shadow texture per shadowing model (`shadow_proj_tex_size` 128, `shadow_proj_texture_count` 2,
  `shadow_proj_max` 1, `shadow_proj_per_frame` 1) rendered by engine+0x170070: clear, viewport inset by one texel on
  each side, VIEW = look-at from the light to the model, PROJECTION = orthographic (zn 1.0, zf 20000.0) or
  perspective when `shadow_proj_perspective`, AMBIENT = shadow colour clamped to 0..255; the model is drawn flat into
  the texture.
- Blur (`shadow_proj_blur` 1): four taps averaged by the pixel shader of 11.2 (shadow blur) or fixed function
  (engine+0x174480), sample spacing `shadow_proj_blur_spacing` (1.0).
- Projection onto the world with fade (`shadow_proj_fade` 1, engine+0x18a450), alpha `shadow_proj_alpha` (1.0),
  minimum colour component `shadow_proj_min_colour` (40), maximum distance `shadow_proj_max_dist` (200),
  back-face culling of receivers `shadow_proj_cull_back` (1).

### 7.6 Stencil mark (engine+0x1b7cb0)

With `model_stencil_mark` (default 0): before drawing a model, write stencil 1 where the model fails the depth test
(STENCILFUNC ALWAYS, REF 1, ZFAIL REPLACE, FAIL and PASS KEEP, colour writes off, Z write off), used by
through-wall render styles. Off by default; needs a stencil depth format (2.2).

### 7.7 Sockets and the held weapon (folded from the render-styles addendum S5, spec-fold-6)

- Attach points ("sockets") are FMT_LTB 11.1 records (node, name, rotation, position, scale). DeCombat (dc-render `ModelInstance::socket_transform` / `attach`): socket world
  transform = model world x node pose x (socket scale, rotation, position); the attached object's transform = that x (attachment offset, rotation) of the `CF_ATTACH` entry
  (`UDP_SESSION_2009_B.md` 4.2 item 8). The engine's own attachment update was not traced: PLAUSIBLE.
- OBSERVED (ROM-gated test `rom_weapon_attached_at_the_hand_socket`): the default body's grip socket (name key `c969c8aa86333373/9`) is socket 2 on node 27, the socket the
  reference stack names in every player's attachment list (B 3.5, 3.5.1); in the standing pose its world position lies inside the bounds of that node's vertices and the weapon model
  overlaps them.
- worldview `--character` attaches the third-person model of weapon row `--weapon ROW` (REN-10 proposed: default the lowest row with a third-person model; in the EU-test ROM that
  row is a melee weapon, `--weapon 40` shows a gun). Viewer only: in a session the server's weapon child object is the drawn weapon (B 3.5).

## 8. Sprites, particles, poly grids, line systems, canvases, ClientFx

Render-object types (table VA 0x6d546c, 12 entries; the draw function and the setting it checks first identify the
type, engine RVAs):

| entry | draw | gate setting | our type |
|---|---|---|---|
| 0, 5 | none | - | invisible / camera |
| 1 | 0x1a94f0 (hook 0x1a9330) | `draw_models`, `draw_solid_models`, `draw_translucent_models` | model |
| 2, 9 | 0x1a95c0 | `draw_world_models` | world model, container |
| 3 | 0x1af6b0 | `draw_sprites` | sprite |
| 4 | 0x1af280 | `dynlight_objects`, `dynlight_world` | light |
| 6 | 0x1af040 | `draw_particles` (in 0x1af250) | particle system |
| 7 | 0x1ad0a0 | `draw_poly_grids` | poly grid |
| 8 | 0x16a300 | `draw_line_systems` | line system |
| 10 | 0x1a99c0 | `draw_canvases` | canvas (game-drawn) |
| 11 | 0x1a9c90 (hook 0x1a9b40) | `draw_volume_effects` | volume effect |

### 8.1 Sprites (engine+0x1b1210, quads at engine+0x1af730 / 0x1b0b80)

Camera-facing quads built on the CPU, FVF XYZ | DIFFUSE | TEX1 (0x142, stride 24), drawn as 2-triangle fans with
DrawPrimitiveUP (multi-frame and rotating sprites use longer fans). Blend from the object helper (7.1); sprites
attached to the eye use the player-view camera (4.2). Colour setups: `TEXTURE x DIFFUSE` for colour and alpha with an
optional TFACTOR tint (engine+0x1af4a0); a fade variant (engine+0x1af390) blends toward TFACTOR by the current alpha
with alpha `(1 - T.a) x (1 - D.a)` (decode of the PUBLIC argument modifiers; UNPROVEN as to its use). Sprites that
ignore depth are drawn last with Z off (7.1). Sprite files are FMT_SPR (separate spec).

### 8.2 Particles (engine+0x1af060, batch at engine+0x1bb310, buffers at engine+0x1aabc0)

Particle systems are drawn from a dynamic vertex buffer of 0x23280 bytes (6000 vertices of FVF 0x142) and a static
16-bit index buffer of 9000 indices (1500 quads, two triangles each); blend and fog from the object helper (7.1);
texture addressing CLAMP; LIGHTING on for lit systems (6.4). An effect-shader variant draws with a 36-byte vertex
and binds the stage-0 texture by name (engine+0x1babb0, uses the same names as 7.2).

### 8.3 Poly grids (engine+0x1adaa0, four code fragments)

Height-field surfaces (water) generated on the CPU into a buffer of `poly_grid_buffer` (2048) vertices and drawn
with DrawIndexedPrimitiveUP. Options: `env_poly_grids` (1) environment map, `bump_poly_grids` (0) environment bump
through the ps 1.0 bump shader of 11.2 (constant c0 alpha set by the engine, texture names `h:6da5c734d1710125/8` and
`h:e8f018424eea625e/8` for the effect path), `fresnel_poly_grids` (1) a view-angle alpha term. The bump matrix comes
from engine+0x1b4740: BUMPENVMAT00 = BUMPENVMAT11 = `env_bump_amount * scale`, the off-diagonal terms and the
luminance scale / offset from the same scale. The exact vertex layout and fresnel formula are UNPROVEN (fragmented
function; section 15).

### 8.4 Line systems (engine+0x16a3e0)

WORLD = object transform, texture off, FVF XYZ | DIFFUSE (16 bytes), LINELIST via DrawPrimitiveUP, WORLD restored.

### 8.5 Canvases and volume effects

Canvases call back into the game, which draws through the engine draw-primitive interface (12.3); volume effects
draw dynamic geometry with the object blend helper (engine+0x1abbe0 -> engine+0x1aabc0); `dbg_volume_bounds` draws
their boxes.

### 8.6 ClientFx

ClientFx.fxd creates engine objects of the types above (sprites, particles, poly grids, models, lights, line
systems, volume effects) and sets their flags; it does not talk to the device (OBSERVED: its import table names only
the C/C++ runtime and KERNEL32, no Direct3D module). The renderer behaviour it needs is
therefore fully described by the object types of this section and section 7 (the FX file formats are FMT_SPR_LTC_FX).

## 9. Screen glow and full-screen effects

### 9.1 Screen glow (engine+0x167900 and callees)

Runs after the object queue when `screen_glow_on` (default 0; the game turns it on through the setting) with a square
glow texture of `glow_texture_size` (256):

1. Resources (engine+0x163010): a blend texture, a glow texture (both render-target textures of the requested size)
   and a depth surface of the same size (depth format from 2.2); recreated when the size changes.
2. Glow scene (engine+0x1673c0): save render target, depth surface and viewport; bind the glow texture and its depth;
   viewport = texture size; clear colour + depth to black (retry without stencil); fog: FOGSTART / FOGEND =
   `glow_fog_near` / `glow_fog_far` (0 / 2000), FOGCOLOR black, FOGENABLE = `glow_fog_on` (0). Camera = the scene
   camera (4.1) built for the glow rectangle (engine+0xfd4e0); then the same steps as a scene: object collection
   (engine+0x1579f0), world blocks (engine+0x10fb70), solid world models, models, canvases (engine+0x1a9910,
   0x1a9340, 0x1a9a00), translucent world models and models into the sort queue under the translucent state, the sort
   queue (engine+0x1699a0). Which objects end up black (only glowing parts surviving) is decided by the glow
   variants of the render paths and by the game's texture-to-glow-texture remap table (renderer slots 0x138..0x140,
   which also supply two textures for the glow passes); the per-object rule is UNPROVEN.
3. Blur (engine+0x164e10): build the kernel (engine+0x1638e0): `n = clamp(glow_filter_taps, 0, 64)` (14),
   `half = (n - 1) / 2`; for tap `i` in `0..n`: offset `x = i - half + glow_tap_shift` (-0.1), weight
   `w = glow_amp_a * exp(-glow_radius_a * x^2 / half^2) + glow_amp_b * exp(-glow_radius_b * x^2 / half^2)` with the
   defaults amp a = 0.03, radius a = 0.36, amp b = 0.22, radius b = 4.5 (computed as `amp / 2^(log2(e) * r * x^2 /
   half^2)`); taps with `w <= 0.01` are dropped and the list is padded with zero taps to a multiple of 4. The blur runs
   horizontally from the glow texture into the blend texture, then vertically back: Z off, ZFUNC ALWAYS, alpha test
   off, U/V CLAMP on samplers 0 and 1, first quad overwrites and later quads add (ONE / ONE). With pixel shaders
   (`glow_use_ps`, default 1, and the shader assembled): four taps per quad, FVF XYZRHW | TEX4, the four tap
   weights in pixel-shader constants c0..c3 (each replicated to four components), the glow filter shader of 11.2;
   tap `k` samples at `uv + offset_k * texel * dir`. Without: two taps per quad with fixed-function stages
   (FVF XYZRHW | DIFFUSE | TEX2 = 0x244, twelve stage states per quad, engine+0x164ac0).
4. Composite (engine+0x165eb0): one screen quad (FVF XYZRHW | DIFFUSE | TEX1, stride 28) sampling the blurred glow
   texture with stage 0 SELECTARG1(TEXTURE) for colour and alpha, fog off, Z off, ZFUNC ALWAYS, alpha test off, blend
   ONE / ONE (additive over the frame). `glow_uv_scale` (0.5) is read by the blur driver (engine+0x164e10); whether
   it scales the tap spacing or the composite coordinates is UNPROVEN.
5. Debug views draw the glow texture or the kernel (`glow_show_texture`, `glow_show_filter`).

DeCombat: this maps directly to two wgpu render passes into 256x256 targets plus one additive full-screen draw; the kernel
is computed on the CPU each time the settings change.

### 9.2 Full-screen post effects (game-driven, engine provides the plumbing)

The game module (CShell) loads the six effect files of 11.3 at start (cshell+0x1a4660) with game ids 1..5 and 100,
each with a vertex declaration it supplies, and drives them per pass through the engine effect object (vtable VA
0x689fa0, engine+0x16b9e0..0x16c170): set technique by name (GetTechniqueByName + SetTechnique), set float, float
array, vector and matrix by name, set a texture by name, and set a texture by render-target id (engine+0x16bd30:
render-target manager engine+0x100130, lookup engine+0xff9b0, the target's texture to SetTexture). Per-pass parameter
values are chosen by the game's callback (cshell+0x298dc0, section 11.3). Render targets are created by the game
through the engine (engine+0x1004b0: a one-level RENDERTARGET texture in the DEFAULT pool plus a matching depth
surface) and filled with StretchRect copies of the back buffer (engine+0x100390, engine+0x100410).

## 10. Textures

### 10.1 Device format slots (engine+0x101240) and the choice per DTX file (engine+0x100e50)

The texture manager object at VA 0x7e5dd8 holds three capability bytes (VA 0x7e5dd9..0x7e5ddb: block formats DXT1,
DXT3, DXT5 accepted by the device) and eight 12-byte slots at VA 0x7e5e00 `{enabled u8, device format u32, bytes per
pixel u32}` filled at device creation by probing formats (IDirect3D9 slot 10, usage 0, resource type TEXTURE):

| slot | first choice, fallbacks (PUBLIC D3DFORMAT) | missing => |
|---|---|---|
| 0 | A8R8G8B8 | renderer init fails |
| 1 | A1R5G5B5, A4R4G4B4 | fails |
| 2 | A4R4G4B4, A1R5G5B5 | fails |
| 3 | R5G6B5, A1R5G5B5, A4R4G4B4 | fails |
| 4 | A8R8G8B8, A1R5G5B5, A4R4G4B4 (2D surfaces, 12.2) | fails |
| 5 | lightmaps: with `full_colour_lightmaps` R8G8B8, X8R8G8B8, A8R8G8B8; else (or none of those) X1R5G5B5, A1R5G5B5 | fails |
| 6 | bump: V8U8, else Q8W8V8U8 | warning only |
| 7 | luminance bump: X8L8V8U8 when V8U8 probes ok, else L6V5U5 when V16U16 probes ok (the probe and the stored format differ, as coded) | warning only |

Choice for one DTX (engine+0x100e50, `fmt` = DTX format code, `fl` = DTX flags, FMT_DTX 3 and 5):

1. Block formats 4 / 5 / 6 with `use_bc_textures` (default 1) and the matching capability byte: native DXT1 / DXT3 / DXT5.
2. Otherwise: `fl & 0x1000` -> slot 7; else `fl & 0x800` -> slot 6; else when `fl & 0x2` is CLEAR, `full_colour_textures`
   (default 1) is on and slot 0 exists -> slot 0 (A8R8G8B8); else `fl & 0x100` -> slot 1; else `fl & 0x80` -> slot 2;
   else `fl & 0x1` -> slot 1; else slot 3 (R5G6B5).

Correction to FMT_DTX section 5 (to be applied there): the 32-bit slot is taken when flag 0x2 is clear, so flag 0x2
means "allow the 16-bit path", and flags 0x100 / 0x80 / 0x1 only matter on that 16-bit path. With the shipped
defaults every non-block ROM texture (none has 0x2 except 4 files) is uploaded as A8R8G8B8.

DeCombat (wgpu): code 3 -> `Bgra8Unorm` (bytes B, G, R, A as stored); codes 4 / 5 / 6 -> `Bc1RgbaUnorm` / `Bc2RgbaUnorm` /
`Bc3RgbaUnorm` when the adapter has BC support, else decode to RGBA8 on load. Lightmaps -> `Bgra8Unorm` with alpha 255.
Never reproduce the 16-bit fallbacks.

### 10.2 Upload and level selection

As FMT_DTX 4.2 and 6 (load-time level skip, `max_texture_side` cap (16384), `max_tex_aspect`, upload level count,
square-only devices, cube textures for six-face files, device priority). `keep_textures_resident` (0) is read by the
upload and release paths (engine+0x100760, 0x101f20, 0x101fa0); its exact effect is UNPROVEN.

### 10.3 Per-bind sampler and alpha state (engine+0x1020b0 -> engine+0x100a50)

Binding texture `t` to stage `s` (lazily uploading it first):

- MIPMAPLODBIAS(s) = `mip_bias` (default -1.0) + the texture's own bias from its DTX mip-bias command (default 0),
  cached per stage.
- ALPHAREF = the texture's alpha reference from its DTX alpha-reference command, 0 when absent (engine+0x101b10),
  set on every bind; alpha testing itself is switched by the pass (world batches 5.2, render styles 7.3, draw-prim
  modes 12.3). In the world pass ALPHAFUNC is GREATER, elsewhere the default GREATEREQUAL unless a style says
  otherwise. OBSERVED in the ROM: 617 DTX files carry an alpha reference (FMT_DTX 7).
- Filters and anisotropy per 3.3 for samplers 0..3; addressing WRAP unless the pass sets CLAMP (lightmaps, glow, 2D,
  particles).

### 10.4 Secondary textures from the DTX command (FMT_DTX 7) and texture scripts

- Detail texture: second stage reading the second coordinate set, combined ADDSIGNED (`detail_add` = 1) or
  MODULATE; its coordinates are scaled by `detail_uv_scale` (1.0) through a stage texture matrix (readers in
  undefined code at engine+0x189cc1, 0x19c151).
- Environment map: second stage; for models the coordinates are generated per stage with TEXTURETRANSFORMFLAGS =
  COUNT3 and a texture matrix built from the view rotation and `env_uv_scale` (1.0) (engine+0x1b49f0); for world
  sections the generation mode comes from the bucket's texture-script hook (below) or the vertex data (UNPROVEN which).
- Environment bump: BUMPENVMAP stage with the 2x2 matrix and luminance scale/offset of engine+0x1b4740
  (`env_bump_amount` 1.0, `env_bump_scale` 1.0).
- DOT3 bump: DOTPRODUCT3 of the normal map with TEXTUREFACTOR holding the packed light vector (buckets 11, 12, 21, 22).
- Texture scripts (TEXTURESCRIPTS.rez: 6 `.tfs` files, 55..209 bytes; TEXTUREEFFECTGROUPS.rez: 2 `.tfg` files, 75 /
  140 bytes) drive per-stage texture transforms. The hook (engine+0x194370, bucket slot 21) handles up to 4 stages and
  2 script slots per section; per stage the script returns a coordinate mode mapped to TEXCOORDINDEX generation flags
  (1 -> CAMERASPACENORMAL, 2 -> CAMERASPACEREFLECTIONVECTOR, 3 -> PASSTHRU, other -> CAMERASPACEPOSITION), a flags
  word mapped to TEXTURETRANSFORMFLAGS (0x8 -> COUNT1, 0x10 -> COUNT2, 0x20 -> COUNT3, else COUNT4; 0x4 adds
  PROJECTED), and a 4x4 matrix uploaded as TEXTUREn (transposed). The script byte code and its interpreter are not
  specified here (section 16, item 8).

## 11. Shader programs in the ROM

### 11.1 Fixed function versus programmable

Everything in sections 5 to 10 and 12 is fixed function except:

- the three assembly pixel shaders of 11.2, assembled from text at run time (engine+0xf5080: D3DXAssembleShader
  then CreatePixelShader; the shader object also has a pre-compiled path, but the three ROM shaders are loaded through
  path strings that name the text files);
- assembly vertex shaders with a declaration (engine+0xf3410), used for effect pieces (7.4);
- effect files of 11.3, compiled at run time with D3DXCreateEffect (engine+0x16c170: optional vertex declaration,
  shader flags 0 or, with `effect_software_shaders`, 0x181 (PUBLIC: debug, force pixel shader software no-opt, no
  preshader), an effect pool chosen by pool id (engine+0xfaa10), an include hook pointer at VA 0x6e103c, error text
  logged on failure).

Programmable paths are optional: the glow falls back to fixed function, the bump shaders to plain env mapping, the
refraction effect to a multiply technique.

### 11.2 PS.rez pixel shaders

PS.rez holds 26 entries: 3 assembly sources, 3 byte-identical text copies of them, 3 pre-compiled blobs, and 17 UI
model files unrelated to shaders. Only the sources are loaded (by path, strings below); no engine string names the
copies or the blobs.

| id (sha256[:16] of content) | bytes | engine path string | loaded at | version | computes (our words) | constants set by the engine |
|---|---|---|---|---|---|---|
| 29e33277fc04002b | 316 | h:9a4a779560aaf1dc/17 | engine+0x162d16 | ps 1.1 | `out = sum_i c[i] * tex_i(t_i)` over 4 stages | c0..c3 = blur tap weights, each replicated to 4 components, written by engine+0x1645f0 through engine+0xf4920 (SetPixelShaderConstantF start 0) |
| 86dafb82b750e794 | 338 | h:ab5453f59460d112/17 | engine+0x1745fa | ps 1.1 | `out = 0.25 * sum_i tex_i(t_i)` over 4 stages (weights baked in) | none |
| 196e7668b0e8e54e | 531 | h:f5aefea8f0f418e9/17 | engine+0x1adcde | ps 1.0 | sample a normal map (t0), transform by the 3x3 matrix in texture coordinates 1..3 (eye vector in their 4th components), reflect and sample a cube environment map (t3); `out.rgb = 2 * v0.rgb * env.rgb`, `out.a = c0.a * v0.a` | c0 (alpha), from the poly-grid path (value UNPROVEN) |

Pre-compiled blobs (not loaded): content ids edade6f91fa6a42a (168 bytes), 0d2a512962b91c5b (136), de164ee78c6515c0
(192); layout OBSERVED: 4-byte tag, u32 1, u32 byte length, then Direct3D shader byte code (version token ps 1.1 or
1.0, PUBLIC token format). Text copies: same content ids as the sources.

WGSL rewrite: shader 1 = 4-tap weighted sum with a uniform `vec4 weights`; shader 2 = 4-tap average; shader 3 = normal
map + per-vertex tangent frame + cube map reflection.

### 11.3 SHADERS.rez effects (loaded and driven by the game module)

SHADERS.rez holds 17 entries: 6 effect sources, 9 DTX images (content ids: one 5,570,724-byte image; five of
262,308 bytes forming 3 distinct contents; two of 131,236 bytes with one content; one of 8,356 bytes) and 2 sprite
files (38, 40 bytes). The game loads the effects at start (cshell+0x1a4660) with these ids and runs them through the
engine effect object of 9.2; per-pass values come from the game callback (cshell+0x298dc0). All full-screen effects
share one vertex program: position snapped to the clip-space corners by the sign of its xy, uv = `0.5 * xy + 0.5` with
v flipped; Z test and Z write off; the input render targets are bound by id.

| our name | game id | content id | bytes | rez entry name (upper case, with extension) | techniques / passes | behaviour (our words) | inputs set per pass by the game |
|---|---|---|---|---|---|---|---|
| `fx_focus_blur` | 1 | c329bbcef17e4a66 | 4219 | h:dc04ce223e853a05/8 | 1 / 4 | P0: 4-tap box downsample at +-1 texel diagonals; P1: vertical 3-tap (0.25, 0.5, 0.25) at +-8 texels; P2: horizontal 3-tap; P3: `scene * mask.a + blurred * (1 - mask.a)` | viewport size (2 floats); P0 scene = RT 5; P1 blurred = RT 1; P2 scene = RT 2; P3 scene = RT 5, blurred = RT 3, mask = a game texture |
| `fx_noise_overlay` | 2 | a3a72b4e824b2899 | 2479 | h:1eb0b3a7f4f12b50/9 | 1 / 1 | `scene * noise(uv + offset)` (other declared terms unused) | viewport size; offset = (rand/32767, rand/32767, 0, 0) per frame; scene = RT 0; noise = game texture |
| `fx_night_vision` | 3 | c09679254d074d53 | 2513 | h:cfed04d133c63c47/14 | 1 / 1 | `c = scene; c *= c + (0.5,1,0.5,0); c *= c + (0.9,1,0.9,0); out = c * noise(uv + offset) * 10` | as id 2 with its own noise texture |
| `fx_desaturate` | 4 | 48f0a3e4ea5158f0 | 1062 | h:3db4e83eb3c1a3ce/11 | 1 / 1 | `g = dot(c, (0.3, 0.59, 0.11)); out = (lerp(g, c, amount), 1)`, point sampling | amount = a game console value (default 1.0; file default 0.5); scene = RT 0 |
| `fx_bloom` | 5 | 9905875916c69301 | 5747 | h:51225e80e61a7287/8 | 1 / 4 | P0: downsample (4 taps at 0/2 texels of `0.25 / size`), alpha = `smoothstep(0.8, 1, luminance)`; P1/P2: 7-tap blur (0.05, 0.1, 0.2, 0.3, 0.2, 0.1, 0.05) horizontally then vertically with step `0.125 / size`; P3: `0.95 * scene + 0.5 * blur + 0.0 * blur.a` (half-texel offsets) | viewport size; P0 scene = RT 4; P1 = RT 1; P2 = RT 2; P3 scene = RT 4, blur = RT 3 |
| `fx_refraction_sprite` | 100 | 1823729eb716fd41 | 2743 | h:c55d2245a2da8c48/10 | 2 / 1 each | A (ps 2.0, vs 1.1): normal from the stage-0 texture (`xy * 2 - 1`, z rebuilt), faded toward +z by vertex alpha; sample a copy of the back buffer at the projected screen position plus `0.02 * normal.xy`; `out = refracted * vertex colour`; cull CCW, no blending. B (fallback): multiply blend ZERO / SRCCOLOR | world, view-projection, model-view-projection, model-view and view matrices from the game camera; back-buffer copy bound by the engine (game calls engine methods +0x40 / +0x48 on its render interface) |

Parameter names are interop facts of these files; implementers obtain them from the user's ROM. Name ids (sha16/len)
used by the game per effect: viewport size h:494b3f203b26cf03/13 (ids 1,2,3,5); random offset h:3cb2279868627484/11
(2,3); scene targets h:6ae19cf1cce86ce4/8 (1), h:ede3b48638f90095/10 (2,3,4), h:3ffbe212f0586c91/8 (5); refraction
textures h:6da5c734d1710125/8 and h:e8f018424eea625e/8 (the same names the engine binds for pieces, 7.2), back buffer
h:ac52bbd877da6050/10; matrices h:2e0d3a411563b596/11, h:f1ef5391b015c772/14, h:f9500a4812278518/13,
h:b8a3acd7ea371669/9, h:2bcb43cbc8f6b7ef/4.

WGSL rewrite (DeCombat): each effect becomes a full-screen pass chain over offscreen targets with the numbers above;
render-target ids are our own small enum (0 scene copy, 1..5 game-sized intermediates) created at the sizes the game
requests. The pass inputs/outputs follow the game callback; the render-target sizes come from the game and are not
fixed by the engine (UNPROVEN until the game side is specified).

## 12. HUD and 2D path

### 12.1 Optimized 2D mode (`r_begin_2d` engine+0x176cf0, `r_end_2d` engine+0x176e90)

Begin (only inside a scene): save FOGENABLE, then fog off, Z test off, Z write off, alpha blending on with SRCALPHA /
INVSRCALPHA; save sampler 0 MIN/MAG filters and set them to LINEAR with `bilinear_2d` (default 1) or POINT, or
ANISOTROPIC with `anisotropy_2d`. End: Z test and Z write on, FOGENABLE restored, blending off, filters restored. The
flag at VA 0x7e7ae0 blocks 3D scenes meanwhile (3.3).

2D blend modes (`r_set_2d_blend`, engine+0x176f70, jump table VA 0x5d703c; mode stored at VA 0x7e7f4c, values above 6
rejected):

| mode | SRCBLEND | DESTBLEND | effect |
|---|---|---|---|
| 0 | SRCALPHA | INVSRCALPHA | alpha |
| 1 | ONE | ZERO | opaque copy |
| 2 | ONE | ONE | add |
| 3 | DESTCOLOR | ZERO | multiply |
| 4 | DESTCOLOR | SRCCOLOR | multiply x2 |
| 5 | ZERO | INVSRCCOLOR | darken by inverse |
| 6 | ONE | INVSRCCOLOR | screen |

### 12.2 2D surfaces

Surfaces created by the game become textures in format slot 4 (A8R8G8B8, VA 0x7e5e34) when `surfaces_as_textures`
(default 1) is set (engine+0x1770a0); blits (engine+0x177380 plain, engine+0x177830 scaled, engine+0x177c80 warped)
draw one 2-triangle fan of FVF XYZRHW | DIFFUSE | TEX1 (0x144, stride 28) with blending on in the current 2D mode, and
record the rectangle for the dirty-region present (3.4). Back-buffer copies to and from surfaces use UpdateSurface
(engine+0x175010) and StretchRect (engine+0x100390); a TGA screenshot path reads the back buffer (engine+0x175f80).

### 12.3 Draw-primitive interface (game HUD, menus, canvases)

The game draws its HUD through an engine object whose vtable (81 methods) is at VA 0x6767c0. State is set by
methods and applied before each draw:

- transform mode (object +0x10): screen space uses FVF XYZRHW | DIFFUSE | TEX1 with rhw = 1.0 (positions in pixels);
  the other modes use FVF XYZ | DIFFUSE | TEX1 with the WORLD / camera transforms (engine+0x30220, engine+0x2fef0,
  the latter also offers the player-view camera of 4.2).
- colour op (object +0x14, engine+0x26630): 0 vertex colour only; 1 `T * D` (colour and alpha); 2 `T + D`; 3 `T`;
  4 two textures `T0` then `* T1` (texture coordinate sets 0 and 1), alpha from the textures; 5 alpha `T * D` only;
  6 colour `T + D`; 7 colour `T * D`; 8 colour `2 * T * D`; 9 colour `4 * T * D`.
- blend (object +0x18, engine+0x269d0): 0 off; 1 ONE/ONE; 2 INVDESTCOLOR/ONE; 3 SRCALPHA/INVSRCALPHA; 4
  SRCCOLOR/INVSRCCOLOR; 5 DESTCOLOR/INVDESTCOLOR; 6 SRCCOLOR/DESTCOLOR; 7 SRCALPHA/ONE; 8 SRCALPHA/ZERO; 9
  SRCCOLOR/ONE; 10 DESTCOLOR/ZERO.
- alpha test (object +0x20, engine+0x26c70): 0 off; 1 LESS; 2 LESSEQUAL; 3 GREATER; 4 GREATEREQUAL; 5 EQUAL; 6
  NOTEQUAL (reference from the bound texture, 10.3).
- depth (engine+0x26bf0): 0 test + write; 1 test only; 2 off. Cull (engine+0x26dd0): 0 NONE, 1 CCW, 2 CW. Clipping
  (engine+0x263b0): 0 off, 1 or 2 on. Viewport per object (engine+0x26f80, +0x4300).
- Primitives are sent with DrawPrimitiveUP in batches of at most 41 triangles (123 vertices of a 125-entry staging array) in
  the object (engine+0x27300 and its sibling methods engine+0x277e0..0x2e860 for triangles, quads, fans, strips and lines in
  the three vertex layouts); an optional effect id runs the batch through an effect (game callback as in 7.2).

DeCombat: one WGSL "ui" pipeline family keyed by (colour op, blend, alpha test, depth, cull). Direct3D 9 places pixel
centres on integer coordinates (PUBLIC: "Directly Mapping Texels to Pixels"); add 0.5 to screen-space positions when
converting to wgpu so text and HUD art stay texel exact.

### 12.4 Fonts

Fonts are TrueType files in FONTS.rez (OBSERVED: 7 single fonts, 17,112..1,169,072 bytes, and 3 collections, 2,978,176
..13,517,576 bytes). The engine installs them with the OS (engine+0x1d53c0), creates GDI fonts, rasterises the glyph
set into a texture plus a glyph table (engine+0x1d5830, using glyph outlines and text extents, engine+0x1d5750), and
the game draws text as textured quads through 12.3. A wide-character path renders strings into surfaces
(engine+0xdee40). DeCombat: rasterise the user's ROM fonts with a TrueType rasteriser into an atlas at the sizes the game
requests; glyph metrics must follow the font's own advance widths (PUBLIC TrueType `hmtx`) so line breaks match.

## 13. Render settings table

The renderer registers 184 console settings from a static array at VA 0x6d39a8..0x6d4ae8 (record = 0x18 bytes:
kind tag pointer (VA 0x6860c0 float, 0x6860c8 integer, 0x6860d0 a third kind used once), default as f32, name
pointer, two zero words, current value; the array start is referenced from static-initialiser code at
engine+0x20a7c0). The value column below is the VA the code reads; readers are the engine functions that
reference that VA (a `*` marks a reference inside code decompiler left undefined). Defaults are the shipped values;
the game may override any of them at run time. Names are ours; the ROM name is identified only by its hash.

| value VA | type | default | ROM name | our name | readers (engine RVA; * = inside undefined code) |
|---|---|---|---|---|---|
| 0x6d39bc | int | 0 | h:737c28f0d105a87c/13 | `keep_textures_resident` | 0x100760, 0x101f20, 0x101fa0 |
| 0x6d39d4 | f32 | 0.1 | h:0dddea9153179400/16 | `pv_near` | 0x1b4870, 0x2fef0 |
| 0x6d39ec | f32 | 7 | h:571eca124b7183a6/15 | `pv_far` | 0x1b4870, 0x2fef0 |
| 0x6d3a04 | f32 | 50 | h:d37dc4e3483ff849/10 | `pv_fov_deg` | 0x1b4870, 0x2fef0 |
| 0x6d3a1c | f32 | 1.2 | h:36f22272eead6df8/13 | `pv_aspect` | none |
| 0x6d3a34 | f32 | 128 | h:8791079403af9287/21 | `pv_env_velocity` | 0x1b74a0 |
| 0x6d3a4c | f32 | 0.4 | h:85ee79019940a312/18 | `pv_env_scale` | none |
| 0x6d3a64 | f32 | 5 | h:7b3fd92e3e6ac00f/23 | `pv_env_clamp` | none |
| 0x6d3a7c | int | 1 | h:f9f58bb9c808a970/8 | `draw_player_view` | 0x1b29b0 |
| 0x6d3a94 | int | 1 | h:44a2f2d11a9b1d85/8 | `fog_use_table` | 0xf05f0 |
| 0x6d3aac | int | 0 | h:b42e4232c66d28c4/9 | `fog_on` | 0x16b770 |
| 0x6d3ac4 | int | 255 | h:6a3aa136d4a2c341/4 | `fog_r` | 0x16b770 |
| 0x6d3adc | int | 255 | h:cb7cae2a3c3069b3/4 | `fog_g` | 0x16b770 |
| 0x6d3af4 | int | 255 | h:ccd55fe4a2191454/4 | `fog_b` | 0x16b770 |
| 0x6d3b0c | f32 | 0 | h:63cb48901b40a07e/8 | `fog_near` | 0x16b770 |
| 0x6d3b24 | f32 | 2000 | h:068c8f33716eac9c/7 | `fog_far` | 0x16b770 |
| 0x6d3b3c | f32 | 0 | h:10eb8275f09917ef/11 | `sky_fog_near` | 0x1b4d50 |
| 0x6d3b54 | f32 | 2000 | h:3885263f3d410645/10 | `sky_fog_far` | 0x1b4d50 |
| 0x6d3b6c | int | 1 | h:71e0852eeaf4fcde/14 | `detail_textures` | 0x136815*, 0x136903* |
| 0x6d3b84 | f32 | 1 | h:41c3a1fe190cbc51/18 | `detail_uv_scale` | 0x189cc1*, 0x189ce7*, 0x19c151*, 0x19c177* |
| 0x6d3b9c | int | 1 | h:298197c5c732ff29/16 | `detail_add` | 0x185564*, 0x18596b*, 0x197641*, 0x197a86* +4 |
| 0x6d3bb4 | int | 1 | h:537efda06b8855a7/9 | `env_add` | 0x185ce4*, 0x18612c*, 0x186490*, 0x186ac1* +14 |
| 0x6d3bcc | f32 | 0.0005 | h:7a7ca7088f48cb87/11 | `env_pan_rate` | none |
| 0x6d3be4 | f32 | 1 | h:f9664444f3ae189c/8 | `env_uv_scale` | 0x1b49f0 |
| 0x6d3bfc | int | 1 | h:e09450956b0dc9d1/12 | `env_maps` | 0x136859*, 0x13689f*, 0x136933*, 0x136961* |
| 0x6d3c14 | int | 1 | h:398de3359b2110f6/10 | `env_bump` | 0x136872*, 0x13688e*, 0x136991*, 0x1369c1* |
| 0x6d3c2c | f32 | 1 | h:7aa536d81d4d8954/15 | `env_bump_scale` | 0x189dc3*, 0x189de9*, 0x189ed3*, 0x189ef9* +4 |
| 0x6d3c44 | f32 | 1 | h:4576286878705bb1/19 | `env_bump_amount` | 0x1b4740 |
| 0x6d3c5c | int | 1 | h:92b52cfa46c4fcda/11 | `dot3_bump` | 0x1368ba*, 0x1369f1* |
| 0x6d3c74 | int | 1 | h:4651b63eed798898/14 | `dot3_env_bump` | 0x1368d2*, 0x136a2b* |
| 0x6d3c8c | int | 0 | h:464dce37c16d4a8d/12 | `dot3_saturate` | 0x187692*, 0x187980, 0x187f13*, 0x1882a0 +6 |
| 0x6d3ca4 | int | 0 | h:b1f6f3fd8ae57f0a/13 | `dbg_world_tree` | 0xf83c0 |
| 0x6d3cbc | int | 0 | h:9da034db297d7cb0/16 | `dbg_block_boxes` | 0x17e110 |
| 0x6d3cd4 | int | 0 | h:e22837b21298873f/10 | `dbg_block_tris` | 0x1101b6* |
| 0x6d3cec | int | 0 | h:e567b4dcfa5f7b04/10 | `dbg_block_current` | 0x10fb70, 0x1579f0 |
| 0x6d3d04 | int | 0 | h:2b1168d10308ff84/10 | `dbg_block_all` | 0x10fb70, 0x1579f0 |
| 0x6d3d1c | int | 0 | h:e0cba4d2637ab383/14 | `dbg_block_frustum` | 0x10fcd9*, 0x1579f0 |
| 0x6d3d34 | int | 0 | h:f751a76eda451aa6/18 | `dbg_skip_occluders` | 0x10fcfd*, 0x157560 |
| 0x6d3d4c | int | 0 | h:793e7b7345143c5e/20 | `dbg_block_show_occluders` | 0x11018b*, 0x1577ef* |
| 0x6d3d64 | int | 1 | h:083c3427b47c252b/20 | `dbg_block_sort_occluders` | 0x1578a3* |
| 0x6d3d7c | int | 0 | h:4142dde1354511e0/11 | `dbg_block_timing` | 0x1101c7* |
| 0x6d3d94 | int | 1 | h:a239751bfa3a3e69/11 | `draw_world_blocks` | 0x1100af* |
| 0x6d3dac | int | 0 | h:d60fbe29feb792ee/23 | `dbg_block_old_occludee` | 0x192e80 |
| 0x6d3dc4 | int | 0 | h:bb9038c829431aac/18 | `dbg_block_slivers` | 0x192ad0 |
| 0x6d3ddc | int | 0 | h:bed680b19c8fc9fe/25 | `ignore_object_groups` | 0x16a7c3*, 0x16a7f0, 0x16b4b0, 0x16b580 |
| 0x6d3df4 | int | 1 | h:22867a734ee7df71/11 | `vsync_on_flip` | none |
| 0x6d3e0c | f32 | -1 | h:b310e7012078d3f5/10 | `mip_bias` | 0x100a50 |
| 0x6d3e24 | int | 1 | h:7e7be88a1279444b/9 | `trilinear` | 0x16b770, 0xf16e0 |
| 0x6d3e3c | int | 0 | h:0b5f0615e32450ba/11 | `anisotropy` | 0x16b770, 0xf16e0 |
| 0x6d3e54 | int | 0 | h:08df0dd140b8b5d7/13 | `anisotropy_2d` | 0x176df2*, 0x176e35* |
| 0x6d3e6c | int | 0 | h:f104bbc5db3798f5/7 | `use_ref_device` | 0x1595a0, 0x159620 |
| 0x6d3e84 | int | 2 | h:ed4f243bf8cc56fe/15 | `back_buffer_count` | 0xf15e0 |
| 0x6d3e9c | int | 32 | h:a533c19fde11f346/9 | `depth_bits` | 0x100578*, 0x163010, 0xf15e0, 0xf2760 |
| 0x6d3eb4 | int | 0 | h:53c652ae206a416f/15 | `stencil_bits` | 0x100573*, 0x163010, 0xf12c0, 0xf15e0 +2 |
| 0x6d3ecc | int | 0 | h:8ef73e3475262001/18 | `force_copy_swap` | 0xf15e0 |
| 0x6d3ee4 | int | 0 | h:b5afd70b9c46c5e0/21 | `msaa_samples` | 0xf15e0 |
| 0x6d3efc | int | 0 | h:0412b0335f1b5060/10 | `accept_msaa` | 0xf15e0 |
| 0x6d3f14 | int | 0 | h:a1a0a9de4b56946a/25 | `fullscreen_by_mode_change` | 0xa720, 0xf15e0, 0xf6f10, 0xfe2c0 |
| 0x6d3f2c | int | 0 | h:6fcc57ab88c15369/18 | `force_sw_vertex` | 0x15e7a0, 0x15f740, 0x161510, 0x1a7f90 +1 |
| 0x6d3f44 | int | 1 | h:4211d62323bea8cc/6 | `dither` | 0x16b770 |
| 0x6d3f5c | int | 1 | h:eae2418c60b37435/10 | `use_bc_textures` | 0x100c31*, 0x100e50, 0x101b10 |
| 0x6d3f74 | int | 0 | h:99e9b9348e6b28d2/17 | `max_tex_aspect` | 0x100b80 |
| 0x6d3f8c | int | 1 | h:d91ddc40e0aa10c4/13 | `full_colour_textures` | 0x100e50 |
| 0x6d3fa4 | int | 1 | h:2ad1f7992023e77c/14 | `full_colour_lightmaps` | 0x101240 |
| 0x6d3fbc | int | 16384 | h:ad0035f4ac1f46f3/14 | `max_texture_side` | 0x100c70 |
| 0x6d3fd4 | int | 1 | h:1e5dbcb1131ebd53/8 | `saturate` | 0x184de8*, 0x184e0d*, 0x185155*, 0x18519a* +66 |
| 0x6d3fec | int | 1 | h:d3bae110ad106af3/8 | `bilinear` | 0x16b770, 0xf16e0 |
| 0x6d4004 | int | 1 | h:48651fde859a68fd/10 | `bilinear_2d` | 0x176e0f*, 0x176e52* |
| 0x6d401c | int | 1 | h:9b1c2c69508242c2/16 | `surfaces_as_textures` | 0x1770a0 |
| 0x6d4034 | int | 0 | h:c6290fe0f0d3b0f4/13 | `cull_world_tree` | 0x16b4b0 |
| 0x6d404c | f32 | 12000 | h:12462a1852d9fd08/4 | `far_z` | 0x1673c0, 0xfd610 |
| 0x6d4064 | int | 1 | h:bd0e30ea7e0fc4d2/21 | `prevent_frame_buffering` | 0xf1d70 |
| 0x6d407c | int | 0 | h:8c482c7c13b237a7/22 | `show_frame_buffering` | 0xf1d70 |
| 0x6d4094 | int | 0 | h:7b7454f67b1f4729/19 | `multipass_light_match` | 0x1a88f0 |
| 0x6d40ac | int | 0 | h:35d30e77fb22f80f/21 | `use_zero_weights_for_disable` | 0x15fae0 |
| 0x6d40c4 | int | 0 | h:629370cd955fde39/10 | `full_bright` | 0x184c02*, 0x1851bc*, 0x197392* |
| 0x6d40dc | u32 (third kind) | 0 | h:4bfc9635c6295c1c/15 | `lightmap_tfactor` | 0x184c08*, 0x1851cd*, 0x19740d* |
| 0x6d40f4 | f32 | 1 | h:ce1e193b4c5febbc/6 | `gamma_r` | 0x1057d*, 0x105d9*, 0x1060d*, 0x16b950 +1 |
| 0x6d410c | f32 | 1 | h:88dd7ca713902f9c/6 | `gamma_g` | 0x105a1*, 0x105d3*, 0x16b950, 0xf7060 |
| 0x6d4124 | f32 | 1 | h:02765b90087342d3/6 | `gamma_b` | 0x105c5*, 0x105e1*, 0x16b950, 0xf7060 |
| 0x6d413c | int | 256 | h:1346362d6b468ac3/24 | `dynlight_texture_size` | 0x18c680, 0x18cd80, 0x18cf40 |
| 0x6d4154 | int | 0 | h:c6d78882bc4ce1fd/23 | `dynlight_backfacing` | 0x112170, 0x1145d0, 0x116a80, 0x119000 +13 |
| 0x6d416c | f32 | 10 | h:bb6096f539098f96/29 | `dynlight_backfacing_epsilon` | 0x18d440 |
| 0x6d4184 | int | 0 | h:0064582ed70a4555/37 | `dynlight_backfacing_show` | none |
| 0x6d419c | f32 | 1 | h:12862111d3c3e6ed/12 | `light_add_poly` | 0xf7a80 |
| 0x6d41b4 | int | 1 | h:b4b441221ee50198/21 | `dynlight_objects` | 0x1af281* |
| 0x6d41cc | int | 1 | h:2b2d13fba35e20a8/26 | `dynlight_world` | 0x1af2a5* |
| 0x6d41e4 | int | 0 | h:cd854c96eb0b939d/20 | `draw_shadow_casting_lights` | 0x1b29b0, 0x1bd3c0 |
| 0x6d41fc | int | 1 | h:3ed0f7176fe758e6/15 | `model_shadow_limit` | none |
| 0x6d4214 | int | 1 | h:efefe954f78c519c/19 | `all_model_shadows` | 0x173fd0, 0x174271*, 0x1b1669*, 0x1b29b0 |
| 0x6d422c | int | 1 | h:8a938b245a68f6d0/23 | `shadow_proj_on` | 0x1b1660*, 0x1b29b0 |
| 0x6d4244 | int | 1 | h:34b7eb08a9386582/27 | `shadow_proj_max` | 0x173fd0 |
| 0x6d425c | int | 128 | h:536b0ee77d2ea633/27 | `shadow_proj_tex_size` | 0x173c90 |
| 0x6d4274 | int | 0 | h:fed15cabdc8a653d/20 | `shadow_proj_lod` | 0x16fa10 |
| 0x6d428c | int | 1 | h:d7d214ece418dd3c/22 | `shadow_proj_tween` | 0x173fd0 |
| 0x6d42a4 | int | 1 | h:e82a91fd9a2aae8e/28 | `shadow_proj_perspective` | 0x173fd0 |
| 0x6d42bc | int | 0 | h:f5d71ed052be448e/27 | `shadow_proj_show_lights` | 0x173fd0 |
| 0x6d42d4 | int | 0 | h:ee08668867636f9a/30 | `shadow_proj_show_texture` | none |
| 0x6d42ec | int | 0 | h:a445cd5e20bebb7f/30 | `shadow_proj_show_plane` | 0x172760 |
| 0x6d4304 | f32 | 40 | h:ab4e0277a61f7ff4/34 | `shadow_proj_min_colour` | 0x173d60 |
| 0x6d431c | f32 | 200 | h:96b2fe9873f40d91/28 | `shadow_proj_max_dist` | 0x1713f0 |
| 0x6d4334 | f32 | 1 | h:8c715da35076fc3a/22 | `shadow_proj_alpha` | 0x173d60 |
| 0x6d434c | f32 | 1.1 | h:31edad3fa518b29a/36 | `shadow_proj_area_scale` | 0x16f400 |
| 0x6d4364 | int | 1 | h:20085ac56248602d/29 | `shadow_proj_cull_back` | 0x18ab82*, 0x18b050 |
| 0x6d437c | int | 2 | h:50dc4c1bd7e60303/28 | `shadow_proj_texture_count` | 0x173c90 |
| 0x6d4394 | int | 0 | h:c163f131ca5a995f/25 | `shadow_proj_tint_fill` | 0x170070, 0x1717b0 |
| 0x6d43ac | int | 1 | h:771e280536781759/28 | `shadow_proj_blur` | 0x174480 |
| 0x6d43c4 | f32 | 1 | h:dc4dae86a4c14cac/33 | `shadow_proj_blur_spacing` | 0x16db58*, 0x16dbf8*, 0x1717b0, 0x172044* |
| 0x6d43dc | int | 1 | h:a916cdc1986b3f38/35 | `shadow_proj_per_frame` | 0x174480 |
| 0x6d43f4 | int | 1 | h:68bf264f836d7913/21 | `shadow_proj_fade` | 0x18a450 |
| 0x6d440c | int | 1 | h:f69a571a02821e2d/29 | `shadow_proj_blur_ps` | none |
| 0x6d4424 | f32 | 0.5 | h:fce66baa737a324e/35 | `shadow_proj_dim_offset` | 0x1713f0 |
| 0x6d443c | f32 | 0.05 | h:f03d37142bf33a02/16 | `model_sun_variance` | 0x1b1cc0, 0x1b21b0 |
| 0x6d4454 | int | 1 | h:f9a5be58de9a8406/13 | `model_sun` | 0x1b21b0 |
| 0x6d446c | f32 | 1 | h:af630f52c590ec5d/15 | `model_saturation` | 0x1b16e0 |
| 0x6d4484 | int | 4 | h:40a49c5700941bc7/14 | `model_light_limit` | 0x1b21b0 |
| 0x6d449c | int | 1 | h:576dfcf9bd2faee8/11 | `light_models` | 0x1b21b0 |
| 0x6d44b4 | int | 1 | h:b88887974e3dae4e/17 | `model_ambient` | 0x1b21b0 |
| 0x6d44cc | int | 0 | h:5d5b07e0cd8bee62/25 | `model_light_skip_root` | 0x1b21b0 |
| 0x6d44e4 | int | 0 | h:abb74291f234abdf/14 | `model_lod_offset` | 0x1b1ff0, 0x1bc1f0, 0x1bc8a7* |
| 0x6d44fc | int | 1 | h:e0a9609ff68c3b57/13 | `texture_models` | 0x1b1ff0 |
| 0x6d4514 | int | 1 | h:682bb8b680a9c812/18 | `zbias_style_passes` | none |
| 0x6d452c | int | 0 | h:ba3daeb82787d059/20 | `dbg_model_boxes` | 0x1b2c40 |
| 0x6d4544 | int | 0 | h:11f780adb206542b/29 | `dbg_model_lights` | 0x1b29b0, 0x1bd3c0 |
| 0x6d455c | int | 0 | h:11ea83dbfedb5a07/23 | `dbg_model_skeleton` | 0x1b2c40, 0x1bc1f0 |
| 0x6d4574 | int | 0 | h:f492a9d0daf74b3e/19 | `dbg_model_obbs` | none |
| 0x6d458c | int | 0 | h:e883d2ebc769ad01/28 | `dbg_model_normals` | 0x1b2c40 |
| 0x6d45a4 | int | 1 | h:37d9aa695e347da6/12 | `draw_canvases` | 0x1a99c1* |
| 0x6d45bc | int | 1 | h:5d5243a66cd2fe2a/10 | `draw_models` | 0x1a94f1* |
| 0x6d45d4 | int | 1 | h:7e8a1ae49992a317/15 | `draw_solid_models` | 0x1a9518* |
| 0x6d45ec | int | 1 | h:b006ac2a53a81bab/21 | `draw_translucent_models` | 0x1a9511* |
| 0x6d4604 | int | 2 | h:826b295bdc9de09b/9 | `draw_world` | 0x1673c0, 0x16b620 |
| 0x6d461c | int | 1 | h:f1cca775574661d1/7 | `draw_sky` | 0x193ea0 |
| 0x6d4634 | int | 1 | h:aebf92d035835095/11 | `draw_sprites` | 0x1af6b1*, 0x1b4d50 |
| 0x6d464c | int | 1 | h:1b2fca52afc76c48/13 | `draw_poly_grids` | 0x1ad0a1*, 0x1b4d50 |
| 0x6d4664 | int | 1 | h:e71d41971364d080/13 | `draw_particles` | 0x1af250 |
| 0x6d467c | int | 1 | h:a84347a33295e87a/17 | `draw_volume_effects` | 0x1a9c91* |
| 0x6d4694 | int | 1 | h:6cbe07ea2d2615e3/15 | `draw_line_systems` | 0x16a301* |
| 0x6d46ac | int | 1 | h:b885beaabff4b5da/15 | `draw_world_models` | 0x1a95c1*, 0x1b4d50 |
| 0x6d46c4 | int | 0 | h:8a2f17ed2e77f4df/19 | `dbg_mem_stats` | 0x1101e8*, 0x1102ff* |
| 0x6d46dc | int | 0 | h:363c4b50f375155c/11 | `dbg_render` | 0x1024f0 |
| 0x6d46f4 | int | 0 | h:6a502a83372318a2/14 | `dbg_poly_counts` | 0xf7840 |
| 0x6d470c | int | 0 | h:bf6783054d2b1b99/17 | `dbg_texture_counts` | 0xf75b0 |
| 0x6d4724 | int | 0 | h:c10b2a6b8ae04d89/17 | `dbg_texture_memory` | 0xf75f0 |
| 0x6d473c | int | 0 | h:04b765747018cb13/9 | `wireframe` | 0xf16e0, 0xf83c0 |
| 0x6d4754 | int | 0 | h:4e3da79221383f8c/15 | `wireframe_models` | 0x1a88f0 |
| 0x6d476c | int | 1 | h:31282d347653e946/8 | `lightmaps` | 0x1367e2*, 0x136aa0* |
| 0x6d4784 | int | 0 | h:0ede98e6a6346340/8 | `draw_flat` | 0x1367d4*, 0x136a97* |
| 0x6d479c | int | 0 | h:37081750d97f9115/13 | `lightmaps_only` | 0x1367f7* |
| 0x6d47b4 | int | 1 | h:f587306b1ff82c6d/10 | `sort_translucent` | 0x169ce0 |
| 0x6d47cc | int | 0 | h:bb50f682cc20672e/24 | `dbg_object_counts` | 0x169a70 |
| 0x6d47e4 | int | 0 | h:b929d7468af61f60/14 | `dbg_cull_counts` | 0xf7720 |
| 0x6d47fc | int | 0 | h:15f65da3eba034d6/19 | `dbg_model_info` | 0xf79d0 |
| 0x6d4814 | int | 0 | h:226cd22b33d571c4/19 | `dbg_piece_info` | 0x1b94e0 |
| 0x6d482c | f32 | 1 | h:26414a712f3cd2dc/8 | `sky_scale` | 0x193ea0 |
| 0x6d4844 | int | 0 | h:5f483004cf36e238/13 | `all_sky_portals` | none |
| 0x6d485c | f32 | 10000 | h:8e9b8847490ee962/7 | `sky_far_z` | 0x193ea0 |
| 0x6d4874 | int | 1 | h:1c8e601e48d2fbea/15 | `env_poly_grids` | 0x1adaa0 |
| 0x6d488c | int | 0 | h:d89bc1698ab4029c/16 | `bump_poly_grids` | 0x1adaa0 |
| 0x6d48a4 | int | 1 | h:b47a8d1badc28a8d/16 | `fresnel_poly_grids` | 0x1adaa0 |
| 0x6d48bc | int | 2048 | h:ed8154fb96a46323/18 | `poly_grid_buffer` | 0x1aeaed*, 0x1aeb4b*, 0x1aec20*, 0x1aec45* |
| 0x6d48d4 | int | 0 | h:1b2739a98cb91e61/23 | `dbg_volume_bounds` | 0x1abbe1* |
| 0x6d48ec | int | 1 | h:2b21ac643760e46e/19 | `dynlight_sprites` | 0x1af530 |
| 0x6d4904 | int | 0 | h:ea9e17e5ef4b00a7/16 | `screen_glow_on` | 0xf7e70 |
| 0x6d491c | int | 0 | h:ac2587e245f9512e/21 | `glow_show_texture` | 0x163a70, 0x167900 |
| 0x6d4934 | f32 | 0.5 | h:10beb4296d86bd64/26 | `glow_show_scale` | 0x163260, 0x163a70 |
| 0x6d494c | int | 256 | h:0276db495534e13c/21 | `glow_texture_size` | 0xf7e70 |
| 0x6d4964 | f32 | 0.5 | h:ba90db306025bd54/17 | `glow_uv_scale` | 0x164e10 |
| 0x6d497c | int | 14 | h:c3a6ab0cf2846d72/20 | `glow_filter_taps` | 0x1638e0 |
| 0x6d4994 | f32 | 0.03 | h:3da0ba38d9320e5d/19 | `glow_amp_a` | 0x1638e0 |
| 0x6d49ac | f32 | 0.36 | h:d47ae6bc45cbe5fd/22 | `glow_radius_a` | 0x1638e0 |
| 0x6d49c4 | f32 | 0.22 | h:270e19584c25ad32/19 | `glow_amp_b` | 0x1638e0 |
| 0x6d49dc | f32 | 4.5 | h:64ebe85406d1a996/22 | `glow_radius_b` | 0x1638e0 |
| 0x6d49f4 | f32 | -0.1 | h:cf5ca88c59a6c92a/20 | `glow_tap_shift` | 0x1638e0 |
| 0x6d4a0c | int | 0 | h:d5397c90329fad95/20 | `glow_show_filter` | 0x167900 |
| 0x6d4a24 | f32 | 0.5 | h:11390cdd353e9a14/25 | `glow_show_filter_scale` | 0x163a70 |
| 0x6d4a3c | f32 | 0.4 | h:75a41b24fc2034ab/25 | `glow_show_filter_range` | 0x163a70 |
| 0x6d4a54 | int | 0 | h:d7e5ed98f1b47b3f/19 | `glow_fog_on` | 0x1673c0, 0x16b770 |
| 0x6d4a6c | f32 | 0 | h:c2f5ab28aba71d1b/18 | `glow_fog_near` | 0x1673c0, 0x16b770 |
| 0x6d4a84 | f32 | 2000 | h:1beb41148f589f93/17 | `glow_fog_far` | 0x1673c0, 0x16b770 |
| 0x6d4a9c | int | 1 | h:bd381d304fcaeb7b/18 | `glow_use_ps` | 0x1673c0 |
| 0x6d4ab4 | int | 0 | h:d93111c052dfc47b/27 | `effect_software_shaders` | 0x16c170 |
| 0x6d4acc | int | 0 | h:c63b63d1b2bb7846/26 | `effect_debug_includes` | 0x1b3270, 0x1b36a0*, 0x1b3770 |
| 0x6d4ae4 | int | 0 | h:de53759916aa7797/16 | `model_stencil_mark` | 0x1a9570 |


## 14. ROM validation counts

All OBSERVED read-only over the user ROM with the scratch tools of the evidence line (counts and sizes only).

| check | result |
|---|---|
| Engine.exe imports from the Direct3D runtime | 1 entry point of `d3d9.dll`, 15 of `d3dx9_35.dll` |
| ClientFx.fxd imports | C/C++ runtime and KERNEL32 only (no Direct3D) |
| render settings records (13) | 184, contiguous, every record has a known kind tag |
| world sections over 21 worlds (5.2) | 46,805: kind 1 16,593; kind 2 6,274; kind 4 22,233; kind 8 611; kind 9 1,094; no other kinds |
| section triangles (5.2) | kind 1 866,837; kind 2 555,882 = kind 4 544,586 + kind 9 11,296 |
| sections with a second texture | every kind 8 and kind 9 section (611 + 1,094), no other section |
| sections with an effect name | 18 (all kind 1) |
| lightmap payloads (6.1) | 6,274, all in kind-2 sections; RLE decode gives exactly `w*h*3` bytes in 6,274 of 6,274; all sides powers of two; largest side 16: 203, 32: 328, 64: 466, 128: 923, 256: 4,354 |
| 68-byte render vertex (5.2), first 200 vertices of every block, 619,292 sampled | unit vector at 32: 619,172; at 44: 618,738; at 56: 618,808; colour word at 28 with alpha 0xff: 598,665 |
| SHADERS.rez | 17 entries: 6 effect sources, 9 DTX, 2 sprite files |
| PS.rez | 26 entries: 3 shader sources, 3 identical text copies, 3 pre-compiled blobs, 17 model files |
| RS.rez | 62 entries: 61 render-style files (637..1,723 bytes), 1 zero-length entry |
| TEXTURESCRIPTS.rez / TEXTUREEFFECTGROUPS.rez | 6 scripts (55..209 bytes) / 2 groups (75, 140 bytes) |
| FONTS.rez | 7 TrueType fonts, 3 TrueType collections |
| DTX files whose flags select the 16-bit path (10.1) | 4 of 11,271 (flag 0x2; count taken from the FMT_DTX census) |
| shader path strings in Engine.exe | 3 (the 11.2 sources); no string names the pre-compiled blobs |


## 15. Addresses a decompiler re-run should define as functions

Targets reached through tables or vtables (or no reference at all) that the decompiler function table does not contain.
All are RVAs; most are short methods of the classes named (the pure-virtual stub engine+0x1de6d4 is excluded).
Generated from the tables listed; complete list:

- frame limiter (1): 17380
- renderer function table (1.3) (39): f0790 f0940 f6a90 f6ac0 f6af0 fd820 fd830 fd860 fd880 fd8a0 fd8c0 fd900 fd990 fd9a0 fda30 1007e0 101af0 166780 1667a0 1667c0 16b690 16b6b0 16b6d0 174cf0 174d00 174db0 174e60 174e90 174ed0 174ef0 174fa0 1763b0 176ca0 176cf0 176e90 176f60 176f70 177060 177090
- draw-primitive interface vtable VA 0x6767c0 (12.3) (57): 264d0 264e0 272c0 272e0 2a290 2e3e0 2ed60 2ee90 2ef40 2ef70 2efa0 2efd0 2f000 2f030 2f060 2f090 2f0c0 2f110 2f140 2f170 2f330 2f3a0 2f410 2f480 2f4f0 2f540 2f590 2f5e0 2f630 2f6a0 2f710 2f760 2f7b0 2f800 2f840 2f880 2f8b0 2f8e0 2f930 2f980 2f9b0 2f9e0 2fa60 2fae0 2fb80 2fc20 2fc40 2fc60 2fc80 2fca0 2fcd0 2fd80 2fd90 2fda0 2fdd0 303d0 30420
- effect object vtable VA 0x689f58/0x689fa0 (9.2) (14): 16ba90 16bac0 16baf0 16bb20 16bb50 16bb80 16bbb0 16bc30 16bcf0 16bd30 16bd80 16bda0 16bf40 16bfd0
- render-object type table VA 0x6d546c (8) (17): 167a40 168170 1681c0 168250 16a300 1a94f0 1a95c0 1a99c0 1a9ae0 1a9af0 1a9b40 1a9c90 1abf20 1ad0a0 1af040 1af280 1af6b0
- world shader vtables VA 0x6881e4..0x689330, 0x68a5d8 (5.2) (403): 1028d0 102930 102980 102990 1112a0 1112b0 1112c0 1112d0 1112e0 1112f0 111300 111310 111980 112330 112400 112ea0 113670 113680 113690 1136a0 1136b0 1136c0 1136d0 1136e0 114350 114790 114860 115320 115480 115b20 115b30 115b40 115b50 115b60 115b70 115b80 115b90 116800 116c40 116d10 1177d0 117930 1180a0 1180b0 1180c0 1180d0 1180e0 1180f0 118100 118110 118d80 1191c0 119290 119de0 119f40 11a470 11a480 11a490 11a4a0 11a4b0 11a4c0 11a4d0 11a4e0 11b150 11b590 11b660 11c120 11c280 11c920 11c930 11c940 11c950 11c960 11c970 11c980 11c990 11d610 11da50 11db20 11e5f0 11e750 11ede0 11edf0 11ee00 11ee10 11ee20 11ee30 11ee40 11ee50 11fac0 11ff00 11ffd0 120a90 120bf0 121290 1212a0 1212b0 1212c0 1212d0 1212e0 1212f0 121300 121f70 1223b0 122480 122f40 1230a0 123740 123750 123760 123770 123780 123790 1237a0 1237b0 123e50 1243f0 124820 1248f0 125360 1254c0 125b40 125b50 125b60 125b70 125b80 125b90 125ba0 125bb0 126830 126c60 126d30 1277c0 127920 127fb0 127fc0 127fd0 127fe0 127ff0 128000 128010 128020 128ca0 1290d0 1291a0 129c30 129d90 12a420 12a430 12a440 12a450 12a460 12a470 12a480 12a490 12b110 12b540 12b610 12c0a0 12c200 12c890 12c8a0 12c8b0 12c8c0 12c8d0 12c8e0 12c8f0 12c900 12d580 12d9b0 12da80 12e510 12e670 12ed00 12ed10 12ed20 12ed30 12ed40 12ed50 12ed60 12ed70 12f9e0 12fe10 12fee0 1309a0 130b00 131180 131190 1311a0 1311b0 1311c0 1311d0 1311e0 1311f0 131e70 1322a0 132370 132e10 132f70 133600 133610 133620 133630 133640 133650 133660 133670 1342f0 134720 1347f0 135290 1353f0 135c60 135cd0 135d40 135da0 135e10 135e80 135ef0 135f60 135fd0 136040 136050 136080 1360b0 1360e0 136110 136120 136190 136200 136270 1362e0 136350 1378c0 139e90 13be80 13de10 13fe70 141e00 143dc0 145d50 147ce0 149b70 14ba80 14d990 14f8a0 1517b0 153740 155650 183bd0 184010 184030 184050 184070 1840b0 1840d0 1840f0 184110 184180 1841a0 1841c0 184240 184260 184280 1842a0 1842c0 1842f0 184340 184360 184380 184400 184420 184440 1844f0 184510 184b60 184c00 184d40 185080 1850e0 1853a0 1857d0 185b20 185f50 185fc0 186400 186910 186bb0 186d00 186e30 187250 1872c0 1874e0 187910 187d30 188740 188c00 188fa0 189000 189520 189630 1896c0 189a70 189c80 189d80 189e90 18a1b0 18a1e0 18a1f0 18a200 18a210 18a220 18a230 18a240 194f50 194f90 194fc0 195000 195020 195240 195290 1952c0 1952e0 195570 1955b0 1955e0 195600 195890 1958c0 1958e0 195b70 195bb0 195c30 195c50 195c70 195cc0 195cf0 195d10 195fa0 196010 196040 196060 196f80 196fd0 1970b0 197300 197360 1974b0 1979c0 197c30 198340 1986d0 198c40 198ea0 199050 199580 1995f0 199810 199ca0 199d10 199ff0 19a910 19c110 19c210 19c330 19c730 19c7b0 19cda0 19d430 19db80 19e380 19ea30 19f180 19fd40 19fd50 19fd60 19fd70 19fd80 19fd90 19fda0 19fdb0 1a09f0 1a0e40 1a0f10 1a19d0 1a2240 1a41c0 1a41e0 1a4220 1a4270 1a42a0 1a42e0 1a4330 1a4360 1a43a0 1a4400 1a4950 1a4ae0 1a5400 1a5710 1a6100 1a63c0 1a6440
- object-queue draw callbacks pushed as function pointers (7.1): 1a9610 1a9790 16a3e0 1a99a0 1abbe0
- effect-section bucket (5.2 bucket 13): flush engine+0x1a1b40 (its other undefined slots are in the list above)
- code regions without a function: the bucket selector body after engine+0x1367c4 up to about engine+0x136fa0
  (switch cases and lazy constructors, 5.2); the poly-grid renderer engine+0x1adaa0, which the table splits into four
  fragments (8.3); the setting readers marked `*` in section 13.


## 16. Open questions

1. Game-side frame order and the sizes of the post-effect render targets (CShell; the engine only provides the
   plumbing of 9.2). Needs the CShell render loop specified or an OBSERVED capture.
2. Whether the frame limiter body at engine+0x17380 is reachable (no reference found; 2.6).
3. Lightmap channel order (6.1): the converter masks say B, G, R in memory; confirm with a screenshot diff.
4. Light-group patch scaling (6.2): how the patch byte and the group colour combine before truncation (the DeCombat
   reading `/255` is not compared with the client).
5. The third component of the world vertices' texture coordinates (5.2 vertex formats) and which file field feeds it.
6. Dynamic-light attenuation textures (5.4): the falloff baked into the square and "Z" textures and the exact texture
   matrices.
7. ~~Render-style files (7.3): their format and the contents of the enum translation tables filled by engine+0x1b50d0.~~ CLOSED by `FMT_RENDERSTYLE_2009.md` (spec-fold-6);
   its own open items (options bytes 1..7, the piece comparator, the bump matrix, NULL stage texture, V-RS1) are listed there.
8. Texture-script byte code and interpreter (10.4; TFS/TFG files).
9. Translucent queue order (7.1): heap comparator direction (expected back to front).
10. Poly-grid vertex layout, fresnel term and the c0 value of the bump shader (8.3, 11.2).
11. Source of the per-batch alpha-test flag of world sections (5.2 slot 20).
12. Model pre-pass and projected shadows (7.1 step 1, 7.5): receiver selection and the projection onto the world.
13. Glow pass (9.1): the per-object rule that makes non-glowing objects black and the meaning of the two
    game-supplied glow textures.
14. Scene-description block at +0x48 (3.3): floats 6..11 are the sky box (4.4, M1, closed); floats 0..5 carry the outer sky box on the
    wire (REFERENCE + ROM, two worlds, spec-fold-8), but their client use and the +0x368 camera-builder argument stay open.
15. Environment-map coordinate generation for world env buckets (10.4).
16. FMT_DTX section 5 must be corrected for flag 0x2 and the slot table of 10.1 (coordinator action).
17. DEPTHBIAS: the default-state value (2.4, loaded from a register that also holds 0.0 for the slope bias) and the
    writer during model passes (7.2).
18. Device-format slot 7 (10.1) probes one format and stores another; confirm on hardware whether it matters (no ROM
    texture uses flags 0x800 / 0x1000).
19. Euler-to-quaternion composition order at engine+0x5b6c0 (4.1; Appendix Z R7).
20. Comparison of the DeCombat lightmap uv, light-group scaling and culling readings (6.1, 6.2, 5.1) with the 2009 client
    on screen.
21. NARROWED (spec-fold-6, 6.3.1): the sun visibility factor is read (EVIDENCE engine+0x1b1cc0 / 0x1b1ba0) and the world object is REFERENCE-CONFIRMED for the reference server
    (its id 22 equals the DSV-40 / REN-11 rule exactly); open: the 2009 server's own choice, and the ray test in DeCombat (REN-7 uses factor 1).
22. The base of the static light coefficients (the stack slot later stored as the radius, PLAUSIBLE; M4).
23. Blend-set pairing of the two secondary trackers for the 60-node bodies (7.4, M7).
24. ~~Model colour op of the ROM render styles (MODULATE or MODULATE2X; 7.3, M10).~~ CLOSED by the data (`FMT_RENDERSTYLE_2009.md` 9: MODULATE2X with an emissive
    material on the default character's skin pieces and on 284 of 291 styled weapon models).
25. Morph (vertex-animated) playback: the 36 kind-6 bodies are drawn in their base pose (M6).
26. The sun visibility ray test (6.3.1) in DeCombat (REN-7 keeps factor 1).
27. The engine's attachment update routine (7.7) and the 64 mask words of `CF_ATTACH` (B 4.2 item 8).
28. Screenshots of the 2009 client: V-RS1 (`FMT_RENDERSTYLE_2009.md` 11 item 5) and an outdoor world with the sun (6.3.1).
29. The XY attenuation map and the coordinate transforms of the dynamic world lights (5.4; the "Z" texture is read).
30. The first-person model's placement per weapon (4.2: the position the cshell+0x1a52e0 command reads and sets; its default is not traced; REN-15 / CLI-28 revised).
31. The order of blended first-person pieces relative to translucent world objects (4.2 per-piece switch; REN-12).
32. Whether the engine sets the lights again after VIEW = identity for eye-attached models (REN-13; our lighting equals the world pass).
33. Whether the client clamps `S2C_DIR_LIGHT` colour channels above 255 (6.3.1: one ROM world's rule value is 314.29; DeCombat sends it unclamped).

## Appendix Z. Evidence folded from the render-view addendum (2026-10-24)

Source: the dc-render census examples (`render_census`, `light_groups`, `world_model_props`) over the 21 ROM worlds,
read-only; counts only. Property names are given as `sha256[:16]/length`. Grade: OBSERVED (data) unless stated; none of
these was compared with the 2009 client on screen.

- **R1 (5.2, 6.1; addendum A1, A2, A3, A6).** Lightmap uv pair correction and the kind-2 observations quoted in 5.2.
  This closes the sentence "the converter that moves it ... is UNPROVEN" of 6.1 (that sentence assumed the wrong uv pair).
- **R4 (6.2; addendum A4).** Slot / patch / colour counts quoted in 6.2 (4,364 patches in 2 worlds).
- **R5 (5.1 step 6; addendum A5).** World-model counts quoted in 5.1 step 6 (1,913 render models).
- **R7 (4.1; addendum A7).** Spawn yaw convention; the composition order stays UNPROVEN.
- **R8 (5.1; addendum A8).** Culling convention check.
- R2 / R3 / R6 are the addendum's A2 / A3 / A6, folded into R1 (same observations).

## Appendix M. Evidence folded from the render-models addendum (spec-fold-5, 2026-10-25)

Source: `RENDER_2009_addendum_models.md` (render-models worker, now a stub), identical to the staged
`staging/render-models_RENDER_2009.md`. Grades as stated there: EVIDENCE = clean decompiler output, DISASM = read-only
disassembly, OBSERVED = counts over the user ROM with the dc-render census examples, DeCombat = our decision. Applied inline: M1 -> 4.4,
M2 -> 5.3, M3 / M4 / M5 -> 6.3, M6 / M7 / M8 -> 7.4, M9 -> 7.1, M10 -> 7.3, open items -> 16 items 14, 21-25. The text below is the
addendum's evidence, kept for the record (its section numbers refer to this spec).

### M1. Sky camera eye (fills §4.4 "translation taken from a second position ... UNPROVEN" and open item 14)

- EVIDENCE engine+0x193ea0: the sky camera matrix is a copy of the main camera matrix (camera context +0x3c, 16 floats)
  with its translation replaced by the context's +0x35c / +0x360 / +0x364; near 0.01 (immediate 0x3c23d70a), far
  `sky_far_z`; `sky_scale` is passed three times as the camera builder's per-axis scale argument; the builder also gets
  the main context's +0x368 as its last argument (meaning UNPROVEN). Sequence: engine+0xf6a50 applies the sky context,
  engine+0x1b4d50 draws the sky objects, engine+0xf6a50 applies the main context again, engine+0xf67c0 clears with flags
  2 (depth only).
- EVIDENCE engine+0xfb8c0 (camera builder, tail): for every camera it writes +0x35c..+0x364 as
  `sky_box_min + (sky_box_max - sky_box_min) * t` per axis, with `t = (eye - world_min) / (world_max - world_min)` when
  the scene description is of mode 1 (the game scene) and `t = 0.5` otherwise.
  - `world_min` / `world_max` are read at +0x20 / +0x2c of the object behind VA 0x7e59e0. That pointer is filled by an
    interface registry at static initialisation (DISASM engine+0x20bf70: `push 0; push 0x7e59e0; push <name>`; name
    `ae2a799b65c520eb/23`, the shared world-BSP interface). +0x20 / +0x2c are where the world loader stores
    `extent_min` / `extent_max` (FMT_DAT85.md §4 items 3-4), so the box is the world extents.
  - `sky_box_min` / `sky_box_max` are scene description +0x60..+0x74, i.e. floats 6..11 of the 12-word block at scene
    +0x48. §3.3 says that block is copied from client state +0x9cc; UDP_SESSION_2009_A.md §4.8 says `S2C_SKY_TABLE`
    copies its 48-byte block to client state +0x9cc. So the sky table's block carries the sky view box in its floats
    6..11.
- EVIDENCE (host): engine+0x9c5e0 initialises the server's copy of the block to floats 0..5 = (-10, -10, -10, 10, 10,
  10) and floats 6..11 = 0; engine+0x96ec0 (a server interface method) replaces all 12 words with what the game module
  supplies; engine+0x8d5a0 sends them raw. The client's camera code reads only floats 6..11 (floats 0..5: UNPROVEN).
- DeCombat (dc-render `sky.rs`): sky eye as above; when no server block is at hand the view box is a zero-size box at
  the centre of the sky world models' geometry (no sky parallax). Proposed for dc-dserver's `S2C_SKY_TABLE`: the same
  box (centre of the sky world models' render bounds) in floats 6..11, the host defaults in floats 0..5, the ids of the
  sky world model objects in the table.
- Verified on screen (DeCombat renderer only): the sky of all 21 worlds draws from the first spawn without seams with
  the zero-size centre box. Not compared with the 2009 client.

### M2. Sky pass details (§5.3, §5.1 step 3)

- EVIDENCE engine+0x1b4d50: ZWRITEENABLE and ZENABLE off, FOGSTART / FOGEND = `sky_fog_near` / `sky_fog_far`, stage 1
  COLOROP DISABLE; for each sky object with visible bit 0x1: world models (type 2) when `draw_world_models`, through the
  opaque state when alpha 255 and object flags (+0x88) & 0x42 are clear, else the translucent state (additive with flag
  bit 0x2); poly grids (type 7) and sprites (type 3) likewise.
- OBSERVED: 24 sky world models over the 21 worlds (1 or 2 per world), every one linked to a nested render world; all
  119 of their sections are kind 1 (vertex-lit); 6 sky portals per world (126).
- DeCombat: the sky world models are the world objects of the sky-world-model class (the server names them in the sky
  table); without the block visibility walk every portal part in front of the camera counts for the scissor rectangle
  (clipped at the near plane, clamped to the target). From the first spawn the rectangle was the whole frame on 21 of 21
  worlds.

### M3. The model light set (§6.3, replaces the "details" of the light list)

EVIDENCE: engine+0x1b21b0 (builder), 0x1b3ca0 (sorted insert), 0x1ba460 (contribution of a record at a point),
0x1ba320 / 0x1ba360 / 0x1ba3d0 (directional / point / spot record builders), 0x1b1800 (static-light callback),
0x1b3b30 (capacity), 0x1b3b70 (reset), 0x1b3ba0 (ambient add), 0x1be550 (upload: AMBIENT + up to 8 D3D lights of 0x68
bytes, the rest disabled).

1. Capacity = min(`model_light_limit`, 8). Position = the object position (+0xc8), or with `model_light_skip_root` the
   root node's position.
2. `light_models` off, or object flag 0x200 (+0x88): ambient = (255, 255, 255), no lights.
3. Otherwise the ambient accumulator starts at 0 and gets: the light-grid sample at the position when
   `model_ambient` (M5); the sun when `model_sun` and the global light colour (world interface +0x154..+0x15c) is not
   black, as a directional record with colour x a visibility factor (engine+0x1b1cc0, re-evaluated when the model moved
   more than `model_sun_variance` + 0.001) and convert-to-ambient fraction from the world interface +0x160; every
   dynamic light without flag 0x20 (+0x84) as a point record, colour bytes +0x94..+0x96, radius +0x1cc, attenuation
   (1, 0, 19 / r^2) (the law of §6.3); the static lights of the world light tree over the box position +- the model
   radius (callback engine+0x1b1800).
4. Insert (engine+0x1b3ca0): key = (1 - f) * max channel of the record's contribution at the position (f = the
   record's convert-to-ambient fraction). The list is kept sorted by key, descending. A record that ranks below a full
   list adds its whole contribution to the ambient; a record that enters a full list pushes the last one out, whose
   contribution goes to the ambient; an inserted record's colour is scaled by (1 - f) and f x contribution goes to the
   ambient.
5. Contribution (engine+0x1ba460): directional = colour; inside `d^2 < r^2`: point = colour x att, spot = colour x att x
   (cos - cos_half) / (1 - cos_half) when cos > cos_half; att = 1 / (c0 + c1 d + c2 d^2) (kind 0), 1 - d / r (kind 1),
   (1 - d^2 / r^2)^2 (kind 2); zero outside the radius.
6. engine+0x1b16e0: the object colour (+0x94..+0x96) is multiplied by `model_saturation` when `saturate` is on.

DeCombat (dc-render `lights.rs`, `model.wgsl`): the same set built on the CPU, evaluated per vertex in the shader with
each record's own falloff form and N.L (public D3D9 vertex lighting); the sun's visibility factor is 1 (no trace yet).

### M4. Static model lights from the object list (§6.3 "static lights found through the world light tree")

EVIDENCE engine+0xcea90 (client scan of the object list), DISASM engine+0xcf10f..0xcf180:

- Classes (case-sensitive compares): `dbcd5e7bb7a0f538/5`, `22b4211a53edd69c/8`, `3cf839804e3c200b/11`.
- Properties by type: real: radius `bc4d3952329d2f76/11`, convert-to-ambient `dd6d73b963113d14/16`, bright scale
  `1ae3f3a79e685b27/11`, object bright scale `639a93a207df88a6/17`, cone angle `5608c2a19433a83e/3` (degrees, stored as
  cos(angle x pi / 360)); colour: `13b4d1b9cdf6ee57/10` or `9a072808beba8a7a/10`; vec3: attenuation coefficients
  `189127eb5517ce75/8`, exponents `c40ad9b59d1eaa88/7`, position `704e75860aa0d98d/3`; bool: lights-objects
  `9b88161ea169513c/12`, fast-lights-objects `09ba7c7657f8c9e1/16`, cast shadows `334ec37c9d52bc14/11`; euler: rotation
  `57b5e2fc1bba18bb/8` (spot direction); text: light group `87b681a48dde3f52/10` (hashed to an id), attenuation kind
  `1201a8d9c29928f4/11` with case-insensitive values `ed0cb080d1dbcd61/3` -> 0, `4d02e4a79f39de49/7` -> 2,
  `e6950b45d25c2027/6` -> 1.
- A record is created only when lights-objects is true and fast-lights-objects is false. Colour x object bright scale x
  bright scale. Coefficient i x pow(base, exponent i) (three `pow` calls; the base is the stack slot the record later
  stores as its radius: PLAUSIBLE). Point record when cos_half <= -0.99, else spot (engine+0x1b1800).
- OBSERVED: 1,883 light objects (1,112 / 763 / 8 per class); lights-objects true on 1,583, fast on 898; 695 records;
  every one of the 695 uses the quartic form (so the coefficients do not matter for the ROM); 428 point, 270 spot (cos_half
  -0.17 .. 0.97); convert-to-ambient 0 (203), 0.5 (379), others 0.15 .. 0.75; radii 64 .. 20,480; largest scaled colour
  channel 337.4. Light count per world 0 .. 161.
- DeCombat: missing scales count as 1, a missing kind as kind 0, missing coefficients as the dynamic law.

### M5. Light-grid sampling (§6.3 "trilinear in our reading")

EVIDENCE engine+0x4ebd0 (sampler), engine+0x33b70, engine+0x1be930; DISASM engine+0x4ef00..0x4f168:

- Cell per axis = trunc((p - origin) / cell) clamped to [-1, dim - 1]; the step to the neighbour is 0 at the last index
  or when the index is below 0; a negative index reads cell 0; the trilinear weights are the fractions of the unclamped
  position; each channel clamped to 0..255; no grid: grey 0x80.
- Byte order: the sampler writes channel 0 to result byte 2, channel 1 to byte 1, channel 2 to byte 0; AMBIENT is
  0xff << 24 | byte 2 << 16 | byte 1 << 8 | byte 0, so the first stored byte of a grid sample is RED (unlike the B, G, R
  reading of the lightmaps, §16 item 3).
- OBSERVED (`grid_levels`): grid means 0 .. 46 per world (lightmap texel means 0.4 .. 56); at the first spawns 2 .. 129.

### M6. Animation data (§7.4; FMT_LTB.md §13)

- OBSERVED (`model_census`, 1,461 models with clips): rotation keys are unit quaternions stored (x, y, z, w): frame 0 of
  the best-matching clip equals the bind local rotation with median angle error 0.0025 rad (below 0.01 in 766 files),
  against 0.50 for the inverse and 2.9 for (w, x, y, z); tracks are parent-relative: the best-clip frame-0 translation
  equals the bind local translation (median error 0.0000, below 0.01 in 858 files); vertices of rigid meshes lie in the
  bind pose's model space (centroid nearer the node's bind origin in 1,108 meshes, nearer the model origin in 36, node at
  the origin in 1,257), skinned likewise (2,931 of 2,997 bind boxes contain a used node origin); largest node count 221.
- DeCombat: positions linear and rotations slerp (shorter arc) between the keys at the marker times; time wraps at the
  last marker time; bone = global(pose) x inverse(global(bind)); D3D vertex blending for spans (first `bones - 1` stored
  weights, last = 1 - sum).

### M7. The three trackers and blend sets (§7.4; SESSION_B_S2C_addendum_dserver3.md S3-5)

- DeCombat reading: trackers applied in order onto the bind pose (main with weight 1 everywhere, then tracker 1 with
  blend set 0, tracker 2 with blend set 1); per node `pose = mix(pose, clip, w)` (lerp / slerp); a missing set counts
  as weight 1; nodes the clip's model lacks keep the pose.
- OBSERVED (`character_census`): the 53-node bodies' blend set 0 weights 34 nodes with mean bind height 9.0 (upper body)
  and set 1 19 nodes at -14.5 (legs), set 2 sums 0.6; the 60-node bodies have set 0 45 nodes at 71.3, set 1 sum 0.6, set 2
  15 nodes at 32.5. For the 60-node bodies the server's tracker 2 / set 1 pairing therefore selects an almost empty set
  (open question for the dserver: pick the set per body, or the client maps sets by name).
- OBSERVED: the composed clip list is the body's own clip (1) then the child's (1,130 for 53-node bodies, 1,150 for
  60-node bodies); the player clips by name key are 96 / 99 / 6 / 47 (53-node) and 94 / 97 / 6 / 48 (60-node).

### M8. The face model (character object)

- OBSERVED: face row 0's model carries the 53 node names of body row 0's model (52 with the same bind transform);
  face rows of 60-node bodies share 42 names. DeCombat: the face model follows the body's pose node by node name.

### M9. Translucent and additive world models (§5.1 step 6, §7.1)

- EVIDENCE engine+0x1a9790: additive world models (+0x88 bit 0x2) save and set SRCBLEND = SRCALPHA, DESTBLEND = ONE,
  FOGCOLOR = 0, draw with the normal section renderer (engine+0x1a9610), restore; translucent ones draw under the
  translucent state already set (7.1 step 7); afterwards the translucent state is re-applied.
- OBSERVED: 339 visible blended world models (194 translucent, 145 additive; 8 more objects with a non-opaque blend mode
  are invisible); their sections: kind 1 465, kind 2 1, kind 4 1.
- DeCombat: the object's alpha property scales the fragment alpha; the lightmap pair of a blended model falls back to the
  vertex-lit recipe (kind 2 left out); objects sorted far to near by squared eye distance to the object position.

### M10. Model state where render styles are not decoded (§7.3, open item 7)

- DeCombat: no culling (default CULLMODE NONE, §2.4), Z test + write, alpha test at the texture's reference with
  GREATEREQUAL when it has one, stage 0 MODULATE (not 2X). OBSERVED consequence: on dark worlds the models are darker
  than the lightmapped world (light grid around 20-30 against lightmaps doubled by `saturate`); whether the ROM render
  styles use MODULATE2X must be settled with the render-style format or a screenshot of the 2009 client.

### Open items of the addendum (now section 16 items 21-25)

1. Floats 0..5 and the +0x368 argument of the sky camera.
2. The sun visibility factor (engine+0x1b1cc0) and which world object the server takes the global light from
   (`S2C_DIR_LIGHT`, B §8.4).
3. Blend-set choice for the 60-node bodies (M7).
4. Model colour op (M10).
5. Morph (vertex-animated) playback: the 36 kind-6 bodies are drawn in their base pose.

## Appendix S. Evidence folded from the render-styles addendum (spec-fold-6, 2026-10-25)

Source: `RENDER_2009_addendum_styles.md` (render-styles worker, branch `render-styles`, merged as 5610c8b; now a stub), identical to the staged
`staging/render-styles_RENDER_2009.md` (diff 0 after line-end normalisation). Grades as stated there: EVIDENCE = clean decompiler output, DISASM = read-only
disassembly, OBSERVED = counts over the user ROM, PLAUSIBLE, DeCombat = our decision (the proposed rows REN-1..REN-11 and DSV-40, formerly `DSV-next-dirlight`, are in
`DC_DEFAULTS.md` as PENDING since the Wave Q fold).

| item | applied to | change | grade |
|---|---|---|---|
| S1 | 7.3, 2.4 (note), 16 items 7 / 24 | the style format is `FMT_RENDERSTYLE_2009.md`; M10's plain state superseded; the model pass culls by its style (CCW in 47 of 69 ROM passes) | EVIDENCE / DISASM (format), OBSERVED (census) |
| S2 | new 6.3.1, 6.3 light-set bullet, `UDP_SESSION_2009_B.md` 8.4 | `extra` = convert-to-ambient; the sun visibility factor; the server writer and the global light object; REN-7, REN-11 | EVIDENCE (client, server writer), OBSERVED + PLAUSIBLE (the class), **REFERENCE-CONFIRMED** (new in this fold: the reference stack's id 22 in tdm2 / tdm4 equals the rule exactly) |
| S2.1 | 6.3.1 last two bullets | DSV-40 (proposed): dc-dserver sends id 22 from that object | D (proposed; implementation assigned to the dserver-light worker) |
| S3 | 6.2 | light-group grids in the model ambient; REN-9 | EVIDENCE, OBSERVED |
| S4 | `FMT_LTB.md` new section 14 (and 13 gap) | LOD switching by distance | EVIDENCE, OBSERVED |
| S5 | new 7.7 | sockets and the held weapon; REN-10 | OBSERVED (ROM-gated test), PLAUSIBLE (composition) |
| S6 | 5.4 | the "Z" attenuation texture | DISASM; the falloff stays UNPROVEN |
| open items 1-4 | 16 items 26-28 (item 2 = item 19) | | |

The text below is the addendum's evidence as promoted on master 4b27ae7, kept for the record (its section numbers refer to this spec; "staged" / "proposed" wording is the
addendum's own).

Status: PROPOSED (staged for a spec fold; nothing here is applied to `specs/RENDER_2009.md`).
Worker: render-styles (branch `render-styles`). Grades as in RENDER_2009_addendum_models: EVIDENCE = clean decompiler output of
`Engine.exe`, DISASM = our read-only capstone disassembly of the ROM `Engine.exe`, OBSERVED =
counts over the user's ROM (dc-render examples `style_census`, `world_lights`, ROM-gated tests `rom_styles`),
PLAUSIBLE = reading that fits all evidence without proof, DeCombat = our decision (proposed REN-1..REN-11,
`staging/render-styles_DC_DEFAULTS.md`). Engine and ROM strings appear only as `sha256[:16]/length`.

### S1. Render styles (fills §7.3 "format not specified" and open item 7; resolves M10 and render-models open question 1)

- The format, the per-pass state, the translation tables of engine+0x1b50d0, the default style (engine+0x162890) and
  the ROM census are in the new draft `specs/FMT_RENDERSTYLE_2009.md`.
- Replace §7.3's last sentence by: "The file format is FMT_RENDERSTYLE_2009 (render-style object vtable VA 0x689870,
  loader slot 12 engine+0x161bb0, pass setup engine+0x1a88f0, texture binding engine+0x1a8240, default style
  engine+0x162890 used for empty slots, engine+0x1b9390)".
- M10 replaced: models draw with their styles. The default character's skin pieces use MODULATE2X with an emissive
  material, so the brightness gap of the plain state is closed by the data (FMT_RENDERSTYLE §9).
- §2.4 note: the model pass's culling is the style's (CCW in 47 of 69 ROM passes), not the default NONE.

### S2. Global light (fills open item 2 of the models addendum and B §8.4 "meaning PLAUSIBLE")

- Client side (EVIDENCE): `S2C_DIR_LIGHT` (engine+0xa31f0) hands `dir` to the world interface method +0x144, `color` to
  +0x14c and `extra` to +0x154; the model light builder (engine+0x1b21b0) reads them at world interface data +0x148..
  +0x150 (direction), +0x154..+0x15c (colour), +0x160 (convert-to-ambient fraction): when `model_sun` is on and the
  colour is not black it adds a directional record with colour x the sun visibility factor and inserts it with the
  fraction `extra` (the share that goes to the ambient). So `extra` = convert-to-ambient.
- Visibility factor (EVIDENCE engine+0x1b1cc0 / engine+0x1b1ba0): a ray from the model position along the sun offset
  hits the world; the factor is 1 when the hit surface has the sky flag (surface +4 bit 4), 0 otherwise, with a
  bisection over `model_sun_variance` (0.05) between two test points for soft edges; re-evaluated when the model moved
  more than `model_sun_variance` + 0.001. DeCombat: factor 1 (REN-7, the ray test is not implemented).
- Server side (EVIDENCE engine+0x9c030, engine+0x97000 / 0x97020 = server interface slots +0x1c4 / +0x1c8 filled by
  engine+0x9b1f0): the game module registers "the global light object" (host +0x7b4); registering a new one sets option
  bit 0x200 on every client, which makes engine+0x8d6c0 send `S2C_DIR_LIGHT`: `dir` = rotated +Z of the object's
  rotation, `color` = light colour (+0x10..+0x18) x two scales (+0x1c, +0x20), `extra` = +0x24 (server light data of
  the object through the server interface table registered under `bd0fb80c9303b9b7/17`).
- Which world object the 2009 game module registered is not in the client ROM (the server game module is not part of
  it). OBSERVED: exactly one world class carries the message's inputs and nothing else of a light: class
  `7cea843ab1383e94/14`, one object on 19 of the 21 worlds (none on the two others), properties {name, position, euler
  rotation `57b5e2fc1bba18bb/8`, int, colour `9a072808beba8a7a/10` (0..255), bright scale `1ae3f3a79e685b27/11`, object
  bright scale `639a93a207df88a6/17`, convert-to-ambient `dd6d73b963113d14/16`}; the client's own light scan
  (engine+0xcea90) ignores the class. All 19 point down (dir.y -0.50 .. -0.96), colour x scales 22 .. 314, fractions
  0 .. 0.5. PLAUSIBLE: this is the global light.
- DeCombat (dc-render `global_light`): dir = euler rotation (same composition as the other light objects, order
  UNPROVEN §16 item 19) applied to +Z; colour = colour x bright scale x object bright scale (missing scales 1);
  extra = convert-to-ambient (missing 0). worldview uses it as the default sun (`--no-sun` / `--sun` override).

#### S2.1 Proposal for dc-dserver (staged, not approved): filling `S2C_DIR_LIGHT`

1. At world load, find the first object of class `7cea843ab1383e94/14` (hash match as above). None: do not send the
   message (the client's colour stays black and it adds no sun; PLAUSIBLE, the 2009 initial value at +0x154 is
   zero-initialised world data).
2. Send `[22][dir: 3 x f32][color: 3 x f32][extra: f32]` reliably to each client once its world is loaded (with the
   other world-bound tables, UDP_SESSION_2009_A §5), and again to every client if the light changes:
   - `dir` = `q * (0, 0, 1)` for the object's euler rotation `q` (the writer's formula
     `(2(wy + zx), 2(zy - wx), 1 - 2(y^2 + x^2))`, B §8.4); 
   - `color` = colour property (0..255 per channel) x bright scale x object bright scale;
   - `extra` = convert-to-ambient property (0 when missing).
3. Units: colour in 0..255 like the static light records (the client divides by 255 when it uploads lights, §6.3);
   PLAUSIBLE, not compared with a 2009 capture (no capture of id 22 yet).
4. Proposed row DSV-next-dirlight (staging/render-styles_DC_DEFAULTS.md).

### S3. Light-group grids in the model ambient (fills §6.2 last sentence and models addendum M5)

EVIDENCE engine+0x4ebd0 (sampler) -> engine+0x4e6a0 (group add) -> engine+0x4e370 (one cell), engine+0x4f6a0 (grid
load):

- Each light-group grid (FMT_DAT85 §10.5) keeps its colour (initially the file colour; the game switches groups by
  changing it), its box in main-grid cells (offset .. offset + dims) and one byte per cell, x fastest then y then z.
- Grids with a colour channel below 1/255 are kept at the end of the list; the add loop stops at the first of them.
- After the sampler has gathered the eight corner samples of the main grid, each corner gets, per grid whose box
  holds the corner cell `(i0 + dx, i0y + dy, i0z + dz)` (clamped base index, +0 / +1, without the main grid's edge rule),
  `colour_c x byte`; then the trilinear blend and the clamp to 0..255.
- OBSERVED: 4 grids on 2 worlds (2 each), 25 lit cells; one grid has a zero green channel and so adds nothing.
- DeCombat (dc-render `AmbientGrid::with_groups`): every group on at intensity 1 (REN-9, as the lightmap side,
  CD-next-a), worldview `--light-groups K` scales both.

### S4. LOD switching (fills FMT_LTB §13 "LOD switching by distance")

EVIDENCE engine+0x1b1ff0: per model entry and piece (pieces hidden by the object's piece mask +0x244 skipped):
`dist` = |camera position (VA 0x7e5d58) - object position (+0xc8)|, or 0 for an object with flag 0x40 (param +8) or
+0x1c8 set; LOD = 0 when `dist` <= 0, else the first `i` with `dist < lod_dist[i + 1]` (the last LOD past the last
distance); then `model_lod_offset` (VA 0x6d44e4, default 0) added and the result clamped to [0, lod_count - 1]; the LOD's
render object is piece +0x24 [lod]; textures, style and priority come from the piece (its last LOD record).
OBSERVED: 109 multi-LOD pieces, all with 3 LODs at (0, 400, 800); their LOD records never differ in slots, style or
priority. The default body: 3,762 / 1,878 / 1,134 triangles at LOD 0 / 1 / 2. No DC default needed.

### S5. Sockets and the held weapon (extends B §3.5)

- DeCombat (dc-render `ModelInstance::socket_transform` / `attach`): socket world transform = model world x node pose x
  (socket scale, rotation, position); the attached object's transform = that x (attachment offset, rotation). The
  engine's attachment update was not traced (PLAUSIBLE).
- OBSERVED (ROM-gated `rom_weapon_attached_at_the_hand_socket`): the default body's grip socket (name key
  `c969c8aa86333373/9`) is socket 2 on node 27; in the standing pose its world position lies inside the bounds of the
  node's vertices and the weapon model overlaps them.
- worldview `--character` attaches the third-person model of weapon row `--weapon ROW` (default: the lowest row with a
  third-person model, REN-10; that row is a melee weapon in the EU-test ROM, `--weapon 40` shows a gun).

### S6. Dynamic lights on the world (§5.4): partly read, not implemented

- DISASM engine+0x18aca0..0x18ad2a (the "Z" texture, 128 x 1, engine+0x18abc0): texel 0 = 0xffffffff; texel `i >= 1`
  = grey `trunc(255 x max(0, (i - 1) / 126 x 1.2 - 0.2))` with alpha 0xff (constants VA 0x68a2e8 = 1.2, VA 0x676b70 =
  0.2, VA 0x673cc8 = 255.0).
- The XY map (engine+0x18cf40, which calls engine+0x18c720 and engine+0x18cd80) and the coordinate transforms (engine+0x18d090) were not
  decoded; §5.4's falloff stays UNPROVEN. Not implemented (the viewer has no dynamic light sources).

### Open (new)

1. The sun visibility ray test (S2) in DeCombat.
2. Euler composition order of light objects (shared with §16 item 19).
3. Attachment update routine (S5) and the 64 mask words of `CF_ATTACH`.
4. Screenshots of the 2009 client for V-RS1 (FMT_RENDERSTYLE §11 item 5) and an outdoor world with the sun.

## Appendix H: fold record (spec-fold-7, 2026-10-25)

Sources: `reports/{render-playerview, dserver-light}.md`; the staged rows `staging/{render-playerview, dserver-light}_DC_DEFAULTS.md`
(REN-12 .. REN-16, CLI-28 revised, DSV-40 details; PROPOSED; the REN / CLI rows are also in `DC_DEFAULTS.md` as PENDING). Every RVA and VA in the new text is the
render-playerview worker's (its own capstone disassembly of the user's Engine.exe and ROM reads); none was re-derived here. No existing grade was lowered.

| place | change | grade |
|---|---|---|
| 4.2 | constants CONFIRMED (fov 50 / near 0.1 / far 7.0 console-variable records, viewport MinZ 0 MaxZ 0.1, VIEW identity, restore); per-piece switch PLAUSIBLE; placement OPEN; DeCombat implementation paragraph | CONFIRMED (DISASM + ROM read), PLAUSIBLE, OPEN; D (proposed), TESTED |
| 4.3 | the 0.1 CONFIRMED; DeCombat depth range implemented and tested; near-plane note | CONFIRMED; D, TESTED |
| 6.3.1 | DSV-40 implemented (19 / 21 worlds, unclamped 314.29, send point) | D, TESTED |
| 7.2 | per-piece switch pointer | PLAUSIBLE |
| 16 | items 30-33 new | - |

Count: CONFIRMED facts 6 (viewport, projection call and its four arguments, the console-variable records, VIEW identity, restore, the 0.1 of 4.3), PLAUSIBLE 1, OPEN 1, D paragraphs 3.

## Appendix I: fold record (spec-fold-8, 2026-10-25)

Source: `CAPTURE_rust3_RUSTSTACK_VALIDATION.md` 3.2 and its section 11 proposal; the block rule is stated in full in `UDP_SESSION_2009_B.md` 8.5.

| place | change | grade |
|---|---|---|
| 4.4 | the sky table block = the sky object's inner box (floats 6..11, M1 confirmed) and outer box (floats 0..5); DSV-45 | REFERENCE + ROM; D |
| 16 item 14 | narrowed: the content of floats 0..5 is known, their client use is not | - |

The Appendix M proposal for dc-dserver's `S2C_SKY_TABLE` (centre box in floats 6..11, host defaults in floats 0..5) is superseded by DSV-45, which sends the reference stack's rule.
