# FMT_SPR_LTC_FX: sprite, texture-script, effect and minor content formats of the 2009 client

Scope: the content formats left after FMT_DTX / FMT_LTB / FMT_DAT85: `.spr` sprite animations, `.tfg` texture
effect groups and `.tfs` texture scripts (both read by the engine), the effect files in the effect archive (`.fxf`
read by the shell and handed to the effect module, `.fcf` editor tree), the effect type registry exported by the
effect module, `.ltc` compressed sources, and the small families `.wst`, `.lta`, `.pk`, `.pcx`/`.bmp`/`.psd` and the
52 entries without an extension. Not covered: shaders (`.fx`, `.psh`, `.ps`), fonts (`.ttf`, `.ttc`), game tables
(`.txt`, `.gms`), stray tool files (`.db`, `.max`, `.scc`); section 9.6 only classifies them.
Evidence sources: decompiler output of the 2009 `Engine.exe` (`engine+0xRVA`, image base 0x460000), `CShell.dll`
(`cshell+0xRVA`, base 0x37100000) and `ClientFx.fxd` (`clientfx+0xRVA`, base 0x410000) under
``; read-only capstone disassembly and constant reads of the same ROM binaries; header and grammar
scans of every ROM entry of each type with scratch dumpers in `staging/fmt_spr_ltc_fx/`
(counts only). PUBLIC references are named where used.
Status: DRAFT (code-derived plus full ROM scans; nothing here is live-tested against the running client)

Conventions. Little-endian. `u8/u16/u32/i32/f32` as usual. "stream read n" means the engine's file stream object
reads n raw bytes (stream vtable slot +8); "stream string" is the engine stream's length-prefixed string read
(slot +0xC). Names in `code` are ours. ROM strings are never quoted: where a literal matters it is given as
`sha256[:16]/length` of its exact bytes. File extensions are named because they are the subject of this spec.

## 1. ROM inventory (all 95 archives under the client folder)

Entry counts by extension, from `inventory.py` over the 95 `.rez` archives (archive directory walk as in
`evidence/rezls.py`). Sizes are the sum of entry sizes in bytes.

| extension | entries | bytes | archives holding them | section |
|---|---|---|---|---|
| `.spr` | 233 | 45,408 | 6 archives (190 in the effect-art archive) | 2 |
| `.tfg` | 2 | 215 | 1 | 3 |
| `.tfs` | 6 | 660 | 1 | 4 |
| `.fxf` | 1 | 5,140,629 | effect archive | 5 |
| `.fcf` | 1 | 39,798 | effect archive | 7 |
| `.ltc` | 111 | 264,494 | 85 effect-art archive, 26 prefab archive | 8 |
| `.wst` | 4 | 4,790 | 1 (character body archive) | 9.1 |
| `.lta` | 4 | 5,008,846 | 2 | 9.2 |
| `.pk` | 3 | 16,080 | 1 | 9.3 |
| `.pcx` / `.bmp` / `.psd` | 3 / 15 / 1 | 156,086 / 675,912 / 21,854 | 1 / 2 / 1 | 9.4 |
| (no extension) | 52 | 3,144 | 5 | 9.5 |

Who reads what (searched as literals and by caller chains in all three modules, `extsearch` step in the report):
only `.spr` (engine), `.tfg`/`.tfs` (engine, by directory and archive name rather than extension) and `.fxf`
(shell) have a loader in the 2009 client. No module contains the extension text of `.ltc`, `.lta`, `.wst`, `.pk`,
`.fcf`, nor the keywords that open the text sources of section 8/9.2 (0 hits each in all three binaries). Those
families are authoring leftovers that the runtime never opens; a reimplementation can ignore them (tier DROP) and
only needs to tolerate their presence in the archives.

## 2. `.spr` sprite animation

### 2.1 Who loads a sprite

| what | evidence |
|---|---|
| sprite loader: looks up the file, refuses a missing file (engine error code 0x2D), parses, refuses a bad file (0x2E), caches the parsed sprite per file entry and links it into a global list | engine+0xa1970 |
| sprite parser (section 2.2) | engine+0xa7430 |
| sprite free (frame arrays, animation array, object) | engine+0xa73b0 |
| playback cursor init: cursor = {sprite, animation 0, current frame = frame 0 of animation 0, time 0, flags 3} | engine+0xa72f0 |
| playback cursor advance (section 2.4) | engine+0xa7330 |
| texture-or-sprite dispatch by the last three characters of a texture name, compared case-insensitively against the 3-letter tags `DTX` and `SPR` at VA 0x67b7b0 / 0x67b7ac | engine+0x81150 (world/model texture slot), engine+0xa1b20 (per-slot texture table of a renderable, slot stride 0x14 cursors) |
| world surface textures: a name ending in `.spr` (case-insensitive, 4-char suffix) is loaded as an animated sprite, anything else as a texture | engine+0x1818e0 calls engine+0x1786a0 |
| poly-grid texture setter accepts a sprite | engine+0xf940 |
| resource preload by resource kind: kind 1 = sprite | engine+0x14150 |
| effect module path properties carry the extension tag `spr` or `dtx` before a `|` (section 5.6) | clientfx+0x11d80 |

### 2.2 File layout

One animation per file. The parser allocates an animation array of length 1 (engine+0xa7430: object +0x10 = 1,
array of 0x3C-byte records at object +0xC) and then reads, for that one animation:

| off | size | our name | rule / meaning | stored at (animation record) | evidence |
|---|---|---|---|---|---|
| 0x00 | u32 | `n_frames` | number of frames; a frame-handle array of `4*n_frames` bytes is allocated | +0x24, array ptr +0x20 | engine+0xa7430 (1st read, alloc) |
| 0x04 | u32 | `frames_per_second` | the engine computes `1000 / frames_per_second` (integer milliseconds per frame) at load time; 0 would fault | +0x2C; `+0x28 = (1000 / fps) * n_frames` (animation length in ms) | engine+0xa7430 (`div` at 0xa7551) |
| 0x08 | u32 | `flag_a` | only the low byte is kept | +0x30 (u32 of the byte) | engine+0xa7430 (3rd read, `movzx` at 0xa7560) |
| 0x0C | u32 | `flag_b` | only the low byte is kept | +0x38 | engine+0xa7430 (4th read, 0xa7568) |
| 0x10 | u32 | `key_word` | kept whole | +0x34 | engine+0xa7430 (5th read, 0xa7570) |
| 0x14 | records | `frames[n_frames]` | per frame: `u16 len` then `len` bytes of a texture path (no terminator in the file); `len > 1000` aborts the parse (sprite rejected); the name is NUL-terminated in a 1 KiB buffer and resolved to a texture handle | handle in `frames[i]` | engine+0xa7430 (`cmp ax,0x3e8`), engine+0x259d0 (name to texture handle) |

