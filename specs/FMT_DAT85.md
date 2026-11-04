# FMT_DAT85: world file (`.dat`, version 85) as loaded by the 2009 Engine.exe

- Status: SPEC. Every section below parses all 21 world files of the user ROM byte-exactly to end of file
  (validator: `staging/dat85/dat85dump.py`, section 11).
- Module: `engine` = 2009 EU-test `Engine.exe`, image base 0x00460000. Every RVA is `engine+0xRVA`
  (VA = RVA + 0x460000). Where decompiler's C was garbled (stack-parameter confusion in the larger readers) the
  read order was taken from a disassembly of the same 2009 binary (capstone over the read-only ROM copy); the
  cited RVAs are the addresses of the individual `stream->read` calls.
- Sources used: `Engine.exe/*.c` (2009), `decompiler/Engine.exe.funcs.orig.tsv`, the
  2009 `Engine.exe` bytes (disassembly, vtable and string reads), and observation of the 21 ROM world files
  through `evidence/rezls.py` + `evidence/romdata._directory` (read only). General LithTech format
  knowledge was used for orientation only; no SDK or other proprietary source text was opened.
- All names in this document are ours. ROM policy: counts, sizes and SHA-256 prefixes only. No texture names,
  class names, property names, strings or geometry from the ROM are reproduced. Where the spec has to identify
  a ROM string (class or property name) it gives `sha256(name)[:16]` plus its length; an implementation
  identifies it by hashing the strings it reads from the user's file at run time.

Byte order is little-endian. `vec3` = 3 x f32 (12 bytes). `lstr` = u16 byte count followed by that many bytes,
no terminator in the file (reader engine+0x44b80: copies at most `max-1` bytes, skips the rest, NUL-terminates
in memory). `zstr` = NUL-terminated bytes.

## 1. Stream interface used by every reader

All readers take one stream object; its vtable (one instance at engine+0x21e030) has:

| slot | our name | evidence |
|---|---|---|
| +0x08 | `read(dst, n)` | engine+0x21e038 -> 0xa7950 |
| +0x0c | `read_lstr(dst, max)` | engine+0x44b80 (u16 count via `read(.,2)`, then bytes; overflow skipped byte by byte) |
| +0x14 | `error()` (nonzero = failed) | checked after the object list, engine+0x94116 / 0x94540 |
| +0x18 | `seek(abs_pos)` | engine+0xcb9a1 (seek 0 before the header) |
| +0x1c | `tell(&pos)` | engine+0x940fb |

## 2. Load entry points

| what | engine RVA | notes |
|---|---|---|
| server "start world": builds `<name>.dat`, opens it in the world archive, loads, then objects | 0xa12f0 | `"%s.dat"` unless the name already has a 4-char extension |
| server world load wrapper (error -> invalid-world-file code 0x2a) | 0x9c090 | calls the world-BSP interface slot +0x1c with the stream |
| world body loader (header, info string, extents, quadtree, BSP models, light grid, blockers) | 0xcf3f0 | in vtable at engine+0x220108 (pointer at 0x220114); not in the decompiler function table, read from disassembly |
| header reader | 0xce8f0 | called from 0xcf42e and 0xcb9bc |
| BSP model body reader | 0x33060 | one call per model (twice for dual models, section 5.1) |
| quadtree reader / node recursion | 0xc0220 / 0xbfb20 | |
| object list reader (server object creation) | 0x93fd0 | called from 0xa1617 with the header's object offset |
| object list scan for light objects (client) | 0xcea90 | same layout, run straight after the BSP models |
| blind-data reader | 0xc2cd0 | interface registered at engine+0x209360, used by the server at 0xa1563 |
| light-grid reader / RLE expander | 0x4e470 / 0x4dff0 | |
| blocker-polygon reader / one polygon | 0xc7050 / 0xc5c70 | interface registered at engine+0x2093c0 |
| particle-blocker reader / one polygon | 0xcde30 / 0xcd530 | interface registered at engine+0x209580 |
| client render-data entry (seek to render offset, then load) | 0xcb990 | vtable engine+0x21fe10, slot +0x24; calls slot +0x4c = 0xccae0 |
| render-data reader: renderer world body | 0xccae0 -> renderer table slot +0x114 (=0xfd990) -> 0xf0d90 -> 0x137610 | table slot filled at engine+0xfe96b |
| render block | 0x1818e0 | |
| render light group / poly with plane | 0x180990 / 0x17e6b0 | |
| render light-group grid | 0xcbed0 + 0x4f6a0 | after the renderer body, inside 0xccae0 |

## 3. Header (60 bytes at offset 0)

Read by engine+0xce8f0 as 15 consecutive `read(.,4)` calls (0xce902 ... 0xce9d1); the caller seeks to 0 first
(0xcb9a1). Offsets are absolute file positions.

| off | type | our name | rule | read at | stored by 0xcf3f0 in |
|---|---|---|---|---|---|
| 0x00 | u32 | `version` | must be 85 (0x55), else the reader returns 0 (load fails) | 0xce902, check 0xce905 | local |
| 0x04 | u32 | `objects_at` | object list (section 6) | 0xce91b | this+0x08 |
| 0x08 | u32 | `blind_at` | blind data (section 7) | 0xce929 | this+0x0c |
| 0x0c | u32 | `lightgrid_at` | light grid (section 8) | 0xce937 | this+0x04 |
| 0x10 | u32 | `blockers_at` | blocker polygons (section 9.1) | 0xce945 | this+0x10 |
| 0x14 | u32 | `pblockers_at` | particle-blocker polygons (section 9.2) | 0xce953 | this+0x14 |
| 0x18 | u32 | `render_at` | render data (section 10) | 0xce961 | this+0x18 |
| 0x1c | 8 x u32 | `reserved` | read and dropped (0xce96f ... 0xce9d1); zero in all 21 files | | |

