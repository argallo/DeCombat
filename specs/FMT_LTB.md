# FMT_LTB: model container (.ltb) as loaded by the 2009 Engine.exe

- Status: SPEC. Derived from the decompiler output of the 2009 Engine.exe, plus disassembly of the 2009 functions that
  decompiler did not emit as C, plus a full structural walk of every `.ltb` entry in the user ROM.
- Module: `engine` = 2009 EU-test `Engine.exe` (sha256 `bb9a1e41b430a09ab8722e1de31f89dc53a00e5a96abbe236f65c4ceddcf574f`),
  image base 0x00460000. Every RVA below is `engine+0xRVA` (VA minus 0x460000).
- Sources used: `Engine.exe/*.c` (2009) and `decompiler/Engine.exe.funcs.orig.tsv`.
  `extracted/.../Engine.exe` was opened read-only for vtable slots, two float constants, and disassembly of the
  render-object readers at 0x15e4d0 / 0x15f890 / 0x161210, the LOD loop at 0x6e720, the clip reader at 0x6eb40
  and the track reader at 0x6dd60. Our own scratch tools are `staging/ltb/{ltbdump,ltbpeek,dis,vt}.py`,
  built on `evidence/rezls.py`. No SDK or other proprietary source text was used. All names here are ours.
- ROM data policy: counts and histograms only. No names, strings, vertex data or asset values are reproduced.
- Validation: the walker described in section 12 reads all 1,542 loadable model entries in the ROM (95 `.rez`
  archives, 1,610 `.ltb` entries) through to the exact last byte. Every render-object length field matches.

Byte order is little-endian. In this spec `str` always means a `u16 length` followed by `length` bytes, with no
terminator. The engine reads it through the stream's string slot into a 0x1000-byte buffer (engine+0x6db70).
The length prefix and the missing terminator are OBSERVED in every file. `vec3` = 3 x f32, `quat` = 4 x f32,
`mat4` = 16 x f32.

## 1. Entry points

| what | engine RVA | notes |
|---|---|---|
| model file load (top level, drives everything below) | 0x6f380 | checks the `.ltb` extension, reads the header and the sections in order, maps an error index to a message table |
| size-hint block reader | 0x6fd30 | 15 dwords, section 3 |
| memory estimate from size hints | 0x6fc70 | used only to size the allocation |
| string reader (u16-prefixed) | 0x6db70 | 0x1000-byte limit |
| piece + LOD reader | 0x6e720 | section 5 |
| render-object factory (dispatches through the renderer table) | 0x26070 | falls back to an empty base object (vtable VA 0x6764fc) |
| render object reader, rigid (kind 4) | 0x161210 | vtable VA 0x6897bc slot 6; constructor 0x161800 stamps kind 4 at object +4 |
| render object reader, skinned (kind 5) | 0x15f890 | vtable VA 0x68977c slot 6; constructor 0x160240 stamps kind 5 |
| skinned body, per-vertex-blend tail | 0x15f160 | section 6.3 |
| skinned body, palette tail | 0x15f2d0 | section 6.3 |
| render object reader, morph (kind 6) | 0x15e4d0 | vtable VA 0x689730 slot 6; constructor 0x15ea80 stamps kind 6 |
| stream flags -> vertex code | 0x15e2f0 | section 6.1 |
| vertex code -> byte stride | 0x15e240 | section 6.1 |
| node tree reader (recursive) | 0x6ed10 | section 7 |
| blend-set list reader | 0x6f290, item 0x6e9c0 | section 8 |
| clip (animation) reader | 0x6ec80 -> 0x6eb40 -> 0x6e690 -> 0x6dd60 | section 10 |
| attach-point reader | 0x6f160 | section 11 |
| per-child clip overrides | 0x6e0d0 | section 11.2 |
| default model fallback on load failure | 0x94890, 0x94a10 | the engine loads a fixed default model path instead |
| render statistics dump (names the three mesh families) | 0xf79d0 | debug output only |

## 2. File header (0x14 bytes)

The loader reads this as one 0x14-byte block (engine+0x6f380, read length 0x14). The render-object readers get a
pointer to the same block and check it again.