After the last frame the parser asks the stream for its error state (slot +0x14) and rejects the sprite if it is
set. The animation name is a fixed engine default (8-character literal at VA 0x67dfac), not read from the file.
Remaining animation record fields (+0x00..+0x1F name) are the engine's.

Meaning of `flag_a`, `flag_b`, `key_word`: no reader of animation +0x30/+0x34/+0x38 was found in the engine
(UNPROVEN). Their slot order and the ROM values (section 2.5) are consistent with "two booleans and a 32-bit
colour-key word", which is how a reimplementation should store them until a consumer is found.

### 2.3 Frame names

Frame paths are archive-relative texture paths; in the ROM every one of the 1,192 frame names ends in `.dtx`
(spr_dump `frame_ext`), longest 93 bytes, none contains a NUL. Paths use backslashes.

### 2.4 Playback (behaviour a renderer must match)

engine+0xa7330, called with a millisecond delta:

- if the cursor flag bit 0 (playing) is set, `time += delta_ms`;
- if `time < length_ms` or flag bit 1 (loop) is set: `time %= length_ms`; else `time = length_ms - 1` (hold last
  frame);
- `frame = (time / (1000 / fps)) % n_frames` (integer arithmetic); the cursor's current handle becomes
  `frames[frame]`;
- if `length_ms == 0` the current handle is cleared (no frame), which also covers `fps > 1000`.

The init (engine+0xa72f0) sets flags = 3 (playing + looping).

### 2.5 ROM validation (spr_dump.py, 233 entries)

| check | result |
|---|---|
| parse with 2.2 ends exactly at the entry end | 231 |
| zero-length entry | 1 |
| rejected by the engine rule (first frame name length 0x2012 > 1000; header words are filled with 0x20 bytes) | 1 |
| `n_frames` over the 231 parsed files = 1 / 2..7 / 8..31 / >= 32 / 0 | 156 / 22 / 49 / 3 / 1 (the 0-frame file is 20 bytes, header only) |
| `frames_per_second` values over the 231 parsed files | 16 distinct values, all in 1..30 (the rejected file would read 0x20202001) |
| `flag_a`, `flag_b`, `key_word` all zero | 230 of the 232 non-empty files; the other 2 (one parsed, one rejected) hold 0x20202020 in all three |
| frame name extension `.dtx` | 1,192 of 1,192 |

Implementation notes: guard `fps == 0` (the 2009 code would divide by zero; no ROM file has it); treat a name length
above 1000 as a broken sprite; tolerate the 0-byte entry (loader: stream error, sprite rejected).

## 3. `.tfg` texture effect group

A texture effect group binds up to two texture-coordinate stages of a world surface to texture scripts
(section 4). Both file families sit in their own archives, which the engine mounts by name in its fixed archive
list (engine+0x8000).

### 3.1 Who loads it

| what | evidence |
|---|---|
| world render data: per texture section a 0x105-byte effect-group name is read; when non-empty the group is loaded and attached to the section | engine+0x1818e0 (calls engine+0x182140, engine+0x183800) |
| group loader: returns a cached group by name (reference count at group +0xE4), else builds the path `<dir>\<name>` with the 20-byte directory literal sha256[:16] `b9edfec472226554`, opens it, parses, caches | engine+0x183800, cache lookup engine+0x182bf0, cache insert engine+0x183520 |
| group object init (two 0x6C-byte stage records, dirty flag +0xE0 = 1) | engine+0x1940a0 |
| stage binders: "own script" engine+0x194020, "share another stage" engine+0x194060 | |
| per-frame stage evaluation (section 4.4) | engine+0x1940e0 |

### 3.2 File layout

| off | size | our name | rule | evidence |
|---|---|---|---|---|
| 0x00 | u32 | `version` | must be 1, else the group is discarded | engine+0x183800 (`cmp [..],1` at 0x183986) |
| 0x04 | u32 | `n_stages` | clamped to at most 2 | engine+0x183800 (0x1839b6) |
| 0x08 | stage[n_stages] | stages | see below | |

Stage record (variable length), read for stage index `i`:

| field | size | rule | evidence |
|---|---|---|---|
| `kind` | u32 | 0 = unused stage, nothing more is read; 1 = share; 2 = own script; other values: nothing more is read | engine+0x183800 (0x1839e0..0x183a34) |
| `channel` | u32 | present when `kind != 0`; texture-coordinate channel the stage drives; a stage with channel 0 is left inactive by both binders | engine+0x194020, engine+0x194060 (`param != 0` test) |
| `source_stage` | u32 | only for `kind == 1`; must be < 2; stage `i` then reuses the matrix of stage `source_stage` | engine+0x194060 (stage +0x68 = address of the source stage) |
| `script` | stream string | only for `kind == 2`; u16 length then that many bytes (no terminator in the file), max 0x104; a rooted archive path of a `.tfs`. The u16-length layout is confirmed by the ROM scan (3 / 3); the stream method itself was not disassembled | engine+0x183800 (stream slot +0xC, 0x104) |
| `param_count` | u32 | only for `kind == 2` | engine+0x183800 (0x183a55) |
| `params` | f32[param_count] | only the first 6 are kept (the rest are read and dropped); they become script registers 0x20..0x25 (section 4.3) | engine+0x183800 (`cmp edi,6` at 0x183a72) |

For `kind == 2` the script is loaded (section 4.2); if that fails the stage stays unbound. The six parameters are
stored in the stage record next to the script pointer (stage +0x48..+0x5C, the `puVar7[-6..-1]` slots of
engine+0x1940e0).

### 3.3 ROM validation (tfs_dump.py, tfg part)

| check | result |
|---|---|
| files | 2 |
| `version` = 1, `n_stages` = 2 | 2 / 2 |
| stage kinds | 3 x kind 2, 1 x kind 0 |
| channels used | 1 (twice), 2 (once) |
| script paths ending in `.tfs`, no NUL inside | 3 / 3 |
| `param_count` = 6 | 3 / 3 |
| bytes left after the last stage | 0 in both files |

## 4. `.tfs` texture script

A texture script is a tiny register-machine program that produces a 4x4 texture-coordinate matrix every frame.

### 4.1 Who loads it