OBSERVED on all 21 files: offsets strictly increasing, each section ends exactly where the next begins, and
the render data ends exactly at end of file. The world body (section 4) starts at 0x3c and ends exactly at
`objects_at`.

Server use: 0xa1545..0xa1556 seeks to `blind_at` (shared-BSP object +0x0c) before the blind loader, and 0xa1617
passes `objects_at` (shared-BSP object +0x08) to the object loader. Client use: 0xcb9dc seeks to `render_at`.

## 4. World body (starts at 0x3c)

Read by engine+0xcf3f0 after the header.

| order | type | our name | read at | notes |
|---|---|---|---|---|
| 1 | u32 | `info_len` | 0xcf45d | |
| 2 | u8[info_len] | `info_text` | 0xcf499 | allocated `info_len+1`, NUL-terminated in memory; plain text world settings, parsed later at 0xcf6bb (engine+0x81010) |
| 3 | vec3 | `extent_min` | 0xcf4b1 | this+0x20 |
| 4 | vec3 | `extent_max` | 0xcf4be | this+0x2c; the loader keeps `1/(max-min)` per axis (0xcf4ce..0xcf514) |
| 5 | vec3 | `world_offset` | 0xcf4cb | this+0x38 |
| 6 | quadtree | section 4.1 | 0xcf519 -> 0xc0220 | |
| 7 | u32 | `model_count` | 0xcf531 | |
| 8 | model[model_count] | section 5 | loop 0xcf561..0xcf6aa | |

Then, without a seek, the client runs the light-object scan (0xcf6d3 -> 0xcea90) over the bytes that follow,
which are the object list: OBSERVED `objects_at` equals the end of the last model in all 21 files.

### 4.1 Quadtree (engine+0xc0220, nodes engine+0xbfb20)

| order | type | our name | read at |
|---|---|---|---|
| 1 | vec3 | `tree_min` | 0xc0248 call |
| 2 | vec3 | `tree_max` | second 12-byte read |
| 3 | u32 | `tree_nodes` | read into this+0x08; `tree_nodes-1` child records of 0x40 bytes are allocated |
| 4 | u32 | `tree_reserved` | read and kept; 0 in all 21 files |
| 5 | bits | `tree_shape` | engine+0xbfb20 |

`tree_shape` is a pre-order bit stream, least significant bit first, one byte read whenever the bit counter
reaches 8 (counter starts at 8, so the first node reads the first byte). Bit 1 = the node has 4 children (the
recursion runs 4 times, stride 0x40 over a 0x100 range), bit 0 = leaf. Total nodes = 1 + 4 x (set bits), so
the stream is `ceil(tree_nodes / 8)` bytes. OBSERVED: the parsed node count equals `tree_nodes` in all 21 files,
and `tree_min/max` equal `extent_min/max`.

## 5. World model (BSP) record

Each model in the body list:

| order | type | our name | read at | notes |
|---|---|---|---|---|
| 1 | u32 | `model_reserved` | 0xcf5aa | 0 in all 21 files (1 934 models) |
| - | - | (tell) | 0xcf5b6 | position remembered for a second pass |
| 2 | BSP body | section 5.2 | 0xcf5f8 (`mode=1`) | stored in record+0x0c |

### 5.1 Dual models

After the first pass, if bit 1 of the model's `bsp_flags` (record+0x5e, test at 0xcf60c) is set, the loader
seeks back to the remembered position (0xcf61b) and reads the same bytes again with `mode=0` (0xcf65d) into a
second record (+0x04). OBSERVED: every model except exactly one per map has the bit set (table in section 11). A
reimplementation reads the body once and keeps both roles.

### 5.2 BSP body (engine+0x33060)

Header of the body, in file order:

| order | type | our name | read at | notes |
|---|---|---|---|---|
| 1 | u32 | `bsp_flags` | 0x3308a | low 16 bits kept at +0x5e (bit 1 = dual, 5.1) |
| 2 | lstr (max 0x40) | `bsp_name` | 0x330a0 | the link key to objects (section 6.4) |
| 3 | u32 | `n_points` | 0x330ae | |
| 4 | u32 | `n_planes` | 0x330bc | |
| 5 | u32 | `n_surfaces` | 0x330ca | |
| 6 | u32 | `n_portals` | 0x330d8 | must be 0 (check 0x33155, else load fails with 2) |
| 7 | u32 | `n_polys` | 0x330e6 | |
| 8 | u32 | `n_leaves` | 0x330f4 | 0 in all 21 files |
| 9 | u32 | `n_poly_verts` | 0x33102 | not used for allocation; OBSERVED = sum of all polygon vertex counts |
| 10 | u32 | `reserved_a` | 0x33110 | 0 in all files |
| 11 | u32 | `reserved_b` | 0x3311e | 0 in all files |
| 12 | u32 | `n_nodes` | 0x3312b | stored at +0x10 |
| 13 | vec3 | `box_min` | 0x33138 | +0x38 |
| 14 | vec3 | `box_max` | 0x33145 | +0x44 |
| 15 | vec3 | `origin` | 0x33152 | +0x50 |
| 16 | u32 | `texname_bytes` | 0x33174 | |
| 17 | u32 | `n_textures` | 0x33182 | |
| 18 | zstr[n_textures] | `texture_names` | byte loop 0x331c5..0x331db | total length must be `texname_bytes` (OBSERVED exact) |
| 19 | u8[n_polys] | `poly_vert_counts` | 0x33200 loop (first pass, sizing) then again 0x33350 loop after seeking back (0x33239) | |
| 20 | leaf[n_leaves] | below | loop 0x333b5..0x33431 | |
| 21 | plane[n_planes] | 16 bytes each | single read 0x33443 | |
| 22 | surface[n_surfaces] | 8 bytes each | loop 0x33452..0x33499 | |
| 23 | poly[n_polys] | variable | loop 0x334b0..0x3355a | |
| 24 | node[n_nodes] | 14 bytes each | loop 0x33571..0x33615 | |
| 25 | vec3[n_points] | `points` | single read 0x33646 | |
| 26 | i32 | `root_node` | 0x33654 | node index, or -1 / -2 (sentinels, below) |
| 27 | u32 | `trailing_count` | 0x33696 | must be 0 (0x3369f), then `error()` must be 0 (0x336a5) |