| off | size | our name | rule | evidence |
|---|---|---|---|---|
| 0x00 | u8 | `file_kind` | must be 1 (model). Any other value gives load error index 0x1e (the engine logs a wrong-file-type message) | engine+0x6f380; engine+0x161210 / 0x15f890 / 0x15e4d0 (`cmp byte [hdr],1`) |
| 0x01 | u8 | (pad) | 0 in all model files | OBSERVED |
| 0x02 | u16 | `container_rev` | must be 9; otherwise every render-object reader fails | engine+0x161233, +0x15f8ad, +0x15e4ed (`cmp word [hdr+2],9`) |
| 0x04 | 16 bytes | (reserved) | never read; zero in all model files | OBSERVED |

One field follows the block:

| size | our name | rule | evidence |
|---|---|---|---|
| u32 | `model_rev` | must be >= 23 (0x17). Otherwise the engine logs an unsupported-revision message and the load fails. No other branch on this value was found: it is passed to the piece reader, which does not test it | engine+0x6f380 |

Two other `file_kind` values occur in files named `.ltb`:

- 5 (with `container_rev` 3): render-style files. They load through a different path (engine+0x94c70 ->
  engine+0x95730) and are out of scope here.
- 9 (an ASCII header tag from byte 2 onward): a third container type. The model loader rejects it with index 0x1e.

## 3. Size hints (15 x u32)

engine+0x6fd30 reads these straight into the model object (+0x00..+0x38). engine+0x6fc70 turns them into one
allocation size (the model arena), and the loader then checks the stream error flag. The loader does not
compare the hints with the real section counts, so a reimplementation can ignore them or use them as capacity
hints. The meanings below come from correlating the hints with section counts over 1,542 files (OBSERVED):

| idx | our name | observed relation |
|---|---|---|
| 0 | `hint_markers` | equals the total clip markers (section 10.2) in 1,541 / 1,542 |
| 1 | `hint_clips` | equals the clip count, 1,542 / 1,542 |
| 2 | `hint_nodes` | equals the node count, 1,542 / 1,542 |
| 3 | `hint_pieces` | equals the piece count, 1,541 / 1,542 |
| 4 | `hint_children` | equals the child-model slot count (including self), 1,542 / 1,542 |
| 5..7 | - | non-zero in most files; no exact relation found (probably vertex or key payload sizes) |
| 8 | - | close to the piece or LOD totals (1,510-1,534 / 1,542) but not exact |
| 9 | `hint_attach` | equals the attach-point count, 1,542 / 1,542 |
| 10 | `hint_blend_sets` | equals the blend-set count, 1,542 / 1,542 |
| 11..14 | - | 11 and 12 are always non-zero; the estimator rounds 13 and 14 up to a multiple of 4, so they are probably byte totals |

## 4. Top-level section order

All steps are in engine+0x6f380 unless noted otherwise. The error numbers are the engine's internal indices into
its message table (VA 0x67ac90). We do not reproduce the messages.

1. The header block and `model_rev` (section 2).
2. The size hints (section 3). Error if the stream failed.
3. `str command` -> model +0x3c (free-form model command text).
4. `f32 radius` -> model +0x48. It is later raised to the largest radius among the children.
5. `u32 box_count`, then `box_count` x 0x44-byte records. These are read and **discarded**: the engine prints a
   warning that this build does not support boxes. The count is 0 in every loadable ROM file.
6. `u32 piece_count`, which must be < 0x41 (64 at most), else error 4. Then `piece_count` pieces (section 5);
   errors 5 and 6.
7. The node tree, root first (section 7). Error 8.
8. No read: the node lookup is rebuilt. Error 9 (engine+0x66970).
9. The blend sets (section 8). Error 10.
10. No read: each LOD render object runs its post-load hook (vtable slot 7).
11. `u32 child_count`, which must be 1..32, else error 0xc. Then `child_count - 1` x `str child_path` (section 9).
12. `u32 clip_count`, then `clip_count` clips (section 10). Errors 0x16 and 0x18.
13. No read: child clip indices are remapped.
14. The attach points (section 11). Error 0x1b.
15. The per-child clip overrides (section 11.2). Error 0x1a.
16. The stream status must be clean, else error 0x1c.