| what | evidence |
|---|---|
| script cache: returns a loaded script by case-insensitive path, else constructs one (0x144-byte object) and loads it | engine+0x183600 |
| script object constructor / destructor / buffer free | engine+0x1b4170, engine+0x1b4250, engine+0x1b41c0 |
| file parser (section 4.2) | engine+0x1b4280 |
| evaluator (section 4.4), script vtable slot 1 (vtable at VA 0x68a91c) | engine+0x1b4500 |
| flag word accessor (slot 2) returns object +0x134; mode accessor (slot 3) returns object +0x138 | engine+0x1b41a0, engine+0x1b41b0 |
| opcode handler table, 10 entries at VA 0x6d66b0 | engine+0x1b4090..engine+0x1b4220 |
| unary function table, 6 entries at VA 0x6d6698 | engine+0x1b3ff0..engine+0x1b4080 |

### 4.2 File layout

| off | size | our name | rule / meaning | evidence |
|---|---|---|---|---|
| 0x00 | u16 | `version` | must be 1, else the load fails | engine+0x1b4280 (`cmp word,1` at 0x1b42fd) |
| 0x02 | u32 | `names_len` | byte length of the parameter-name block that follows; the parser skips it with a seek (tell + `names_len`) | engine+0x1b4280 (slots +0x1C tell, +0x18 seek) |
| 0x06 | `names_len` bytes | `param_names` | authoring names of the user parameters: `u8 count`, then `count` NUL-terminated names. Not read by the engine | ROM scan only |
| +0 | u32 | `coord_mode` | 0..6, mapped to object +0x138 and bit 0x80 of +0x134, see table below | engine+0x1b4280 (jump table at VA 0x6144d4) |
| +4 | u32 | `out_mode` | 0..3, ORs 0x10 / 0x20 / 0x24 / 0x44 into +0x134; other values add nothing | engine+0x1b4280 (jump table at VA 0x6144f0) |
| +8 | u32 | `update_flags` | bit 0 sets +0x134 bit 0x2, bit 1 sets +0x134 bit 0x1 | engine+0x1b4280 (0x1b44ad..0x1b44c0) |
| +12 | u32 | `const_count` | number of constants | engine+0x1b4280 |
| +16 | f32[`const_count`] | `constants` | stored from object +0xA8 upward = registers 0x28.. ; the 2009 loader does not bound this count | engine+0x1b4280 (loop at 0x1b4390) |
| next | u32 | `code_len` | byte length of the program | engine+0x1b4280 (stored at +0x140) |
| next | `code_len` bytes | `code` | instructions, 4 bytes each (section 4.4) | engine+0x1b4280 (buffer at +0x13C) |

`coord_mode` to object state (engine+0x1b4280):

| `coord_mode` | +0x138 | +0x134 bit 0x80 |
|---|---|---|
| 0 | 1 | no |
| 1 | 0 | no |
| 2 | 2 | no |
| 3 | 1 | yes |
| 4 | 0 | yes |
| 5 | 2 | yes |
| 6 | 3 | no |

The evaluator's caller (engine+0x1940e0) uses bit 0x80: when set, the script matrix is combined with a camera
matrix chosen by slot 3 (+0x138 == 0: one fixed engine matrix; otherwise one assembled from camera globals) through
the engine's 4x4 multiply (engine+0x450b0). The meaning of the +0x138 values 1..3 and of the `out_mode` bits beyond
that is UNPROVEN (consumed by the renderer, not traced here).

### 4.3 Register file

Registers are 32-bit floats at object +8 + 4*r, addressed by a byte (engine+0x1b4500):

| registers | object offset | content | evidence |
|---|---|---|---|
| 0x00..0x0F | +0x08 | scratch | engine+0x1b4500 |
| 0x10..0x1F | +0x48 | output matrix, row-major 4x4; reset to identity before every run | engine+0x1b4500 (stores of 1.0 at +0x48, +0x5C, +0x70, +0x84) |
| 0x20..0x25 | +0x88 | the 6 stage parameters (section 3.2); copied in before the run and copied back after it, so a script can keep state in them | engine+0x1b4500 (in at 0x1b4518, out at 0x1b4656) |
| 0x26 | +0xA0 | caller value 0: running time of the group (group +0xD8 plus the value at global VA 0x7e59d8 +0x40, which looks like the frame delta; UNPROVEN units) | engine+0x1b4500, engine+0x1940e0 |
| 0x27 | +0xA4 | caller value 1: running time minus the group's stored time, i.e. time since the previous update | engine+0x1940e0 |
| 0x28.. | +0xA8 | file constants | engine+0x1b4280 |
| 0x48..0x4A | +0x128 | a 3-vector fetched from the camera service before each run (slot +0x74 of the global at VA 0x7eea88) | engine+0x1b4500 |

The object is 0x144 bytes, so a byte register index above 0x4E reaches past it. A reimplementation must bound both
`const_count` (at most 0x20 to stay below 0x48) and every register index.

### 4.4 Instructions

Each instruction is 4 bytes: `op, a, b, dst`. The evaluator walks the code buffer, calls `handler[op](regs, a, b)`
and stores the float result in register `dst` (engine+0x1b4500, call through VA 0x6d66b0, `fstp` to `[edi+ecx*4+8]`).
`op` is not range-checked by the 2009 engine.

| op | result | evidence |
|---|---|---|
| 0 | `r[a] + r[b]` | engine+0x1b4090 |
| 1 | `r[a] - r[b]` | engine+0x1b40b0 |
| 2 | `r[a] * r[b]` | engine+0x1b40d0 |
| 3 | `r[a] / r[b]` | engine+0x1b40f0 |
| 4 | floating remainder `fmod(r[a], r[b])` (C runtime, PUBLIC: C99 7.12.10.1) | engine+0x1b4110 (CRT import thunk engine+0x1deabc) |
| 5 | `func[a](r[b])`, see below | engine+0x1b4130 |
| 6 | `-r[a]` | engine+0x1b4150 |
| 7 | `r[a]` (move) | engine+0x1b4160 |
| 8 | `min(r[a], r[b])` | engine+0x1b41f0 |
| 9 | `max(r[a], r[b])` | engine+0x1b4220 |

| func | result | evidence |
|---|---|---|
| 0 | `cos(x)` (radians) | engine+0x1b3ff0 |
| 1 | `sin(x)` | engine+0x1b4000 |
| 2 | `tan(x)` | engine+0x1b4010 |
| 3 | square wave: `+1.0` if `sin(x) >= 0` else `-1.0` | engine+0x1b4020 (constants 1.0 at VA 0x672988, -1.0 at VA 0x675b4c) |
| 4 | square wave on `cos(x)`, same rule | engine+0x1b4050 |
| 5 | degrees to radians, `x * 0.017453292` | engine+0x1b4080 (constant at VA 0x676914) |

