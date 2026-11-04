# FMT_DTX: texture container as loaded by the 2009 Engine.exe

- Status: SPEC (derived from decompiler output of the 2009 Engine.exe plus a header-only scan of the user ROM)
- Module: `engine` = 2009 EU-test `Engine.exe`, image base 0x00460000, every RVA below is `engine+0xRVA`
- Sources used: `Engine.exe/*.c` (2009), `extracted/.../Engine.exe` read-only for three
  small constant tables, own scratch dumpers `staging/fmt_dtx_{hist,odd,cmd}.py` built on
  `evidence/rezls.py`. No SDK or other proprietary source text was used; all names here are ours.
- ROM data policy: counts only. No pixel data, no texture names, no file contents are reproduced.

Byte order is little-endian throughout. "Header" means the 0xA4-byte block at file offset 0.

## 1. Load entry points

| what | engine RVA | notes |
|---|---|---|
| container parser (`tex_parse`) | 0x3e0e0 | reads header, validates, allocates, reads levels; error tag string is the engine's own |
| in-memory texture allocator | 0x3dee0 | sizes the level chain, builds per-level records |
| level size function (`level_bytes(fmt,w,h)`) | 0x82c70 | see section 4 |
| bytes-per-pixel table (9 entries, indexed by format code) | 0x21b7b8 (VA 0x67b7b8) | read from the binary; see section 3 |
| "system texture" loader (calls the parser, then parses command string) | 0x89a60 | section 7 |
| Direct3D upload (format choice, mip trimming, cube path) | 0x101b10 | section 6 |
| format code + flags -> device surface format | 0x100e50 | section 6 |
| first level that fits the device size cap | 0x100c70 | section 6 |
| square / aspect-ratio clamp of upload size | 0x100b80 | section 6 |
| video-memory accounting per texture | 0x100d20 | section 6 |
| command-string statement matcher | 0x23240 | case-insensitive keyword, minimum arg count |
| missing world texture fallback to a default texture path | 0x19c40 | engine falls back to a fixed default `.dtx` |
| runtime-created texture (no file) with channel masks | 0x30c00 | gives the channel layout of each format code |

## 2. Header (0xA4 bytes)

The parser reads exactly 0xA4 bytes (engine+0x3e0e0, read call with length 0xa4) and later copies the whole
block (0x29 dwords) into the in-memory texture object at object offset +8 (same function, copy loop). Object
offsets used by other functions are therefore header offset + 8; the "obj" column records that mapping so the
consumers can be checked.

| hdr off | size | our name | rule / meaning | evidence | obj off |
|---|---|---|---|---|---|
| 0x00 | u32 | `kind` | must be 0, else the parser fails with its "invalid data" code (0x42) | engine+0x3e0e0 (first check) | +0x08 |
| 0x04 | u16 | `width` | top-level width in texels | engine+0x3e0e0 (`*out_w = hdr[4]`), engine+0x3dee0 writes obj+0x0c | +0x0c |
| 0x06 | u16 | `height` | top-level height | engine+0x3e0e0, engine+0x3dee0 writes obj+0x0e | +0x0e |
| 0x08 | i32 | `version` | must be -5 (0xFFFFFFFB), else "invalid version" (0x45); the allocator also stamps -5 | engine+0x3e0e0, engine+0x3dee0 | +0x10 |
| 0x0C | u16 | `level_count` | number of mip levels stored; must be 1..8 else 0x42 | engine+0x3e0e0 | +0x14 |
| 0x0E | u16 | `section_count` | not read by this loader; the object field is forced to 0 after load | engine+0x3e0e0 (tail: obj+0x16 = 0), engine+0x3dee0 | +0x16 |
| 0x10 | u32 | `flags` | see section 5 | engine+0x3e0e0, engine+0x101b10, engine+0x100e50 | +0x18 |
| 0x14 | u32 | `user_word` | copied, no consumer found in the loader or uploader | engine+0x3e0e0 (copy only) | +0x1c |
| 0x18 | u8 | `lod_group` | if < 4, adds a per-group level-skip bias at load time (section 4.2) | engine+0x3e0e0, table at engine+0x26b284 | +0x20 |
| 0x19 | u8 | `upload_levels` | max levels to create on the device; 0 means 8 | engine+0x101b10 (obj+0x21) | +0x21 |
| 0x1A | u8 | `format` | pixel format code, 0 means 3 (section 3) | engine+0x3e0e0, engine+0x30480, engine+0x101b10 (obj+0x22) | +0x22 |
| 0x1B | u8 | `upload_skip` | levels to drop at upload, ignored when the level data is uploaded as native block-compressed | engine+0x101b10 (obj+0x23) | +0x23 |
| 0x1C | u8 | (unnamed) | no consumer found | - | +0x24 |
| 0x1D | u8 | `device_priority` | passed to the device texture's resource-priority setter (D3D9 vtable slot 7) | engine+0x101b10 (obj+0x25) | +0x25 |
| 0x1E | f32 | `scale_minus_one` | renderer stores `value + 1.0` in its texture record | engine+0x101b10 (obj+0x26) | +0x26 |
| 0x22 | i16 | `angle_deg` | converted to radians (x 0.017453292); cos and sin stored in the renderer record | engine+0x101b10 (obj+0x2a/0x2b) | +0x2a |
| 0x24 | char[128] | `command` | NUL-terminated statement list, section 7 | engine+0x89a60, engine+0x101b10 (obj+0x2c) | +0x2c |