The file ends here. Every loadable ROM file has zero trailing bytes (OBSERVED, 1,542 / 1,542).

## 5. Piece and LOD records (engine+0x6e720)

```
piece:
  str   name                         -> piece +0x20
  u32   lod_count                    -> piece +0x2c
  f32   lod_dist[lod_count]          -> array at piece +0x28   (first is 0.0, strictly increasing: OBSERVED)
  u32   lod_lo                       (read into a local; 0 in all files)
  u32   lod_hi                       (read into a local; 0 for 1-LOD pieces, 2 for 3-LOD pieces: OBSERVED)
  lod[lod_count]

lod:
  u32   slot_count                   -> piece +0x00   (1 or 2 in the ROM)
  u32   slot[4]                      -> piece +0x04..+0x10  (material slot indices; each unused tail entry holds
                                                            its own position index, OBSERVED)
  u32   style_index                  -> piece +0x14   (render-style slot; 0..4 seen)
  u8    priority                     -> piece +0x18   (draw-order bias; 0 in most files)
  u32   ro_kind                      -> factory engine+0x26070 (4, 5 or 6; section 6)
  ...   render object body           (read by slot 6 of the new object, which gets the stream and the header)
  u8    used_node_count              -> render object +0x10
  u8    used_node[used_node_count]   -> buffer at render object +0x0c
```

The fields from `slot_count` to `priority` are written into the piece object once per LOD
(engine+0x6e7e0..0x6e827), so after loading, the piece holds the values from its last LOD. If the factory call
fails, the load stops with an "outdated format" error.

Histogram (OBSERVED):

- LOD counts per piece: 1 (5,107 pieces) and 3 (109 pieces).
- Render-object kinds: 4 rigid (2,401), 5 skinned (2,997), 6 morph (36).

## 6. Render-object bodies

All three body kinds start with `u32 body_len`: the number of bytes that follow, up to and including the kind's
last field. It does not include the LOD's `used_node` list. The readers read this value into a local and never
use it. It is exact in all 5,434 bodies (OBSERVED), so a reader can use it to skip unknown kinds.

### 6.1 Vertex streams

Each body carries `u32 stream_flags[4]`. A stream with flags 0 is absent. For a present stream the reader takes
`vertex_count * stride(flags)` bytes as one block. The stride is computed in two steps.

Step 1, engine+0x15e2f0 (flags -> vertex code):

| flag bit | effect on code |
|---|---|
| 0x01 and 0x02 both set | position + normal; the position part depends on the blend kind (see below) |
| 0x10 / 0x20 / 0x40 / 0x80 | 1 / 2 / 3 / 4 texture-coordinate pairs (the first match wins, tested in that order) |
| 0x100 | 24 extra bytes per vertex (two vec3: tangent frame); the code is then treated as custom |

Blend kind -> position part. The values are standard Direct3D FVF codes (public):

- 0 -> 0x12 (xyz + normal).
- 1 / 2 / 3 -> 0x16 / 0x18 / 0x1a (xyz + 1 / 2 / 3 blend weights).
- 4 / 5 / 6 -> 0x1018 / 0x101a / 0x101c (the same, plus the last dword holding 4 x u8 palette indices).

Step 2, engine+0x15e240 (code -> bytes):

- xyz is 12 bytes, plus 4 per blend weight (codes 0x6 / 0x8 / 0xa / 0xc give 16 / 20 / 24 / 28).
- normal +12, diffuse +4, specular +4.
- texture pairs +8 / +16 / +24 / +32.
- +24 if flag 0x100 is set.

Rigid and morph bodies use blend kind 0. In the ROM, stream 0 always has flags 0x13 (position, normal, one UV
pair) and streams 1..3 are always absent (OBSERVED). So the rigid and morph stride is 32, and the skinned stride
is 32 + 4 * (`max_influences` - 1).

Index data, in every kind: `tri_count * 3` x u16, read as one block of `tri_count * 6` bytes.

### 6.2 Rigid (kind 4, engine+0x161210)