When evaluation happens (engine+0x1940e0, per stage of a group): the script runs when the stage is forced (first
frame after load, or the caller's force flag), or when flag 0x2 or bit 0x80 is set and the global counter at VA 0x7e59d4 changed since the group's last update (group +0xDC), or
when flag 0x1 is set and the six parameters differ from the last values seen. After the run the output matrix is
stored in the stage (stage +0x08..+0x47), combined with the camera matrix when bit 0x80 is set.

### 4.5 ROM validation (tfs_dump.py, 6 files)

| check | result |
|---|---|
| `version` = 1 | 6 / 6 |
| name block = `u8 count` + `count` NUL-terminated names, length equals `names_len` | 6 / 6 (counts 0, 1, 4, 4, 5, 6) |
| `coord_mode` values | 6 (3 files), 4 (2), 5 (1) |
| `out_mode` values | 0 (6 files) |
| `update_flags` values | 3 (5 files), 0 (1) |
| `const_count` | 1 (3 files), 2 (3) |
| `code_len` multiple of 4, no bytes after the code | 6 / 6 |
| opcodes used | 0, 1, 2, 4, 5, 6, 7 (79 instructions); funcs used: 0, 1 |
| register classes read or written | scratch, matrix, parameters, the two time registers, constants; none above 0x47 |
| destinations | scratch (50) or matrix (29) only |

## 5. `.fxf` effect group file

The effect definitions (named effect groups made of timed keys, each key an instance of one effect type with a
property list) live in one `.fxf` in the effect archive. The shell parses the file; each key's property list is
built and interpreted by the effect module (section 6).

### 5.1 Who loads it

| what | evidence |
|---|---|
| bind the effect module: load it from the game folder, or, when it is not a loose file, copy the archived module to a temp file and load that; resolve its 9 exports; copy the type registry (section 6) | cshell+0x29af80 |
| enumerate the virtual directory named by the 8-byte literal sha256[:16] `85736ee4731ef67b` and load every entry whose extension is `.fxf` (case-insensitive) | cshell+0x29c7c0 |
| per-file load: encoding detection, groups, post pass | cshell+0x29c560 |
| group list reader | cshell+0x29c4a0 |
| group reader + key ordering pass | cshell+0x29c260 |
| key reader | cshell+0x29bed0 |
| property reader | cshell+0x29bc00 |
| text line readers (one per value shape) | cshell+0x29b350, +0x29b4a0, +0x29b5f0, +0x29b740, +0x29b890, +0x29b9e0 |
| effect type lookup by name: record pointer / index, case-insensitive, 0x90-byte records | cshell+0x29aab0, cshell+0x29ab10 |

### 5.2 Two encodings

cshell+0x29c560 reads the first 7 bytes and compares them case-insensitively with the 7-byte literal
sha256[:16] `914dc79e48187eee`. Equal: text encoding; otherwise binary encoding. The stream is then rewound to 0
and the whole file is parsed in the chosen encoding. The ROM file is text (its first 7 bytes hash to the same
value); the binary layout below is derived from the reader code only (no ROM sample).

### 5.3 Text line model

Every text item is one line. A line reader (cshell+0x29b350 and its five siblings) reads up to 0x800 bytes from the
current position (fewer at the end of the file), cuts the buffer at the first `\n`, applies a `sscanf` format, then
seeks to the line start plus the cut length plus 1. Every format starts with `%s`, which consumes the line's label
token; labels are never compared, only their position matters. The value conversions used are:

| shape | format | value |
|---|---|---|
| word | `%s %s` | first whitespace-delimited token after the label; a missing token leaves the destination unchanged (it is pre-zeroed) |
| unsigned | `%s %lu` | decimal |
| int | `%s %i` | C `%i` semantics (a `0x` or leading `0` prefix would switch the base; ROM values are plain decimal) |
| float(s) | `%s %f`, `%s %f %f %f`, `%s %f %f %f %f`, `%s %f %f %f %f %f` | |

Format literals: cshell VA 0x374f82a8 (word), 0x374f82b0 (unsigned), 0x374f82a0 (int), 0x374e9d78 (float),
0x374f8270 (five floats). A failed read or seek is reported on the shell console and parsing continues. Carriage
returns stay in the line and are harmless because `%s` stops at whitespace. Consequence: any word value that contains
spaces is cut at the first space (section 5.8 counts them).

### 5.4 File and group

| item | text (one line each, in order) | binary | stored (group record) | evidence |
|---|---|---|---|---|
| group count | unsigned | u32 | | cshell+0x29c4a0 |
| per group: name | word | char[128] | +0x00, 128 bytes | cshell+0x29c260 |
| per group: key count | unsigned | u32 (read before the name) | +0x88 | cshell+0x29c260 |
| per group: length | unsigned, milliseconds | u32 milliseconds (after the name) | +0x80 = f32 seconds (`ms * 0.001`, unsigned conversion) | cshell+0x29c260 (constant 0.001 at VA 0x374ce5cc) |
| per group: keys | key count x key (5.5) | same | array at +0x84, 0x54 bytes per key | cshell+0x29c260 |

Binary group order is therefore `u32 key_count, char[128] name, u32 length_ms`; text order is name, key count,
length.

Key ordering pass (cshell+0x29c260, after all keys of a group are read): repeated sweeps; whenever a linked key
(5.5 `linked`) is followed later in the array by the key whose `id` equals its `link_id`, the two records are
swapped; it stops after a sweep without swaps or after `key_count` sweeps. Effect: link targets end up before the
keys that link to them.

Whole-group flag (cshell+0x29c560, after all groups): key +0x3C is set to 1 when `end - start >= length - 0.01`
(seconds), else 0.

### 5.5 Key

| # | item | text | binary | stored (key record, 0x54 bytes) | evidence |
|---|---|---|---|---|---|
| 1 | effect type name | word; cut at the first `;` | char[128], cut at `;` | +0x00 = pointer to the matching 0x90-byte type record (null if no match) | cshell+0x29bed0, cshell+0x29aab0 (separator literal at VA 0x374f82b8) |
| 2 | `id` | unsigned | u32 | +0x08 | cshell+0x29bed0 |
| 3 | `linked` | int (low byte kept) | 40-byte block: u8 `linked`, 3 bytes pad | +0x14 (u8) | cshell+0x29bed0 (read of 0x28 bytes at 0x29bfdc) |
| 4 | `link_id` | unsigned | block +4: u32 | +0x18 | same |
| 5 | `attach_node` | word | block +8: char[32] | +0x1C, 31 chars + NUL | same (`strncpy` 0x1F) |
| 6 | `start` | float, seconds | f32 | +0x0C, then clamped to [0, group length] | cshell+0x29bed0 |
| 7 | `end` | float, seconds | f32 | +0x10, clamped the same way | cshell+0x29bed0 |
| 8 | `repeat` | unsigned | u32 | passed to the property object (below) | cshell+0x29bed0 |
| 9 | (ignored) | unsigned | u32 | read and dropped | cshell+0x29bed0 |
| 10 | (ignored) | float | 4 bytes | read and dropped | cshell+0x29bed0 |
| 11 | (ignored) | float | 4 bytes | read and dropped | cshell+0x29bed0 |
| 12 | `prop_count` | unsigned | u32 | | cshell+0x29bed0 |
| 13 | properties | `prop_count` x property (5.6) | same | into a 0x200-entry scratch array; entries beyond 512 are not read (in text mode that would desynchronise the stream) | cshell+0x29c4a0 (scratch 0x200), cshell+0x29bed0 (bound test at 0x29c142) |

Rules applied after reading (cshell+0x29bed0):

- a key that is linked to its own `id` is made unlinked;
- the type is looked up again by index (cshell+0x29ab10); the shell asks the effect module for a property object of
  that type (export ordinal 2, clientfx+0x7980); the object receives `(duration = end - start, repeat)` through its
  slot 1 (clientfx+0x10a0: object +8 = duration, +4 = `repeat * duration`), then the property array and count
  through its slot 2; if slot 2 fails the object is released with export ordinal 4 (clientfx+0x7a40) and the key
  keeps no property object (key +0x04 = 0). The group reader does not check the key result, so a bad key stays in
  the group without properties.

### 5.6 Property

Property record: 0x104 bytes = `char name[128]` at +0x00, `u32 type` at +0x80, value area (128 bytes) at +0x84
(cshell+0x29bc00; the effect module builds the same 0x41-dword record, clientfx+0x28d0).

| encoding | layout | evidence |
|---|---|---|
| text | line 1: word = name; line 2: int = type; line 3: value per type (table) | cshell+0x29bc00 (text branch) |
| binary | `u8 name_len`, `name_len` bytes of name, `u32 type`, value bytes per type (table) | cshell+0x29bc00 (binary branch at 0x29bdb7, jump table at VA 0x3739bea8) |

| type | our name | text value line | binary value bytes | stored at +0x84 | evidence |
|---|---|---|---|---|---|
| 0 | `string` | word | 128 | char[128] | cshell+0x29bc00 |
| 1 | `int` | int | 4 | i32 | same |
| 2 | `float` | float | 4 | f32 | same |
| 3 | `choice` | word: `<index>,<option>,<option>...` | 128 | char[128]; the effect module takes the integer before the first `,` (`strtok` + `atoi`) | cshell+0x29bc00, clientfx+0x16f0 |
| 4 | `vec3` | 3 floats | 12 | f32[3] | cshell+0x29bc00 |
| 5 | `vec4` | 4 floats | 16 | f32[4] | same |
| 6 | `colour_key` | 5 floats `t c1 c2 c3 c4`, colour components in 0..1 | 8 | +0x84 f32 `t`; +0x88 u32 = `(int(c4*255) << 24) or (int(c3*255) << 16) or (int(c2*255) << 8) or int(c1*255)`, conversion by the C runtime float-to-int helper | cshell+0x29bc00 (0x29bd32..0x29bda8, 255.0 at VA 0x374d08a8) |
| 7 | `path` | word: `<ext>` then a vertical bar then `<archive path>` | 128 | char[128]; the effect module splits at the vertical bar, the second token is the path; a second token of three full stops means "none" | cshell+0x29bc00, clientfx+0x11d80 |
| other | | no value line is read (text) / no value bytes (binary) | | | cshell+0x29bc00 (default case) |

Property names are matched by each effect type's parser case-insensitively (`_stricmp`); names it does not know are
skipped (clientfx+0x16f0 for the properties every type shares, then the type's own parser, for example
clientfx+0x17290). Section 6 gives the declaration entry point per type.

### 5.7 Instances

Group playback, key timing and the per-type runtime are behaviour of the effect module and the shell's effect
manager; they are outside this format spec. Facts from the loader that a player needs: times are seconds relative to
the group start; a key is alive from `start` to `end`; `repeat` scales the property object's total life
(clientfx+0x10a0); linked keys attach to the key with `id == link_id` in the same group, which the ordering pass
places earlier in the array.

### 5.8 ROM validation (fxf_dump.py, fxf_check2.py; one file, 218,461 lines)

| check | result |
|---|---|
| encoding | text (first 7 bytes hash `914dc79e48187eee`) |
| grammar of 5.4 to 5.6 consumes the file exactly | yes, 0 non-blank lines left |
| longest line | 137 bytes (limit 0x800) |
| groups / keys | 240 / 1,641 (keys per group: 176 groups below 10, 39 in 10..19, 18 in 20..29, 5 in 30..39, 1 in 40..49, 1 at 50 or more) |
| effect type name resolves in the registry (section 6) | 1,641 / 1,641; with a trailing `;` 934, without 707 |
| types used (registry index: keys) | 0: 855, 1: 202, 3: 198, 11: 134, 4: 93, 6: 52, 5: 48, 16: 28, 2: 23, 17: 7, 14: 1 |
| `linked` keys | 377; all targets exist in the same group; 56 targets come later in the file (the ordering pass moves them); 0 self-links |
| `start <= end`, both within [0, group length] | 1,641 / 1,641 |
| `repeat` | 1 for all keys |
| `attach_node` non-empty | 0 |
| properties | 66,016 total, at most 72 per key; by type 0: 2,757, 1: 3,994, 2: 13,984, 3: 21,825, 4: 7,174, 5: 4,400, 6: 5,731, 7: 6,151 |
| value token counts per type | int, float: 1; vec3: 3; vec4: 4; colour_key: 5; choice: the index before the first `,` parses as an integer in 21,825 / 21,825 |
| values cut by the space rule (5.3) | group names 1; `string` values 3; `path` values 4; `choice` values 8,882 (harmless: the index precedes any space) |
| `path` values containing the vertical bar | 6,151 / 6,151; placeholder second token 3,834 |
| duplicate key ids inside a group | 0 groups |

## 6. Effect type registry of the effect module

### 6.1 Module interface

The 2009 effect module is a PE DLL (image base 0x410000) with 9 exports. The shell resolves all of them by name and
gives up on the module if any is missing (cshell+0x29af80). Export names are given as sha256[:16]/length.

| ordinal | rva | our name | name hash / len | behaviour | evidence |
|---|---|---|---|---|---|
| 1 | 0x1bef0 | `set_database` | `5b9b965b9f33cedd` / 17 | receives the shell's game-database interface (called first, optional) | clientfx+0x1bef0, cshell+0x29af80 |
| 2 | 0x7980 | `create_props` | `6fd3befdd6995459` / 16 | `(type index) -> property object`; a switch over the 19 types | clientfx+0x7980 |
| 3 | 0xba20 | `delete_instance` | `651dccb134a0ed23` / 8 | returns an effect instance to its per-type pool, selected by the instance's type field (+0x1C) | clientfx+0xba20 |
| 4 | 0x7a40 | `free_props` | `9955a6756dcbcafb` / 14 | destroys a property object through its virtual destructor | clientfx+0x7a40 |
| 5 | 0x4730 | `type_count` | `936f64c5c7e48496` / 8 | returns 19 (0x13) | clientfx+0x4730 |
| 6 | 0xc940 | `type_record` | `5b17080e4814feb8` / 8 | `(out record, index)`: fills a 0x90-byte type record (6.2) | clientfx+0xc940 |
| 7 | 0x4750 | `set_focus` | `6e2dcd3b6c030800` / 13 | stores a byte flag (application focus) in a module global | clientfx+0x4750 |
| 8 | 0x4760 | `set_create_hook` | `a7b1bec77156bc2a` / 19 | stores two shell values (a create callback and its context) in module globals | clientfx+0x4760 |
| 9 | 0x4740 | `set_player` | `dc134ac5a5c5015d` / 11 | stores the local player handle in a module global | clientfx+0x4740 |

The shell copies `type_count()` records (one `type_record(i)` call each) into its own 0x90-stride array
(cshell+0x29af80) and resolves `.fxf` type names against that copy (section 5.5).

### 6.2 Type record (0x90 bytes)

| off | size | our name | meaning | evidence |
|---|---|---|---|---|
| 0x00 | char[128] | `name` | type name used in `.fxf` keys (case-insensitive match, section 5.5) | clientfx+0xc940 |
| 0x80 | u32 | `type_flags` | per-type constant (values below); no consumer traced (UNPROVEN meaning) | clientfx+0xc940 |
| 0x84 | fn ptr | `create` | allocates an instance of the type from its pool | clientfx+0xc940 |
| 0x88 | fn ptr | `delete` | always the ordinal-3 export | clientfx+0xc940 |
| 0x8C | fn ptr | `declare_props` | fills a property list with the type's property declarations (name, value type, default; same 0x104-byte record as 5.6), used for authoring and defaults | clientfx+0xc940, clientfx+0x28d0 (append one declaration) |

An index of 19 or more yields a sentinel record: a 14-byte invalid-type name, flags 0, all three pointers null.

### 6.3 The 19 types of the 2009 module

`decl` = number of declaration-append calls (clientfx+0x28d0) in the type's own `declare_props` function, counted
in the decompiler output (calls inside loops count once); `shared` = the shared declaration function it calls first
(clientfx+0x7a50 declares the 12 properties every type has). `fxf keys` = keys of that type in the ROM file.

| index | our name | name hash / len | `type_flags` | `create` | `declare_props` | decl | shared | fxf keys |
|---|---|---|---|---|---|---|---|---|
| 0 | particles | `8c7cd19b782c034e` / 14 | 0x17 | clientfx+0xc830 | clientfx+0x1dc80 | 30 | 0x7a50 | 855 |
| 1 | sprite | `610cc33146e5be61` / 6 | 0x17 | clientfx+0xc840 | clientfx+0x23740 | 9 | 0x7a50 | 202 |
| 2 | empty (anchor only) | `fa1d2db62d4d952e` / 4 | 0x14 | clientfx+0xc8b0 | clientfx+0x1c910 | 4 | 0x7a50 | 23 |
| 3 | model | `05d155e975caebb4` / 8 | 0x17 | clientfx+0xc850 | clientfx+0x16880 | 28 | 0x7a50 | 198 |
| 4 | dynamic light | `3a7c035a9d047515` / 9 | 0x17 | clientfx+0xc860 | clientfx+0xdae0 | 2 | 0x7a50 | 93 |
| 5 | sound | `d9dfcd3d1511b47f` / 9 | 0x1c | clientfx+0xc870 | clientfx+0x21090 | 10 | 0x7a50 | 48 |
| 6 | camera shake | `2e632023d520b1c5` / 9 | 0x22 | clientfx+0xc880 | clientfx+0x3dd0 | 2 | 0x7a50 | 52 |
| 7 | camera wobble | `31c339c4f400a354` / 7 | 0x22 | clientfx+0xc890 | clientfx+0x43c0 | 5 | 0x7a50 | 0 |
| 8 | debris chunk | `722697335a33c224` / 11 | 0x13 | clientfx+0x4780 | clientfx+0x2f00 | 8 | 0x7a50 | 0 |
| 9 | model debris chunk | `aec8312c467542ee` / 14 | 0x13 | clientfx+0xc8a0 | clientfx+0x15910 | 8 | 0x7a50 | 0 |
| 10 | falling particles | `e0bd461d6454d556` / 12 | 0x37 | clientfx+0x4790 | clientfx+0xeb20 | 17 | 0x7a50 | 0 |
| 11 | trail | `37d3a5d2172ecf93` / 9 | 0x17 | clientfx+0xc8c0 | clientfx+0x222d0 | 12 | 0x7a50 | 134 |
| 12 | random sound | `e62daa0961f3a3b7` / 15 | 0x14 | clientfx+0xc8d0 | clientfx+0x206c0 | 9 | 0x7a50 | 0 |
| 13 | sprite system | `d7f4686854dc7a3d` / 12 | 0x17 | clientfx+0xc8e0 | clientfx+0x24950 | 21 | 0x7a50 | 0 |
| 14 | spawn group | `c05f736cea8ca9b1` / 8 | 0 | clientfx+0xc8f0 | clientfx+0xd1e0 | 2 | none found | 1 |
| 15 | flare sprite | `470cd96d88c2fe56` / 13 | 0x11 | clientfx+0xc900 | clientfx+0x10bb0 | 10 | sprite's (clientfx+0x23740) | 0 |
| 16 | lightning | `77425462fb006b12` / 11 | 0x17 | clientfx+0xc910 | clientfx+0x12170 | 22 | 0x7a50 | 28 |
| 17 | model system | `ff5af605d800f0ce` / 11 | 0x17 | clientfx+0xc920 | clientfx+0x18710 | 15 | 0x7a50 | 7 |
| 18 | damage system | `81fcc021a4d9930c` / 14 | 0 | clientfx+0xc930 | clientfx+0xd510 | 5 | 0x7a50 | 0 |

Our names are descriptive guesses from the module (UNPROVEN until each type is traced); they are not the module's names, which are identified by hash only.
Each type's property object has its own parser in its vtable slot 2 (example: type 3, vtable at VA 0x43bccc,
parser clientfx+0x17290, which first calls the shared parser clientfx+0x16f0 and then matches its own names). The
declared property names and defaults are ROM data and are read from the module at run time when needed; a
reimplementation needs only the key/property encoding of section 5 and the per-type behaviour.

