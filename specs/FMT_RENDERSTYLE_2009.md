# FMT_RENDERSTYLE_2009: render-style files (`.ltb`, file kind 5) and how the 2009 engine applies them

Status: DRAFT 1 (worker render-styles, 2026-10-25). Not live-validated against the 2009 client; every ROM file parses
and re-encodes byte for byte with the reader described here (OBSERVED, ROM-gated test `rom_style_census_counts`).

- Module: `engine` = 2009 EU-test `Engine.exe` (sha256 `bb9a1e41...cf574f`), image base 0x460000; every RVA is
  `engine+0xRVA`.
- Sources: the clean decompiler tree (`Engine.exe/<rva>.c`) and read-only capstone disassembly of
  the user's ROM `Engine.exe` from a scratch script (for the style object's methods, which decompiler did not emit as
  functions). Direct3D 9 enum values and the meaning of the fixed-function stage operations are PUBLIC (Direct3D 9
  documentation). No SDK or other proprietary source text was used. All names are ours.
- Grades: **PROVEN** = decompiler output of the clean tree; **DISASM** = our disassembly; **OBSERVED** = counts over the user's
  ROM (dc-render example `style_census`, ROM-gated tests); **UNPROVEN** = reading without evidence; **DeCombat** = our
  decision (proposed DC defaults REN-1..REN-11, in `DC_DEFAULTS.md` as PENDING since the Wave Q fold; first staged in `staging/render-styles_DC_DEFAULTS.md`).
- Referenced by `RENDER_2009.md` 7.3 (spec-fold-6), which no longer leaves the style format open; the global light, light-group ambient, LOD and sockets of the same
  worker are in `RENDER_2009.md` 6.3.1, 6.2, 7.7 and `FMT_LTB.md` 14.
- ROM data policy: counts, histograms and `sha256[:4]` tags of upper-cased paths only.

## 1. Where styles come from

| step | rule | grade |
|---|---|---|
| model piece | every LOD record carries a `style_index` (FMT_LTB §5); the loader writes it into the piece once per LOD, so the piece keeps the value of its **last** LOD record and every LOD is drawn with it | PROVEN engine+0x6e720 (FMT_LTB §5), engine+0x1b9390 reads the piece |
| object style list | an object carries, per model entry, up to **8** style slots (object +0x1344, 8 per model entry, 32 model entries); on the wire they are the `style_count` / `style_file` ids of the model file block (UDP_SESSION_2009_B §3.4) | PROVEN engine+0x6bd20 (`style < 8 && model < 0x20`), engine+0xa2700 |
| lookup | style = object slot `[model][piece.style_index]`; an index >= 8, an empty slot or a file that did not load gives the renderer's **default style** (section 6) | PROVEN engine+0x1b9390 (`if (!found || !style) style = *(VA 0x7e84e0)`) |
| server side | the game module names the style files in its create struct (8 x 0x80-byte names at create +0x204c, next to 32 skin names at +0x104c); a name that cannot be found is logged and replaced by the fixed path `RenderStyles\default.ltb` | PROVEN engine+0x94c70, engine+0x94d50, engine+0x94e10 |
| client load | the client's file table loads the file through the render-style manager (vtable VA 0x688074, method +0x30 = engine+0x1022b0) which creates a style object (engine+0xf0990 -> constructor engine+0x162710, vtable VA 0x689870) and calls its loader (slot 12 = engine+0x161bb0); failure logs a "could not load" line and leaves the slot empty | PROVEN engine+0x1022b0, engine+0x162710; DISASM vtable slots |
| ROM tables | a player body row names `RenderStyle0..k`, a face row `RenderStyle0..k`, a weapon row `HHRenderStyle0..k` (third-person), `PVRenderStyle0..k` (player view); the reference server sent exactly these lists (UDP_SESSION_2009_B §3.5.1) | OBSERVED (table keys); REFERENCE |

## 2. File layout (loader engine+0x161bb0, DISASM)

Little-endian. All offsets in bytes.

