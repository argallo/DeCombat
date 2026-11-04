# GLOSSARY_UDP: shared vocabulary for the 2009 game-connection specs

- **Title:** GLOSSARY_UDP, names coined by this project for the UDP game connection of the 2009 client.
- **Scope:** every name used in `UDP_TRANSPORT_2009.md` (transport) and `UDP_SESSION_2009.md` (session layer).
  The names are ours. Only the numeric values, bit widths and orders are wire facts; each row cites the 2009
  `Engine.exe` function (`engine+0xRVA`, image base 0x00460000) that proves it.
- **Rules for adding rows:** append, never rename an existing row (rename = new row + "replaced by"). Keep names
  free of SDK vocabulary. One section per spec author.
- **Status:** draft 1. **Date:** 2026-10-24.

## T. Transport layer (author: udp-transport-2009)

### T.1 Datagram kinds

| name | meaning | evidence |
|---|---|---|
| `OOB_MAGIC` | 0x01337F10, first 32 bits of every out-of-band datagram (wire bytes `10 7F 33 01`) | engine+0xb5d70, engine+0xb6850 |
| oob datagram | out-of-band (unconnected) datagram: `[OOB_MAGIC:u32][opcode:u3][payload]`, no check byte | engine+0xb5120 |
| link datagram | datagram of an established peer: `[check8][has_ctl][ctl]?[has_rel][rel]?[has_unrel][unrel]?` | engine+0xb4a70 |
| link | per-peer connection object (one per remote IP:port) | engine+0xb48b0 |
| driver | the socket owner holding the link table, host flag, session name, app id | engine+0xb5d70 |
| `check8` | XOR of the four bytes of CRC-32 (0xEDB88320, init ~0, final ~) over the link datagram body | engine+0x80f70, engine+0xb1440 |

### T.2 Out-of-band opcodes (3 bits after `OOB_MAGIC`)

| value | name | payload | evidence |
|---|---|---|---|
| 0 | `OOB_INFO_ASK` | `app_id:128`, `probe_tag:u8` | engine+0xb4260 |
| 1 | `OOB_INFO_REPLY` | `ok:u1`; if ok `session_name:zstr8`, `probe_tag:u8`, `host_flag:u1`, `link_count:u8`, `host_byte:u8` | engine+0xb5120 |
| 2 | `OOB_JOIN_ASK` | `app_id:128`, `rate_bps:u32` | engine+0xb6850 |
| 3 | `OOB_JOIN_REPLY` | `accepted:u1`; if 0 `same_app:u1` | engine+0xb5120 |
| 4 | `OOB_ECHO_ASK` | `echo_id:u24` | engine+0xb71e0 |
| 5 | `OOB_ECHO_REPLY` | `echo_id:u24` | engine+0xb5120 |

### T.3 Link datagram blocks and fields

| name | meaning | evidence |
|---|---|---|
| `has_ctl` / `has_rel` / `has_unrel` | 1-bit presence flags, in this order; an absent trailing flag reads 0 | engine+0xb4a70 |
| ctl block | `[ctl_op:u4][...]` | engine+0xb2500 |
| `CTL_ACK` (ctl_op 0) | `ack_seq:u13`, `has_sack:u1`, `sack_mask:u8`; also the periodic keep-alive ("heartbeat" in the engine log) | engine+0xabc10, engine+0xb2500 |
| `CTL_BYE` (ctl_op 1) | disconnect, no code | engine+0xb3920 |
| `CTL_BYE_CODE` (ctl_op 3) | disconnect with `bye_code:u32` | engine+0xb3a10 |
| `ack_seq` | last reliable sequence received in order (cumulative ack) | engine+0xabc10 |
| `sack_mask` | bit d = reliable block `ack_seq + 2 + d` was received out of order | engine+0xabc10, engine+0xb2500 |
| rel block | `[rel_seq:u13][tail_cont:u1]` then entries `[vlen][bits]` separated by `more:u1`, closed by `more = 0` | engine+0xb2e20, engine+0xb2690 |
| `rel_seq` | 13-bit reliable block sequence, first = 1, wraps 8191 -> 0 | engine+0xb2e20 |
| `tail_cont` | 1 = the last entry continues as the first entry of the next rel block (split message) | engine+0xb2e20 |
| `more` | 1 = another entry follows in this rel block | engine+0xb2690 |
| unrel block | entries `[vlen][bits]` until `vlen = 0` or end of datagram | engine+0xb2c20 |
| `vlen` | variable-length bit count: `[7 bits][more][3 bits][more]{[bit][more]}` | engine+0xab9e0, engine+0xaba90 |
| entry | one message (a bit string) inside a rel or unrel block; what the session layer receives | engine+0xb2690, engine+0xb2c20 |
| sent record | sender's copy of one rel block: seq, entries, first_send, next_check, acked, resend count | engine+0xb2e20 |
| ack-due | link flag: an ack must go out with the next datagram | engine+0xb2690, engine+0xaf450 |