## 7. `.fcf` effect editor tree

No module of the 2009 client contains the `.fcf` extension text or reads this file (section 1). It is the effect
editor's saved tree view: folder nodes that group the effect names of the `.fxf`, plus the editor's open windows.
Tier DROP; documented so tools can show or ignore it. Line model as in 5.3 (label token, then value; labels are
positional).

Grammar (fcf_dump.py):

```
tree      := siblings
siblings  := COUNT(n)  node{n}
node      := NAME(word)  IMAGE(int)  SELECTED_IMAGE(int)  HAS_CHILDREN(0|1)  STATE(int)  [siblings if HAS_CHILDREN]
trailer   := COUNT(m)  window{m}
window    := EFFECT_NAME(word)  LEFT(int)  TOP(int)  RIGHT(int)  BOTTOM(int)
file      := tree trailer
```

Nesting is by order only (the tab indentation in the file is cosmetic).

| check (1 file) | result |
|---|---|
| grammar consumes the file exactly | yes, 0 lines left |
| sibling blocks / nodes / nesting depth | 31 / 477 / 3 levels (root block plus 2) |
| nodes with children | 30, all with image index 0 (folders); the other 447 nodes use image index 2 (effects) |
| trailer windows | 2 |

## 8. `.ltc` compressed source

### 8.1 What the client does with it: nothing