```
header      0x14 bytes, read as one block:
  +0x00 u8   file_kind          must be 5            (DISASM engine+0x161bdc)
  +0x01 u8   (not checked)      OBSERVED non-zero in every ROM file; keep it for byte-exact rewrites
  +0x02 u16  container_rev      must be 3            (DISASM engine+0x161be3)
  +0x04 16 bytes                not read
u32 data_len                    must be <= 1,000,000 (DISASM engine+0x161bf9); OBSERVED = the single variant's length word
u32 variant_count               must be 1..32        (DISASM engine+0x161c15, +0x161c2f)
variant[variant_count]:
  u32 len_word                  read into a local and not used; OBSERVED = bytes from the material to the end of the
                                last pass block (the options block not included)
  material  0x44 bytes          read into style +0x0c (section 3)
  u32 pass_count                each pass is added through slot 3 (engine+0x1626a0), which refuses a fifth pass;
                                a refused pass fails the whole load (DISASM engine+0x161c9d)
  pass[pass_count]:
    0x214 bytes  pass record    section 4
    u8   has_extra
    if has_extra: u8 a, u32 b, u8 c, u32 d      stored with the pass (slot 10, engine+0x162250, 16 bytes); use not traced
  8 bytes options               read with one stream call; on failure (the file ends) the engine logs a failure line
                                and goes on without it; otherwise stored by slot 8 (engine+0x162480) and returned by
                                slot 9 (engine+0x1624c0); byte 0 non-zero sends the piece through the effect path
                                instead of the passes (section 7)
  then: slot 13 (engine+0x161f20, returns 1) and slot 16 (engine+0x161d80, device capability check, section 5);
  the first variant that passes both is kept and the loader returns success; otherwise the next variant is read
```

Notes:

- The loop over variants does not clear the pass list between variants (DISASM engine+0x161c38..0x161d54), so the
  passes of a refused variant would stay; no ROM file has more than one variant (OBSERVED), so this never happens.
- A file that ends inside the options block of a variant still loads (the read fails and is ignored).

## 3. Material (0x44 bytes, style +0x0c..+0x4c)

| file off | size | meaning | where the pass setup copies it | grade |
|---|---|---|---|---|
| +0x00 | 4 x f32 | ambient RGBA | device material Ambient | DISASM engine+0x1a89c4 |
| +0x10 | 4 x f32 | diffuse RGBA | material Diffuse (its alpha is the vertex alpha) | DISASM engine+0x1a89ee |
| +0x20 | 4 x f32 | emissive RGBA | material Emissive | DISASM engine+0x1a8a15 |
| +0x30 | 4 x f32 | specular RGBA | material Specular | DISASM engine+0x1a8a64 |
| +0x40 | f32 | specular power | material Power; forced to 0 when specular RGB is black; SPECULARENABLE = power > 0 | DISASM engine+0x1a8a8d..0x1a8aed |

With the Direct3D 9 fixed-function lighting equation (PUBLIC) and the engine's lights (RENDER_2009 §6.3: AMBIENT render
state = light-grid colour, lights' ambient 0, diffuse = specular = colour / 255), a lit vertex colour is
`emissive + ambient_mat x grid + sum(diffuse_mat x light x N.L x att)`, alpha = `diffuse_mat.a`, saturated.

The object alpha: when an object's alpha is below 0.99 the model pass copies the style material, multiplies the
diffuse alpha (and two further alpha words) by the object alpha and sets it for the draw (PROVEN engine+0x1b94e0,
`fStack_2c < 0.99`).

## 4. Pass record (0x214 bytes)

Four texture-stage records of 0x78 bytes at +0x000, +0x078, +0x0f0, +0x168, then the raster words.

### 4.1 Stage record (0x78 bytes)