Notes:

- The field order (width/height before the version word) is what the 2009 code reads and what every ROM file
  carries; do not reorder it to match other engines' descriptions.
- Bytes 0x18..0x23 are a 12-byte block in which only the offsets listed above have a known consumer.

## 3. Format codes

From the size function (engine+0x82c70), the bytes-per-pixel table (engine VA 0x67b7b8), the pitch rule in the
allocator (engine+0x3dee0), the channel-mask setup for runtime textures (engine+0x30c00 calling
engine+0x829c0) and the device-format chooser (engine+0x100e50):

| code | level bytes | row pitch | channel layout | device path | ROM count |
|---|---|---|---|---|---|
| 0 | as code 3 (every consumer maps 0 to 3) | - | - | - | 0 |
| 1 | 1 x w x h (table) | 0 (not set) | not seen | - | 0 |
| 2 | 2 x w x h | 2w | 16-bit; runtime textures use 4-4-4-4, 1-5-5-5 or 5-6-5 masks | slot chosen by flags | 0 |
| 3 | 4 x w x h | 4w | 32-bit, A mask 0xFF000000, R 0x00FF0000, G 0x0000FF00, B 0x000000FF (bytes B,G,R,A in memory) | slot chosen by flags | 6,777 |
| 4 | w x h / 2 | 0 | 4x4 blocks, 8 bytes per block (DXT1) | FourCC `DXT1` if supported | 3,030 |
| 5 | w x h | 0 | 4x4 blocks, 16 bytes (DXT3) | FourCC `DXT3` | 536 |
| 6 | w x h | 0 | 4x4 blocks, 16 bytes (DXT5) | FourCC `DXT5` | 928 |
| 7 | 1 x w x h | w | 8-bit single channel (meaning of channel not established) | - | 0 |
| 8 | 3 x w x h (table) | 0 | 24-bit (table only) | - | 0 |

The engine's size rule for block formats is the plain `w*h/2` or `w*h`, with no rounding up to whole 4x4
blocks (engine+0x82c70). In the ROM every block-compressed file stops its chain at a level of at least 4x4, so
the two rules agree; a writer for our tools must respect the same lower bound.

## 4. Level chain

### 4.1 File layout

Immediately after the header, levels are stored largest first. Level `i` has `w_i = width >> i`,
`h_i = height >> i` (plain shift, engine+0x3e0e0 inner loop and engine+0x3dee0) and occupies
`level_bytes(format, w_i, h_i)` bytes. There is no per-level header and no padding.