| order | type | our name | dest |
|---|---|---|---|
| 1 | u32 | `body_len` | local |
| 2 | u32 | `vertex_count` | +0xa4 |
| 3 | u32 | `tri_count` | +0xa8 |
| 4 | u32 | (influences, unused) | local; 1 in all files |
| 5 | u32 | (unused) | local; 1 (2,400 bodies) or 2 (1 body) |
| 6 | u32[4] | `stream_flags` | +0xac |
| 7 | u32 | `node` | +0xa0 (the single node that moves the whole mesh) |
| 8 | streams | vertex blocks | +0xc0.. |
| 9 | u16[3*tri_count] | indices | +0xd0 |

### 6.3 Skinned (kind 5, engine+0x15f890)

| order | type | our name | dest |
|---|---|---|---|
| 1 | u32 | `body_len` | local |
| 2 | u32 | `vertex_count` | +0xa8 |
| 3 | u32 | `tri_count` | +0xac |
| 4 | u32 | `max_influences` (1..4) | +0xa4 |
| 5 | u32 | `palette_influences` (used only on the palette tail, where it must be 2..4) | +0xa0; 1..4 in the ROM, equal to `max_influences` in 2,508 / 2,997 |
| 6 | u8 | `has_palette_map` | +0xdb; 0 in all files |
| 7 | u32[4] | `stream_flags` | +0xb0 |
| 8 | u8 | `palette_path` | local; selects which tail reader runs |

Tail when `palette_path` = 0 (engine+0x15f160):

1. The blend kind is `max_influences - 1`. Values outside 1..4 fail.
2. The vertex streams, then the indices.
3. `u32 span_count` (+0xc4), then `span_count` x 12-byte spans (+0xc8):

| span off | type | meaning (OBSERVED over all 2,997 skinned bodies) |
|---|---|---|
| 0 | u16 | first vertex of the span (spans are contiguous; the last one ends at `vertex_count`) |
| 2 | u16 | vertex count of the span |
| 4 | u8[4] | indices of the nodes that drive the span; 0xff = unused |
| 8 | u32 | running end of the span's indices (strictly increasing; the last equals `3 * tri_count`) |

Tail when `palette_path` = 1 (engine+0x15f2d0):

1. `u32` -> +0xcc, then `u32` -> +0xd0.
2. The blend kind is 4 / 5 / 6 for `palette_influences` = 2 / 3 / 4. Other values fail.
3. If `has_palette_map` is set: `u32 n`, then `n` x u32 (+0xdc, palette slot -> node).
4. The vertex streams, then the indices.

No ROM file uses the palette tail (OBSERVED: 0 / 2,997).

### 6.4 Morph / vertex-animated (kind 6, engine+0x15e4d0)

| order | type | our name | dest |
|---|---|---|---|
| 1 | u32 | `body_len` | local |
| 2 | u32 | `vertex_count` | +0xb0 (the count used for the vertex blocks) |
| 3 | u32 | `base_vertex_count` | +0xc4 (equals `vertex_count` in 23 of 36 bodies, smaller in the other 13) |
| 4 | u32 | `tri_count` | +0xc8 |
| 5 | u32 | (influences) | +0xac; 0 in the ROM |
| 6 | u32 | (unknown) | +0xa8; 0 in the ROM |
| 7 | u32[4] | `stream_flags` | +0xb4 |
| 8 | u32 | (unknown, 1..5 in the ROM) | +0xa4 |
| 9 | u32 | `node` | +0xa0 |
| 10 | streams | vertex blocks | +0xd8.. |
| 11 | u16[3*tri_count] | indices | +0xe8 |
| 12 | u32 + u32[n] | `n`, then n x u32 (by our reading, a vertex -> base-vertex map; `n` never exceeds `vertex_count`) | +0xcc / +0xd0 |

The animated positions themselves are stored in the clip tracks (section 10.3, with `morph` = 1).

## 7. Node tree (engine+0x6ed10, recursive, depth-first, root first)

```
node:
  str   name
  u16   index           -> node +0x0c
  u8    flags           -> node +0x0e
  mat4  model_xform     4 rows of 4 f32, row-major; the last row is (0,0,0,1) in all 48,730 nodes (OBSERVED)
  u32   child_count
  node  children[child_count]
```