| off | type | meaning | applied by | grade |
|---|---|---|---|---|
| +0x00 | u32 | texture source: 0 = none (null bound), 1..4 = the piece's texture 0..3 (the object's texture at the piece's material slot k; slot >= the piece's slot count gives null), > 4 = keep whatever is bound | engine+0x1a8240 | PROVEN + DISASM engine+0x1a8278 |
| +0x04 | u32 | colour op (table 4.3) | COLOROP | DISASM engine+0x1a8d92 |
| +0x08 / +0x0c | u32 | colour arg 1 / 2 (table 4.4) | COLORARG1 / 2 | DISASM engine+0x1a8dfb / 0x1a8e1a |
| +0x10 | u32 | alpha op (table 4.3) | ALPHAOP | DISASM engine+0x1a8db9 |
| +0x14 / +0x18 | u32 | alpha arg 1 / 2 | ALPHAARG1 / 2 | DISASM engine+0x1a8e41 / 0x1a8e60 |
| +0x1c | u32 | texture-coordinate source (table 4.5) | TEXCOORDINDEX | DISASM engine+0x1a8e7f |
| +0x20 / +0x24 | u32 | U / V address (table 4.5) | ADDRESSU / ADDRESSV | DISASM engine+0x1a8eab / 0x1a8ecb |
| +0x28 | u32 | filter (table 4.5) | MINFILTER, MAGFILTER, MIPFILTER | DISASM engine+0x1a8eea..0x1a8f4a |
| +0x2c | u8 (+3) | texture transform on (low byte tested; the upper bytes hold leftovers) | | DISASM engine+0x1a8f50 |
| +0x30 | 16 x f32 | texture matrix, row-major, handed to SetTransform(TEXTURE0 + stage) | | DISASM engine+0x1a9099 |
| +0x70 | u8 (+3) | projected (low byte): TEXTURETRANSFORMFLAGS gets 0x100 | | DISASM engine+0x1a8f5f |
| +0x74 | u32 | coordinate count (D3DTTFF_COUNTn); TEXTURETRANSFORMFLAGS = count (| 0x100); 0 when the transform is off | | DISASM engine+0x1a8f68 |

When the texture-coordinate source is 7..9, the stage matrix is multiplied with a matrix built from the camera (VA
0x7e5a44, its rotation part transposed) before SetTransform, i.e. the generated camera-space vector is taken back
to world space (DISASM engine+0x1a8f8b..0x1a908d). If both the colour and the alpha op of a stage are 0, the remaining
stages are not set (DISASM engine+0x1a8de5).

### 4.2 Raster words