### T.4 Out-of-band fields

| name | meaning | evidence |
|---|---|---|
| `app_id` | 128-bit application id, Combat Arms value per PROTOCOL.md §4.4 (PROVEN) | engine+0xb6850, engine+0xb5120 |
| `rate_bps` | requested link rate in bits per second | engine+0xb6850, engine+0xb5120 |
| `probe_tag` | 8-bit info-probe tag, 0..31 timing slot or 0xFF = untimed | engine+0xb4260 |
| `session_name` | host session name, `zstr8` | engine+0xb5120 |
| `link_count` | number of live links on the host (the player count) | engine+0xb5120, engine+0xb4ff0 |
| `host_flag`, `host_byte` | info-reply fields set from the host parameters; meaning UNPROVEN | engine+0xb6690 |
| `echo_id` | 24-bit echo identifier | engine+0xb71e0 |
| `accepted`, `same_app` | join reply bits: accepted; else same_app = 1 refused, same_app = 0 app id mismatch | engine+0xb6850 |

### T.5 Field encodings used inside messages

| name | layout | evidence |
|---|---|---|
| `zstr8` | 8-bit units + 8-bit 0 | engine+0x80e30, engine+0x80450 |
| `zstr16` | 16-bit units + 16-bit 0 (reader proven, writer UNPROVEN) | engine+0x80490 |
| `lstr16` | u16 byte count + bytes | engine+0x5b900, engine+0x5bf40 |
| `lstr16h` | u16 byte count, 0xFFFF = none, 0 = empty, + bytes | engine+0x5b560, engine+0x5bc10 |
| `nest16` | u16 bit count + nested message bits | engine+0x5b9d0 |
| `objref16` | u16 object id, 0xFFFF = none | engine+0x5b530, engine+0x5cde0 |
| `f32` | IEEE-754 binary32 bit pattern as 32 bits | engine+0x5b3d0, engine+0x5b650 |
| `pos48` | compact position `[px:u16][py:u16][pz:u16]`, 19/18/19-bit fixed point over a box | engine+0x5c040, engine+0x5c320, engine+0x21910 |
| `vec72` | compact vector `[major:f32][r1:u16][r2:u16][vflags:u8]` (velocities) | engine+0x5b360, engine+0x5b5d0, engine+0x21540 |
| `vflags` | `vec72` flag byte: bits 0-1 high bits of r2, 2 sign r2, 3-4 high bits of r1, 5 sign r1, 6-7 major axis | engine+0x21540 |
| `yaw8` | s8 yaw, `yaw = s8 * 2π/254` rad | engine+0x5b990, engine+0x5b8a0 |
| `quat128` | `[x:f32][y:f32][z:f32][w:f32]` | engine+0x5b3d0, engine+0x5b650 |
| `rot24` / `rot48` | compact rotation, 3 signed bytes (short form, first byte negative) or 6 | engine+0x5bdd0, engine+0x21b90 |

### T.6 Timers, limits and state