What the reader does with the matrix:

- It copies the matrix to node +0x24.
- It builds an inverse at node +0x64 by transposing the rotation and negating the rotated translation.
- It stores `parent_inverse * model_xform` as the local transform at node +0xa4 (via engine+0x6c930).

The root is called with an identity parent; engine+0x6f380 builds that identity matrix on the stack. So the file
stores model-space bind transforms, and the local transforms are derived from them. The node count equals
`hint_nodes` in every file.

## 8. Blend sets (engine+0x6f290, item engine+0x6e9c0)

```
u32 set_count
set[set_count]:
  str  name
  u32  n                must equal the node count, else the load fails (engine+0x6f290 compares item +0x0c
                        with model +0x14)
  f32  weight[n]        one per node, in node order
```

The ROM has 350 sets, all with `n` = node count (OBSERVED). Each set is a per-node mask used when blending clips.

## 9. Child models (engine+0x6f380, loop after the blend sets)

`u32 child_count` (1..32). Slot 0 is the model itself and has no record. Each further slot is a `str path` naming
another model file whose clips this model borrows. When the loader callback is enabled, it loads that model. A
child that has more than one child slot of its own is refused (error 0xf, engine status 0x4d), so nesting goes
only one level deep. In the ROM, 935 files have 1 slot and 607 have 2.

## 10. Clips (animations)

`u32 clip_count`, then for each clip: a header (10.1), its markers (10.2) and one track per node (10.3).

### 10.1 Clip header (engine+0x6ec80, engine+0x6eb40)

| order | type | our name | dest |
|---|---|---|---|
| 1 | vec3 | `extent` | clip info +0x04 (section 11.2 can replace it) |
| 2 | str | `name` | clip +0x1c |
| 3 | u32 | `codec` | local; selects the track encoding (10.3) |
| 4 | u32 | `blend_ms` | clip +0x14 (transition time, by our reading) |
| 5 | u32 | `marker_count` | |
| 6 | marker[marker_count] | | array of 8-byte records at clip +0x08 |

### 10.2 Marker

A marker is `u32 time_ms` followed by `str command` (engine+0x6eb40 loop). The marker times are the clip's key
times. Each track channel below has either one key or `marker_count` keys.

### 10.3 Tracks

engine+0x6e690 walks the node tree in the same depth-first order as section 7, and engine+0x6dd60 reads one
track per node.

Codec 0 (uncompressed):

```
u8 morph                  -> track +0x14
if morph != 0:
  repeat marker_count times:  u32 n, then n x vec3      (vertex positions for a kind-6 mesh)
  (the position and rotation channels are set to "none")
else:
  vec3 pos[marker_count]  then quat rot[marker_count]
```

Codecs 1, 2 and 3 (each channel has its own key count):

```
u32 pos_count,  then pos_count keys in the position encoding
u32 rot_count,  then rot_count keys in the rotation encoding
```

A count of 0 means "no channel" (0 bytes), 1 means a constant channel (one key), and any other value means a
keyed channel. Encodings per codec:

| codec | position key | rotation key |
|---|---|---|
| 0 | vec3 f32, 12 bytes | quat f32, 16 bytes |
| 1 | vec3 f32, 12 bytes | quat f32, 16 bytes |
| 2 | 3 x i16, 6 bytes, value = i16 * 1/16 | 4 x i16, 8 bytes, value = i16 * (1/32767) |
| 3 | vec3 f32, 12 bytes | 4 x i16, 8 bytes, value = i16 * (1/32767) |

Evidence:

- Channel objects: engine+0x641a0 / 0x641e0 (none), 0x64220 / 0x642a0 (f32 position, keyed / constant),
  0x64260 / 0x642e0 (f32 rotation), 0x64320 / 0x643a0 (i16 position), 0x64360 / 0x643e0 (i16 rotation).
- Element sizes come from slot 1 of each channel object's vtable: 12 / 16 / 6 / 8 bytes at
  engine+0x62470 / 0x62510 / 0x626b0 / 0x62750.