| off | type | meaning | grade |
|---|---|---|---|
| +0x1e0 | u32 | blend mode (table 4.6); 0 = ALPHABLENDENABLE off | DISASM engine+0x1a8c91 |
| +0x1e4 | u32 | depth mode: 0 test + write, 1 test only, 2 off (byte tables at VA 0x7e83bf / 0x7e83bc) | DISASM engine+0x1a8cfa, 0x1a8d1c |
| +0x1e8 | u32 | cull: 0 NONE, 1 CCW, 2 CW | DISASM engine+0x1a8d3d |
| +0x1ec | u32 | TEXTUREFACTOR (0xAARRGGBB) | DISASM engine+0x1a8d5e |
| +0x1f0 | u32 | ALPHAREF, set after the textures are bound (overrides the texture's own reference) | DISASM engine+0x1a82c3 |
| +0x1f4 | u8 (+3) | lighting (low byte; upper bytes hold leftovers in 4 ROM passes) | DISASM engine+0x1a896f, 0x1a899d |
| +0x1f8 | u32 | ZFUNC (compare table 4.6) | DISASM engine+0x1a8c32 |
| +0x1fc | u32 | alpha test: 0 = ALPHATESTENABLE off, else on with ALPHAFUNC = compare table | DISASM engine+0x1a8bea |
| +0x200 | u32 | FILLMODE: 0 WIREFRAME, 1 SOLID; `wireframe_models` forces WIREFRAME | DISASM engine+0x1a8c53, 0x1a8c74 |
| +0x204 | u8 (+3) | bump block on | DISASM engine+0x1a90ca |
| +0x208 | u32 | bump stage (the matrix is built by engine+0x1a8570) | DISASM engine+0x1a90d6 |
| +0x20c / +0x210 | f32 | bump luminance scale / offset (BUMPENVLSCALE / BUMPENVLOFFSET of that stage) | DISASM engine+0x1a90e5, 0x1a910c |

Lighting (pass +0x1f4): on -> LIGHTING on with the style material; off -> LIGHTING off, except that for a pass after
the first, with `multipass_light_match` (default 0) set, the first pass's flag decides and the material is diffuse
white, everything else black (DISASM engine+0x1a8961..0x1a8bcb). With LIGHTING off and no vertex colour in the model
vertex format (FMT_LTB §6.1) the diffuse input is white (PUBLIC D3D rule).

### 4.3 Operation tables (engine+0x1b50d0 fills VA 0x7e8318.., PROVEN; capability bits DISASM engine+0x161e10)

Colour op value -> Direct3D op: 0 DISABLE, 1 SELECTARG1, 2 SELECTARG2, 3 MODULATE, **4 MODULATE2X**, 5
BLENDCURRENTALPHA, 6 BLENDFACTORALPHA, 7 ADD, 8 DOTPRODUCT3, 9 BUMPENVMAP, 10 BUMPENVMAPLUMINANCE, 11 ADDSIGNED, 12
ADDSIGNED2X, 13 SUBTRACT, 14 MODULATEALPHA_ADDCOLOR, 15 MODULATEINVALPHA_ADDCOLOR, 16 BLENDTEXTUREALPHA. The capability
check tests D3DTEXOPCAPS bit 0x10 (MODULATE2X) for value 4, which confirms the table. The table index is not bounded:
values 17..27 read the alpha table that follows it (OBSERVED: one ROM pass uses colour op 19 on stage 1, which reads
SELECTARG2).

Alpha op value -> 0 DISABLE, 1 SELECTARG1, 2 SELECTARG2, 3 MODULATE, 4 BLENDCURRENTALPHA, 5 BLENDFACTORALPHA, 6 ADD, 7
ADDSIGNED, 8 ADDSIGNED2X, 9 SUBTRACT, 10 BLENDTEXTUREALPHA.

### 4.4 Arguments

0 CURRENT, 1 DIFFUSE, 2 TEXTURE, 3 TFACTOR (colour and alpha tables equal). PROVEN engine+0x1b50d0.

### 4.5 Coordinates, addressing, filtering (PROVEN engine+0x1b50d0)

- Coordinate source 0..3 = vertex set 0..3; 4 / 5 / 6 = camera-space normal / position / reflection vector (with the
  stage index ORed in, as Direct3D requires); 7 / 8 / 9 = the same with the camera matrix applied (4.1).
- Address 0 WRAP, 1 CLAMP, 2 MIRROR, 3 MIRRORONCE.
- Filter 0 = point / point / no mips, 1 = linear / linear / point mips, 2 = linear / linear / linear mips, 3 =
  anisotropic / anisotropic / linear mips, 4 = point / point / point mips.

### 4.6 Blend and compare (PROVEN engine+0x1b50d0)

Blend value -> (SRCBLEND, DESTBLEND): 1 (ONE, ONE), 2 (INVDESTCOLOR, ONE), 3 (SRCALPHA, INVSRCALPHA), 4 (SRCCOLOR,
INVSRCCOLOR), 5 (DESTCOLOR, INVDESTCOLOR), 6 (SRCCOLOR, DESTCOLOR), 7 (SRCCOLOR, ONE), 8 (SRCALPHA, ZERO), 9 (SRCALPHA,
ONE), 10 (DESTCOLOR, ZERO). Compare value -> 0 ALWAYS, 1 LESS, 2 LESSEQUAL, 3 GREATER, 4 GREATEREQUAL, 5 EQUAL,
6 NOTEQUAL.

## 5. Device check (slot 16, engine+0x161d80, DISASM)

For every pass: the number of stages whose colour op is non-zero must not exceed the device's simultaneous texture
count (VA 0x7e4d94), and every colour op 1..10 / 16 and alpha op 1..6 / 10 used must have its D3DTEXOPCAPS bit
(VA 0x7e4d90). A variant that fails is skipped (section 2). DeCombat does not check (every op is emulated).

## 6. Default style (engine+0x162890, built by engine+0x1a81b0 at start-up and kept at VA 0x7e84e0; DISASM)

Material: ambient (1, 1, 1, 1), diffuse (0.8, 0.8, 0.8, 1) (constant VA 0x6898c4), emissive and specular (0, 0, 0, 1),
power 20 (VA 0x6898c0). One pass: stage 0 = piece texture 0 (source 1), colour MODULATE(TEXTURE, DIFFUSE), alpha
MODULATE(TEXTURE, DIFFUSE), coordinate set 0, wrap, filter 2, no transform (count 2); stages 1..3 colour / alpha
DISABLE with the same arguments; blend off, depth test + write, cull CCW, TEXTUREFACTOR 0x80808080, alpha reference
128 with the alpha test off, lighting on, ZFUNC LESSEQUAL, solid; bump off (stage 1, scale 1, offset 0); no extra block;
options cleared.

## 7. Draw order and per-pass sequence (engine+0x1b94e0, PROVEN)

The model pass builds one entry per drawn piece (4 textures, style, player-view flag, priority, ...), sorts the list
(engine+0x1b92a0; comparator not traced, UNPROVEN), and for each run of pieces with the same style asks the style for
its options block (slot 9): when byte 0 is zero, for every pass the pass setup (engine+0x1a88f0), then per piece the
texture binding (engine+0x1a8240) and the draw; when byte 0 is non-zero the pieces go through the effect path
(RENDER_2009 §7.2: textures bound to the effect parameters, the game module's callback, every effect pass). A style
without an options block returns nothing and the caller tests its own uninitialised buffer (PROVEN engine+0x1b94e0
line `(*style->slot9)(buf); if (buf[0] == 0)`; DISASM engine+0x1624da returns 0 without writing). OBSERVED: byte 0 is
0 in all 48 ROM files that carry the block, so no ROM style selects the effect path; DeCombat draws the passes.

## 8. ROM census (OBSERVED, `style_census`, counts and tags only)

- RS archive: 62 entries = 61 render-style files of 637..1,723 bytes + 1 zero-length entry (unloadable). Every file:
  1 variant, `data_len` = the variant length word = bytes from the material to the end of the last pass block, no
  trailing bytes; 48 files carry the options block, 13 end before it; 7 passes carry the extra block.
- Passes per file: 1 (54 files), 2 (6), 3 (1). Active stages per pass: 1 (50), 2 (15), 3 (2), 4 (2).
- Stage-0 colour op of the first pass: **MODULATE2X 25**, MODULATE 23, SELECTARG1 6, ADD 4, BUMPENVMAP 3.
- Pass words: blend off 27 / (SRCALPHA, INVSRCALPHA) 26 / (SRCALPHA, ONE) 8 / (SRCCOLOR, ONE) 3 / (ONE, ONE) 2 /
  (DESTCOLOR, ZERO) 2 / (SRCCOLOR, INVSRCCOLOR) 1; lighting on 61, off 8; cull CCW 47, NONE 21, CW 1; depth test + write
  41, test only 28; ZFUNC LESSEQUAL 68, NOTEQUAL 1; alpha test off 55, GREATER 8, LESS 4, GREATEREQUAL 1, LESSEQUAL 1;
  fill SOLID 69; bump on 3; TEXTUREFACTOR 0x80808080 in 64 passes.
- Stages: coordinate set 0 71, camera-space normal 11, camera-space reflection 10, set 1 1, set 3 1; transform on 13
  (count 2: 9, count 3: 4); address WRAP 87, MIRROR 5, CLAMP 2; filter 2 77, 1 16, 0 1.
- Tables (rows naming each style, by `sha256[:4]` tag of the upper-cased path): body rows (`[Model<n>] RenderStyle<k>`)
  use 5 styles: `94167e6e` (MODULATE2X) 29, `d90d3a08` (MODULATE) 33, `5579eb7a` (SELECTARG1) 33, `52732164` 3,
  `8e4a3bef` 1; face rows `94167e6e` 32, `cb171938` (MODULATE) 22, three others 1 each; weapon third-person
  (`HHRenderStyle`) `94167e6e` 284, `cb171938` 7 (297 rows with a third-person model, 7 without a style); player-view
  (`PVRenderStyle`) `cb171938` 610, `94167e6e` 493, `52732164` 284, `b08d7a09` 33, `b4525d42` (2 passes) 33, `0e5b064e` 1.
- The default character (body row 0, face row 0, the reference stack's choice for a player without a look): body style slots
  [`94167e6e` MODULATE2X, `d90d3a08` MODULATE, `5579eb7a` SELECTARG1 + alpha test GREATEREQUAL 160]; its 7 pieces use
  slot 0 (the four skin pieces: MODULATE2X with material ambient 0.29, diffuse 0.76, emissive 0.27, specular 0.39 / power
  10) and slot 2 (the three team-mark / emblem pieces: texture only, alpha-tested); the face uses `94167e6e` twice.
- World objects: only the capture-flag class (`e576ae011fa1c731/8`, 30 objects) names a style (property
  `70a4f797c5e00a4e/12` -> `8e4a3bef`, MODULATE).

## 9. Consequence for the model brightness question (render-models open question 1)

Settled by the data: the default character's skin pieces and 284 of 291 styled weapon models use MODULATE2X with a
material whose emissive term (0.27) and ambient (0.29 x grid) light the model even in a dark grid. DeCombat render of
the default character (ROM-gated test, mean luminance of the model pixels, plain state -> styles): BRUSHWOOD 3.6 ->
23.3, CABIN FEVER 7.5 -> 26.8, COLDSEED 30.0 -> 46.5 (OBSERVED in our renderer; not compared with the 2009 client).

## 10. DeCombat (dc-model `style`, dc-render `style.rs`, `model.wgsl`, `model_gpu.rs`)

The parser keeps every word raw (round-trips the 61 ROM files byte for byte); the renderer translates through the
tables above and emulates the four stages in WGSL (public D3DTEXTUREOP / D3DTA definitions, saturation after each
stage, specular added after the stages), one pipeline per pass raster state, one draw per pass. DeCombat choices
(proposed REN-1..REN-6): an empty stage reads opaque black (UNPROVEN for D3D9), coordinate sets 1..3 read set 0
(the ROM vertices carry one set), the bump ops pass the current value through, MIRRORONCE is drawn as MIRROR, the
wireframe fill value is drawn solid, pieces are stable-sorted by priority with blending pieces last.

## 11. Open items

1. Bytes 1..7 of the options block (OBSERVED values (0, 0), (6, 55), (6, 56), (74, 20), (82, 21) in bytes 1..2, the
   rest 0) and the 10-byte extra block (not traced; possibly effect / shader parameters).
2. The piece-list comparator (engine+0x1b92a0 -> engine+0x618ee0).
3. The bump matrix (engine+0x1a8570) and the BUMPENVMAP styles (3 ROM files).
4. Whether a NULL stage texture reads black or white in the 2009 client's D3D9 driver path (REN-1).
5. Screenshot comparison with the 2009 client (V-RS1: default character in BRUSHWOOD, same camera).

## 12. Fold record (spec-fold-6, 2026-10-25)

The header now points at the PENDING rows REN-1..REN-11 in `DC_DEFAULTS.md`; `RENDER_2009.md` 7.3 references this file instead of leaving the format open (its open item 7
and item 24 are closed). The rest of the render-styles addendum went to `RENDER_2009.md` 2.4, 5.4, 6.2, 6.3.1, 7.7, Appendix S and `FMT_LTB.md` 14. No content of this file was changed.