Leaf (only when `n_leaves > 0`; none in the ROM): u16 `n`; if `n == 0xFFFF` one u16 (an index, 0x333d9);
otherwise `n` times { u16 id (0x333f0), u16 `len` (0x333fe), `len` bytes skipped with tell+seek (0x3340a,
0x3341d) }.

Plane (16 bytes): vec3 normal, f32 distance (read as one block into +0x04; layout OBSERVED: normals have unit
length). Plane convention (OBSERVED over all 21 maps, Appendix Z D1): a polygon's corners satisfy `dot(normal,
corner) = dist` (3,161,010 of 3,180,577 corners within 0.1 units).

Surface (8 bytes, read as one 8-byte block at 0x3345d):

| off | type | our name | stored |
|---|---|---|---|
| 0 | u32 | `surf_flags` | surface+0x04 (0x33471) |
| 4 | u16 | `surf_texture` | surface+0x0a (0x3347d); OBSERVED always `< n_textures` |
| 6 | u16 | `surf_texflags` | surface+0x08 (0x3348a) |

Polygon (8 + 4 x `poly_vert_counts[i]` bytes, one read of that length at 0x334d0):

| off | type | our name | rule |
|---|---|---|---|
| 0 | u32 | `poly_surface` | must be `< n_surfaces` (0x334d7) else load fails |
| 4 | u32 | `poly_plane` | must be `< n_planes` (0x334e4) |
| 8 | u32[k] | `poly_points` | each must be `< n_points` (0x33527) |

Node (14 bytes, three reads):

| off | type | our name | read at | rule |
|---|---|---|---|---|
| 0 | u32 | `node_poly` | 0x33581 | must be `< n_polys` (0x33588) |
| 4 | u16 | `node_reserved` | 0x335ad | |
| 6 | 2 x i32 | `node_front`, `node_back` | 0x335bb (8-byte read) | -1 -> engine sentinel at VA 0x6ca7c8, -2 -> sentinel at VA 0x6ca7cc, else must be `< n_nodes` |

Polygon winding (OBSERVED, Appendix Z D2): corners are counter-clockwise seen from the front, `cross(c1 - c0, c2 - c1)`
points along the plane normal for 964,510 of 964,512 polygons.

Node child order (OBSERVED over the 21 maps, Appendix Z D3): `node_front` (the first i32 of the pair) is the child on the
**negative** side of the node polygon's plane and `node_back` the positive side (1,218,026 vs 8,630 placements of
child-subtree root polygons lying strictly on one side). The names are therefore historical; MOVEMENT_2009 9.8 uses
negative / positive. Confirmation against the loader's in-memory layout stays UNPROVEN (MOVEMENT_2009 16.1 item 5).
Polygons that cross a split plane are referenced by nodes in both subtrees (about three nodes per polygon over the maps,
for example BRUSHWOOD 64,977 nodes for 22,615 polygons).

Surface flag bit 0 is the collision flag MOVEMENT_2009 9.8 tests (PROVEN there); a sizeable share of the polygons
(render-only surfaces) do not carry it (OBSERVED, Appendix Z D6).

Memory bookkeeping (not needed to parse): each polygon record in memory is `0x1c + 4k` bytes rounded up to 4
(0x33213..0x33225); surfaces 12 bytes, nodes 16 bytes.

## 6. Object list (at `objects_at`)

Read by engine+0x93fd0 (server creates objects), same layout scanned by 0xcea90 (client, light classes only).

| order | type | our name | read at | notes |
|---|---|---|---|---|
| 1 | u32 | `n_objects` | 0x940d3 | after `seek(objects_at)` at 0x940c5 |
| 2 | object[n_objects] | | loop | |

Object:

| order | type | our name | read at | notes |
|---|---|---|---|---|
| 1 | u16 | `obj_len` | 0x940ef | bytes that follow, up to the end of this object (OBSERVED exact for all 10 372 objects); 0xcea90 uses it to skip non-light classes (tell 0xceb14, seek to tell+`obj_len` at 0xceb82) |
| - | | (tell) | 0x940fb | |
| 2 | lstr (max 0x100) | `class_name` | 0x9410f | looked up in the game module's class table (0x94135); unknown class logs and skips creation |
| 3 | u32 | `n_props` | 0x941e6 | |
| 4 | prop[n_props] | | loop 0x94222..0x94461 | |

Property:

| order | type | our name | read at | notes |
|---|---|---|---|---|
| 1 | lstr (max 0x100) | `prop_name` | 0x94231 | |
| 2 | u8 | `prop_type` | 0x9423f | table below |
| 3 | u32 | `prop_flags` | 0x9424d | editor flags; kept opaque |
| 4 | u16 | `prop_len` | 0x9425b | payload size |
| 5 | payload | | 0x94320 (`read_lstr`, type 0) or 0x94330 (`read`, other types) | |

Property types (jump table at engine+0x946a4, dispatch 0x9434c; payload sizes OBSERVED over all 276 740
properties of the 21 files):

| code | our name | payload | engine conversion |
|---|---|---|---|
| 0 | `text` | lstr: u16 n + n bytes; `prop_len = n + 2` | 0x9436c -> 0x92b00 |
| 1 | `vec3` | 12 | 0x9435c -> 0x92a40 |
| 2 | `rgb` | 12 (3 x f32) | same case as 1 |
| 3 | `real` | 4 (f32) | 0x94388 -> 0x92c00 |
| 4 | `bits` | 4, **stored as f32**, truncated to integer (`cvttss2si` at 0x9439e) | 0x92c90 |
| 5 | `bool` | 1 byte, nonzero = true (0x943be) | 0x92d00 |
| 6 | `int` | 4, **stored as f32**, truncated (same case as 4) | 0x92c90 |
| 7 | `euler` | 16: 3 x f32 angles in radians used (0x943e8 then euler-to-quaternion 0x5b6c0), 4th f32 unused (0.0 in all 10 372) | |

Codes above 7 are skipped (0x9434f). Code 4 does not occur in the ROM; all other codes do.

### 6.1 Fixed leading properties (OBSERVED, all 10 372 objects)

| index | type | flags | name hash (sha256[:16], length) | meaning |
|---|---|---|---|---|
| 0 | text | 0 | `dcd1d5223f73b3a9`, 4 | object name (unique key; links to `bsp_name`, 6.4) |
| 1 | vec3 | 0x00100000 | `704e75860aa0d98d`, 3 | position |
| 2 | euler | 0 | `57b5e2fc1bba18bb`, 8 | orientation |

The client light scan compares these property names against literals in the engine (0xcea90 region); a
reimplementation should match by hash, or by index/type as above.

### 6.2 Enumerating gameplay objects

The engine itself does not interpret classes; it hands each class name to the game module. To enumerate
gameplay objects, match `class_name` by hash. Roles are our own labels, inferred from each class's property
set and placement; classes seen in the ROM (21 files, 10 372 objects, 54 distinct classes):

| our role | class hash (sha256[:16]) | len | maps | total | geometry |
|---|---|---|---|---|---|
| player spawn point | `6fe7e0eb8921d10c` | 14 | 21 | 1560 | point (pos + euler); has two `text` filters, hashes `e5792f5953852926`/8 (mode filter) and `1e1a1c078af2887a`/5 (team filter) |
| item / pickup spawn point | `c25f64269d13d012` | 16 | 14 | 177 | point; mode filter `e5792f5953852926` |
| kill / fall volume | `df93348241153dc0` | 11 | 21 | 1008 | brush: geometry is the BSP model with the same name (6.4); damage `real` `0c874f581a4534f1`/6, damage kind `text` `29df1a8f796fdf57`/10 |
| generic trigger volume | `8b9c643731c94119` | 7 | 21 | 569 | box: `vec3` box-extent property (flags 0x4; half or full extent not resolved), hash `2d7b3d0c6d75ff11`/4 |
| game-mode controller | `0878a7b251301f5d` | 14 | 21 | 77 | point; carries mode start/end command strings |
| capture flag | `e576ae011fa1c731` | 8 | 15 | 30 | point |
| flag capture zone | `0d6a598a1816da49` | 15 | 15 | 30 | box (same box-extent property); team `text` `5985039f106df054`/4, mode filter |
| bomb zone | `ab0d16e1dda32b6c` | 15 | 6 | 12 | box |
| world settings (one per map) | `9819e9e19f000398` | 15 | 21 | 21 | point |
| startup command | `892748f74ce86ab8` | 14 | 16 | 22 | point |
| ladder | `df0663641d3ef583` | 6 | 12 | 47 | brush (name-linked) |
| breakable brush | `ebbd45154c24defa` | 9 | 21 | 243 | brush (name-linked) |
| plain world model | `33b5bf07771368ec` | 10 | 21 | 485 | brush (484 of 485 name-linked) |
| sky world model | `689cfa21cab3e0cb` | 17 | 21 | 24 | brush (name-linked) |
| hazard (gas) volume | `4713c0097e04864f` | 9 | 5 | 20 | brush (name-linked) |
| camera point | `7b8770f70d81e7f7` | 11 | 21 | 67 | point |

Per-map counts of the main roles are in section 11.2. Visibility and blend mode of world models are object properties
(hashes in RENDER_2009 Appendix Z R5); kill volumes, ladders and hazard volumes are all flagged invisible (OBSERVED,
Appendix Z D8).

### 6.3 Strings carried by objects

Class names, property names and `text` payloads are the object list's only strings. Command-type `text`
properties carry flag bit 0x00800000 (OBSERVED); they hold script text for the game module.

### 6.4 Object to BSP model link (OBSERVED)

For brush classes the object name (property 0) equals the `bsp_name` of one world model: 1008/1008 kill
volumes, 243/243 breakables, 47/47 ladders, 484/485 plain world models, all doors, water and hazard volumes.
Point and box classes never match. The render side keys its per-model data by the same name (render model
names hashed; "name hash conflict" log at 0x137810).

OBSERVED over the 21 maps (Appendix Z D4, D5): exactly one model per map (21 in all) is linked to no object by name; it
is also the map's only model without the dual flag (bit 1 of `bsp_flags`, 5.1) and holds most of the map's polygons
(the level geometry). Brush models are in world space: for 805 of 814 brush-type models linked to plain-world-model,
breakable or unlabelled objects, the centre of the model's corner bounds lies within the model's extent of its
object's position; the BSP `origin` equals the object position for 208 of them.

## 7. Blind data (at `blind_at`)

engine+0xc2cd0 (slot +0x0c of the vtable at engine+0x21fd10, installed by the constructor at engine+0xc2430):

| order | type | our name | read at |
|---|---|---|---|
| 1 | u32 | `n_blobs` | 0xc2cf0 |
| 2 | blob[n] | `{ u32 size (0xc2d4b), u32 id (0xc2d59), u8[size] (0xc2d83) }` | |

Opaque per-object payloads for the game module. 0 to 33 per map.

## 8. Light grid (at `lightgrid_at`)

engine+0x4e470:

| order | type | our name | read at |
|---|---|---|---|
| 1 | vec3 | `grid_origin` | 0x4e486 (stored +0x10) |
| 2 | vec3 | `grid_cell` | 0x4e494 (reciprocals kept at +0x1c..+0x24) |
| 3 | 3 x u32 | `grid_dims` x, y, z | 0x4e4a1 (+0x04..+0x0c) |
| 4 | u32 | `grid_packed_len` | 0x4e4de; read and dropped by the engine; OBSERVED equals the packed byte count |
| 5 | packed | RGB samples | 0x4dff0 |

Expanded size is `x*y*z*3` bytes. Packing (0x4dff0): repeat until the output is full: read a control byte `c`;
`run = (c & 0x7f) + 1`; if `c & 0x80`, read one 3-byte sample and repeat it `run` times, else copy `run`
3-byte samples verbatim.

## 9. Collision side data

The BSP models (section 5) are the collision and visibility geometry. Two more polygon soups follow.

### 9.1 Blocker polygons (at `blockers_at`)

engine+0xc7050 (vtable engine+0x21fd5c slot +0x0c):

| order | type | our name | read at |
|---|---|---|---|
| 1 | u32 | `n_blockers` | 0xc7065 |
| 2 | poly[n] | each via 0xc5c70: plane normal vec3 + f32 dist (four reads 0xc5c86..0xc5cad, renormalised by engine+0xc37f0), u32 `n_verts` (0xc5ce6), vec3[n_verts] (0xc5d1e loop) | |
| 3 | u32 | `blockers_tail` | 0xc709f; 0 in all files |

### 9.2 Particle-blocker polygons (at `pblockers_at`)

engine+0xcde30 (vtable engine+0x21fec0 slot +0x0c), polygons via 0xcd530:

| order | type | read at | notes |
|---|---|---|---|
| u32 count | 0xcde4c | | |
| per polygon: f32 nx, ny, nz, f32 dist | 0xcd544..0xcd56b | dist negated on load |
| u32 `n_verts`, vec3[n_verts] | 0xcd588, 0xcd5d3..0xcd5f5 | bounding box accumulated |
| u32 tail | 0xcdef5 | 0 in all files |

## 10. Render data (at `render_at`, client only)

engine+0xcb990 seeks to `render_at` and calls 0xccae0, which first calls the renderer's world loader
(renderer table slot +0x114 = 0xfd990 -> 0xf0d90 -> 0x137610) and then reads light-group grids (10.5).

### 10.1 Render world (engine+0x137610, recursive)

| order | type | our name | read at |
|---|---|---|---|
| 1 | u32 | `n_blocks` | 0x137649 (blocks are 0xf4 bytes in memory) |
| 2 | block[n_blocks] | 10.2 | 0x1376cd per block |
| 3 | u32 | `n_render_models` | 0x137722 |
| 4 | per model: lstr (max 0x41) name (0x137793), then a nested render world (recursive call 0x1377cb) | | |

OBSERVED: top level `n_render_models` equals the number of dual BSP models (section 5.1) in all 21 files. Nested render
worlds are stored in world coordinates: every block lies inside the BSP box of the same-named model (RENDER_2009 Appendix Z
R5; Appendix Z D7).

### 10.2 Render block (engine+0x1818e0)

| order | type | our name | read at |
|---|---|---|---|
| 1 | vec3 | `block_center` | 0x181933 |
| 2 | vec3 | `block_half` | 0x181940 |
| 3 | u32 | `n_sections` | 0x1819ae |
| 4 | section[n_sections] | below | loop 0x1819f0..0x181ce4 |
| 5 | u32 | `n_vertices` | 0x181cfb |
| 6 | vertex[n_vertices] | 68 bytes each, one read | 0x181d23 |
| 7 | u32 | `n_triangles` | 0x181d33 |
| 8 | tri[n_triangles] | 4 x u32: 3 vertex indices + 1 source index | 0x181d8b..0x181df7 |
| 9 | u32 | `n_sky_portals` | 0x181e8d; each a plane-poly (10.4) |
| 10 | u32 | `n_occluders` | 0x181f1f; each a plane-poly + u32 id (0x181f82) |
| 11 | u32 | `n_light_groups` | 0x181fd3; each 10.3 |
| 12 | u8 | `child_mask` | 0x182047 |
| 13 | 2 x u32 | `child_ref` | 0x182062 loop; slot unused when its mask bit is clear |

Section:

| order | type | our name | read at |
|---|---|---|---|
| 1 | 2 x lstr (max 0x105) | texture paths | 0x181a0a (loop of 2) |
| 2 | u8 | `shader_kind` | 0x181a21 (switch over values up to 9 at 0x181be4; 0xa/0x14/0xe special-cased) |
| 3 | u32 | `section_triangles` | 0x181a2f (running triangle base kept per section) |
| 4 | lstr (max 0x105) | effect name | 0x181a43 |
| 5 | u32, u32 | `lightmap_w`, `lightmap_h` | 0x181c6e, 0x181c7b |
| 6 | u32 | `lightmap_len` | 0x181c88 |
| 7 | u8[lightmap_len] | lightmap payload | 0x181ca5 |

Vertex (68 bytes): read as one block, so the field split is not code-proven here. OBSERVED over the 21 maps
(dc-render census, Appendix Z D9): position (3 f32 at 0), first uv pair (2 f32 at 12), second uv pair (2 f32 at 20),
colour (u32 ARGB at 28), normal (3 f32 at 32), two unit vectors (at 44 and 56). The first pair is the texture
coordinate of kinds 1/4/8/9 and the LIGHTMAP coordinate of kind 2 (range [0.5/256, 1 - 0.5/256]); the second pair is
non-zero only on kinds 8 and 9 (second texture). The 3 floats at offset 32 are unit vectors in every sampled vertex
(84 207 vertices sampled across the 21 files). Consumers of the vertex block (vertex buffer fill) are still not
traced: UNPROVEN as code, OBSERVED as data. See RENDER_2009 6.1 and Appendix Z R1.

Section item 3 (OBSERVED, Appendix Z D10): the `section_triangles` of a block's sections sum to `n_triangles` in every
block; sections take consecutive triangle ranges in file order.

### 10.3 Render light group (engine+0x180990)

| order | type | read at |
|---|---|---|
| u16 n + n bytes name (hashed in place, `h = h*31 + byte`) | 0x1809bb, 0x1809db loop |
| vec3 colour | 0x180a08 |
| u32 len + u8[len] | 0x180a16, 0x180a34 |
| u32 `n_slots` | 0x180a49 |
| per slot: u32 `n_patches` (0x180a7b); per patch: 4 x u32 (0x180aea..0x180b11), u32 len (0x180b1f), u8[len] (0x180b8f) | |

OBSERVED over the 21 maps (Appendix Z D11): `n_slots` equals the block's `n_sections` in every group (one slot per section,
in section order); patches occur only in slots of kind-2 sections; the four patch words are `x, y, w, h` and the rectangle
lies inside that section's lightmap; patch bytes expand to exactly `w * h` values with the escape "0xff, count = repeat
the previous value count times" (4,364 patches). The group colour is three floats in [0, 1]. The meaning of the group's
`u32 len + bytes` field stays open.

### 10.4 Plane-poly (engine+0x17e6b0)

u8 `n` (0x17e6c5), vec3[n] (0x17e71e loop), vec3 normal (0x17e7c5), f32 dist (0x17e7d2).

### 10.5 Light-group grids (engine+0xccae0 after the renderer body)

| order | type | read at |
|---|---|---|
| u32 `n_grids` | 0xccb3a |
| per grid: u16 n + n bytes name (hashed, 0xcbed0: 0xcbee4, 0xcbf00), vec3 colour (0xcbf2d), 3 x i32 offset (0x4f786), 3 x u32 dims (0x4f794), u8[x*y*z] (0x4f80c) | |

OBSERVED: the last grid ends exactly at end of file in all 21 maps (0 or 2 grids per map).

## 11. Validation

Validator: `python -I staging/dat85/dat85dump.py --json all.json` (2.5 s for all 21).
It parses every section with the layouts above, checks every index bound listed in section 5 and the render
triangle indices, checks section boundaries against the header, and requires end-of-file at the end of 10.5.

Result: **parsed 21/21**. In every file: `version`=85, reserved header words 0, world body ends at
`objects_at`, objects end at `blind_at`, blind data ends at `lightgrid_at`, light grid ends at
`blockers_at`, blockers end at `pblockers_at`, particle blockers end at `render_at`, render data ends at EOF.
`n_portals`=0, `n_leaves`=0, `trailing_count`=0, `n_poly_verts` = sum of polygon vertex counts.

### 11.1 Per-map counts

BSP columns are sums over all models of the map. "dual" = models with bsp_flags bit 1.

| map | bytes | quadtree nodes | BSP models (dual) | BSP nodes | BSP polys | points | planes | surfaces | textures | objects | classes | blind | blockers | p-blockers | light grid | render blocks | r-verts | r-tris | sections | lightmap bytes | r-models | lg grids |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| BRUSHWOOD | 7717853 | 341 | 56 (55) | 64977 | 22615 | 15995 | 22639 | 157 | 186 | 311 | 25 | 33 | 35 | 15 | 32x99x28 | 37 | 56948 | 21159 | 563 | 153225 | 55 | 0 |
| CABIN FEVER | 11197353 | 5461 | 38 (37) | 58144 | 32029 | 22303 | 26636 | 241 | 240 | 399 | 32 | 7 | 126 | 8 | 82x82x82 | 71 | 91593 | 39188 | 1023 | 329713 | 37 | 2 |
| COLDSEED | 18108691 | 1365 | 80 (79) | 82219 | 39263 | 42587 | 37954 | 306 | 321 | 298 | 21 | 0 | 101 | 44 | 61x159x61 | 111 | 156319 | 74422 | 1990 | 2185998 | 79 | 0 |
| DEATHROOM | 16008889 | 341 | 42 (41) | 105201 | 35267 | 26180 | 28499 | 125 | 160 | 228 | 21 | 0 | 192 | 0 | 37x59x37 | 95 | 133373 | 57603 | 899 | 2111383 | 41 | 0 |
| GRAVEDIGGER | 24496461 | 5461 | 96 (95) | 117406 | 49182 | 40982 | 40759 | 334 | 409 | 408 | 25 | 4 | 149 | 16 | 82x82x82 | 185 | 213803 | 93982 | 2429 | 3640391 | 95 | 0 |
| GRAYHAMMER | 23283094 | 1365 | 87 (86) | 84095 | 38757 | 50137 | 35069 | 472 | 466 | 392 | 25 | 0 | 146 | 0 | 102x160x75 | 133 | 186934 | 92810 | 2829 | 4493871 | 86 | 0 |
| JUNK_FLEA | 15673717 | 21845 | 63 (62) | 87759 | 32586 | 33747 | 31843 | 317 | 279 | 317 | 22 | 0 | 42 | 0 | 263x525x263 | 93 | 113675 | 52768 | 1276 | 1271851 | 62 | 0 |
| OVERDOSE | 23852033 | 1365 | 212 (211) | 110370 | 42045 | 37974 | 35124 | 487 | 667 | 586 | 24 | 0 | 38 | 0 | 64x58x59 | 129 | 185471 | 89149 | 2124 | 4545822 | 211 | 2 |
| PUMPJACK | 19958493 | 341 | 38 (37) | 86830 | 36174 | 37501 | 34437 | 206 | 208 | 260 | 20 | 0 | 81 | 0 | 32x160x36 | 113 | 172515 | 81002 | 1759 | 3154489 | 37 | 0 |
| RAPIDSTREAM | 26185760 | 1365 | 141 (140) | 140995 | 49830 | 58269 | 41916 | 674 | 666 | 552 | 25 | 10 | 161 | 0 | 70x73x73 | 169 | 234775 | 121237 | 3264 | 2279202 | 140 | 0 |
| RATTLESNAKE | 24928939 | 1365 | 69 (68) | 94261 | 50406 | 48509 | 46947 | 298 | 322 | 448 | 25 | 10 | 150 | 0 | 40x41x45 | 137 | 211577 | 98859 | 1956 | 4485917 | 68 | 0 |
| ROADKILL | 26467089 | 21845 | 99 (98) | 182499 | 72146 | 50365 | 70711 | 211 | 219 | 295 | 19 | 13 | 60 | 0 | 189x91x181 | 187 | 242337 | 93374 | 1381 | 1348865 | 98 | 0 |
| SANDHOG | 33165709 | 341 | 112 (111) | 147670 | 68160 | 63784 | 63949 | 460 | 392 | 357 | 24 | 7 | 390 | 0 | 33x66x31 | 183 | 306765 | 139265 | 3656 | 3501704 | 111 | 0 |
| SANDHOG_CP1 | 54334765 | 1365 | 118 (117) | 245759 | 113429 | 101287 | 105860 | 503 | 504 | 3017 | 38 | 1 | 459 | 0 | 52x66x66 | 301 | 429998 | 189786 | 5491 | 4277569 | 117 | 0 |
| SNOWVALLEY | 20849375 | 1309 | 94 (93) | 105729 | 49769 | 54477 | 48572 | 368 | 422 | 559 | 25 | 0 | 171 | 0 | 70x132x78 | 131 | 192985 | 86636 | 2006 | 853756 | 93 | 0 |
| TWOTOWERS | 26424556 | 21845 | 118 (117) | 83124 | 40182 | 44317 | 34980 | 308 | 375 | 404 | 23 | 0 | 646 | 0 | 213x132x197 | 137 | 203087 | 100283 | 2172 | 5710021 | 117 | 0 |
| TWOTOWERS_QRT | 24792990 | 21725 | 115 (114) | 74497 | 37710 | 42228 | 34297 | 291 | 360 | 273 | 18 | 0 | 624 | 0 | 184x160x158 | 161 | 199323 | 98499 | 2295 | 5145460 | 114 | 0 |
| WAREHOUSE | 18095257 | 5461 | 110 (109) | 91743 | 38402 | 31128 | 34339 | 414 | 494 | 310 | 16 | 6 | 50 | 0 | 80x81x81 | 145 | 148383 | 67980 | 2115 | 3212853 | 109 | 0 |
| WAREHOUSE_QRT | 19640583 | 5461 | 103 (102) | 96701 | 39703 | 31932 | 35665 | 415 | 482 | 269 | 16 | 6 | 45 | 0 | 80x81x81 | 161 | 171864 | 76012 | 2239 | 2937330 | 102 | 0 |
| WARHEAD | 18986126 | 1081 | 102 (101) | 107942 | 30489 | 39272 | 27524 | 436 | 463 | 444 | 27 | 9 | 136 | 0 | 70x164x57 | 127 | 147735 | 75610 | 1975 | 3147722 | 101 | 0 |
| WAVERIDER | 31297063 | 5461 | 41 (40) | 136877 | 46368 | 56456 | 40348 | 293 | 263 | 245 | 22 | 6 | 187 | 0 | 150x90x153 | 221 | 274681 | 139937 | 3363 | 5028718 | 40 | 0 |

Sky portals: 6 per map (all 21). Occluders: GRAVEDIGGER 6, OVERDOSE 7, SANDHOG 15, SANDHOG_CP1 38, WAVERIDER 13,
others 0. Render light groups: CABIN FEVER 79, OVERDOSE 55, others 0. Totals: 10 372 objects, 276 740
properties, 1 934 BSP models (1 913 dual).

### 11.2 Gameplay object counts per map (roles from 6.2)

| map | player spawn | item spawn | kill volume | trigger | mode ctl | flag | flag zone | bomb zone |
|---|---|---|---|---|---|---|---|---|
| BRUSHWOOD | 80 | 8 | 23 | 25 | 4 | 2 | 2 | 0 |
| CABIN FEVER | 8 | 2 | 8 | 26 | 1 | 0 | 0 | 0 |
| COLDSEED | 80 | 12 | 55 | 17 | 4 | 2 | 2 | 0 |
| DEATHROOM | 84 | 11 | 10 | 19 | 4 | 2 | 2 | 0 |
| GRAVEDIGGER | 98 | 16 | 61 | 19 | 5 | 2 | 2 | 0 |
| GRAYHAMMER | 104 | 17 | 52 | 22 | 5 | 2 | 2 | 2 |
| JUNK_FLEA | 100 | 11 | 38 | 25 | 5 | 2 | 2 | 0 |
| OVERDOSE | 60 | 0 | 88 | 48 | 3 | 0 | 0 | 0 |
| PUMPJACK | 80 | 12 | 20 | 17 | 4 | 2 | 2 | 0 |
| RAPIDSTREAM | 106 | 19 | 79 | 21 | 5 | 2 | 2 | 2 |
| RATTLESNAKE | 88 | 0 | 34 | 20 | 4 | 2 | 2 | 2 |
| ROADKILL | 48 | 0 | 73 | 13 | 3 | 2 | 2 | 0 |
| SANDHOG | 104 | 15 | 67 | 22 | 5 | 2 | 2 | 2 |
| SANDHOG_CP1 | 32 | 0 | 20 | 151 | 1 | 0 | 0 | 0 |
| SNOWVALLEY | 104 | 16 | 50 | 23 | 5 | 2 | 2 | 2 |
| TWOTOWERS | 100 | 12 | 65 | 25 | 5 | 2 | 2 | 0 |
| TWOTOWERS_QRT | 20 | 0 | 75 | 9 | 1 | 0 | 0 | 0 |
| WAREHOUSE | 40 | 0 | 55 | 11 | 2 | 0 | 0 | 0 |
| WAREHOUSE_QRT | 20 | 0 | 56 | 9 | 1 | 0 | 0 | 0 |
| WARHEAD | 124 | 11 | 54 | 30 | 6 | 2 | 2 | 2 |
| WAVERIDER | 80 | 15 | 25 | 17 | 4 | 2 | 2 | 0 |

## 12. Implementation notes for the Rust reader

- A server only needs sections 3, 4 (to reach the end of the models), 5 (collision), 6 and 7, plus 9.1 for
  player blockers. Section 10 is client-only and can be skipped by offset.
- Bounds rules in 5.2 are the engine's own rejection rules (return code 2): reproduce them as errors.
- Types 4 and 6 are f32 on disk; convert with truncation toward zero.
- Never ship ROM strings: identify classes and properties by `sha256(name)[:16]` or by role configuration.

## 13. Open questions

1. Slot numbering of the vtable at engine+0x220108: the server path (0x9c090) calls the world interface at
   slot +0x1c and 0xcf3f0 calls its own slot +0x18 with one argument; the slot base that makes both consistent
   is not resolved. That the server and client share 0xcf3f0 is PLAUSIBLE (the shared-BSP object fields +0x08
   and +0x0c used at 0xa1545/0xa1617 are exactly where 0xcf3f0 stores `objects_at` and `blind_at`).
2. The 68-byte render vertex is read as a single block; its field split is OBSERVED (10.2) but the consumers (vertex
   buffer fill) are not traced, so the split is not code-proven.
3. `surf_flags`, `prop_flags`, `bsp_flags` bits other than bit 1 are kept opaque (`bsp_flags` bit 1 = dual; `surf_flags`
   bit 0 = collision, 5.2).
4. Lightmap payload encoding (10.2 item 7) and the light-group patch payloads (10.3) are decoded (OBSERVED, see the
   addenda below and 10.3); what the light-group `u32 len + bytes` field means is open.
5. The world `info_text` grammar (parsed at engine+0x81010) is not specified here.

## Addendum 2026-10-24 (OBSERVED by the dc-world crate over all 21 maps)

2026-10-24, all 21 maps):

> Column scope: "render blocks", "r-verts" and "r-tris" count the blocks of the top-level render world (10.1)
> only; "sections" and "lightmap bytes" are summed over the top level and every nested render model. "r-models"
> is the top-level count.
> Every render triangle's three vertex indices are below its block's `n_vertices`.

Proposed note for §2 / the ROM model: the world `.dat` entries are stored unencrypted inside the world archive
(first word 85), so a loader must not run them through the archive cipher.

## Addendum 2026-10-24 (from RENDER_2009 §5.2/§6.1): open item 4, lightmap payload, is closed

The lightmap payload uses the same run-length encoding as the light grid (§8); it decodes exactly over all 6,274 lightmaps of the 21 ROM maps (every decoded size is a power-of-two rectangle). Kind-2 render sections carry the lightmaps and duplicate the triangles of kinds 4 and 9. Details and the engine rvas are in RENDER_2009.md.

## Appendix Z. Evidence folded from the dc-sim and dc-render addenda (2026-10-24)

All items are OBSERVED with dc-world's parser or the dc-render census examples (`render_census`, `light_groups`,
`world_model_props`) over the 21 ROM worlds, read-only; counts only. Nothing here is code-proven.

- **D1 plane convention, D2 winding, D3 node child order, D6 surface flag bit 0**: dc-sim worker measurements as quoted in
  5.2. D3 is the same measurement as MOVEMENT_2009 Appendix Z M4.
- **D4 level geometry, D5 world-space brush models**: dc-sim worker measurements as quoted in 6.4.
- **D7 nested render worlds in world coordinates**: every block inside the BSP box of the same-named model (1,913
  render models, 0 blocks outside; RENDER_2009 Appendix Z R5).
- **D8 invisibility of kill volumes, ladders, hazard volumes**: RENDER_2009 Appendix Z R5 (1,183 of 1,913 world-model
  objects carry the invisible flag).
- **D9 vertex field split**: dc-render census; second uv pair zero on kinds 1, 2, 4.
- **D10 section triangle ranges**, **D11 light group slots and patches**: dc-render census (`light_groups`).