- Decoders: engine+0x626c0 (position, factor at VA 0x67a678 = 0.0625) and engine+0x62760 (rotation, factor at
  VA 0x678478 = 3.0518509e-05). Both factors are read from the binary.

A codec value above 3 reads nothing for that node (engine+0x6dd60 returns success), which would desynchronise
the stream. A reader should treat it as an error.

ROM usage (OBSERVED):

- Codec 0 in 6,389 clips, codec 1 in 1, codec 2 in 118, codec 3 in 2.
- In codecs 1-3, every position channel has either 1 key or `marker_count` keys.
- 36 tracks have `morph` = 1, matching the 36 kind-6 bodies.

## 11. Attach points and clip overrides

### 11.1 Attach points (engine+0x6f160)

The engine's own API calls these "sockets".

```
u32 count
item[count]:
  u32   node           must be < the node count, else the load fails
  str   name
  quat  rot            -> item +0x0c
  vec3  pos            -> item +0x00
  vec3  scale          -> item +0x1c
```

OBSERVED: 2,005 items. All node indices are valid, 1,925 rotations have unit length, and 2,004 scales are
(1,1,1).

### 11.2 Per-child clip overrides (engine+0x6e0d0)

For each child slot, including slot 0:

```
u32 count
item[count]:
  str   clip_name      (looked up by name among that child's clips; a duplicate name logs a warning)
  vec3  extent         -> clip info +0x04
  vec3  offset         -> clip info +0x10
```

## 12. Validation (scratch walker)

Command: `python -I staging/ltb/ltbdump.py <every .rez under CA_ROM>` (read-only). Result:

- 1,610 `.ltb` entries in 95 archives.
- 1,542 parsed end to end with 0 trailing bytes (`model_rev` 25: 1,365 files; 23: 177 files).
- 67 are not models (`file_kind` 5: 61 render-style files; `file_kind` 9: 6 files).
- 1 has `model_rev` 18, which the engine refuses (section 2); the walker refuses it the same way.
- 5,434 / 5,434 render-object `body_len` fields are exact.

## 13. Gaps

- The meaning of size hints 5-8 and 11-14, of the two unused rigid dwords, and of morph fields 5, 6 and 8.
- The two palette-tail dwords of kind 5 (+0xcc, +0xd0). No ROM file uses that path.
- The layout of the 0x44-byte box record. This build discards it, and no ROM file has any.
- What happens after load: interpolation between keys and the blend sets are in `RENDER_2009.md` 7.4 and Appendix M (M6, M7, DeCombat readings); LOD switching by
  distance is section 14 (traced, spec-fold-6).

## 14. LOD switching (EVIDENCE engine+0x1b1ff0; from the render-styles addendum S4, spec-fold-6)

Per model entry and piece (pieces hidden by the object's piece mask +0x244 are skipped):

- `dist` = |camera position (VA 0x7e5d58) - object position (+0xc8)|, or 0 for an object with flag 0x40 (param +8) or with +0x1c8 set.
- LOD = 0 when `dist` <= 0, else the first `i` with `dist < lod_dist[i + 1]` (section 5), and the last LOD past the last distance.
- `model_lod_offset` (VA 0x6d44e4, default 0) is added and the result clamped to [0, `lod_count` - 1].
- The LOD's render object is piece +0x24 [lod]; textures, style and priority come from the piece, i.e. from its last LOD record (section 5).

OBSERVED: 109 multi-LOD pieces, all with 3 LODs at distances (0, 400, 800); their LOD records never differ in slots, style or priority; the default body has 3,762 / 1,878 /
1,134 triangles at LOD 0 / 1 / 2. No DeCombat default is needed (dc-render follows this rule).

## 15. Fold record (spec-fold-6, 2026-10-25)

| item | change | grade |
|---|---|---|
| render-styles addendum S4 (`RENDER_2009_addendum_styles.md`, now a stub; evidence kept in `RENDER_2009.md` Appendix S) | new section 14; the section 13 gap narrowed | EVIDENCE, OBSERVED |
| render styles (file kind 5) | the 61 style files that section 12 lists as "not models" are specified in `FMT_RENDERSTYLE_2009.md` | pointer only |