| name | value | evidence |
|---|---|---|
| join retry | 300 ms | engine+0xb6850 |
| join timeout | 10000 ms | engine+0xb6850 |
| stale link on re-join | > 9999 ms since the link traffic stamp | engine+0xb5120 |
| `hb_due` | `now - last_ack_sent >= max(200, L_ms)` | engine+0xad520, engine+0xb3b20 |
| `L_ms` | truncated latency estimate (mean of last 16 round trips, start 200 ms, raised while waiting) | engine+0xabb40, engine+0xad580 |
| `RTO` | `clamp(L_ms, 17, 1000)`; trickle mode at least 500 | engine+0xb3680 |
| trickle | resend-only-the-head mode after a record was resent more than 10 times | engine+0xb3680 |
| pause | no new rel blocks while more than 8 sent records or a record resent more than 2 times | engine+0xb3680 |
| out-of-order slots | 8, accepted window `expected .. expected+8` | engine+0xaf450, engine+0xb2690 |
| split threshold | only messages longer than 255 bits are split | engine+0xb2e20 |
| body budget | 8800 bits (rel block also `<= W/2`) | engine+0xb2e20, engine+0xaf560 |
| receive buffer | 8192 bytes | engine+0xafaa0 |
| unrel TTL | `2 * max(L_ms, 200)` ms | engine+0xaf560 |
| `W` | flow window `rate_bps * 200 / 1000` bits | engine+0xad5e0 |
| datagram cost | `round8(body_bits + 8) + 224` bits | engine+0xb1440 |
| blocked | `2W < used + debt + cost` | engine+0xad7c0 |
| idle drop | more than 90000 ms without receive, bye code 3 | engine+0xb5c30 |
| bye repeat | 4 copies, 100 ms apart | engine+0xb3920, engine+0xb3a10 |
| default host / probe port | 27888 | engine+0xb6690, engine+0xb3ef0 |
| engine protocol version | 7 (carried by the session layer) | engine+0xa2a70, engine+0x8e9f0 |

### T.7 Result and event codes

| name | value | meaning | evidence |
|---|---|---|---|
| `JOIN_TIMED_OUT` | 0x4F | no join reply within 10 s | engine+0xb6850 |
| `JOIN_NO_SOCKET` | 0x50 | could not bind a UDP port | engine+0xb6850, engine+0xb6690 |
| `JOIN_REFUSED` | 0x51 | `accepted = 0, same_app = 1` | engine+0xb6850 |
| `JOIN_WRONG_APP` | 0x52 | `accepted = 0, same_app = 0` | engine+0xb6850 |
| `VERSION_MISMATCH` | 0x4E | engine protocol version is not 7 | engine+0xa2a70 |
| `DROP_STALE` | 3 | link dropped: idle timeout, stale re-join, or plain bye seen by a non-host | engine+0xb5c30, engine+0xb5120, engine+0xb5d70 |
| `DROP_PLAIN` | 0 | plain bye seen by the host | engine+0xb5d70 |

## S. Session layer (author: the UDP_SESSION_2009 agent appends here)

### S.A Session start-up (author: udp-session-2009-a, spec `UDP_SESSION_2009_A.md`)

Message names. `S2C_` = server to client, `C2S_` = client to server; the number is the first byte (`msg_id:u8`)
of the message. Names marked (B) were coined by part B and are used unchanged.