No reader exists. Proof by absence over all three modules (misc_dump.py, extension search in the report): the
extension text `.ltc` occurs 0 times in `Engine.exe`, `CShell.dll` and `ClientFx.fxd` (ASCII, case-insensitive);
the only loaders that choose a parser by extension handle `DTX`/`SPR` (engine+0x81150, engine+0xa1b20,
engine+0x1818e0) and `.fxf` (cshell+0x29c7c0); models are opened by the model loader by their own file kind
(FMT_LTB). Therefore `.ltc` is neither a texture-coordinate nor a light-controller file for the 2009 runtime; the
question "which of the two" has no reader to decide it, and the payload says it is neither (8.2).

### 8.2 What the files are (structure only)

| fact | evidence (counts) |
|---|---|
| every file starts with a u32 0 | 111 / 111 |
| the rest is a high-entropy stream, 7.89 bits per byte over all files | misc probe |
| not zlib / raw deflate at offsets 0..7, not raw LZMA1 with the common property sets at offsets 0..4 | probe (all attempts fail) |
| bytes 4..7 fall into three shared patterns (63, 26, 21 files) plus 1 other, so the compressor is deterministic on a common plain-text opening | probe |
| sizes 623 .. 19,824 bytes | probe |
| the 85 in the effect-art archive share their base name with a compiled model (`.ltb`) in 85 cases | stem match |
| the 26 in the prefab archive share their base name with a `.bmp` thumbnail in 13 cases, with nothing in 13 | stem match |
| the uncompressed equivalents exist in the ROM as text: model sources (`.lta`, 9.2) and one extension-less prefab world source (9.5) | 9.2, 9.5 |