When `flags & 0x400` (six-face texture) is set, the loader loops six times over the full chain (face-major in
the file) and, after the first face only, reads and discards a further 0x20 bytes (engine+0x3e0e0, outer loop).
The allocator multiplies every level size by 6 and stores the six faces of one level contiguously in memory
(engine+0x3dee0), i.e. memory is level-major while the file is face-major.

ROM check: for all 11,271 non-empty DTX entries, `entry_size == 0xA4 + sum(level_bytes)` exactly (zero
trailing bytes in every file). No ROM file has `flags & 0x400`, so the 0x20-byte block is unverified.

### 4.2 Load-time level skip

`skip = global_bias + group_bias[lod_group]` when `lod_group < 4`, else `skip = global_bias`; negative values
clamp to 0, and `skip` is capped at `level_count - 1` (engine+0x3e0e0). `group_bias` is a 4-entry i32 table at
engine VA 0x6cb284 whose shipped values are {0, 0, 1, 1}; `global_bias` lives in .data at VA 0x6df958 (0 in the
image, presumably a setting written at run time; writer not located). Skipped levels are seeked over, not read.
The object then records `width >> skip`, `height >> skip` and `level_count - skip`.

In the ROM `lod_group` is not always 0 (corrected 2026-10-24; dc-tex census, OBSERVED: group 0 in 10,936 files, 1 in 110,
2 in 157, 3 in 40, 5 in 28): the 197 files of groups 2 and 3 get a default skip of one level under the shipped table
{0, 0, 1, 1}; group 5 is above 3 and takes the global bias only.

### 4.3 In-memory level records

The allocator (engine+0x3dee0) allocates the object (0x1C8 bytes) plus one block for all levels and fills a
record of 6 dwords per level starting at object +0x108: width, height, data pointer, data pointer (duplicate),
byte size, row pitch (section 3).

### 4.4 Format conversion on load

The parser asks a renderer hook (function pointer at VA 0x6e1050) whether the stored format is acceptable; a
refusal returns "invalid file" (0x4d). Conversion is attempted when `flags & 0x40` is clear and the caller's
requested format differs from the stored one; `flags & 0x200` suppresses conversion entirely. When converting,
each kept level is read into a scratch buffer and passed to a second renderer hook (VA 0x6e1054) with the
source format and channel masks (32-bit masks as in section 3 for code 3/0, zero masks otherwise). The exact
requested-format argument is muddled in the decompilation (parameter mapping of engine+0x3e0e0); treat as
PLAUSIBLE.

## 5. Flags (header 0x10)

| bit | effect in 2009 code | evidence | ROM files with bit |
|---|---|---|---|
| 0x0001 | device format: picks the same slot as 0x100 (slot 1) instead of slot 3; also sets renderer record bit 0x01 | engine+0x100e50, engine+0x101b10 | 181 |
| 0x0002 | device format: when CLEAR (and the setting at VA 0x6d3f8c is on and slot 0 exists) the 32-bit slot 0 is used; when SET the 16-bit path below is taken (corrected 2026-10-24, RENDER_2009 10.1) | engine+0x100e50 | 4 |
| 0x0008 | no consumer found in loader/uploader | - | 11,271 (all) |
| 0x0040 | "keep stored format": disables load-time conversion | engine+0x3e0e0 | 0 |
| 0x0080 | device format: slot 2 | engine+0x100e50 | 196 |
| 0x0100 | device format: slot 1 | engine+0x100e50 | 121 |
| 0x0200 | never convert on load | engine+0x3e0e0 | 0 |
| 0x0400 | six-face texture: 6x storage, cube surface on the device (renderer record bit 0x02) | engine+0x3e0e0, engine+0x3dee0, engine+0x101b10, engine+0x100d20 | 0 |
| 0x0800 | device format: slot 6; renderer record bit 0x04 | engine+0x100e50, engine+0x101b10 | 0 |
| 0x1000 | device format: slot 7; renderer record bit 0x08 | engine+0x100e50, engine+0x101b10 | 0 |