| name | id | dir | body after `msg_id:u8` | evidence |
|---|---|---|---|---|
| `S2C_VERSION` (B) | 4 | S->C | `net_version:u32` (= 7), `host_rate_bps:u32` | engine+0xa2a70, engine+0x8e9f0 |
| `S2C_LEAVE_WORLD` | 5 | S->C | none; unload the world, view object := none | engine+0xa2960 (DISASM), engine+0x8ceb0 |
| `S2C_LOAD_WORLD` | 6 | S->C | `srv_clock:f32`, `world_file:u16` | engine+0xa3e90 (DISASM), engine+0x1a770, engine+0x8ddd0 |
| `S2C_VIEW_OBJECT` | 7 | S->C | `object_id:u16`; the client's own (or viewed) object | engine+0xa2a50 (DISASM), engine+0x8db60 |
| `S2C_CLIENT_ID` (B) | 12 | S->C | `client_id:u16`, `local_host:u8` (0 for a remote server) | engine+0xa2910, engine+0x8e9f0 |
| `S2C_BUNDLE` | 14 | S->C | `{len:u8, len bits}` until `len = 0`; each part is a message | engine+0xa3f80 (DISASM) |
| `S2C_EVAR` (B) | 15 | S->C | `name:zstr8`, `value:zstr8` | engine+0xa50c0, engine+0x8ddd0 |
| `S2C_SKY_TABLE` | 16 | S->C | `sky_block:384 bits`, `count:u16` (<= 30), `count x u16`; part B calls it the world slot table | engine+0xa44f0 (DISASM), engine+0x8d5a0 |
| `S2C_PRELOAD` (B) | 19 | S->C | `sub:u8` + `u16` list; sub 1 asks for `C2S_LOAD_STATE(1)` | engine+0xa45b0 |
| `S2C_FILE_LIST` | 0x32 | S->C | `{file_id:u16, file_info:u32, name:zstr8}` to the end | engine+0x3e990, engine+0x3f3f0 |
| `C2S_HELLO` (B) | 5 | C->S | `blob_len:u16` (bytes, 258), hello blob | engine+0x18d00, engine+0x90b40 (DISASM) |
| `C2S_BYE` (B) | 6 | C->S | none; host drops the link with bye code 5 | engine+0xa48d0, engine+0x90ae0 |
| `C2S_RATE` (B) | 7 | C->S | `rate_8k:u16` (bits per second / 8000), then `u16` ids | engine+0x1ae50, engine+0x90a80 |
| `C2S_LOAD_STATE` | 9 | C->S | `state:u8`: 0 = world loaded, 1 = preload done (= part B's `C2S_PRELOAD_ACK`) | engine+0xa3e90 (DISASM), engine+0xa45b0, engine+0x90b20 (DISASM) |
| `C2S_FILE_ACK` | 0x36 | C->S | `{entry:u16}`, `entry = file_id \| 0x8000` when accepted | engine+0x3e990, engine+0x3f740 |

Other names.

| name | meaning | evidence |
|---|---|---|
| `srv_conn` | the client's object for its server connection (own client id, view object id, world-loaded flag, request kind) | engine+0x19e80 |
| `start_request` | the request the shell hands to the engine start call (kind 3 = join a remote server, address text at +0x68, hello blob pointer / length at +0x11b4 / +0x11b8) | cs+0x2fac0, engine+0x18d00 |
| hello blob | 258 bytes: 8-bit name (64), second 8-bit string (64), 2 zero bytes, UTF-16 name (128) | cs+0x2fac0 |
| `conn_mgr` | CShell connection manager (global VA 0x3759e22c); holds `start_request` from its offset +4 and the last start result | cs+0x2f7a0, cs+0x2fac0 |
| server file id | 16-bit key of a file announced in `S2C_FILE_LIST`; file references of kind 2 carry it | engine+0xd0c40 (DISASM), engine+0xd09e0 (DISASM) |
| `world_load` | client world load run by `S2C_LOAD_WORLD`; results 0x29 / 0x2a / 0x2b / 1 | engine+0x1a770 |
| entered-world callback | client shell slot +0x4c, called when `world_load` succeeds | engine+0x14c80, cs+0x1a5590 |
| `in_world_test` | CShell test that closes the loading screen (own object, switching-worlds state 2 / 3, entered-world flag, player state) | cs+0x1f3950 |
| `disconnect_box` | CShell reason-code -> message box mapping | cs+0x1bb2f0 |
| `ST_LOBBY`, `ST_JOINING`, `ST_LINKED`, `ST_LOADING_WORLD`, `ST_WORLD_LOADED`, `ST_IN_WORLD`, `ST_FAILED`, `ST_DROPPED` | client start-up states | `UDP_SESSION_2009_A.md` §8.1 |
| phase 1 / 2 / 3, load sub-state 0..3 | the 2009 host's per-client start-up states | engine+0x8e1b0, engine+0x8db60 |

### S.B Session layer, part B: in-world messages (author: udp-session-2009-b, spec `UDP_SESSION_2009_B.md`)

Names shared with part A are marked (A); the same name means the same message. Engine RVAs use image base
0x00460000, CShell RVAs (`cshell+`) image base 0x37100000.

#### S.B.1 Engine message ids

| name | id / dir | meaning | evidence |
|---|---|---|---|
| `S2C_LEAVE_WORLD` (A) | 5, S->C | world gone, empty body | engine+0x8ceb0 |
| `S2C_VIEW_OBJECT` (A) | 7, S->C | `[obj:u16]` the client's view object | engine+0x8d0c0, engine+0x8db60 |
| `S2C_OBJ_GROUP` | 8, S->C | reliable group of length-prefixed records (objects, sounds, removals) | engine+0x8fe60, engine+0x8d200 |
| `S2C_OBJ_STREAM` | 10, S->C | unreliable compact object entries + time entry | engine+0xa4160, engine+0x8bdf0 |
| `S2C_SHELL_MSG` | 13, S->C | wrapper of a CShell game message (`shell_id:u8` first) | engine+0xa5060 |
| `S2C_BUNDLE` (A) | 14, S->C | several inner messages in one | part A 3.4 |
| `S2C_EVAR` (A) | 15, S->C | `[name:zstr8][value:zstr8]` engine variable | engine+0xa50c0, engine+0x90830 |
| `S2C_SKY_TABLE` (A) | 16, S->C | 384-bit block + 30 object ids | engine+0x8d5a0 |
| `S2C_SHELL_OBJ_MSG` | 17, S->C | shell object message without an object | engine+0xa5140 |
| `S2C_PRELOAD` | 19, S->C | `[sub:u8]` preload control | engine+0xa45b0 |
| `S2C_FILE_TOUCH` | 20, S->C | `[file:u16]` file reference lookup | engine+0xa2b00 |
| `S2C_MODEL_CACHE` | 21, S->C | `[op:u8][file:u16]` drop / pin a cached model | engine+0xa2b50 |
| `S2C_DIR_LIGHT` | 22, S->C | direction, colour, extra float of a global light | engine+0xa31f0, engine+0x8d6c0 |
| `S2C_MODEL_ADD_CHILD` | 23, S->C | `[model_file:u16][child_file:u16]` | engine+0x9ebb0 |
| `C2S_BYE` (A) | 6, C->S | disconnect request | engine+0xa48d0 |
| `C2S_RATE` (A) | 7, C->S | `[rate_8k:u16]` + `C2S_SOUND_ACK` body | engine+0x1ae50, engine+0x90a80 |
| `C2S_SOUND_ACK` | 8, C->S | `[id:u16]*` tracked sounds finished on the client (server action PROVEN: marks the client done for tracked-sound entries only; client source: the sound manager's id packet) | engine+0x90a10, engine+0xa6f30, engine+0x1ae50 |
| `C2S_LOAD_STATE` (A) | 9, C->S | `[state:u8]` (part A) | engine+0xa45b0 |
| `C2S_CONSOLE` | 10, C->S | `[command:zstr8]`, local client only | engine+0x1a660, engine+0x92270 |
| `C2S_SHELL_MSG` | 11, C->S | wrapper of a CShell game message | engine+0x13f40 |

#### S.B.2 Records and fields

| name | meaning | evidence |
|---|---|---|
| record head | `[cf_lo:u8 != 0]([cf_hi:u8])[obj:u16]` object record, or `[0][kind:u8]` special record | engine+0x917c0 |
| `rec_bits` | u32 bit length in front of every record of `S2C_OBJ_GROUP` | engine+0x8d200 |
| special kind 0 / 1 / 3 | one-shot sound / tracked sound / removed-object list | engine+0x91dc0, engine+0x91340, engine+0x8d3e0 |
| `cf` | 16-bit change mask of an object record | engine+0x917c0, engine+0xa32f0 |
| `CF_NEW` 0x0001, `CF_POS` 0x0002, `CF_ROT` 0x0004, `CF_FLAGS` 0x0008, `CF_SCALE` 0x0010, `CF_ANIM` 0x0020, `CF_COLOR` 0x0040, `CF_ATTACH` 0x0100, `CF_POS_SNAP` 0x0200, `CF_ROT_SNAP` 0x0400, `CF_FILES` 0x0800, `CF_DIMS` 0x8000 | change mask bits (0x0080 = high-byte marker) | engine+0x917c0, engine+0xa32f0 |
| `kind_byte` | create: bits 0-5 `obj_kind`, bit 6 has blob, bit 7 blob length is 16 bits | engine+0xa3cb0 |
| `OBJ_PLAIN` 0, `OBJ_MODEL` 1, `OBJ_BRUSH` 2, `OBJ_BILLBOARD` 3, `OBJ_LAMP` 4, `OBJ_KIND5` 5, `OBJ_ZONE` 9 | object kinds (purpose PLAUSIBLE) | engine+0xa3cb0, engine+0x917c0 |
| create blob | shell object message carried by a create record | engine+0xa3cb0, engine+0xa4a40 |
| model file block | `[u6]` models; per model `[u16]`, first model `[u6]` subs, `[u6]` textures, `[u4]` styles | engine+0x90bc0, engine+0xa2700 |
| animation block | tracker list `[1]{entry}...[0]`, entry: wide bit, anim u8/u14, state bits, `tsel:u2` + `time8` | engine+0x91600, engine+0xa2530 |
| `time8` | animation time in 8 ms units, width 1 / 6 / 10 / 32 by `tsel` | engine+0x91600 |
| `uf` | 4-bit stream flags: 0x1 `UF_ANIM`, 0x2 `UF_ROT`, 0x4 `UF_POS`, 0x8 `UF_YAW` | engine+0xa4160, engine+0x8bdf0 |
| time entry | `[0xFFFF:u16][0:u4][server_time:f32]` at the end of every `S2C_OBJ_STREAM` | engine+0x8fe60, engine+0xa4160 |
| stream bits | object net word bits 0 / 1 / 2 = animation / position / rotation travel in the stream | engine+0x8bdf0, engine+0x917c0 |
| full precision flag | object `flags` bit 0x0100: raw floats in change records | engine+0x917c0, engine+0xa32f0 |
| yaw-only flag | object `flags` bit 0x0800: stream rotation as `yaw8` | engine+0x8bdf0 |
| client id table | client map of 16-bit ids, slot kind 1 = object, 2 = tracked sound | engine+0x253a0, engine+0x25370 |
| `dmask` | 3-bit difference mask of a sound description against the per-client cache | engine+0x91dc0, engine+0xa3a50 |
| `sflags` | sound flag word: bits 0-1 positional, 0x40 volume, 0x80 pitch, 0x400 start offset, 0x800 at listener | engine+0xa3a50, engine+0xa2e90 |
| `tmask` | tracked sound mask: bit 0 start, bit 1 position, bit 5 PLAUSIBLY stop | engine+0xa3070 |
| `shell_id` | first byte of a shell message, CShell id space (per direction) | cshell+0x1a86b0 |
| `fx_id` | first byte of a shell object message | cshell+0x217760 |
| shell object message | `(object or none, reader)` hand-off to CShell method +0x60 | engine+0xa4a40, cshell+0x1a3d00 |
| `ShellPartA` .. `ShellPartF` | the six handler objects of the CShell dispatcher chain | cshell+0x1a86b0 |
| player update | shell id 0x64, client -> server, unreliable, default 15 per second | cshell+0x5f7f0, cshell+0x1fa060 |
| `pu_flags` | 16-bit flag word of the player update | cshell+0x5f7f0 |
| `yaw_c8` / `yaw_c16` | shell yaw compressions, scales 254/2π and 32767/2π, rounded | cshell+0x19f390, cshell+0x19f3e0 |

### S.F Names registered by the spec folds (spec-fold-4 and spec-fold-5, 2026-10-25)

Rows are appended under the rule of the header: a rename is a new row that says what it replaces. The names are ours; the evidence column names the spec row that proves the wire fact.

| name | id / dir | meaning | replaces | evidence |
|---|---|---|---|---|
| `GS_SPRINT_ALLOWED` | shell 0xca, S->C | `[allowed:bool1]`: own record +0x1f0, the byte the client's sprint test requires; the server answers each `UP_SPRINT_STATE` change with it | `GS_OWN_FLAG` | cshell+0x2e602; PROVEN-CAPTURE reaction, `GAME_MESSAGES_2009_S2C.md` 3.2 |
| `UP_THROW_HOLD` | shell 0x4d, C->S | no fields, sent on every pass of the throw animation key while the throw is held (18 times in tdm4); the release is the following `UP_FIRE` 0x82 of the throwable | `UP_THROW_RELEASE` | region cshell+0x2ebb0; PROVEN-CAPTURE, `GAME_MESSAGES_2009_C2S.md` 4.6 |
| `match_exit` | lobby TCP 0x394, C->S | `+6 u32 phase`: phase 0 on every leave of a running match (with `UP_LEAVE_MATCH`), 0 then 1 after a failed connection | `match_join_failed` | cs+0x18b010; PROVEN-CAPTURE, `PROTOCOL_2009.md` 3.7, 7.1 |
| `OBJ_CHARACTER_REC` | shell object type 0x18 | the 572-bit character record carried by every player character's create (kind-1 object) | (none; the first draft listed players as type 0x01) | factory case 0x18; REFERENCE-CONFIRMED widths, S2C 8.2, 8.2.1 |
| `OBJ_SHOT_EFFECT` | shell object type 0x06 | shot effect relayed with engine id 17 (one per bullet, impacts) | | DISASM cshell+0x2c6770; REFERENCE, S2C 8.2 |
| `OBJ_PROJECTILE` | shell object type 0x13 | thrown grenade blob `[0x13][weapon:u8][ammo:u8][1:u8]` of a kind-1 object | | factory case 0x13; REFERENCE, S2C 8.2 |
| `OBJ_PLAYER_SOUND` | shell object type 0x19 | sound relay record `[k:u8]` then `[u16][u32][vec96]`, or for `k` 0x12 `[u8][u32][vec96][u8][u8]`; `k` 0x0f with the jump (the reading of tdm4 and dserver3; tdm2 read it as a landing) | | cshell+0x20f330; REFERENCE, S2C 8.2 |
| `yaw_c8` (correction) | shell field | the 8-bit shell yaw is the **truncation** toward zero of `wrap(yaw) x 40.425354`, not a rounding (the S.B row above says "rounded"; `yaw_c16` not checked) | the rounding reading of the S.B row | PROVEN-CAPTURE (tdm1, tdm2, tdm4), `UDP_SESSION_2009_B.md` 10.3, `GAME_MESSAGES_2009_C2S.md` 2.1.1 |

#### S.F.1 State of the renames in the crates (spec-fold-6 check of master 96983bf)

| name | crate name now | left over |
|---|---|---|
| `match_exit` (0x394) | `dc_wire::MATCH_EXIT` (name-table entry `match_exit`), `dc-lobby-server` `msg::MATCH_EXIT` (dserver-tidy L-1) | the deprecated alias `MATCH_JOIN_FAILED` is still used by `dc-botclient/src/lobby.rs` (import line 27, writer line 1056) and `dc-client/src/net.rs` (line 810); the alias goes once their owners switch |
| `UP_THROW_HOLD` (0x4d) | `dc-dserver` `UP_THROW_HOLD`, `Up::ThrowHold`, `THROW_FIRE_WINDOW_MS` "since the last hold" (L-2) | none |
| `OBJ_PLAYER_SOUND` `k` 0x0f = the jump | `dc-dserver` `FX_SOUND_JUMP` (was `FX_SOUND_LANDING`; L-3) | none (the tdm2 / tdm4 jump-versus-landing question itself stays open in S2C 8.2) |