Conclusion: compressed text authoring sources (model sources beside their compiled models, prefab world sources
beside their thumbnails). The compression scheme is UNPROVEN and not needed: tier DROP. A reimplementation must not
treat a `.ltc` as loadable content and may skip it when indexing archives.

## 9. Small families

### 9.1 `.wst` weight sets (4 entries, character body archive)

Text, parenthesised lists (S-expression style). No module contains the extension text or a reader (section 1);
the compiled model files carry what the runtime needs (FMT_LTB). Tier DROP.

```
file    := "(" set* ")"
set     := "(" WORD "(" NUMBER* ")" ")"
```

| check (misc_dump.py) | result |
|---|---|
| files parse completely with the grammar above | 4 / 4, 0 tokens left |
| sets | 16 (every set is exactly a name plus one number list) |
| numbers per set | 24 (7 sets), 25 (4), 45 (3), 53 (2) |
| every number is a decimal in [0, 1] | 16 / 16 sets |

Interpretation (UNPROVEN): one blend weight per model node for an animation layer (upper body, lower body, ...).
Line endings differ between files (CR LF in 3, LF in 1); a tool must accept both.

### 9.2 `.lta` text model sources (4 entries)

Text, parenthesised lists; the editor-side source form of a model whose compiled form is `.ltb` (FMT_LTB). No reader
in the client: neither the extension text nor the opening keyword of these files occurs in any module (0 hits,
misc_dump.py). Parentheses balance in 4 / 4. Sizes 12,438 .. 4,255,311 bytes. Tier DROP.

### 9.3 `.pk` waveform peak caches (3 entries, sound folders of the effect-art archive)

Not read by the client (no extension text, no reader). Each sits next to a `.wav` of the same base name (3 / 3).
Structure from the scan (UNPROVEN beyond these counts; it matches the "peak file" caches that audio editors write
next to a sound, PUBLIC as a concept, no public spec used):

| off | size | observation |
|---|---|---|
| 0x00 | u32 | same value in all 3 files (a format tag) |
| 0x04 | u32 | 0x100 in all 3 |
| 0x08 | u32 | entry count; equals `ceil(sample_frames / 256)` for the two 44.1 kHz sounds; 604 for the 22.05 kHz sound of 77,184 frames (about one per 128 frames); the u32 at 0x14 is always this count minus 1 |
| 0x2C | u64 | a Windows FILETIME (PUBLIC: 100 ns ticks since 1601) in 2006..2007, the source sound's time stamp |
| ~0x40 | 8 bytes per entry | `(file size - 0x40) / entry count` is 8.01 in all 3 files |

Tier DROP; derivable from the `.wav` if a tool ever wants it.