Device-format slot priority (engine+0x100e50), used for non-block formats and for block formats when native
block compression is off or unsupported: 0x1000 -> 7, else 0x800 -> 6, else (0x2 clear, setting on, slot 0 enabled) -> 0, else
0x100 -> 1, else 0x80 -> 2, else 0x1 -> 1, else 3. Each slot is a 12-byte entry {enabled, device format, bytes per pixel}
at +0x28 of the object at VA 0x7e5dd8 (= VA 0x7e5e00), filled by engine+0x101240 from device enumeration; a disabled slot
yields format 0 and the upload fails. The slot table (EVIDENCE engine+0x101240; the alternatives of a slot are listed in the
order the filler tries them, PLAUSIBLE that the first one the device supports wins): 0 A8R8G8B8; 1 A1R5G5B5 / A4R4G4B4; 2 A4R4G4B4 / A1R5G5B5; 3 R5G6B5 / A1R5G5B5 / A4R4G4B4; 4 A8R8G8B8 / A1R5G5B5 /
A4R4G4B4; 5 (lightmaps) R8G8B8 / X8R8G8B8 / A8R8G8B8 when the setting at VA 0x6d3fa4 is on, else X1R5G5B5 / A1R5G5B5;
6 V8U8 / Q8W8V8U8; 7 X8L8V8U8 or L6V5U5 (PUBLIC format names). Consequence: with the shipped defaults every non-block ROM
texture goes up as A8R8G8B8 (only 4 files have 0x2). Interpretation for a modern renderer
(DeCombat reading, not 2009 evidence): slot 3 is the default full-colour-with-alpha path; 0x80/0x100/0x1 select
reduced or alternative alpha precision. Our renderer may upload everything as RGBA8 or native BCn and use the
bits only as hints.

Native block upload (engine+0x100e50, engine+0x101b10, engine+0x100d20) requires a global "compressed textures"
setting (VA 0x6d3f5c) and a per-format capability byte (VA 0x7e5dd9/0x7e5dda/0x7e5ddb for codes 4/5/6).

Alpha test: the command-string keyword for an alpha reference (section 7) sets an i16 reference value in the
renderer record (engine+0x101b10). No flag bit selects alpha blending in these functions; blending is decided
by the material/surface that uses the texture, not by the container.

## 6. Upload rules (engine+0x101b10)

1. `first = upload_skip`, unless the texture goes up as native block-compressed (then 0); capped at
   `level_count - 1`.
2. `first = max(first, first level whose w <= cap_w and h <= cap_h)` (engine+0x100c70), where each cap is
   `min(texture-size setting at VA 0x6d3fbc, device maximum)`, 0 meaning 256. No level fits: no upload.
3. `count = upload_levels` (0 -> 8), at most the remaining levels.
4. Block-compressed and final sizes pass through engine+0x100b80: with a device "square only" capability the
   size becomes max(w,h) square; with an aspect-ratio limit (VA 0x6d3f74 > 0) the short side is raised to
   `long / limit`.
5. Six-face textures create a cube surface of edge max(w,h).
6. Device priority from header 0x1D; per-record scale (`hdr 0x1E + 1.0`), angle cos/sin (header 0x22), alpha
   reference and mip bias (section 7).

## 7. Command string (header 0x24)

The string is parsed as statements of whitespace-separated tokens; the matcher (engine+0x23240) compares the
first token case-insensitively (`_stricmp`) and requires a minimum token count. Keywords are interop facts of
the file format, listed with the token count the engine requires (keyword included):

| keyword | tokens | consumer | effect |
|---|---|---|---|
| `AlphaRef` | >=2 | engine+0x101b10 | integer alpha-test reference |
| `MipMapBias` | >=2 | engine+0x101b10 | float LOD bias |
| `DetailTex` | >=2 | engine+0x89a60 | secondary texture name, mode 1 |
| `EnvMap` | >=2 | engine+0x89a60 | environment texture, mode 2 |
| `EnvMapAlpha` | >=2 | engine+0x89a60 | environment texture weighted by alpha, mode 3 |
| `EnvBumpMap` | >=3 | engine+0x89a60 | env + bump textures, mode 4 |
| `EnvBumpMapNoFallback` | >=3 | engine+0x89a60 | same, mode 5 |
| `DOT3BumpMap` | >=2 | engine+0x89a60 | normal-map texture, mode 6 |
| `DOT3EnvBumpMap` | >=3 | engine+0x89a60 | normal + env textures, mode 7 |
| `Effect` | >=2 | engine+0x89a60 | integer effect id plus up to 4 texture names, mode 8 |

engine+0x89a60 tests the keywords in the order DetailTex, EnvMap, EnvMapAlpha, EnvBumpMap, EnvBumpMapNoFallback,
DOT3BumpMap, DOT3EnvBumpMap, Effect and stops at the first match. Too few arguments returns "invalid data".
Referenced textures are loaded recursively by name.

ROM (counts only): 624 files have a non-empty command; 617 contain `AlphaRef`, 3 `EnvMapAlpha`, 2 `DetailTex`;
the rest have no recognised keyword (tool residue, ignored by the engine).

## 8. ROM census (header-only scan, all .rez under `Game/`)

11,272 DTX entries in 95 rez files; one entry is 0 bytes long, the other 11,271 all pass the engine's checks
(`kind` 0, version -5, `section_count` 0, power-of-two dimensions; `lod_group` values in 4.2).

- Format: code 3: 6,777; code 4: 3,030; code 6: 928; code 5: 536.
- `TEXTURES.rez` alone: 2,342 entries (4: 1,693; 6: 587; 3: 45; 5: 17). One UI rez (`UI_SCR_Base.rez`): 390
  entries (3: 389; 5: 1).
- `level_count`: 1: 6,705; 4: 2,854; 6: 632; 7: 410; 5: 394; 2: 132; 3: 117; 8: 27.
- Largest side: 256: 4,159; 64: 2,554; 512: 1,714; 128: 1,364; 1024: 812; 32: 553; 16: 102; 8: 13. Nothing
  above 1024.
- Smallest stored level side: >= 4 for every block-compressed file; 13 code-3 files go down to 2 or 1.
- `flags`: 0x8: 10,807; 0x88: 160; 0x9: 146; 0x108: 119; 0x89: 34; 0x8a: 2; 0xa: 1; 0x109: 1; 0x10a: 1.
- `upload_levels` equals `level_count` in 10,760 files, is 0 (means 8) in 458, smaller in 42, larger in 11 (one of them
  41 with 8 levels).
- `upload_skip`, bytes 0x1C/0x1D, angle: always 0. Float at 0x1E: 0.0 except one file at -1.0.
- `user_word`: 0 in 9,068 files, 20 small distinct values elsewhere (meaning OPEN).

## 9. Open items

- (closed 2026-10-24) Device format behind each flag-selected slot: the slot table of section 5 (filler engine+0x101240).
- Meaning of flag 0x8 (set on every file) and of `user_word`; possibly read by CShell.dll (not checked here).
- Writer of the global level-skip bias at VA 0x6df958 and of the size cap at VA 0x6d3fbc (likely settings).
- The 0x20-byte block after the first face of six-face textures: content unknown, no ROM sample.
- Exact requested-format argument of the conversion decision in engine+0x3e0e0 (decompiler parameter mix-up).

## 10. Fold record (spec-fold-5, 2026-10-25)

The addendum of 2026-10-24 (from RENDER_2009 10.1, staged as `staging/render-2009_FMT_DTX_section5.md`) is applied in place: items 1 and 2 to
the flag table and the slot priority of section 5, item 3 (the slot table and the filler engine+0x101240) and item 4 (the A8R8G8B8 consequence) to section 5, the
`lod_group` census to 4.2 and 8, and the "filler not found" open item of section 9 is closed. Grades as given there (EVIDENCE engine+0x100e50 / 0x101240; OBSERVED
census); nothing dropped. The addendum text stays in git history (master e191a26).