### 9.4 `.pcx`, `.bmp`, `.psd` (public image formats)

| family | entries | header facts (counts) | client use | evidence |
|---|---|---|---|---|
| `.pcx` | 3 (logo archive) | ZSoft PCX version 5, RLE, 8 bits per plane, 3 planes (24-bit) in 3 / 3. PUBLIC: ZSoft PCX technical reference | the shell loads the mouse cursor image from the logo archive through an engine service (cshell+0x1bf010, cshell+0x1ef3a0; the path from that service to the decoder was not traced); the engine's PCX decoder (callers engine+0x23330, engine+0x511c0, engine+0xb88f0, engine+0xbed60) accepts 8 bits per plane with 1 (palette) or 3 (RGB) planes and computes width as `xmax - xmin + 1` (engine+0x50570); the engine also reads an optional loose console background image from the working directory (engine+0x23330, engine+0xbed60), not from the ROM | engine+0x50570 |
| `.bmp` | 15 (14 prefab archive, 1 logo archive) | `BM` header with a 40-byte info header, 24 bits, no compression, file size field equals entry size in 15 / 15. PUBLIC: Windows BITMAPINFOHEADER | no reader found; the engine only writes BMP files (a debug font-atlas dump, writer engine+0x1d52a0, called from engine+0x1d5830). The prefab ones are thumbnails of the `.ltc` prefab sources (section 8) | engine+0x1d52a0 |
| `.psd` | 1 (props archive) | Photoshop version 1, 3 channels, 8 bits, RGB mode. PUBLIC: Adobe Photoshop file format specification | none (no extension text in any module) | |

Only the PCX decoder matters for a reimplementation (the cursor). Use any public PCX decoder that handles version 5
RLE with 1 or 3 planes of 8 bits.

### 9.5 Entries without an extension (52)

Classified by content (misc_dump.py):

| class | count | where | treatment |
|---|---|---|---|
| zero-length | 50 | 4 archives (texture 36, effect-art 12, sprite 1, one other 1) | placeholders; a loader that is handed one gets an empty stream |
| starts with u32 0, high-entropy payload: same shape as `.ltc` (section 8) | 1 | effect-art archive | DROP |
| text, parenthesised world source (the uncompressed form of a prefab source) | 1 | prefab archive | DROP |

No extension-less entry is referenced by a loader that dispatches on extension, so none of them is runtime content.

### 9.6 Other families present but outside this spec

| family | entries | classification |
|---|---|---|
| `.fx` | 8 | text effect-shader sources (D3D effect framework source, PUBLIC: D3DX effect format) |
| `.psh` | 6 | text pixel-shader assembly sources |
| `.ps` | 3 | small binary container: 4-byte tag, u32 version 1, u32 size, then a D3D9 shader token stream (version token `ps_1_1`, PUBLIC: D3D9 shader bytecode docs) |
| `.ttf` / `.ttc` | 16 / 3 | TrueType fonts / collections (PUBLIC) |
| `.txt`, `.gms` | 56, 143 | game tables and mode setups (separate specs) |
| `.db` | 2 | Windows thumbnail caches (OLE compound files), stray |
| `.max` | 1 | 3D authoring scene (OLE compound file), stray |
| `.scc` | 1 | source-control bookkeeping file, stray |

## 10. Validation summary

All scans are read-only over the 95 archives; dumpers in `staging/fmt_spr_ltc_fx/`.

| format | entries | parser claim checked | result | dumper |
|---|---|---|---|---|
| `.spr` | 233 | header + frame list of 2.2 ends exactly at entry end | 231 exact, 1 empty, 1 rejected by the engine's 1000-byte name rule | `spr_dump.py` |
| `.tfg` | 2 | 3.2 layout, version 1, stages, string, params | 2 / 2 exact | `tfs_dump.py` |
| `.tfs` | 6 | 4.2 layout, name block, constants, code multiple of 4, opcodes < 10, funcs < 6, registers below 0x48 | 6 / 6 exact | `tfs_dump.py` |
| `.fxf` | 1 | text grammar 5.3 to 5.6, all type names resolve, link targets exist | exact, 240 groups, 1,641 keys, 66,016 properties | `fxf_dump.py`, `fxf_check2.py` |
| `.fcf` | 1 | grammar of section 7 | exact | `fcf_dump.py` |
| `.ltc` | 111 | u32 0 header, no client reader | 111 / 111; 0 extension hits in 3 modules | `misc_dump.py`, report probes |
| `.wst` | 4 | grammar of 9.1 | 4 / 4 | `misc_dump.py` |
| `.lta` | 4 | balanced lists, no client reader | 4 / 4 | `misc_dump.py` |
| `.pk` | 3 | header counts vs. the sibling `.wav` | 3 / 3 consistent | report probe |
| `.pcx` / `.bmp` / `.psd` | 3 / 15 / 1 | public header magic and fields | all match | report probe |
| no extension | 52 | classification by content | 50 empty, 1 compressed source, 1 text source | `misc_dump.py` |

## 11. Open questions

1. `.spr` animation fields +0x30 / +0x34 / +0x38 (`flag_a`, `flag_b`, `key_word`): no consumer found in the engine.
   A renderer trace (sprite draw path) should settle them; all real ROM files store 0.
2. `.tfs` `out_mode` bits (0x10/0x20/0x24/0x44) and the `coord_mode` values 1..3 of object +0x138: consumed by the
   renderer, meaning not traced. ROM uses `out_mode` 0 and `coord_mode` 4, 5, 6 only.
3. Texture-script time units: register 0x26/0x27 come from a global at VA 0x7e59d8 (+0x40) that looks like the
   frame delta; units (seconds) UNPROVEN.
4. `.fxf` binary encoding is code-derived only (no ROM file uses it); the 3 pad bytes after `linked` and the
   absence of a terminator in binary property names need a writer-side test once a tool emits the binary form.
5. Effect type record `type_flags` (+0x80) values 0x11..0x37: no consumer traced in the shell.
6. Effect type meanings in 6.3 are descriptive guesses; per-type behaviour belongs to a later effect-runtime spec.
7. `.ltc` compression scheme unidentified (not needed: no runtime reader).
8. The PCX decoder at engine+0x50570 is assumed to be the one behind the shell's cursor service; the service
   slot (shell interface +0x12C) was not followed into the engine.
9. Note: several files of the allowed `ClientFx.fxd/*.c` carry
   applied class/function names and "matched" comments from byte-matching (about 209 files contain such
   annotations); this spec does not use any of them, but the ClientFx decomp should get the same default-name
   cleanup the other two modules received.

