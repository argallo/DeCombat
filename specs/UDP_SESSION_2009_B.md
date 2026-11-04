# UDP_SESSION_2009_B: steady-state in-world session messages of the 2009 client (clean-room)

- **Title:** UDP_SESSION_2009 part B, the session-layer messages exchanged once the client is in a world.
- **Scope:** server -> client engine object messages (create, update, remove), the wrapper that carries
  shell-level game messages and how Engine.exe hands it to CShell.dll, the engine's own sound / FX messages,
  time and ping, console messages; client -> server per-frame update, the shell-to-server game message wrapper,
  periodic messages; reliability and ordering. Part A (`UDP_SESSION_2009_A.md`, written in parallel) covers
  connect -> loaded world and is referenced, not repeated. Transport (bits, blocks, encodings) is
  `UDP_TRANSPORT_2009.md`. Names are ours and are registered in `GLOSSARY_UDP.md` section S.
- **Evidence sources:** decompiler output of the 2009 EU-test `Engine.exe` (`Engine.exe/<rva>.c`,
  image base 0x00460000, cited `engine+0xRVA`) and `CShell.dll` (`CShell.dll/<rva>.c`, cited
  `cshell+0xRVA`); for a few CShell functions that the plain run did not create, the signature re-decompile
  `decompiler/decomp_sig/b0*/gc/<rva>.c` (allowed source; only case values, bit widths and slot offsets are used,
  never its identifiers); the `*.funcs.orig.tsv` tables; method-table addresses read from the user's own ROM
  binaries (PE-DATA, addresses only); the sibling clean specs `UDP_TRANSPORT_2009.md`, `UDP_SESSION_2009_A.md`,
  `GLOSSARY_UDP.md`. Since 2026-10-25 the client-to-server forms (sections 10 and 11) and the player-character object forms
  (section 3.5) are checked against captures of the real 2009 client (Appendix C; grades PROVEN-CAPTURE and REFERENCE-CONFIRMED).
- **Status:** DRAFT 1, complete for review; not implemented. Client-to-server forms: PROVEN-CAPTURE (tdm1, tdm2, tdm4, section 13.3 and Appendix C). Server-to-client forms: REFERENCE-CONFIRMED only (the reference server wrote them, the real client accepted them); the 2009 server's own forms stay unknown.
- **Date:** 2026-10-24

Evidence grades as in `UDP_TRANSPORT_2009.md`: **PROVEN** (literally visible in the decompiler output), **PLAUSIBLE**
(reconstructed where decompiler lost an argument or a `this`), **UNPROVEN** (not visible; confirm by capture or by a
decompiler re-run before relying on it). Two capture grades are added (as in `UDP_TRANSPORT_2009.md`): **PROVEN-CAPTURE** (bytes the
real 2009 client sent, or a client behaviour visible in them; 2009 evidence, never downgraded) and **REFERENCE-CONFIRMED** (a form
written by the reference server that the real client accepted; shows what the client accepts, never a rule value). Captures are cited
`OBSERVED: captures/<id>/capture.jsonl` with the capture id (tdm1 = 20261025_000014, tdm2 = 20261025_001409,
tdm4 = 20261025_002236).

## 0. Contents

1. Function map
2. Message envelope and id space
3. Server -> client: object create
4. Server -> client: object update
5. Server -> client: object remove
6. Server -> client: game-message wrapper and the Engine -> CShell hand-off
7. CShell game message id table (ids only)
8. Server -> client: engine sound / FX messages
9. Time sync, ping, console
10. Client -> server: per-frame update
11. Client -> server: game-message wrapper and periodic messages
12. Reliability and ordering
13. UNPROVEN items and decompiler re-run list

Conventions. Bit order, `uN`, `sN`, `f32`, `zstr8`, `objref16`, `pos48`, `vec72`, `yaw8`, `quat128` and the `vlen`
entry framing are exactly as in `UDP_TRANSPORT_2009.md` sections 2, 3 and 15. A **message** is one transport entry
(one reliable or unreliable entry, after split-message reassembly). "Reliable" means the message travels in the
rel block (transport section 8), "unreliable" in the unrel block (transport section 9). Engine VAs are given as
`engine+0xRVA` (image base 0x00460000); CShell VAs as `cshell+0xRVA` (image base 0x37100000). "PE-DATA" marks a
value read from the user's own ROM binary (read-only, method tables only, nothing quoted from it but addresses).

## 1. Function map

### 1.1 Engine.exe, client side (receives server -> client messages)

| role (our name) | engine RVA | grade |
|---|---|---|
| client message handler table init (`dn_table_init`), 256 slots at VA 0x6e18e0 | 0xa5230 | PROVEN |
| client message dispatcher (`dn_dispatch`): reads `msg_id:u8`, calls slot, unknown id -> fallback object at VA 0x6e1ce8 method +0x0c | 0xa52f0 | PROVEN |
| client receive loop: pull messages from every link, dispatch until one returns non-zero | 0xa53e0 | PROVEN |
| pull next message from a driver (driver method +0x78 = engine+0xb6310) | 0x7ca30 | PROVEN |
| per-link inbox pop (one inbox per link, link+0x80 head / +0x84 tail) | 0xb0b20 | PROVEN |
| object record reader: create part (`obj_create_read`) | 0xa3cb0 | PROVEN |
| object record reader: change part (`obj_change_read`) | 0xa32f0 | PROVEN |
| object record driver (create or change, then hand trailing bits to CShell) (`obj_record_apply`) | 0xa4a40 | PROVEN |
| model file block reader (`model_files_read`) / one model entry | 0xa2870 / 0xa2700 | PROVEN |
| animation block reader (`anim_block_read`) / tracker apply | 0xa2530 / 0xa2300 | PROVEN read order, PLAUSIBLE meaning (register arguments lost) |
| compact object stream reader (`S2C_OBJ_STREAM`, id 10) | 0xa4160 | PROVEN |
| removed-object list reader (group record kind 3) | 0xa2980 | PROVEN |
| one-shot sound record reader (group record kind 0) | 0xa3a50 | PROVEN |
| tracked sound record reader (group record kind 1) / its sound body | 0xa3070 / 0xa2e90 | PROVEN |
| shell message hand-off (`S2C_SHELL_MSG`, id 13) | 0xa5060 | PROVEN |
| object-less shell object message (`S2C_SHELL_OBJ_MSG`, id 17) | 0xa5140 | PROVEN |
| sub-reader from the current position to the end (wraps a packet view into a reader object, method table VA 0x679eb0, PE-DATA) | 0xa49a0, 0x5c6e0 | PROVEN |
| client id (`S2C_CLIENT_ID`, id 12) | 0xa2910 | PROVEN |
| engine variable set (`S2C_EVAR`, id 15) / variable store | 0xa50c0 / 0x22c30 | PROVEN |
| preload control (`S2C_PRELOAD`, id 19) | 0xa45b0 | PROVEN |
| file reference touch (`S2C_FILE_TOUCH`, id 20) | 0xa2b00 | PROVEN |
| model cache control (`S2C_MODEL_CACHE`, id 21) | 0xa2b50 | PROVEN |
| directional light (`S2C_DIR_LIGHT`, id 22) | 0xa31f0 | PROVEN |
| version check (`S2C_VERSION`, id 4; part A) | 0xa2a70 | PROVEN |
| handlers of ids 5, 6, 7, 8, 14, 16, 23 | 0xa2960, 0xa3e90, 0xa2a50, 0xa4ef0, 0xa3f80, 0xa44f0, 0xa2c50 (not in the function table; read in the disassembly) | DISASM (rules in part A 4.6-4.10, 3.4 and here 3.1, 8.7) |

### 1.2 Engine.exe, server side (the same Engine.exe can host; its writers show the exact server -> client bits)

| role (our name) | engine RVA | grade |
|---|---|---|
| server message handler table init (`up_table_init`), 256 slots at VA 0x6e1230 | 0x925e0 | PROVEN |
| server message dispatcher: reads `msg_id:u8`, unknown id -> queued for the server game code | 0x923b0 | PROVEN |
| per-client world tick (`world_tick_for_client`): builds one id 8 and one id 10 message | 0x8fe60 | PROVEN (writer target of two writes PLAUSIBLE) |
| object selection by priority for the id 8 message (budget half) | 0x8f360 | PROVEN |
| per-object record emit (length prefix + record) | 0x8d350, 0x8d200 | PROVEN |
| object record writer (`obj_record_write`) | 0x917c0 | PROVEN |
| model file block writer / one model entry | 0x90db0 / 0x90bc0 | PROVEN |
| animation block writer | 0x91600 | PROVEN |
| removed-object record writer (kind 3) | 0x8d3e0 | PROVEN |
| one-shot sound record writer (kind 0) + length prefix | 0x91dc0 + 0x92170 | PROVEN |
| tracked sound record writer (kind 1) + length prefix / its selection loop | 0x91340 + 0x92080 / 0x8b450 | PROVEN |
| compact stream selection (budget rest) / one entry / entry plus attachments | 0x8f8b0 / 0x8bdf0 / 0x8bfc0 | PROVEN |
| send to one client (+ optionally its attached clients) with flags | 0x90e80 | PROVEN |
| send to every client with flags / to every in-world client, reliable | 0x90e30 / 0x90f10 | PROVEN |
| leave-world notice (id 5) | 0x8ceb0 | PROVEN |
| view-object notice (id 7) | 0x8d0c0, 0x8db60, 0x8e000 | PROVEN |
| sky table (id 16) / directional light (id 22) | 0x8d5a0 / 0x8d6c0 | PROVEN |
| engine variable push (id 15) | 0x90830, 0x8ddd0 | PROVEN |
| model child add broadcast (id 23) | 0x9ebb0 | PROVEN |
| engine-generated shell messages (shell ids 0x00, 0xc1, 0xc2) | 0x9ee60, 0x9e2e0, 0x9dfe0 / 0x9e2a0 | PROVEN |
| client message handlers ids 6, 7, 8, 10 | 0x90ae0, 0x90a80, 0x90a10, 0x92270 | PROVEN |
| client message handlers ids 5, 9, 11 | 0x90b40, 0x90b20, 0x922d0 (not in the function table; read in the disassembly) | DISASM (id 5: part A 5.2; id 9: `state != 0` -> sub-state 3, else 1; id 11: section 11.1) |

### 1.3 Engine.exe, client side (sends client -> server messages)

| role | engine RVA | grade |
|---|---|---|
| send a message on a connection with flags (bit 0 = reliable, forced 0 on a local connection) | 0x7d890 -> 0x7cce0 -> 0x7bf30 | PROVEN |
| shell game message to server (`C2S_SHELL_MSG`, id 11), engine client interface slot +0x158 (method table VA 0x674b08, PE-DATA) | 0x13f40 | PROVEN |
| console command to server (`C2S_CONSOLE`, id 10) | 0x1a660 | PROVEN |
| rate and tracked-sound acknowledgements (`C2S_RATE`, id 7), called once per client frame | 0x1ae50, from 0x14f90 | PROVEN |
| disconnect request (`C2S_BYE`, id 6) | 0xa48d0 | PROVEN |
| preload-done report (`C2S_LOAD_STATE`, id 9 state 1; part A) | 0xa45b0 case 1 | PROVEN |
| hello (`C2S_HELLO`, id 5; part A) | 0x18d00 | PROVEN |
| server address query, client interface slot +0x15c | 0xd8d0 | PROVEN |

### 1.4 CShell.dll

| role (our name) | cshell RVA | grade |
|---|---|---|
| client shell object, base method table VA 0x374eaf84; derived (the instantiated one) VA 0x374d026c (PE-DATA) | 0x1aa670 (base teardown) | PROVEN |
| game message entry, method +0x54 of the derived table: records the server IPv4 + port, then calls the base dispatcher | 0x32dc0 (+0x23c4c0) | PROVEN, argument passing PLAUSIBLE |
| game message dispatcher (`shell_msg_dispatch`): reads `shell_id:u8`, builds a sub-reader, offers it to a handler chain | 0x1a86b0 | PROVEN |
| object message entry, method +0x60 (both tables): object, reader | 0x1a3d00 | PROVEN |
| object message (effect) dispatcher: reads `fx_id:u8`, switch | 0x217760 | PROVEN |
| `ShellPartB` handler with a large id switch (method +0x0c of the table at VA 0x374ce808), see 7.1 | 0x2c430 | PROVEN |

## 2. Message envelope and id space

**2.1 Envelope.** Every session message, in both directions, is `[msg_id:u8][body]`. There is no length field: the
body ends where the transport entry ends (the entry length is the `vlen` of the transport). Readers that loop "until
the end" (the id 8 record loop and its kind 3 record, 10, 19 sub-ops 2..6, client ids 7 and 8) rely on this; the kind
1 record reads one sound and does not loop (3.1).
EVIDENCE: client dispatcher engine+0xa52f0 (`read(8)`, slot call), server dispatcher engine+0x923b0 (same), loop
conditions `position < length` in engine+0xa4160, engine+0xa2980, engine+0x90a10, engine+0xa45b0, and the id 8
handler engine+0xa4ef0 (DISASM).

**2.2 Unknown ids.** Client side: an id whose slot is empty is handed to a fallback object (VA 0x6e1ce8, method
+0x0c, not decompiled; part A section 3.3 traces it to the file-list reader engine+0x3e990, which accepts only id 0x32
`S2C_FILE_LIST` and ignores everything else), and dispatch continues. Server side: an id whose slot is empty goes to
the sending client's file announcer (engine+0x923b0 -> engine+0x3f740), which accepts only the file-list answers 0x36
(`C2S_FILE_ACK`, part A) and 0x38 and ignores every other id. So in both directions **an unknown id is dropped
silently**; game traffic always travels inside the shell wrappers (ids 11 and 13). EVIDENCE: engine+0xa52f0,
engine+0x923b0, engine+0x3f740 (`read(8)` compared with 0x36 and 0x38).

**2.3 Dispatch return value.** A client handler returns 0 to continue; a non-zero value stops the client's receive
loop and is reported as an error code (for example 0x2c for a malformed object record, 1 for a bad model block).
EVIDENCE: engine+0xa53e0, engine+0xa4a40, engine+0xa2b50. Part A section 3.3 follows the code further: the error
ends the session (the client drops its server connection and shows a disconnect reason). A server must never send a
message that the client rejects; the rejection paths are listed per message.

**2.4 Server -> client ids** (client slot table at VA 0x6e18e0, filled by engine+0xa5230; senders on the server side).

| id | our name | part | body summary | client handler | server writer | reliability (writer flags) |
|---|---|---|---|---|---|---|
| 4 | `S2C_VERSION` | A | `[ver:u32 = 7][host_rate:u32]` | 0xa2a70 | 0x8e9f0 | UNPROVEN (writer arguments lost) |
| 5 | `S2C_LEAVE_WORLD` | B | empty | label 0xa2960 (not decompiled) | 0x8ceb0 | reliable |
| 6 | `S2C_LOAD_WORLD` | A | `[srv_clock:f32][world_file:u16]` | label 0xa3e90 | 0x8ddd0 | reliable |
| 7 | `S2C_VIEW_OBJECT` | B | `[obj:u16]` | label 0xa2a50 | 0x8d0c0, 0x8db60, 0x8e000 | reliable, also to attached clients |
| 8 | `S2C_OBJ_GROUP` | B | length-prefixed records (section 3-5, 8) | label 0xa4ef0 -> 0xa4a40 | 0x8fe60 | reliable |
| 10 | `S2C_OBJ_STREAM` | B | compact entries + time entry (section 4.3, 9.1) | 0xa4160 | 0x8fe60, 0x8f8b0 | unreliable |
| 12 | `S2C_CLIENT_ID` | A | `[client_id:u16][flag:u8]` | 0xa2910 | 0x8e9f0 | UNPROVEN |
| 13 | `S2C_SHELL_MSG` | B | `[shell_id:u8][shell body]` (section 6) | 0xa5060 | game code (not decompiled); engine-generated 0x9ee60, 0x9e2e0, 0x9dfe0 | engine-generated: reliable; game code: caller's choice (UNPROVEN) |
| 14 | `S2C_BUNDLE` | A/B | `{[len:u8][len bits]}` inner messages, see part A section 3.4 (DISASM grade there) | label 0xa3f80 | none found | UNPROVEN |
| 15 | `S2C_EVAR` | A/B | `[name:zstr8][value:zstr8]` | 0xa50c0 | 0x8ddd0, 0x90830 | reliable |
| 16 | `S2C_SKY_TABLE` | A/B | `[block:384 raw][count:u16 = 30][count x objref16]` (part A 4.8) | label 0xa44f0 | 0x8d5a0 | reliable |
| 17 | `S2C_SHELL_OBJ_MSG` | B | shell object message without object (section 6.3) | 0xa5140 | none found | UNPROVEN for the 2009 server; REFERENCE: always unreliable (tdm1: 140 of 140), the client accepts it |
| 19 | `S2C_PRELOAD` | A/B | `[sub:u8][...]` | 0xa45b0 | 0x8cb60, 0x8ccd0, 0x9e140, 0x9ea50 | reliable |
| 20 | `S2C_FILE_TOUCH` | B | `[file:u16]` | 0xa2b00 | none found | UNPROVEN |
| 21 | `S2C_MODEL_CACHE` | B | `[op:u8][file:u16]` | 0xa2b50 | none found | UNPROVEN |
| 22 | `S2C_DIR_LIGHT` | B | `[dir:3 x f32][color:3 x f32][extra:f32]` | 0xa31f0 | 0x8d6c0 | reliable |
| 23 | `S2C_MODEL_ADD_CHILD` | B | `[model_file:u16][child_file:u16]` | label 0xa2c50 | 0x9ebb0 | reliable, to every client |

All other ids (0..3, 9, 11, 18, 24..255) have an empty client slot (engine+0xa5230 clears all 256 slots first);
of those only 0x32 (`S2C_FILE_LIST`, part A 4.4) is used, through the fallback of 2.2.

**2.5 Client -> server ids** (server slot table at VA 0x6e1230, filled by engine+0x925e0; senders on the client side).

| id | our name | part | body | server handler | client writer | reliability |
|---|---|---|---|---|---|---|
| 5 | `C2S_HELLO` | A | `[len:u16][len bytes]` | label 0x90b40 | 0x18d00 | reliable |
| 6 | `C2S_BYE` | B | empty | 0x90ae0 | 0xa48d0 | reliable |
| 7 | `C2S_RATE` | A/B | `[rate_8k:u16]` then the id 8 body | 0x90a80 | 0x1ae50 | reliable |
| 8 | `C2S_SOUND_ACK` | B | `[id:u16]` repeated to the end (tracked-sound acknowledgements, 10.2) | 0x90a10 | no stand-alone writer found (reached through id 7) | n/a |
| 9 | `C2S_LOAD_STATE` | A | `[state:u8]` (part A 5.3) | label 0x90b20 | 0xa3e90 (label), 0xa45b0 case 1 | reliable |
| 10 | `C2S_CONSOLE` | B | `[command:zstr8]` | 0x92270 | 0x1a660 | reliable |
| 11 | `C2S_SHELL_MSG` | B | `[shell_id:u8][shell body]` (section 11) | label 0x922d0 | 0x13f40 | caller's flag bit 0 (CShell passes 1 at almost every call site) |

The server slot table holds only ids 5..11 (engine+0x925e0); besides those the server accepts only the file-list
answers 0x36 / 0x38 (part A 5.4) through the fallback of 2.2.

## 3. `S2C_OBJ_GROUP` (id 8) framing and the object create record

**3.1 Group framing.** Built once per server tick per in-world client (engine+0x8fe60, only for client state 3,
see 12.3) and sent **reliable**, and only when it carries more than its id byte (`bits > 8`).

```
[msg_id:u8 = 8] { [rec_bits:u32] [record: rec_bits bits] }*      until the end of the message
```

Every record is written into a scratch writer first and appended after its exact bit length as a u32
(EVIDENCE: engine+0x8d200, engine+0x8d3e0, engine+0x92170, engine+0x92080: `write(len, 32)` then append). The client
handler (engine+0xa4ef0, DISASM; arguments `(srv_conn, reader)`) loops while the reader position is below the end:

1. `rec_bits = read(32)`; remember the position after it.
2. `lo = read(8)`; if `lo & 0x80`, `cf = lo | read(8) << 8` (bit 7 stays set in `cf`), else `cf = lo`.
3. `cf != 0`: `obj = read(16)`, then `obj_record_apply(srv_conn, reader, obj, cf)` (engine+0xa4a40).
4. `cf == 0`: `kind = read(8)`. Kind 0: one-shot sound (engine+0xa3a50, register arguments `srv_conn`, reader); a
   non-zero result is logged and returned. Kind 1: `tmask = read(8)`, `track = read(16)`, then the tracked-sound
   reader engine+0xa3070 `(srv_conn, reader, track, tmask)`, which reads one sound and does not loop. Kind 3: the
   removed-object reader engine+0xa2980, which reads `u16` ids until the end of the **message**. Any other kind:
   error 0x2c (logged; the session ends, 2.3).
5. A non-zero record result is returned at once. Otherwise the position must equal the remembered position plus
   `rec_bits`; a mismatch returns 0x2c.

There is no sub-reader: every record reader works on the message reader itself, and `rec_bits` is only checked. So a
kind 3 record must be the last record of its message and must reach exactly the end (the 2009 writer emits it
last, 5.1), and every other record's `rec_bits` must equal the bits its reader consumes. A debug line prints the
client's server time (`srv_conn+0x18`) as a float before the loop when the log level (VA 0x6df910) is above 3.

**3.2 Record head.** The first byte tells an object record from a special record:

```
object record : [cf_lo:u8 != 0] ([cf_hi:u8] when cf_lo bit 7 is set) [obj:u16] [fields selected by cf]
special record: [0:u8] [kind:u8] [kind body]
```

- `cf` is the 16-bit change mask of section 4.1. Writer (engine+0x917c0): if any bit of `cf & 0x8F00` is set, write
  `(cf & 0xFF) | 0x80` then `cf >> 8`; else write only `cf & 0xFF`. Bit 7 of the low byte is therefore the
  "second byte follows" marker and never a field flag. The server never writes a record with `cf = 0`
  (engine+0x917c0 returns without writing), so a leading 0 byte is unambiguous.
- Special kinds: 0 = one-shot sound (section 8.2), 1 = tracked sound (8.3), 3 = removed objects (section 5).
  Kind 2 and kinds 4..255 make the client stop with error 0x2c (engine+0xa4ef0 default path, DISASM); no writer of
  them exists. Never send them.
- Client: `obj_record_apply` (engine+0xa4a40) receives `(reader, obj, cf)`; `cf` bit 0 selects create.

**3.3 Create record** (`cf` bit 0 = `CF_NEW`). Writer engine+0x917c0 (create part), reader engine+0xa3cb0.

The server first normalises a create mask: it clears `CF_FILES` (0x0800) and `CF_POS_SNAP` (0x0200), sets `CF_POS`
(0x0002), and sets `CF_FLAGS` (0x0008) when the object has any flag, user flag or extra flag word (EVIDENCE:
engine+0x917c0). The reader then adds `CF_POS_SNAP` itself (engine+0xa32f0: `if cf & 1: cf |= 0x200`).

```
[kind_byte:u8]                      bits 0-5 obj_kind, bit 6 has_blob, bit 7 blob_len is 16 bits
[create_pos: 3 x f32]               x, y, z (raw floats, always present; REFERENCE-CONFIRMED tdm1/tdm2: the reference server writes 0, 0, 0 in every create and the client accepts it, because the create always carries CF_POS)
if has_blob: [blob_bits: u8 or u16] [blob: blob_bits bits]
kind-specific part:
  obj_kind 1 : model file block (3.4)
  obj_kind 2 : [name:zstr8]                        (reader keeps at most 127 characters)
  obj_kind 9 : [name:zstr8] [code:u16]
  obj_kind 3 : [file:u16]
  other kinds: nothing
```

then the change fields of section 4.2 for the remaining `cf` bits, in the fixed order of that section.

- Writer: `kind_byte = obj_kind | 0x40 if blob_bits != 0 | 0x80 if blob_bits > 255`; the blob length field is 8 bits
  when `blob_bits < 256`, else 16 bits; the blob is appended bit-exact (EVIDENCE: engine+0x917c0). Reader mirrors it
  (EVIDENCE: engine+0xa3cb0). `blob_bits` counts bits (the reader advances its position by that many bits).
- The blob is not interpreted by the engine. It is collected into a scratch writer and, after the object has been
  added to the client world, handed to CShell together with the new object (shell object message, section 6.3).
  EVIDENCE: engine+0xa3cb0 (append), engine+0xa4a40 (`if bits != 0` -> engine+0xa49a0 -> CShell method +0x60).
  The hand-off target of the append is a lost `this` pointer: PLAUSIBLE.
- If the id `obj` is in use on the client it is released first (an object is removed, a tracked sound is stopped;
  the client keeps objects and tracked sounds in one id table, 8.3), then the new object is created: create
  replaces. EVIDENCE: engine+0xa4a40, id table lookup engine+0x253a0 / engine+0x25370.
- If adding to the client world fails the client logs it and continues; a bad model block (3.4, model count 0)
  aborts the record with code 1. EVIDENCE: engine+0xa4a40, engine+0xa2870.

`obj_kind` names (ours; the engine keeps 12 per-kind object lists on the server, engine+0x8f360 walks
`server+0x268 + 16 * k` for k = 0..11). Field-shape evidence only; the purpose column is PLAUSIBLE:

| obj_kind | our name | create extras | other kind-specific behaviour | purpose (PLAUSIBLE) |
|---|---|---|---|---|
| 0 | `OBJ_PLAIN` | none | no colour block (writer drops 0x40) | logic / point object |
| 1 | `OBJ_MODEL` | model file block | animation block, attachment masks, re-sendable model files | animated model |
| 2 | `OBJ_BRUSH` | name | none | named world geometry piece |
| 3 | `OBJ_BILLBOARD` | file id | scale has 2 components, 0x0020 carries a u64, 0x0800 resends the file id | flat textured object |
| 4 | `OBJ_LAMP` | none | colour block adds `[radius:u16]` | point light |
| 5 | `OBJ_KIND5` | none | no colour block | UNPROVEN |
| 9 | `OBJ_ZONE` | name, code u16 | no colour block | named volume with a 16-bit code |
| 6, 7, 8, 10, 11 | `OBJ_KIND6` ... | none seen | none seen | UNPROVEN |

**3.4 Model file block** (writer engine+0x90db0 / engine+0x90bc0, reader engine+0xa2870 / engine+0xa2700). All
values are 16-bit file reference ids of the session file table (part A); 0xFFFF = empty slot.

```
[model_count:u6]                         0 = invalid (reader fails the record with code 1)
repeat model_count times (entry m = 0, 1, ...):
  [model_file:u16]
  if m == 0: [sub_count:u6] then (sub_count - 1) x [sub_file:u16]      (sub_count counts the entry itself)
  [tex_count:u6]   then tex_count x [tex_file:u16]                      (at most 32; trailing empties trimmed)
  [style_count:u4] then style_count x [style_file:u16]                  (at most 8; trailing empties trimmed)
```

The reader stores every listed value into fixed arrays of 32 / 32 / 8 slots without clamping and pads the unused
slots with 0xFFFF; a count above the cap overruns those arrays (engine+0xa2700). A server must keep
`sub_count <= 32`, `tex_count <= 32`, `style_count <= 8`.
The meaning of the three lists (sub-models, textures, render styles) is PLAUSIBLE from their caps; their order and
widths are PROVEN. The writer writes `sub_count = 0` when the model has no file.

**3.5 Player character objects (V-S2C-1 settled; REFERENCE-CONFIRMED, tdm2 and tdm1).** A player is not a plain object. Every
player character created on the real client's link by the reference server is a **kind-1 model object** with a **type 0x18 shell blob**
(the 572-bit character record of `GAME_MESSAGES_2009_S2C.md` 8.2, 580 bits with the type byte) and a second kind-1 object for the
held weapon, attached at socket 2. All records below were parsed by the real client without error and the session went on
(accepted); that the client rendered them is the user's observation, not visible on the wire. All 15 character blobs of tdm2
(creates and `0xa6` sub 0x11 repeats) have the same 580 bits.

| field (3.2 / 3.3 / 4.2 order) | remote player on the viewer's link (bot, tdm2 seq 794) | notes |
|---|---|---|
| `cf` | 0x058b = `CF_NEW`, `CF_POS`, `CF_FLAGS`, marker, `CF_ATTACH`, `CF_ROT_SNAP` | no `CF_ANIM`, no `CF_DIMS`, no `CF_SCALE`, no `CF_COLOR` (the viewer's own character and a still unspawned one add `CF_ANIM` and `CF_DIMS`, below) |
| `kind_byte` | 0xc1 = kind 1, blob present, 16-bit blob length | `blob_bits` 580 |
| `create_pos` | (0, 0, 0) | not the position; the true position is the `pos48` of `CF_POS` |
| model block | 2 entries: entry 0 body model, `sub_count` 2 (the body's child model), 5 textures (two body skins, then one team-mark texture three times), 3 render styles; entry 1 face model, 1 texture, 2 styles | every count within the caps of 3.4; every file id was announced by `S2C_FILE_LIST` before the record |
| flag block | `flags` 0x3803, `flags2` 0x0318, `uflags` 0x00401020, `flags3` 0 | bit 0x0800 (yaw only) set, 0x0100 (full precision) clear: compact `pos48` / `vec72` |
| rotation | `quat128` yaw-only (for example (0, -0.7716, 0, 0.6361)) | for remote viewers only (CD-67: the owner gets none) |
| attachments | one entry: child = the weapon object, socket 2, offset (0, 0, 0), rotation (0, 0, 0, 1); end marker; then the 32 mask pairs of a kind-1 object, all zero | |

Flags life cycle (REFERENCE): a player is created before it spawns with `flags` 0x0802 (hidden form), `CF_DIMS` = (15, 50, 15), and, on its
owner's link, `CF_ANIM` with three trackers (4.4). After the joiner's 0x72 a change record adds the face model (model block of two
entries), `flags` -> 0x2802 and re-sends the attachment list; after the joiner's 0x87 a change record sets `flags` 0x3803. Reading of the
bits 0x0001 / 0x1000 (set when the player spawns) as "visible" / "in play" and 0x2000 (record complete) is PLAUSIBLE; the client's use of
them was not traced. The same sequence repeated for a re-join (id reuse, tdm2). The reference server sent the `CF_ROT_SNAP` record also to
the character's own client, which the 2009 writer rule of 4.2 forbids; DeCombat follows the 2009 rule (CD-67).

Weapon child (tdm2 seq 794, 2,629 bits): `cf` 0x05ab (`CF_NEW`, `CF_POS`, `CF_FLAGS`, `CF_ANIM`, marker, `CF_ATTACH`, `CF_ROT_SNAP`),
`kind_byte` 0x01 (kind 1, no blob), model block of one entry (the drawn weapon's third-person model, 1 texture, 1 style), an animation
block with the main tracker only (`anim` 0, `tsel` 0), flags 0x0003 / 0x0010 / 0x00000010 / 0 (0x0002 before the player spawns), the
parent's position and rotation, an empty attachment list and 32 zero mask pairs. The parent names it in its attachment list. **The weapon a
player holds is shown by a `CF_FILES` (0x0800) change record on this child** (model, texture, style of the new weapon, within 47 ms of the
player's 0x7f, to every client including the owner); the weapon child never appears in the object stream. A thrown grenade is a
further kind-1 object (blob `13 07 09 01` = type 0x13, weapon 7, ammo 9, flag 1; model with one texture and no style; scale (2, 2, 2); dims
(6, 6, 8); `pos48` with a velocity), removed by a kind-3 record when it explodes (its full create form, flight records, stream entries and blast order: 3.5.4).

Player box and files. The reference stack's half extents in every teleport (`MOVEMENT_2009.md` 12.4), in the type 0x18 record and in the dims block
equal the user dims of the stand clip of the ROM body model (the same value the 2009 client reads for its own standing box). Characters'
files are announced with single-entry `S2C_FILE_LIST` messages (ten at the player's placement: skins, team mark, three render styles,
body model, child model and the third-person model and skin of another loadout weapon; four at the introduction: face model and skin, the
drawn weapon's skin and model); every name used by a record was announced before it (part A 4.4). Many `0xa6` notes reached the client
before the create of their object and were dropped by it; send them after the create.

Differences between the bot-only reference run and the real-client captures (each REFERENCE): the remote create of an already spawned
character has no `CF_ANIM` / `CF_DIMS` (tdm2) while the owner form has both; record bytes +0x2c / +0x2d are (1, 255) before the spawn and (3, 2)
after it, completed by `0xa6` sub 0x11 (the bot run sent (3, 2) at once); byte +0x34 is 1 for the team-1 player and 0 for the team-2
player in tdm2 and the engine client id in the bot run (both readings fit every capture so far; three or more players decide);
visibility (flags 0x3803) follows the player's 0x87 in both. Proposed DeCombat rows (staged, not approved): CV2-1 (the character create
form above), CV2-8 (remove character and weapon child in one kind-3 record, in the datagram of 0x6c).

**3.5.1 Reference-run detail (bot-only run of the reference server, 60 s, two `dc-botclient` bots; REFERENCE-CONFIRMED at parse level; from the dserver-slice3 addendum, folded 2026-10-25).** Where it differs from the real-client
captures of 3.5 the real-client form decides.

- **Initial create of a character** (both bots received both characters in the initial object group): `cf` 0xa1ab = `CF_NEW` | `CF_POS` | `CF_FLAGS` | `CF_ANIM` (0x0020 and its alias 0x2000) | `CF_ATTACH` | `CF_DIMS` (3,730 bits);
  `kind_byte` 0xc1; `create_pos` (0, 0, 0) (the position travels in the position block; the reader adds `CF_POS_SNAP`); blob 580 bits (S2C 8.2.1); model block of 2 entries (body: `sub_count` 2, 5 textures, 3 styles; face: 1 texture,
  2 styles); animation block of 3 trackers, each `anim 0, state0 1, state1 1, tsel 2, time 0` (main tracker, then id 1 with weight set 0, then id 2 with weight set 1); `flags` 0x2802, `flags2` 0x0318, `uflags` 0x00401020, `flags3` 0
  (0x0800 yaw only; `flags2` 0x0100 lets the dims through); `pos48` + `vec72` 0 (no full-precision flag); one attachment (the weapon object, socket 2, offset 0, rotation (0, 0, 0, 1)), end marker, 64 zero words; dims = the player's half
  extents.
- **Files** (OBSERVED, compared by sha256 with the reference stack's names; the ROM table keys are given by section only): model 0 = the `ModelFile` of the character row, its sub file = the child model that the body file lists
  (`FMT_LTB.md` 9) in the body's folder; textures = the row's `UpperSkin<k>` and `LowerSkin<k>`, then the `TeamMark` `TeamName0` file three times (body material slots 2..4: the team mark and the two emblem pieces); styles =
  `RenderStyle0..2`; model 1 = the `ModelFile` of the face row, texture `Skin0`, styles `RenderStyle0..1`. For bots with an all-zero look and no character item the reference stack used rows `Model0`, skin variant 0, `Face0` and mark index 0 for both
  teams. After the batched `S2C_FILE_LIST` at link creation the reference stack announced the character files with single-entry lists (ten at the player's placement, four at the introduction), ids counting up from 8, names upper case with `\`
  separators; every name a record used was announced before it.
- **Weapon child:** `cf` 0x01af = `CF_NEW` | `CF_POS` | `CF_ROT` | `CF_FLAGS` | `CF_ANIM` | `CF_ATTACH` (2,629 bits); `kind_byte` 0x01; model block of 1 entry (the drawn weapon's `HHModel`, `sub_count` 1, texture `HHSkin0`,
  style `HHRenderStyle0`); one tracker `anim 0, state 1 1, tsel 0, time 0`; flags 0x0002, 0x0010, 0x00000010, 0; rotation (0, 0, 0, 1); empty attachment list + 64 zero words. The parent names it with socket 2, the body socket
  whose name key is `c969c8aa86333373/9` (socket index 2 in every ROM body that has it).
- **Spawn change records:** about 65 ms after the create (the spawn) the character got `cf` 0x468a (alias 0x4000 | `CF_ROT_SNAP` | `CF_POS_SNAP` | `CF_FLAGS` | `CF_POS`; 376 bits) with `flags` 0x3803, position and rotation, and the weapon
  `cf` 0x000a (240 bits) with `flags` 0x0003 and position; one tick later the character got `cf` 0x0002 (position, velocity 0: the follow-up of 4.2 item 6). While a player turned, the reference stack also sent reliable `cf` 0x0480
  (`CF_ROT_SNAP` only, 160 bits) about every 70-100 ms to both bots in addition to the stream's yaw; DeCombat relies on the stream (yaw8 in every entry).
- **Animation clip keys** (the ROM clip name keys, by hash): standing: tracker 1 `bc721b0c9c7aa226/7`, tracker 2 `7f007bf2ff2bcc78/3` (the stand clip of `MOVEMENT_2009.md` 9.2); walking forward (`FWD`): tracker 1
  `f9c53f256431fb05/7`, tracker 2 `5f8e2d7e3aa7a17d/3`. With the EU-test ROM these are the indices 0 / 96 / 6 and 0 / 99 / 47. The main tracker's time ran with the server time (about time modulo the clip length); tracker times restarted
  with the motion.
- **Player box:** the half extents of every teleport, of the type 0x18 record and of the dims block equal the user dims of the stand clip (key `7f007bf2ff2bcc78/3`) of the ROM body; the per-child clip override carries the same extent;
  the 2009 client reads its own standing box from that clip, so a server takes it from the same place (DSV-27).
- **World objects of the initial group** (`dc-dserver` creates the zones and flags of them since slice 4, 3.5.2; since dserver-fixes the reference stack-equivalent world set,
  DSV-37 `world`, and since dserver-round2 the sliding doors, DSV-46): 80 creates: 6 of `obj_kind` 0, 6 of kind 1 (the two characters, the two weapon models and two pickups with 116-bit type 0x17 blobs), 24 of kind 2 (named world
  geometry), 6 of kind 3 (billboards, `cf` 0x049b) and 38 of kind 9 (named volumes). The real-client captures show the same classes (230 creates in tdm1: kinds 0 x22, 1 x5, 2 x33, 3 x110, 9 x60).
- **The TDM set on the real client's link of the tdm6 world** (REFERENCE, ACCEPTED; capval-rust3 7.1, folded by spec-fold-8; the 15,673,717-byte world of tdm6 and rust3): 74
  creates at world load (seq 722 + 743): 2 flag zones (kind 0, type 0x37, 409 bits, `cf` 0x0003), 1 use-target zone (kind 0, type 0x37, 1,081 bits, `cf` 0x0483), 1 camera
  (kind 0, type 0x0d, 96 bits, `cf` 0x048b, flags 0x0100 / 0 / 0 / 0), 1 effect object (kind 0, type 0x64 + 15 bytes of text, `cf` 0x000b, flags 0 / 0 / 0x00800000 / 0), 2
  capture flags (kind 1, type 0x17 kind 2, 116 bits, `cf` 0x01a3), the two characters and their weapon children (kind 1), 12 breakable props (kind 2, `cf` 0x000b, full
  precision, flags 0x3101 / 0 / 0x1e000121 (6) or 0x14000121 (6) / 0), 7 plain world models (kind 2: six with 0x3101 / 0x0040 (translucent) / 0x00000001, 0x0a000001 or
  0x28000001, one with 0x3101 / 0 / 0x00000001: a visible, solid model of 1,138 polygons whose box spans the map), 2 sky world models (kind 2, compact position, 0x0001 / 0x0020
  / 0 / 0), 6 sprites (kind 3, `cf` 0x049b, one sprite file, flags 0x0081 / 0x0002 / 0x00000001 / 0, scale (0.5, 0.5)) and 36 fall-death volumes (kind 9, type 0x0b, 578 bits,
  all identical, container code 7, `cf` 0x000b, flags 0 / 0 / 0x00400005 / 0). Kind-2 positions are the centre of the linked world model's box, not the object's own position
  property. Never created: player and item spawns, triggers, lights, explosions, sound objects (the world's sounds travel as tracked sounds, 8.3), mode controllers, the
  start-up command, the world settings (their content travels as `S2C_EVAR`, 8.6) and the camera point.
- **Start-up removals** (REFERENCE, mode-dependent; capval-rust3 7.2): the bot's link of tdm6, joined 6 s earlier, also got 5 more world models and volumes, then flag changes at
  167.406 and **removals** at 167.422 / 167.484, before the real client joined; three of them are named by the world's start-up-command object (switch off / make non-solid),
  the other two by command texts of triggers that the mode controllers fire at their start (they swap the two large models). So the reference stack's TDM set is the classes above
  minus what the world's own start-up commands switch off; the exact command-to-object mapping and how a "switch off" becomes a removal on the wire are UNPROVEN. The flag
  words on the bot's link carried 0x0400 in every object, a bit absent on the real client's link (PLAUSIBLE "not the host / a bot", UNPROVEN).
- **The client collides with what the server creates** (SIM + PROVEN-CAPTURE; capval-rust3 7.3): in tdm6 the real client stood on floors of the large plain model in 32 of
  the 37 updates whose box overlapped its polygons; in rust3, where our server had not created it, the client's box passed through that model's walls, floors and ceilings in 37
  updates. A server must therefore create the solid world models of the set. DeCombat: DSV-37 `-worldobjects world` (dserver-fixes; on the tdm6 world 21 of 21 model-linked
  objects by name and flag words, 36 volumes, 6 sprites, 1 effect, the sky slot; PENDING in `DC_DEFAULTS.md`).
- **Sliding doors** (REFERENCE; dserver-round2 G2, folded by spec-fold-8): kind 2 with the type-0x39 blob (S2C 8.2), `cf` 0x000b, flags 0x3101 / 0 / 0x00200121 (high byte a
  per-model code, 0x14 on two of the three doors) / 0, full-precision position = the model origin (tdm4: 3 doors). No change record of a door was ever sent in tdm1 / tdm2 /
  tdm4 (no door moved): **the moving form is UNPROVEN** (V-DOOR-1: a capture in which a player walks through a door; DeCombat sends `cf` 0x0002 change records with the raw
  position and velocity per tick, DSV-46). The door open rule of the 2009 server (proximity or the use key: the doors carry a player-activate property; whether the client
  sends anything for a door was not checked) is open as well.
- In the bot-only run no weapon message of another player was relayed (the bots sent no 0x7f, 0x80, 0x81 or 0x76); the real-client captures supply the relays (S2C 8.2).

**3.5.2 World-file objects as DeCombat creates them (D, DSV-37 proposed; folded 2026-10-25 by spec-fold-5).** The 2009 server's source for these records is UNPROVEN (V-GM-2); the
forms below are the reference stack's, which the real client accepted (REFERENCE-CONFIRMED, tdm1 / tdm2, `GAME_MODES_2009.md` 4.3 / 4.4). `dc-dserver` places them in the **first object
group** of a session, with ids from the lowest free id at the first placement, after the file list announced their model and texture files (part A 4.4).

| world-file object | record | blob | grade |
|---|---|---|---|
| objective zone (bomb site, flag capture zone) | kind-0 object, `cf` `CF_NEW` \| `CF_POS`, `create_pos` (0, 0, 0), `pos48` at the object | type 0x37 (`GAME_MODES_2009.md` 4.3): `z_a` / `z_b` -1.0, the `vec72` field (`z_pos` there) = the object's box-extent property, as in the reference stack's records (so the 4.3 reading of that field as the marker position is in question: UNPROVEN), `z_team` from the object's team text through the ROM team names (else 0xfe), `z_class` 2 bomb / 7 flag (`-zoneclass own`) or the room's class (`room`, the reference stack's form), `z_value` the zone number, `z_kind` 4 / 6, `z_text` the object name | D (choice); form REFERENCE-CONFIRMED |
| capture flag | kind-1 model object: model = the object's model file, textures / styles from the ROM attachment rows of the flag look of team % 2 (`Skin0`, `Skin1`, `RenderStyle0`), one animation tracker, empty attachment list | type 0x17 kind 2 `[state = team][team][at_base 0]` (the 116-bit form of 4.4 there) | D; form REFERENCE-CONFIRMED |
| named brush (`-worldobjects all` only) | kind-2 record with the brush name and two flag words built from its properties (0x0100 \| visible 0x0001 \| ray-hit 0x1000 \| solid 0x2000; second word 0x0040 translucent / 0x0042 additive; the sky model 0x0001 / 0x0020; user word 1 when visible) | none | UNPROVEN rule (29 of 31 words agree, below) |

Not created: the kind-8 use-target zone (class 9 in the TDM world), kind-9 volumes with type 0x0b blobs (kill volumes, ladders), sprites, effect objects, doors, pickups of
kinds 3 / 5 / 6 at load. ROM-gated bit identity (TESTED, run 2026-10-25 in this fold's worktree on master e191a26 with the user's ROM and the tdm2 capture):

```
python cargo_env.py cargo test -p dc-dserver --test rom_world rom_world_objects_match -- --nocapture
tdm2 world 0ce83218ed0d3fa3: reference stack zones 5, bomb / flag zones 4 (ours equal 4), flags 2 (equal 2), brushes 31 (ours 33, same names 31, same flag words 29)
test rom_world_objects_match_the_reference_tdm2 ... ok   (1 passed, 0 failed)
```

So with `-zoneclass room` the four kind-4 / kind-6 zone blobs equal the reference stack's bit for bit, the two flag blobs and the names of their model, texture and style files equal the reference stack's (the test compares names, case-insensitive), all 31 of the reference stack's named brushes exist among our 33, and 29 of their 31 flag-word pairs agree (the brush rule stays UNPROVEN and off by default). The equality
says the DeCombat writer reproduces the accepted form; it says nothing about the 2009 server's choice. *Status after Wave Y / Z (spec-fold-8):* the default is now `-worldobjects world` (DSV-37 amendment of dserver-fixes:
model-linked objects in their start-script state, kill volumes and ladders, scaled sprites, placed effects; the volume blobs equal the reference stack's bit for bit on tdm4 / tdm6), and
sliding doors are created by DSV-46 (dserver-round2); the "Not created" list above is the slice-4 state. The real-client set these rules approximate is in 3.5.1.

**3.5.3 Start and join order (D, DSV-36).** The order of the creates relative to the shell messages is in `GAME_MESSAGES_2009_S2C.md` 10.1 (DeCombat paragraph): characters and the
3.5.2 objects are created before `S2C_VIEW_OBJECT` and before the 0x6b records that name them (3.2.1 field 5 there), the joiner's character becomes visible (flags 0x3803) after its 0x87.

**3.5.4 Thrown objects (REFERENCE, ACCEPTED; tdm2 and tdm4; answers questions Q1-Q9 of the dserver-tidy worker, spec-fold-6, 2026-10-25).** Each capture holds one grenade that the
real client threw at its own feet (tdm2: release 0x82 at 109.547 on conn 6, `dir` y -0.933; tdm4: 108.156, `dir` y -0.911). Decoded with our own readers of 3.1-4.4 (the `dc-botclient`
capture reader and a scratch record dump outside the tree; every record's length equalled its `rec_bits`); positions decoded with the box of the captures' world (`0ce83218ed0d3fa3`,
read from the user's ROM at run time). Seq numbers are the capture's datagram events; object ids 413 (tdm2) and 423 (tdm4). Velocities, speeds, the bounce and the fuse are the
reference stack's rule values: recorded to identify the form, never adopted (DSV-39 has its own).

| step | tdm2 | tdm4 | reading |
|---|---|---|---|
| to the thrower only, reliable, in the datagram of the create, before the object group | 0x66 kind 1 `[weapon 7][ammo_kind 9][ammo 0.0][slot 3][extra 1]` (seq 8110) | the same 112 bits (seq 7690) | the grenade count (S2C 4.3 row 0x66): Q9 |
| create, one reference stack tick after the release, every link | `cf` 0x85bf, `kind_byte` 0x41, `create_pos` (0, 0, 0), blob 32 bits `13 07 09 01`, model block of 1 entry (model + 1 texture, no style), animation block with the main tracker only (narrow form, `anim` 0, `state0` 1, `state1` 1, `tsel` 0, time 0), flags 0x1201 / 0x0100 / 0x00400020 / **`flags3` 0**, scale (2, 2, 2), `pos48` + `vec72`, `quat128`, no attachment entry + 64 zero mask words, dims (6, 6, 8): 2,845 bits (seq 8110 / 8111) | the same form and length (seq 7689 / 7690) | Q4 (`flags3` 0), Q5 (animation block) |
| create position | already on the floor, 51.6 units below and beside the release origin (a throw straight down lands in the reference stack's first step) | the release origin within 0.3 units | the position after the server's first step (PLAUSIBLE) |
| create rotation (x, y, z, w) | (0.009, -0.097, 0.100, 0.990) | (-0.007, 0.030, -0.057, 0.998) | a full quaternion with small pitch and roll, not the thrower's yaw-only rotation; its source is UNPROVEN (Q5, V-PROJ-3) |
| with the create, unreliable, every link | type 0x19 `k` 0x12 throw sound (S2C 8.2) | the same | |
| reliable change records during the flight | one, 31 ms after the create: `cf` 0x44a6 = `CF_ANIM` (main tracker in the wide form with `state0` 0), `CF_POS` with velocity 0 (the follow-up of 4.2 item 6), `CF_ROT` yaw-only (seq 8118 / 8119) | 5 x `cf` 0x4486 (position + velocity and rotation, 108.219-108.375; the fifth with velocity 0), 9 x `cf` 0x0484 (rotation only while it rolls, 108.375-108.672), 1 x `cf` 0x04a4 at rest (`CF_ANIM` `state0` 0 and a yaw-only rotation, 108.687) | Q6: yes |
| stream entries (id 10) during the flight | `uf` 4 (`UF_POS` only: no yaw, no animation) in every stream message until the removal, 74 per link; `has_vel` 1 in the first only, then the same position without velocity | 73 per link: 15 with a moving position and a velocity (a falling arc with one bounce), then 58 at the rest point without velocity | Q6: yes, the reference stack streamed the projectile |
| fuse end, one tick, every link | removal record `[413]` (the last record of its group, 5.1), the type 0x06 blast (S2C 8.2), 0xa6 [0x18][the thrower's character] sub 0x20 (128 bits, reliable, meaning UNPROVEN); to the hurt player 0x74 and 0x66 kind 2 | the same set (removal `[423]`) | 2.48 / 2.47 s after the create |
| order of removal and blast | the blast (unreliable) in an earlier datagram than the group with the removal: conn 6 seq 8517 (blast) < 8519 (0x74) < 8522 (0x66) < 8523 (removal); conn 5 seq 8516 < 8528 | one datagram per link (seq 8064, 8065): the reliable block (0xa6 notes, 0x74, 0x66, the group with the removal) before the unreliable block with the blast (transport block order) | Q8: the client accepted both; removal and blast need only share the tick |
| the blast's `from` / `to` | `from` = the release 0x82's `origin` bit for bit; `to` = the last streamed position within 0.9 units | the same; within 1.3 units | Q3 (`origin`, not `weapon_pos`), Q1 / Q2 in S2C 8.2 |

tdm6 (REFERENCE, ACCEPTED; aa-triage-grenades item 5, folded by spec-fold-8): the real client threw twice on conn 6, objects 263 (create 240.922 seq 9361, removal 243.406 seq 9727)
and 277 (create 326.640, removal 329.109), with **73 and 64 stream entries** (`uf` 4) and **24 and 19 reliable change records** (263: 9 x `cf` 0x4486, 14 x 0x0484, 1 x 0x04a4;
277: 8 / 10 / 1). Throw 1 in detail: the create with velocity (1476, 151, -428); in each tick 240.937-241.172 a `cf` 0x4486 record (position, velocity, rotation) and a stream
entry with the same position and velocity (the vertical velocity falling by about 54 per tick); the first bounce at 241.234 (seq 9409: stream velocity (405, 92, -118), the
record's velocity 0); `cf` 0x0484 rotation-only records while it rolled (seq 9412-9491); at rest (241.718) one `cf` 0x04a4 record (main tracker `state0` 0, yaw-only rotation)
and stream entries without velocity until the removal. The create form equals the one above. Over the four reference stack throws: 64-74 stream entries and 1-24 change records per
flight (tdm2 had one change record and 74 entries, so the stream is PLAUSIBLY the main carrier). Also once per session (REFERENCE): two grenade engine variables at the first throw
(tdm2 seq 8118 / 8119, tdm4 seq 7689 / 7690, tdm6 seq 9409 at the first bounce); whether the client reads them is UNPROVEN (row EV-G, PENDING in `DC_DEFAULTS.md`).

Not in either capture: what the 2009 client draws when a server sends the create and nothing else (DeCombat today): whether its type 0x13 object or the engine moves the grenade
by itself or it stays at the create position (UNPROVEN, V-PROJ-1); a joiner during a flight (V-PROJ-5); blob widths for ids above 255 (Q7, V-PROJ-4). First observation
since (spec-fold-8): rust4 on our stack, 3 throws with 0 stream entries and 0 change records each (PROVEN-CAPTURE of our stream) and the user's report "throwing grenade didn't
show the throw but the explosion happened": the client does **not** move the object by itself (V-PROJ-1, OBSERVED, one report; a screen recording still settles it).

DeCombat (D, DSV-39 proposed; `dc-dserver` `game/projectile.rs`, master 96983bf, TESTED in `game_sim`): the create in the form above (position = the release origin, velocity = the
launch velocity, one main tracker at time 0, rotation = the thrower's yaw); no change records and no stream entries while it flies (the server integrates the flight for the blast
point and the damage only); at the fuse the removal, then the blast; no 0x66 kind 1 count, no 0xa6 sub 0x20. The blast fields and the open differences are in S2C 8.2 (DeCombat
paragraph). *Status after Wave AA (aa-grenades merged; spec-fold-8):* every thrown object is streamed from its create to its removal by default (DSV-41 amendment, 4.3;
headless round: 74-75 entries per object on both links), and the release answer 0x66 kind 1 carries clip + stock (DSV-39 Q9 amendment; S2C 4.3.1); change records during the
flight (DSV-41b) and 0xa6 sub 0x20 are still not sent.

**3.5.5 Death and respawn of a player character (REFERENCE, ACCEPTED; tdm6 and tdm4; capval-rust3 5, aa-triage-red-dot 1.1, folded by spec-fold-8).** On the user link of tdm6
(conn 6) the user killed the bot (object 252) at 246.609; the bot respawned at 253.703.

| seq / t | record | content |
|---|---|---|
| 10126 / 246.547 | stream of 252 | tracker 1 on clip 110 (100 ms), 62 ms before the kill block. **Corrected by spec-fold-9:** clip 110 is the rifle family's still fire one-shot, not a hit reaction (the reference stack streamed no hit clip); see `UDP_SESSION_2009_B_addendum_anims.md` A1 |
| 10128 / 246.609 | shell | 0xa6 notes, 0xd6 (HiSec case), 0x6d, 0xb5; no 0x67 and no 0x6b to the killer |
| 10134 / 246.609 | change of the victim's character | `cf` 0x0188 (`CF_FLAGS` + `CF_ATTACH`): flags 0x0802 / 0x0318 / 0x00401020 / 0 (the pre-spawn hidden form of 3.5), empty attachment list |
| 10134 / 246.609 | corpse create | kind 1, `cf` 0x25bb, blob type 0x26 (219 bits, S2C 8.2), model block = the victim's body model, child model, five textures and three styles, one tracker on a death clip (clip 86, 1,766 ms; narrow form, `state1` 0), flags 0x1803 / 0 / 0x00600820 / 0, position = the victim's last position |
| 10134 / 246.609 | other creates, removal | the victim's dropped items (type 0x17 pickups, mostly parked at (-9999.9, -9999.8, -9999.9)) and the HiSec case; the victim's weapon child removed |
| 10134 .. 10964 | stream of 252 | 208 entries during the 7.1 s: the same position, the main tracker on the corpse's clip (4.3) |
| 10964 / 253.703 | respawn | change `cf` 0x2ba8 (`CF_FILES` with the model block again, `CF_ANIM` with three trackers, flags 0x3803, the attachment of a **new** weapon child 269 created in the same tick) |
| 265.422 | removal | the corpse removed 18.8 s after its create (the same for the other corpses; corpse clips 86, 87 or 88; tdm4: clip 76, removed with the pickups after 18.83 s, with 171 `cf` 0x0008 records on it in between) |

The victim got the same character change on its own link (tdm6: 0x0588 / 0x0188 with flags 0x0802 at each of the user's six deaths) plus 0x67 `[3]` / `[2]` (S2C 4.1.1).
Without these records (rust3, OURS: no flag change, no attachment change, no corpse, standing clips 0 / 96 / 6 in 211 stream entries) the real client kept drawing and
hit-testing the dead bot: 20, 6 and 1 more hit claims after three kills (PROVEN-CAPTURE), where in tdm6 it sent none until the respawn. The lifetime and the clip choice are
rule values (recorded, not adopted). DeCombat: DSV-44 (dserver-fixes, PENDING; our rust4 respawn record was `cf` 0x478a), with the stream and respawn-record amendment of
aa-triage-red-dot (PENDING, not implemented; 4.3).

## 4. Object change: reliable change records and the unreliable compact stream

**4.1 Change mask `cf`** (16 bits; writer engine+0x917c0, reader engine+0xa32f0). Names are ours.

| bit | our name | wire meaning |
|---|---|---|
| 0x0001 | `CF_NEW` | create record (section 3.3) |
| 0x0002 | `CF_POS` | position + velocity block |
| 0x0004 | `CF_ROT` | rotation block |
| 0x0008 | `CF_FLAGS` | flag block |
| 0x0010 | `CF_SCALE` | scale block |
| 0x0020 | `CF_ANIM` | animation block (kind 1) / u64 value (kind 3) |
| 0x0040 | `CF_COLOR` | colour block |
| 0x0080 | (marker) | in the low byte only: "high byte follows" (3.2); never a field flag |
| 0x0100 | `CF_ATTACH` | attachment block |
| 0x0200 | `CF_POS_SNAP` | same fields as `CF_POS` (the reader tests 0x0202); the reader sets it itself on create, so it PLAUSIBLY means "place without smoothing" |
| 0x0400 | `CF_ROT_SNAP` | same fields as `CF_ROT` (the reader tests 0x0404); meaning as above, PLAUSIBLE |
| 0x0800 | `CF_FILES` | model file block again (kind 1) / file id again (kind 3) |
| 0x1000 | (server internal) | "record already written this tick"; not meaningful on the wire |
| 0x2000 | (server alias) | the writer turns it into 0x0020; the reader tests 0x2020, so it is harmless if present |
| 0x4000 | (server alias) | the writer turns it into 0x0002 (follow-up position, 4.2 item 6) |
| 0x8000 | `CF_DIMS` | dimension block |

A change record (no `CF_NEW`) for an object id the client does not have makes the client stop with error 0x2c
(engine+0xa4a40: object lookup failure returns the error). A server must only send change records for objects whose
create record went out earlier on the same reliable channel (12.2 keeps the order).

**4.2 Change fields**, always in this order, each present only when its bits are set (writer engine+0x917c0,
reader engine+0xa32f0; both PROVEN and identical):

1. `CF_FILES` (0x0800): kind 1: model file block (3.4); kind 3: `[file:u16]`; other kinds: nothing.
2. `CF_ANIM` (0x2020): kind 1: animation block (4.4); kind 3: `[value:u64]` (low 32 bits, then high 32);
   other kinds: nothing.
3. `CF_FLAGS` (0x0008): `[flags:u16][flags2:u16][uflags:u32][flags3:u32]`. The writer clears bits 0x0024 of
   `flags`; the reader keeps its own bits 0x0024 and the upper half of its 32-bit flag word and replaces the rest.
   Two `flags` bits change the wire format of later fields: **0x0100 = full precision** (items 6 and 7 switch to raw
   floats) and **0x0800 = yaw only** (the stream sends `yaw8`, 4.3). Because this block precedes the position block,
   a flag change already applies to the same record. Meaning of the other bits: later GAME_RULES spec.
4. `CF_COLOR` (0x0040): `[r:u8][g:u8][b:u8][a:u8][extra:u8]`, and for kind 4 also `[radius:u16]` (the writer
   truncates a float, the reader stores `float(radius)`). The writer drops this bit for kinds 0, 5 and 9.
5. `CF_SCALE` (0x0010): `[sx:f32][sy:f32]` then `[sz:f32]` except for kind 3 (the reader then uses 1.0).
6. `CF_POS` / `CF_POS_SNAP` (reader 0x0202, writer 0x4202): when `flags & 0x0100 = 0`: `[pos:pos48][vel:vec72]`;
   otherwise `[pos:3 x f32][vel:3 x f32]`. There is no "has velocity" bit here. Writer rule: a record caused by a
   position change carries the current velocity and leaves a follow-up pending (0x4000); the follow-up record (next
   tick) repeats the position with velocity 0, so the client stops extrapolating. A record without a real position
   change also sends velocity 0. EVIDENCE: engine+0x917c0 (pending-flag handling around the velocity copy).
7. `CF_ROT` / `CF_ROT_SNAP` (0x0404): `[rot:quat128]` in both precision modes (compact mode writes the four floats
   through engine+0x5b3d0, full precision copies the same 128 bits raw; the reader uses engine+0x5b650 or a raw
   128-bit read). The reliable rotation is therefore always 128 bits.
8. `CF_ATTACH` (0x0100): a list, then a kind-1 extra:

   ```
   { [child:u16 != 0xFFFF][socket:u32][offset:3 x f32][rot:quat128] }* [0xFFFF:u16]
   if kind 1: [masks: 32 x (u32, u32)]          (64 words = 2048 bits)
   ```

   The writer puts only the low 16 bits of the socket value into the u32. The reader first clears the object's
   attachments, then adds one per entry (engine+0xa32f0 calls engine+0x7e230 / engine+0x7e920; argument passing
   lost, PLAUSIBLE). The meaning of the 32 mask pairs is UNPROVEN (per-part visibility bits are one candidate).
9. `CF_DIMS` (0x8000): `[dims:3 x f32]`. The writer drops it unless `flags2 & 0x0100`. The server's stream priority
   uses this vector as the object size (engine+0x8f8b0), hence our name (PLAUSIBLE).

Writer-side suppression (server policy visible in engine+0x917c0; a Rust server should copy it because the client
relies on the stream for these properties):

- The client's own view object (the object named by `S2C_VIEW_OBJECT`, client+0x60) gets no `CF_ROT` / `CF_ROT_SNAP`
  unless the client has option bit 0x08 (client+0x74): the client owns its view rotation.
- Per-object stream bits (object net word at `netinfo + 0x205e`; bit 0 = animation, bit 1 = position, bit 2 =
  rotation): a property marked for the stream is removed from change records (`CF_POS`, `CF_ROT`, and for kind 1
  `CF_ANIM`) and travels in id 10 instead. Create records still force `CF_POS`.

**DeCombat hide / show record of server-side visibility culling** (D, VIS-9 (proposed; the implementation's correction is in `DC_DEFAULTS.md`,
"visibility-design corrections", PENDING); `dc-dserver` `visibility.rs`, merged at master 7fbad44; TESTED (`vis_sim`, record layout
unit tests), RUN with `dc-botclient` (the records parse, no `parse_notes`); **not sent to the real 2009 client yet**; folded by spec-fold-7 from
`reports/dserver-visibility.md`, design and record in `docs/VISIBILITY_CULLING_DESIGN.md` 5.2 and 10):

- Form: a change record (no `CF_NEW`) of an enemy character, `cf` = 0x0202 = `CF_POS` | `CF_POS_SNAP` (low byte 0x82 with the high-byte marker, then 0x02: the
  two-byte head of 3.2), carrying only item 6: `[pos:pos48][vel:vec72]` (raw floats when the object has the full-precision flag 0x0100). Reliable, in the viewer's
  `S2C_OBJ_GROUP` of the tick, before that tick's id 10.
- Hide: `pos` = the world's park point (a point on the bottom face of the `pos48` box that the load-time census sees least from the spawn points), `vel` = 0; the
  enemy's id 10 entries to that viewer stop; the same record once more `2 x max(L, 200)` ms later (L = the lookahead of VIS-5, 100 .. 500 ms), after which no older
  unreliable entry can still leave the server (12.2 item 5). Show: `pos` / `vel` = the true values, and the stream entry follows in the same tick.
- The spawn change record (`cf` 0x468a, 3.5.1) of an enemy that a viewer cannot see at its spawn carries the park point for that viewer, and so does its weapon
  child's `cf` 0x000a; the weapon child gets no hide / show record of its own (it follows by attachment, item 8, PLAUSIBLE; V-VIS-1).
- Evidence it rests on: the reader takes item 6 for either bit (it tests 0x0202; EVIDENCE engine+0xa32f0, PROVEN). **ACCEPTED** (REFERENCE-CONFIRMED; re-read by
  spec-fold-7 from the tdm2 / tdm4 captures with the spec-fold-6 scratch record dump over our own `dc-botclient` / `dc-net-engine` readers; record heads and seq numbers
  only): change records of a character that carry the position block through `CF_POS_SNAP` reached the real client and it went on: the spawn change record `cf` 0x26a8
  (`CF_ANIM` and its alias, `CF_FLAGS` -> 0x3803, `CF_POS_SNAP`, `CF_ROT_SNAP`; tdm2 conn 6 seq 811) and `cf` 0x22a8 (tdm4 conn 6 seq 881), and `CF_POS_SNAP` records
  without `CF_POS` that carried a real velocity, `cf` 0x0280 (tdm2 conn 6 seq 2862) and 0x0680 (tdm4 conn 6 seq 2531). All of these were for the client's **own**
  character (object 189), whose streamed position the client does not adopt (4.3), so they prove the form parses, not what a remote character does with it. The same
  records reached the bot links (conn 5) as the remote-character case, read only by `dc-botclient`; the bot-only reference run's spawn record `cf` 0x468a (3.5.1) is the form
  with both `CF_POS` and `CF_POS_SNAP` (bot readers only). *Correction:* `docs/VISIBILITY_CULLING_DESIGN.md` 5.2 / VIS-9 cite `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md` 1.4
  seq 753 as a real-client case; that record (`cf` 0x2ba8) went to Bot1's link. Not shown by any capture: a 0x0202 record on a **remote** character to the real client,
  moving it by a large distance, and whether the client then places it without a slide (the "no smoothing" reading of `CF_POS_SNAP` stays PLAUSIBLE; V-VIS-1).
*Since rust3 (capval-rust3 9.5, folded by spec-fold-8):* our server sent the bot's character 17 hide / show records of this form to the real client (the report writes them by
their two head bytes, `cf` 0x0282; park point (-32767.9, -65535.8, -32767.9) or the true position; seq 710-10646) and two spawn records `cf` 0x468a with the park point; the
client parsed all of them (no session error, its 0x64 stream went on) and named the bot in hit claims only while it was shown: **ACCEPTED at parse level on a remote
character** (that part of V-VIS-1 is closed, PROVEN-CAPTURE). Whether the client placed the shown character without a visible slide is not on the wire (no playtest report of
sliding).

**4.3 `S2C_OBJ_STREAM` (id 10), unreliable.** Reader engine+0xa4160; writers engine+0x8bdf0 (entry),
engine+0x8bfc0 (entry + attached children), engine+0x8f8b0 (selection), engine+0x8fe60 (time entry).

```
[msg_id:u8 = 10] { [obj:u16] [uf:u4] [entry fields] }*            entries until the end of the message
time entry: [obj:u16 = 0xFFFF] [uf:u4 = 0] [server_time:f32]        written last by the server (section 9.1)
```

| `uf` bit | our name | fields (in this order) |
|---|---|---|
| 0x4 | `UF_POS` | `[pos:pos48][has_vel:u1]` then `[vel:vec72]` if `has_vel` (else velocity 0) |
| 0x8 | `UF_YAW` | `[yaw:yaw8]` |
| 0x2 | `UF_ROT` | `[rot:quat128]`, read **only if `UF_YAW` is clear** |
| 0x1 | `UF_ANIM` | animation block (4.4) |

- Field order on the wire: position, then rotation (yaw8 or quat128), then animation. EVIDENCE: engine+0xa4160,
  engine+0x8bdf0.
- The stream always uses `pos48` / `vec72`, whatever the full-precision flag says (engine+0x8bdf0 calls the
  compact writers unconditionally; the reader has no raw branch).
- Writer rules: `UF_POS` whenever the object's stream position bit is set; `has_vel = |v|^2 > 1e-5`; rotation only
  when it changed since the last stream send of that object (the writer caches the last sent quaternion), as
  `UF_YAW` when the object has flag 0x0800, else `UF_ROT`; `UF_ANIM` whenever the stream animation bit is set.
  Never set both `UF_YAW` and `UF_ROT`.
- REFERENCE-CONFIRMED (tdm2): the reference server streams every remote player character every tick with `uf` = 13 = `UF_POS` | `UF_YAW` |
  `UF_ANIM` (2,732 of 2,732 entries of one bot): `pos48`, `yaw8` (the object has flag 0x0800) and an animation block of **three trackers** in
  the narrow form (`state0` = `state1` = 1, `tsel` 2): main tracker `anim` 0 with no weight set; tracker id 1 with weight set 0 (upper body: its
  clip changes with the held weapon and with firing); tracker id 2 with weight set 1 (lower body: stand and move directions). Animation indices
  count the composed model's clips: the body file's own clips first, then the child model's. For the EU-test ROM the standing player uses
  0 / 96 / 6 and a player walking forward 0 / 99 / 47 (found by sha256 of the ROM clip name keys, not by name). Times grow by 32 ms per entry.
  The weapon child is never streamed. The stream rate is 29.4-29.6 messages per second on every link, the time entry last in each. The frozen
  server also streams the client's own object to it every tick (4,652 entries in tdm1); the real client did not adopt the streamed position
  of its own object (its reports stayed constant, PROVEN-CAPTURE), which supports DSV-21 / CD-67.
- **DeCombat culling** (D, VIS-2 / VIS-9 (proposed); `dc-dserver`, merged at master 7fbad44, on by default, `-visibility on|off`, `[vis]` of dc-config; TESTED,
  RUN with `dc-botclient` only): a viewer's id 10 carries no entry for an enemy character while that pair is hidden (the hide record of 4.2 parks it first);
  teammates, the viewer's own object, corpses, projectiles and world objects are never culled, and the time entry is unchanged. Record and evidence: 4.2,
  DeCombat paragraph; implementation record `docs/VISIBILITY_CULLING_DESIGN.md` 10. A DeCombat bot treats a parked enemy (decoded position on or below the
  world box's bottom face) or one without a stream entry for 1500 ms as unseen (BOT-19 (proposed), bot-vis13, RUN: 9 kills in 300 s with culling on).
- **DeCombat thrown-object stream** (D, DSV-41 (proposed; `DC_DEFAULTS.md` Wave U rows, PENDING); `dc-dserver -projectile-stream on|off`,
  **default off**; TESTED `game_sim` `a_flying_throw_is_streamed_only_when_asked`: off 0 entries, on at least 3 on each of 3 clients and the removal at the fuse; merged at master
  7fbad44): with the switch on, each in-world client gets per tick one entry per thrown object in flight, `uf` = `UF_POS` only (`pos48`, `has_vel`, `vec72`; no rotation, no
  animation), appended after the player entries (so the budget cuts it first), and one final entry with velocity 0 when the object comes to rest. The reference stack form it
  approximates: `uf` 4 entries in every stream message during the flight, plus reliable change records (3.5.4, REFERENCE, ACCEPTED); DeCombat sends no change records during a
  flight. Whether the 2009 type-0x13 client object moves the grenade by itself without any of this (and so which default is right) is V-PROJ-1 (UNPROVEN).
  *Superseded default (spec-fold-8):* `-projectile-stream` is **on** by default since aa-grenades (DSV-41 amendment, PENDING: one entry per tick from the create to the removal,
  the rest position without velocity in every later tick; the once-only rest entry is gone), on the evidence of 3.5.4 (the tdm6 counts and the rust4 observation for V-PROJ-1).
- **A dead character in the stream** (REFERENCE, tdm4 / tdm6; aa-triage-red-dot 1.2, folded by spec-fold-8): the victim's character, hidden by flags 0x0802 (3.5.5), stays in
  the stream for the whole death and its **main tracker switches to the corpse's death clip** (non-looping, `state1` 0; tdm4 object 408: clip 76 from seq 3208 at time 480,
  then 984, then held at 1336 = the clip end from seq 3363; tdm6 object 252: clip 86 from seq 10134, tracker 1 on clip 110, the rifle fire one-shot (not a hit reaction: `UDP_SESSION_2009_B_addendum_anims.md` A1, spec-fold-9)), trackers 1 / 2 running on. Our
  rust4 character kept the standing loop (clip 0, `state1` 1) in all 106 / 224 entries of two deaths (PROVEN-CAPTURE of the difference); whether that is the user's "floating
  red dot" is UNPROVEN (V-REDDOT-1). DeCombat: DSV-44 amendment (PENDING, not implemented).
- Reader rules: an unknown `obj` is parsed and ignored. `UF_ANIM` on an existing object whose kind is not 1 makes
  the reader **return immediately** (rest of the message dropped, no error). A known object is moved only if its
  new position or velocity differs from the current one by more than 0.1 units (squared distance 0.01) or it has
  pending interpolation state, and then only when the new position differs from the client-side prediction by
  more than 0.001 (squared 1e-6); the prediction comes from a run-time service at VA 0x6e1cec (methods +4, +8;
  PLAUSIBLE). EVIDENCE: engine+0xa4160.
- Budget (server policy, engine+0x8fe60 / 0x8f360 / 0x8f8b0): per tick and client the server asks the link for a
  bit budget for the frame time `dt` (seconds; link method +0x04 = engine+0xae950, DISASM) and divides it by 3; id 8
  records are added in "longest unsent first" order until the id 8 message reaches half of it; id 10 entries fill
  the rest, ordered by size x (speed/128 + 1) x (ms since last send + 1) / max(1, distance/128) from the client's
  view point, and the writer is cut back to the last whole entry when the budget is exceeded. The local
  (same-process) client gets everything unprioritised. The link budget, in bits: 0 when the link is stalled
  (link+0x148); else `B = trunc(rate_req * dt)`; after a bucket tick, `cap = (now - last_tick + trunc(dt * 1000)) *
  W / 200` and `B = min(B, cap - used - debt)` (0 when `cap <= used + debt`); then `reserve = queued_reliable_bits +
  queued_unreliable_bits + 14 * max(1, queued_reliable_bits / 8800) + 17 * max(1, trunc(dt * 5))` and the budget is
  `B - reserve`, or 0 when `B < reserve` (the queued bit counts include the `vlen` headers, link+0xb0 and link+0xc8;
  `W`, `used`, `debt`, `last_tick` as in transport 12.2-12.3). A Rust server may use any policy (DeCombat default)
  as long as datagrams stay inside the transport budget.

**4.4 Animation block** (writer engine+0x91600, reader engine+0xa2530; used by `CF_ANIM` and `UF_ANIM`).

```
for each active tracker of the model, main tracker first:
  [more:u1 = 1]
  [wide:u1]
  if wide: [anim:u14][state0:u1]       else: [anim:u8]            (narrow form implies state0 = 1)
  [state1:u1]
  [tsel:u2][time8: W bits]             W = 1, 6, 10, 32 for tsel = 0, 1, 2, 3
  if not the main tracker: [tracker:u8][wide_ws:u1][wset: u8 if wide_ws else u3]
[more:u1 = 0]
```

- The narrow form is used when `anim < 256` and `state0 = 1` (writer). `time8 = (time_ms + 7) >> 3`; the reader
  rebuilds `time_ms = time8 << 3`. `tsel` is chosen from the animation length `L` in ms: with `n = (L + 7) >> 3`,
  tsel 3 if `n >= 1024`, 2 if `n >= 64`, 1 if `n > 8`, else 0 (then `time8` is a single bit); an animation without
  data gets tsel 0 and time8 0. The reader honours any `tsel`.
- The main tracker carries no tracker id (the reader uses 0xFF) and no weight set (reader -1). `wide_ws = 1` when
  the weight set index is above 7.
- If the main tracker is inactive the writer stops at once (the block is just `[0]`); inactive secondary trackers
  are skipped.
- Reader side: before the list it marks every secondary tracker as unconfirmed, creates a missing secondary tracker
  through a run-time interface (VA 0x6e1ce4, method +0x10 then +0x88), applies each entry (engine+0xa2300), and
  afterwards runs a clean-up pass (engine+0xa24f0) that PLAUSIBLY drops trackers that were not confirmed.
- `state0` / `state1` are tracker state bits 0 and 1 (engine+0xa2300 copies them into the tracker flag byte);
  their meaning (playing / looping are the obvious candidates) is UNPROVEN. `anim` is an animation index of the
  model; `wset` an index into the model's weight sets (PLAUSIBLE).

## 5. Object remove and leaving the world

**5.1 Removed-object record** (special record kind 3 inside `S2C_OBJ_GROUP`; writer engine+0x8d3e0, reader
engine+0xa2980):

```
[0:u8][kind:u8 = 3] { [obj:u16] }*      until the end of the record
```

- At most one per id 8 message, written after the object records and the sound records of the tick.
- The writer lists the objects removed since the last tick for this client, skipping any id that already got a
  record this tick (so a re-created id is not removed afterwards).
- Reader: the client id table (8-byte entries, slot kind 1 = object, slot kind 2 = tracked sound; engine+0x253a0,
  engine+0x25370) is shared by objects and tracked sounds, so this list removes both. For each id in use: an object
  is torn down (engine+0x17cf0); a tracked sound is stopped at once (engine+0xd8e20, engine+0xdac90) unless its
  flag word (+0x4c) has bit 0x40, in which case it is left to end by itself (engine+0xa6240; PLAUSIBLE). Unknown
  ids are ignored silently.
- Re-using an id: a later create record with the same id replaces the object (3.3), so ids may be recycled as soon
  as the removal record has been queued (the reliable order keeps removal before re-creation).

**5.2 Leave world** (`S2C_LEAVE_WORLD`, id 5, reliable, empty body; writer engine+0x8ceb0): the server detaches the
client's view object and tells the client that the world is gone. The client handler (label engine+0xa2960) is not
decompiled; part A section 4.10 describes the client reaction at DISASM grade (view object reset, world unloaded,
link kept). It is the steady-state exit back into part A's flow (change of world).

## 6. Shell messages: the wrapper and the Engine -> CShell hand-off

**6.1 Wire form** (both directions use the same inner form):

```
server -> client: [msg_id:u8 = 13][shell_id:u8][shell body ...]      (S2C_SHELL_MSG)
client -> server: [msg_id:u8 = 11][shell_id:u8][shell body ...]      (C2S_SHELL_MSG, section 11)
```

The engine adds only the outer id byte. Everything after it is the bit string that the game code wrote (engine
appends it bit-exact, engine+0x13f40 on the client; the server side does the same for its game code, writer not
decompiled). There is no length, no sequence and no checksum inside the wrapper; the transport entry delimits it.

**6.2 Client path, step by step** (server -> client):

1. Transport delivers the entry; the client receive loop (engine+0xa53e0) dispatches by `msg_id` (engine+0xa52f0).
2. Slot 13 (engine+0xa5060) builds a sub-reader that starts at the bit after the id 13 byte and ends at the end of
   the message (engine+0xa49a0 -> engine+0x5c6e0, reader method table VA 0x679eb0, PE-DATA) and calls the client
   shell object's method **+0x54** with it (`shell` pointer at engine VA 0x6e1ce0, bound at run time). Then it
   releases the reader and returns 0 (never an error, whatever the shell does).
3. CShell method +0x54 of the instantiated (derived) shell table is cshell+0x32dc0: it asks the engine for the
   server address (engine client interface method +0x15c = engine+0xd8d0: IPv4 4 bytes + port u16), stores it in
   the connection manager (cshell+0x23c4c0 on the global VA 0x3759e22c: bytes at +0x11e1..+0x11e4, port at +0x11e6),
   then calls the base dispatcher cshell+0x1a86b0 with the reader (DISASM cshell+0x32dc0..0x32e03).
4. cshell+0x1a86b0 reads `shell_id:u8` (reader method +0x3c = read bits, argument 8), takes the current position
   (reader method +0x30) and makes a sub-reader from there to the end (reader method +0x20), so every shell handler
   receives `(shell_id, reader positioned after shell_id)`.
5. The pair is offered to a fixed chain; the first handler that returns non-zero consumes it (section 7.1).
   An id nobody handles is dropped silently.

The reader object handed to CShell exposes the transport encodings as methods (PE-DATA, method table VA 0x679eb0;
each slot below was matched to the decompiled wrapper and its callee): +0x58 `vec72` (engine+0x5b5d0), +0x88 second
`vec72` form (engine+0x5bce0), +0x5c `pos48` (engine+0x5c320), +0x8c second `pos48` form (engine+0x5c730), +0x60
`quat128` (engine+0x5b650), +0x6c `yaw8` (engine+0x5b990), +0x9c second `yaw8` form (engine+0x5c000), +0x90 compact
rotation `rot24`/`rot48` (engine+0x5bdd0), +0x64 `lstr16` (engine+0x5b900), +0x84 `lstr16h` (engine+0x5bc10), +0x94
`lstr16` peek (engine+0x5bf40), +0x48 `nest16` (engine+0x5b9d0), +0x20 sub-reader from a bit position
(engine+0x42b30). The remaining slots were read in the disassembly (DISASM; the full reader and writer slot
tables are in the transport addendum, section 3): +0x18 sub-reader through the reader's +0xa0, +0x24 bit length,
+0x2c set position (clamped; 0 rewinds), +0x30 current position, +0x34 bits remaining, +0x38 at end, +0x3c read `n`
bits, +0x40 64-bit read, +0x44 raw block read, +0x4c **`zstr8`** read into a buffer of given size (engine+0x80450),
+0x50 `zstr16` read, +0x68 `objref16` read (0xFFFF = none), +0x98 second `objref16` form; +0x28 and +0x70..+0x80 are
helpers that were not followed.

**6.3 Shell object messages.** A second hand-off carries an object pointer: CShell method **+0x60**
(cshell+0x1a3d00 in both tables) receives `(object or none, reader)`. It is used twice:

- After an object create record whose kind byte has the blob bit (3.3): the blob, wrapped in a reader, together
  with the newly created object (engine+0xa4a40).
- `S2C_SHELL_OBJ_MSG` (id 17): the rest of the message after the id byte, with **no** object (engine+0xa5140:
  method +0x60 called with object 0). The handler does nothing when the shell pointer is null.

cshell+0x1a3d00 then: if there is an object, calls a common-interface method on it (VA 0x375964f0 table +0x48,
arguments 3, 1, 1; meaning UNPROVEN); calls the shell's method +0xd4 (cshell+0x41c30, a getter that returns the shell
member at +0x164e0, the effect manager) and passes the pair to the effect dispatcher cshell+0x217760 with that member
as `this`, which reads `fx_id:u8` and switches (7.2); rewinds the reader to bit 0 (reader method +0x2c with 0); and
passes the pair to cshell+0x26e070 (with `this` = the shell member at +0x1a88), which reads the first byte again and
handles `fx_id = 0x64` (object required; reads a string of at most 256 bytes). So every consumer sees the same
message from its first bit, and there is no consumer before the effect dispatcher.

No engine-side reliability is attached to the blob: it travels inside the reliable create record. `S2C_SHELL_OBJ_MSG`
has no server writer in the decompiled C (UNPROVEN whether the 2009 server sends it reliable).

## 7. Shell message id tables (ids only; payloads belong to a later GAME_RULES spec)

**7.1 Server -> client shell ids** (dispatcher cshell+0x1a86b0, chain in this order). The first three stage objects
are members of the shell returned by getters of the derived shell table VA 0x374d026c (DISASM): +0xac =
cshell+0x41c00 -> shell+0xc52c (stage a, handler slot +0x24), +0xa8 = cshell+0x41c40 -> shell+0x168e8 (stage b,
slot +0x0c), +0xb0 = cshell+0x41c10 -> shell+0x16054 (stage c, slot +0x24); then three global singletons (d, e, f)
and the shell's own switch. The member classes are fixed by the shell constructor cshell+0x41820 (DISASM: +0xc52c
built by cshell+0x47e90, table VA 0x374d07ac; +0x16054 by cshell+0x5e550, table VA 0x374d327c over the base
cshell+0x1f20b0, table VA 0x374eefc4; +0x168e8 by cshell+0x2ea60, table VA 0x374ce808). The id lists are PROVEN
from the switches unless marked. Shell id 0x66 appears in stages a and c: stage a consumes it only in one branch
(the branch condition is garbled in the C), otherwise stage c sees it.

| stage | object (our name) | handler method | handler cshell RVA | shell ids handled |
|---|---|---|---|---|
| a | `ShellPartA` = the interface manager (shell member +0xc52c; class table VA 0x374d07ac, base VA 0x374ec3dc; part A 6.1) | +0x24 | 0x49a40 (common exit 0x1b90e0) | 0x36, 0x3a, 0x3b, 0x3c, 0x3d, 0x3f, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x4a, 0x4f, 0x50, 0x51, 0x56, 0x5e, 0x5f, 0x66 (one branch only), 0x68, 0x92; and through the common exit cshell+0x1b90e0 the ids 0x77 and 0x79, only while the interface flag +0x6aa4 is set |
| b | `ShellPartB` (shell member +0x168e8; class table VA 0x374ce808) | +0x0c | 0x2c430 | 0x65, 0x6b, 0x6c, 0x6d, 0x70, 0x7d, 0xa2, 0xb5, 0xbe, 0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc7, 0xc8, 0xca, 0xcb, 0xd3, 0xd5, 0xd6, 0xd7 (sub-id u8 1..5), 0xdb (sub-id u8 1, 2, 3, 4, 6, 7) |
| c | `ShellPartC` = the player object (shell member +0x16054, the only instance of its class; derived class table VA 0x374d327c over the base VA 0x374eefc4; the global VA 0x3759bf60 points to it, set by cshell+0x1f6fb0; part A 8.3) | +0x24 | 0x60d90 (derived), which always ends in the base 0x1f0470 | derived: 0x5a, 0x66, 0xb2, 0xb3, 0xb4, 0xd8, 0xd9, 0xda; base: **0x67 = player state** `[0x67][state:u8]` (slot +0xb8 = cshell+0x61870 -> cshell+0x1f97d0 writes +0x30; part A 8.3 d), 0x6a, 0x74, 0x7f, 0xa0, 0xa3, 0xa4, 0xad, 0xaf (each forwarded to its own method, slots +0xb4 .. +0xd4) |
| d | `ShellPartD` (global VA 0x3759e22c, class table VA 0x374cedc4 over the base VA 0x374f1b4c; part A calls it `conn_mgr`) | +0x24 | 0x23c570 | 0x72, 0x73, 0x9e, 0xac (forwarded to slots +0x3c, +0x40, +0x38, +0x34; DISASM jump table). 0x72 is handled without reading its body (slot +0x3c -> +0x08); 0x73 reloads the weapon table (slot +0x40 -> +0x0c); DISASM cshell+0x23c620, cshell+0x23c610 (`GAME_MESSAGES_2009_S2C.md` 11.1 C6) |
| e | `ShellPartE` (global VA 0x3759e120; derived class table VA 0x374d3194, base VA 0x374f120c) | +0x20 | 0x5ac40 (base part 0x22f690, helper 0x31490) | 0x97, 0x99 (base), 0xc9 (sub-id u8 1..8, cshell+0x31490), 0xc6 |
| f | `ShellPartF` = the derived HUD manager (global VA 0x375acdb8; derived class table VA 0x374d06dc, base VA 0x374f4c9c) | +0x38 | 0x45050 (the base slot +0x38 = 0x26b220 is `return 0` and is replaced in the live object) | 0x7e, 0x84, 0xb6, 0xb7, 0xb9, 0xbb, 0xbc, 0xcd, 0xcf, 0xd0, 0xd4 (0xd4 is handled but not consumed); `GAME_MESSAGES_2009_S2C.md` 7.1 (PE-DATA table; DISASM cshell+0x46ed1..0x46ed9, cshell+0x45083..0x45676). Correction of the earlier "none" (the first draft read the base slot) |
| fallback | the shell itself (switch in cshell+0x1a86b0) | | | 0x00 (cshell+0x1a3c80), 0x86 (cshell+0x22d1c0), 0x88 (cshell+0x1a58f0; values 2 and 3 equivalent, part A 4.11), 0x8e (cshell+0x1a5850), 0x90 (cshell+0x1a58d0), 0x9b (cshell+0x1a5870), 0xa6 (cshell+0x1a3c60) |

Notes:

- Shell id 0x9d (`0d 9d 00 00 00 00`, a u32 body of value 0) is sent by the reference server once per join, right after 0x9e, but appears in
  no stage list above and not in the fallback switch: the 2009 client drops it silently (tdm1: no reaction). Shell id 0x90 is the fallback
  switch's flag (row 0x90); the reference server sends it with `[1]` in the first block of a starting world and `[0]` after the client's 0x87
  (see `GAME_MESSAGES_2009_S2C.md` 7.2).
- Shell ids 0x00, 0xc1 and 0xc2 are also generated by the engine itself (console text and network statistics,
  section 9). A Rust server may send 0x00 as plain text; 0xc1 / 0xc2 are optional.
- Shell id 0x64 is the client -> server player update (section 10); it never appears in the server -> client chain
  above. Shell ids may be reused with a different meaning per direction; the two directions are separate id spaces.
- Stage handlers return 1 when they consumed the id and 0 otherwise (cshell+0x23c570 returns the low byte 0,
  cshell+0x22f690 returns 0 for foreign ids). Stage d and e return a byte that the dispatcher tests as a char.
- The handlers at cshell+0x49a40, cshell+0x2c430, cshell+0x60d90 and cshell+0x1f0470 come from the clean decompile;
  cshell+0x23c570 was read in the disassembly (DISASM), which confirms the case values and slot offsets that the
  first draft took from `decomp_sig` (numbers only).

**7.2 Effect ids of shell object messages** (first byte of a shell object message, 6.3).

| consumer | cshell RVA | ids |
|---|---|---|
| effect dispatcher | 0x217760 | 0x01, 0x03, 0x05, 0x06, 0x0b, 0x0d, 0x10, 0x11, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x21, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2c, 0x2d, 0x2e, 0x2f, 0x30, 0x31, 0x32, 0x33, 0x35, 0x36, 0x37, 0x39, 0x3c |
| second consumer (after rewind) | 0x26e070 | 0x64 (needs an object) |
| (shell method +0xd4 is a getter of the effect manager, the `this` of the effect dispatcher; it consumes nothing, 6.3) | 0x41c30 | DISASM |

Several effect ids share one handler body in the switch (for example 0x03 / 0x13 / 0x1c / 0x24, and the group
0x05 ... 0x39 that falls through to one case); the grouping is visible in cshell+0x217760 and belongs to the effect
spec. Particle systems, decals and similar visuals of the 2009 client are PLAUSIBLY all reached this way: the
engine has no effect message of its own (8), so ClientFx.fxd can only be driven from CShell.

## 8. Engine-handled sound, light and resource messages (server -> client)

The engine itself handles sounds (records inside `S2C_OBJ_GROUP`), a global directional light, a sky table,
engine variables and model / file cache control. It has **no** particle or visual-effect message of its own: every
effect reaches the client as a shell object message (section 6.3) and is interpreted by CShell / ClientFx.
EVIDENCE: the client slot table (engine+0xa5230) has no further ids; the effect ids live in CShell (7.2).

**8.1 Sound description cache.** Both sound record kinds describe a sound through a small per-client cache keyed
by the sound file id. Server: engine+0x91dc0 / engine+0x91340 look the client's entry up (engine+0x8c010) and compute
a 3-bit difference mask `dmask` against it (engine+0x90f60), updating the entry; with no entry the mask is 7.
Client: engine+0xa3a50 / engine+0xa2e90 look up the same entry (engine+0x1a210) and copy every field that `dmask`
carries into it; with no entry a zeroed temporary is used. Fields not carried come from the cache.
Rule for a Rust server: either mirror this cache exactly per client, or always send `dmask = 7` (DeCombat default
proposal; always correct, costs a few bytes).

**8.2 One-shot sound** (special record kind 0 in `S2C_OBJ_GROUP`; writer engine+0x91dc0 framed by
engine+0x92170, reader engine+0xa3a50):

```
[0:u8][kind:u8 = 0][sfile:u16][dmask:u8]
if dmask & 1                      : [sflags:u16]
if dmask & 2                      : [prio:u8]
if (sflags & 3) and (dmask & 4)   : [outer_radius:u16][inner_ratio:u8]
if sflags & 0x40                  : [volume:u8]                      (absent: 100)
if sflags & 0x80                  : [pitch:f32]                      (absent: 1.0)
if (sflags & 3) and not (sflags & 0x800): [pos:pos48]
[tag:u32]
```

- `sflags` here is the value after the cache update (the reader tests the cached word). Bits 0-1: positional
  sound (either bit); 0x0040 volume present; 0x0080 pitch present; 0x0800 "at the listener": the client uses its
  own view object's position instead of a wire position. Other bits are passed to the sound system unchanged
  (meaning UNPROVEN). The reader ORs 0x0200 into its local copy for one-shot sounds.
- `inner = inner_ratio * outer_radius / 255` (reader constant 0.003921569); `outer_radius` is a plain integer
  distance. `tag` is copied into the play request (meaning UNPROVEN).
- Server rules (engine+0x91dc0): a sound flagged "at the listener" is sent with 0x0800 only to the client that owns
  the listener object; every other client gets 0x0800 cleared, bit 1 set and the sound position. A positional
  sound whose position is farther from the client's view point than twice `outer_radius` is not sent to that
  client at all. `prio` is a per-sound byte; `volume` a byte from the sound; `pitch` a float.
- A failed play is logged by the client and returned as an error code of the record (engine+0xa3a50), which
  makes the client stop processing the message (2.3). PLAUSIBLE that a missing sound file triggers it.

**8.3 Tracked sound** (special record kind 1; writer engine+0x91340 framed by engine+0x92080, selected each tick by
engine+0x8b450; reader engine+0xa3070 with body engine+0xa2e90). A tracked sound has an id in the **same 16-bit id
space as objects** (client id table, 5.1) and can be moved or removed later.

```
[0:u8][kind:u8 = 1][tmask:u8][track:u16]
if tmask & 1:                                  (start or restart)
   [sfile:u16][dmask:u8]
   if dmask & 1                    : [sflags:u16]
   if dmask & 2                    : [prio:u8]
   if (sflags & 3) and (dmask & 4) : [outer_radius:u16][inner_ratio:u8]
   if sflags & 0x40                : [volume:u8]                    (absent: 100)
   if sflags & 0x80                : [pitch:f32]                    (absent: 1.0)
   if sflags & 0x400               : [start:u8], and if start = 0xFF then [start:u32]   (milliseconds)
   [tag:u32]
if tmask & 2:  [pos:pos48]                     (position update)
```

- `tmask` bit 0 (start): the client releases whatever uses id `track` (object or sound), then starts the sound,
  `start / 1000` seconds into the file; with 0x0800 the position follows the client's view object. Bit 1
  (position): moves the playing sound (or sets the start position). Bit 5 (0x20): the client calls a sound
  method (engine+0xa6490) on the existing sound; PLAUSIBLE "stop". Other bits UNPROVEN. EVIDENCE: engine+0xa3070.
- Server (engine+0x91340): `tmask` is the per-client pending state of that sound (bit 0 until the start went out,
  bit 1 when its position changed). For a sound attached to an object, "at the listener" is resolved as in 8.2 and
  the object position is copied into the sound position when it is not the listener (sets bit 1).
- Removal of a tracked sound: through the removed-object record (5.1), because the id table is shared.
- REFERENCE-CONFIRMED (tdm1): 19 tracked-sound records (kind 1, `tmask` 3), `dmask` 7 on the first use of a sound file and 0, 4 or 5 on later uses;
  tracked ids are reused after removal (id 0x19d removed at 169.812 s, started again at 170.359 s); no one-shot (kind 0) record occurred in
  the whole session (kind 0 undecided).
- **World sounds at world load** (REFERENCE, ACCEPTED; aa-triage-sounds 3, folded by spec-fold-8): on every SNOWVALLEY link of four reference stack captures (tdm4 c6 seq 807 and
  c5 seq 228, tdm2 c5 seq 223, tdm1 c5 seq 448, menu-frida6 c3 seq 270) the id 8 message with the world-load creates carries, after the creates and before the stream entry,
  **11 tracked-sound records** (`tmask` 3), one per switched-on sound object of the world file, in the world file's order backwards, with track ids from the object id space
  (214 down to 194 in steps of 2 in tdm4); the three sound files were announced in the world-load `S2C_FILE_LIST` (tdm4 c6 seq 152) next to the world. Field rules read off the
  11 pairs (all 11 agree): `sflags` = 0x0130 | (0x0001 if the object is ambient, else 0x0002) | (0x0004 if it loops) | (0x0040 if its volume is not 100), the bits 0x0010 /
  0x0020 / 0x0100 constant (0x0020 UNPROVEN; `AUDIO_2009.md` 3.2 for the others); `prio` = the object's priority; `outer_radius` = its outer radius; `inner_ratio` =
  **trunc**(255 x inner / outer) (159.375 -> 159; a rounding writer would differ); `volume` sent only when it is not 100; `tag` 0; `pos` = the object's position; `dmask` 7 on
  the first use of a file, then 0 / 4 / 5 (8.1). None of the 11 was removed or acknowledged; the pitch form (0x0080) was not observed (pitch 1.0 everywhere). Example (tdm4 c6
  seq 807, the looping wind): `000103d6000800073501001027ff00000000` + `pos48`. Our rust4 round sent no tracked sound (0 records on both links: the user's "no ambient
  sounds", PROVEN-CAPTURE of the absence). DeCombat: DSV-56 / DSV-57 (aa-sounds, merged; ROM + capture test: our 11 records equal the reference stack's on 5 links; PENDING).
- **Pain, fall and bounce sounds** (REFERENCE; aa-triage-sounds 3.3): the reference stack also started a tracked sound at the victim's position with damage messages (`sflags` 0x0132,
  `prio` 2, outer 500, ratio 17; tdm4 c6 seq 2968, 3132, 6695 (fall), 8065) and one per grenade bounce (`prio` 1, outer 1000, ratio 17; seq 7690, 7715, 7765). DeCombat:
  DSV-58 (proposed, not implemented).

**8.4 Directional light** (`S2C_DIR_LIGHT`, id 22, reliable; writer engine+0x8d6c0, reader engine+0xa31f0):

```
[msg_id:u8 = 22][dir: 3 x f32][color: 3 x f32][extra:f32]
```

- Writer: `dir` is the rotated +Z axis of a server-side light object's rotation `q = (x, y, z, w)`:
  `(2(w y + z x), 2(z y - w x), 1 - 2(y^2 + x^2))`; `color` is the light's (r, g, b) multiplied by two scalar
  properties of the light (brightness x scale); `extra` is a further float of the light (server light data +0x24), the **convert-to-ambient fraction** (EVIDENCE since
  spec-fold-6, `RENDER_2009.md` 6.3.1: the model light builder engine+0x1b21b0 inserts the sun record with it as the share that goes to the ambient). Sent
  when the client's "light changed" option bit 0x200 is set (engine+0x8fe60 clears it after the call).
- Reader: hands the three vectors to the client world interface (VA 0x6e1ce4, methods +0x144, +0x14c, +0x154).
  Meaning: the **global (sun) light** of the model lighting, EVIDENCE since spec-fold-6 (the client reads +0x148..+0x160 back in engine+0x1b21b0 and adds one directional
  record when `model_sun` is on and the colour is not black; `RENDER_2009.md` 6.3.1). The server writer's source is "the global light object" the game module registers
  (engine+0x9c030; option bit 0x200 set on every client when a new one is registered).
- REFERENCE-CONFIRMED (tdm2 seq 271 / 742 / 17353, tdm4 seq 278 / 807): the reference server sent one id 22 per link, reliable, during the join, and the real client accepted it; its
  floats equal exactly the values the DeCombat rule DSV-40 / REN-11 computes from the world's light object (`RENDER_2009.md` 6.3.1). DeCombat server: DSV-40 (proposed; not yet
  implemented on master 96983bf, assigned to the dserver-light worker).

**8.5 Sky table** (`S2C_SKY_TABLE`, id 16, reliable; writer engine+0x8d5a0; client handler label engine+0xa44f0
not decompiled; part A 4.8 names it and gives the client side at DISASM grade, including `count > 30` -> result 1):

```
[msg_id:u8 = 16][params: 384 bits raw][count:u16 = 30][count x objref16]
```

The 384-bit block is copied raw from server memory (48 bytes, PLAUSIBLY 12 floats) and the 30 ids come from a
fixed server table. Steady state: re-sent whenever the client's option bit 0x10 is set (engine+0x8fe60 clears the
bit after the call), for example after the server changed its sky objects.
Block layout (REFERENCE + ROM, two worlds; capval-rust3 3.2, folded by spec-fold-8; formerly UNPROVEN): the 12 floats are the two boxes of one sky world-model object,
`pos - dims`, `pos + dims` (floats 0..5, the outer sky box) and `pos - dims x inner`, `pos + dims x inner` (floats 6..11, per axis), where `dims` is the object's sky-dims
property and `inner` its three inner-fraction properties; the table lists that object's id and 0xFFFF in the other 29 entries (tdm6 world: [251, 29 x 0xFFFF], block
(5120, 0, -6656, 5632, 512, -6144, 5375.744, 255.744, -6400.256, 5376.256, 256.256, -6399.744); tdm4 world: [1, 29 x 0xFFFF], centred on (-8192, -3584, -7936)). With two sky
objects of sky index 0 (tdm6 world) the reference stack listed the one with the higher file index (one case; which one a server should list is UNPROVEN). The reference stack sent it after the
preload lists and before `S2C_VIEW_OBJECT` (tdm6 seq 701, tdm4 seq 788) and created both sky world models as kind 2 with flags 0x0001 / 0x0020 / 0 / 0 (3.5.1). The client's
camera reads floats 6..11 (`RENDER_2009.md` 4.4). Without a table and sky objects (rust3) the user saw the sky "very oddly" (OBSERVED; PLAUSIBLE: portals that nothing is
drawn into). DeCombat: DSV-45 (dserver-fixes; bit-identical on both worlds; PENDING).

**8.6 Engine variables** (`S2C_EVAR`, id 15, reliable; writers engine+0x8ddd0 (all variables at world load, part A)
and engine+0x90830 (one variable, broadcast to every client on change); reader engine+0xa50c0 -> engine+0x22c30):

```
[msg_id:u8 = 15][name:zstr8][value:zstr8]
```

The client creates or updates the named variable in its engine variable table and also stores `atof(value)` as its
number. Variable names and values are server configuration. What the client reads (folded by spec-fold-8 from capval-rust3 3.3, aa-triage-world-physics items 2 / 10 and
dserver-round2 G3):

- **The world-settings set** (REFERENCE form, ROM values; client readers DISASM): of the 116 distinct names the reference stack sent (138 messages in all, the same name set in tdm4 and tdm6;
  among them 21 server-configuration variables right after the link, tdm6 seq 211, and 108 in the three datagrams before `S2C_LOAD_WORLD`, tdm6 seq 676-678, tdm4 seq 746-750), 33 are the
  properties of the world's world-settings object under the client's names (colour and wind as components; the table is `CAPTURE_rust3_RUSTSTACK_VALIDATION.md` 3.3), and all 33
  values equal the ROM property on both worlds (66 of 66). At world entry the client copies each into a console value (cshell+0x1a6900 through cshell+0x1a6840; cshell+0x1a9b00
  for the sky-portal and wind values). The client names that differ from the property names exist as NUL-terminated strings in CShell.dll's data (all 16 of them; found by
  hash on the ROM of this machine).
- **Value text:** `%f` for reals and colour / vector components, `1` / `0` for booleans, decimal for integer properties, the text for text properties (tdm6: the 33 values
  against the property types of the ROM world).
- **A variable the server did not send reads 0** (EVIDENCE: the engine's server-variable float read, client slot +0x134 = engine+0xd760, stores 0 into the output before the
  lookup and returns not-found; aa-triage-world-physics). With no `S2C_EVAR` at all (rust3, rust4) the client therefore ran fog off with near / far 0, sky fog 0,
  `all_sky_portals` 0, and a step height of 0 (`MOVEMENT_2009.md` 2). *Superseded:* `CAPTURE_rust3_RUSTSTACK_VALIDATION.md` 3.3 had graded the zero default PLAUSIBLE and read
  the step height as "absent and 30 behave the same"; corrected there.
- **The step height** `h:eda10d3a6476dd56/15` = `30.000000` (REFERENCE: tdm1 conn 5 seq 395, tdm4 conn 5 seq 159 and conn 6 seq 750, tdm6 conn 5 seq 166 and conn 6 seq 678, each
  before `S2C_LOAD_WORLD`); reader cshell+0x1fe6c0 (`MOVEMENT_2009.md` 2, 8.1). The engine's float read has exactly three CShell callers (cshell+0x1a6840, cshell+0x1a9b00,
  cshell+0x1fe6c0), so none of the reference stack's other server-configuration variables (hit-box radii, speeds, waits and similar; rule values, not reproduced) is read through it; two
  speed multipliers among them have no client reader at all (other read paths were not enumerated; capval-rust3 3.3 lists literal uses of a few more names).
- DeCombat: DSV-47 (the 33 world variables; `-evars world|off`, `-evar <name>=<value>`) and DSV-53 (the step height; `-stepheight`), both before `S2C_LOAD_WORLD`, equal to
  the reference stack's on tdm4 / tdm6 (ROM + capture tests; PENDING in `DC_DEFAULTS.md`).

**8.7 Model and file cache control** (part A uses the same messages while loading; listed here because the 2009
server can also send them in a running world):

- `S2C_PRELOAD` (id 19, reliable): `[sub:u8]`; sub 0 = clear (engine+0xa45b0 case 0), sub 1 = answer with
  `C2S_LOAD_STATE(1)` `[9][1]`, subs 2 / 3 / 4 / 5 / 6 = a list of `[file:u16]` to the end: preload model, mark file
  in use, load file, load sound file, preload cached model. In a running world the server sends sub 2 or 6 with one
  file id when a model is first used (engine+0x9e140). Details belong to part A.
- `S2C_MODEL_CACHE` (id 21): `[op:u8][file:u16]`; op 0 = drop the cached model of that file, op 2 = load it and
  pin it (engine+0x8a7e0 with flag 1); any other op = error 0x2c (stops processing). Server writer not found
  (UNPROVEN when it is sent).
- `S2C_FILE_TOUCH` (id 20): `[file:u16]`; the client looks the file reference up (engine+0x259d0, create flag 0).
  Server writer not found; purpose UNPROVEN.
- `S2C_MODEL_ADD_CHILD` (id 23, reliable, to every client; writer engine+0x9ebb0, called from engine+0xd44d0):
  `[model_file:u16][child_file:u16]`: add a child model database to a loaded model. Client handler engine+0xa2c50
  (DISASM): looks both ids up in the client model cache (object at VA 0x6e08b0, engine+0x66350); when either is
  missing it records error text 0x2c and logs, but returns 0, so the session continues; otherwise it adds the child
  (engine+0x66a00). The handler always returns 0.

## 9. Time, ping and console

**9.1 Server time (time sync).** Every `S2C_OBJ_STREAM` message ends with the time entry
`[0xFFFF:u16][0:u4][server_time:f32]` (engine+0x8fe60). Because that entry alone makes the message longer than 8 bits,
the server sends one id 10 message to every in-world client on every server tick, even when no object moved: this
is the periodic time signal. `server_time` is the server's game time in **seconds** as f32 (engine global
`server + 0x4fc`, the same value that `S2C_LOAD_WORLD` carries; advanced by `clamp(frame_ms * 0.001, 0, 0.2) *
time_scale` each server frame and reset to 0.0 at world start, engine+0x9e4e0 and engine+0xa12f0, PROVEN). Client
(engine+0xa4160, time reader engine+0xa2a10): the value is accepted only if it is greater than the stored server time
(so a late, reordered unreliable message cannot move time backwards), and the client sets its expected update period
to `1 / U` seconds, where `U` is an engine integer console variable at VA 0x6cb0fc (PE-DATA initial value 10; its
variable-table entry is VA 0x6cb410, the name is ROM text). The 2009 CShell sets `U` to **60** on a fresh profile:
cshell+0x1a97b6..0x1a9809 (DISASM) reads a second console variable that marks the setting as done and, unless it
equals 1.0, runs two console lines that set the marker to 1 and the variable behind `U` to 60 (that the console line
updates the global through the variable table is PLAUSIBLE, as for the rate variables of transport 5.3). The time
entry may appear anywhere in the message for the reader; the 2009 server always writes it last.

The server tick rate is not fixed by the wire. The in-process 2009 host runs one server frame per client frame
(engine+0x198f0 -> engine+0x9e4e0, PROVEN), so it has no fixed rate; the dedicated 2009 server is not part of the ROM.
DeCombat default proposal: a fixed world tick that matches the client's expected period, 60 per second, with the
budget of 4.3 limiting the bytes (the earlier proposal of 10 per second assumed the PE-DATA value of `U`).

**9.2 Ping.** There is no session-level ping message. Round-trip time is measured by the transport (ack latency,
`UDP_TRANSPORT_2009.md` section 12) and by the out-of-band echo (transport 5.5). The client can query the server
address but not a session ping (engine client interface slot +0x15c, engine+0xd8d0).

**9.3 Network statistics (debug only).** When the server debug variable at VA 0x6df8c0 is non-zero (PE-DATA: 0 by
default), the server sends two engine-generated shell messages reliably to every in-world client after the tick:
shell id 0xc2 `[13][0xc2][tx_bits:u32][tx_count:u32][tx_fails:u32]` (engine+0x9dfe0 / engine+0x9e2a0;
the server's send counters since the last report, kept by engine+0x90e80, then reset) and, per client, shell id 0xc1
`[13][0xc1][client_id:u16][8 x u32]` (engine+0x9e2e0; the eight words are a 36-byte per-client statistics record,
engine+0x7b670). CShell handles both in the game-mode handler (7.1). A Rust server need not send them.

**9.4 Console.**

- Server -> client text: shell id 0x00, `[13][0x00][text:zstr8]`, reliable, to every client (engine+0x9ee60:
  printf-style, at most 511 characters). CShell handles shell id 0 in its own fallback switch (cshell+0x1a3c80):
  it reads at most 49 characters and only formats them into a local buffer, so there is no visible effect (DISASM
  cshell+0x1a3c97 `push 0x32`, cshell+0x1a39f0; `GAME_MESSAGES_2009_S2C.md` 11.1 C5; replaces "PLAUSIBLE: console print").
- Client -> server command: `C2S_CONSOLE` (id 10), `[command:zstr8]`, reliable (engine+0x1a660). The 2009 server
  executes it only for a client that has option bit 0x02 (the local, same-process client; set when the client
  object is created with the local flag, engine+0x8e9f0) and ignores it otherwise (engine+0x92270). A Rust server
  must ignore remote console commands (DeCombat default) unless an operator feature explicitly enables them.
- Engine variable push: section 8.6.

## 10. Client -> server: the per-frame update

**10.1 What the engine sends per frame.** Nothing about movement. Once per client frame the engine runs its input
update (engine+0x14f90): it compares the 255 local command bits with the previous frame and hands the changes to
the shell locally (engine+0x24e90; no wire traffic), then calls `C2S_RATE` (10.2) when the server connection exists
and its flag at connection+0x6c is set (meaning UNPROVEN; PLAUSIBLY "connected"). Player movement, view rotation and
weapon state are sent by CShell as a shell message (10.3).

**10.2 `C2S_RATE`** (id 7, reliable; writer engine+0x1ae50, server handler engine+0x90a80):

```
[msg_id:u8 = 7][rate_8k:u16][id:u16]*            ids to the end (same body as C2S_SOUND_ACK)
```

- `rate_8k = clamp(rate_bps / 8000, 0, 65535)` with `rate_bps` the client's configured link rate (engine global
  VA 0x6cb298, PE-DATA initial 35840; see transport 5.3 and part A 5.5). The server stores `rate_8k * 8000` as the
  client's rate and flags its settings as changed when the value differs.
- Sent only when `rate_8k` differs from the last value sent, or when the client has acknowledgements pending (a
  packet of u16 ids kept by the client sound layer: engine+0x1ae50 copies it from `engine+0xd8e20() + 0xa88`); if
  the rate changed the client also re-applies it to its own link (connection method +0x0c). In a running world with
  nothing pending this message is not sent at all.
- Server: after the rate, the remaining bits are processed exactly as `C2S_SOUND_ACK` (engine+0x90a10): for each id
  whose server id-table entry is a tracked sound (slot kind 2, 8.3) and whose per-client state bit 0 is clear, the
  sound object marks that client as done and decrements its pending-client counter (engine+0xa6f30; only for sounds
  with flag bits 0x428 set and 0x4 clear); at zero the sound is released for reuse. PLAUSIBLE meaning: "these
  tracked sounds have finished on this client". PROVEN-CAPTURE (tdm1): `[7][640][id]` (`07 80 02 9e 01`) is sent about 0.67 s after the start of a tracked sound that the server
  had not removed (two cases, 672 ms each; the server removed each 31-47 ms later); sounds the server removed earlier were never acknowledged,
  nor were 11 tracked sounds started at 139.906 s that were never removed (PLAUSIBLY looping).

**10.3 Shell player update** (shell id 0x64 inside `C2S_SHELL_MSG`, **unreliable**). Builder: virtual method +0x100
of the player object (stage c of 7.1). The derived class (table VA 0x374d327c, the class of the only instance)
implements it as cshell+0x5f7f0, the base class (table VA 0x374eefc4) as cshell+0x1fa060 (PE-DATA; no direct call of
either exists), so **the 2009 client sends the cshell+0x5f7f0 layout**. Both functions are read in the disassembly
(DISASM); field widths and order below are DISASM, meanings PLAUSIBLE.

Send rule (cshell+0x5f7f0): nothing is sent unless the player state (+0x30, set by shell 0x67, 7.1) is **1**. If the
state byte at +0x80 changed since the last send, a separate **reliable** form `[0x64][pu_flags:u16 = 0x0010]
[state:u8]` is sent first (flags 1). The unreliable form is then built only when the connection request kind
(`conn_mgr+4`) is not 3, or when more than `1 / R` seconds of client time (engine client interface method +0x6c,
see 10.4) have passed since the last send, where `R` comes from a client variable (its method +0x5c) and defaults
to **15.0** when the variable is missing. For a remote server (kind 3) the update is therefore limited to `R` per
second. It is sent with flags 0 (unreliable); afterwards the send time is stored and the flag word is reset.

Unreliable form (cshell+0x5f7f0; field order DISASM, meanings PLAUSIBLE):

```
[shell_id:u8 = 0x64][pu_flags:u16]
view block, one of:
  pu_flags & 0x0020 : [yaw_c8:u8][v: 3 x f32]
  pu_flags & 0x0040 : [yaw_c16:u16]                         (only if 0x0020 is clear)
  pu_flags & 0x0080 : [q:quat128]                           (only if 0x0020 and 0x0040 are clear; writer +0x4c)
if pu_flags & 0x0060 : [w: 6 x u32]                          (two 3-float vectors, +0x40c .. +0x420)
if pu_flags & 0x0001 : [offset: 3 x f32]                     (difference of two world positions, raw 96 bits)
if pu_flags & 0x0100 : [q2:quat128]                          (rotation built from +0x48 .. +0x50; writer +0x4c)
if pu_flags & 0x0200 : [value:f32]                           (a weapon-related float)
if pu_flags & 0x0400 : [vec: 3 x f32]                        (raw 96 bits)
[state:u32]                                                  (always; the mover control word, see below)
trailing block from cshell+0x1fed20 (10.4)
```

- `state:u32` is the mover control word (`move_mgr`+0x24, the `mover+0x24` of `MOVEMENT_2009.md` 5.1: FWD 0x1, BACK 0x2,
  JUMP 0x10, CROUCH 0x20, RUN 0x200, ...). Grade PROVEN for the identity (same global and offset, `GAME_MESSAGES_2009_C2S.md`
  2.1), PLAUSIBLE for the meaning of single bits. Both rotation fields (`q`, `q2`) are `quat128` (writer +0x4c, DISASM).
- `v` = three floats of the controller (controller+0x68..+0x70); `q` = a rotation the client builds from `v`
  (PLAUSIBLY Euler angles to quaternion). `yaw_c8 = round(yaw(q) * 40.425354)` (= 254 / 2π, the `yaw8` scale) and
  `yaw_c16 = yaw(q) * 5215.03` (= 32767 / 2π), with `yaw(q) = atan2(2(z x + w y), 1 - 2(x^2 + y^2))`.
  EVIDENCE: cshell+0x19f390, cshell+0x19f3e0 (plain decompile). **PROVEN-CAPTURE (tdm1, tdm2, tdm4): `yaw_c8 =
  trunc(wrap(yaw(q)) * 40.425354)`, toward zero, with the yaw wrapped into (-pi, pi] first; this is the same truncation as the
  engine's `yaw8` writer.** It matched in 2,338 of 2,338 (tdm1), 1,361 + 483 (tdm2) and 1,103 (tdm4) updates; rounding disagrees in
  1,381 of the 2,338 (tdm1). The earlier reading "round(...)" (DISASM of the plain decompile) was wrong. `yaw_c16` was never sent (flag
  0x0040 never set); its conversion is presumably the same truncation (UNPROVEN).
- Flag sources: 0x0020 / 0x0040 are chosen only when a client variable is above 0.0 (positive; 0x0040 when controller+0x19c is set,
  else 0x0020; `GAME_MESSAGES_2009_C2S.md` 12.4); 0x0001 when controller+0x19d is set; 0x0004 / 0x0008 when a mode value toggles (0x0008 when the new
  mode is 1 or 4); 0x0010 is the reliable form above; 0x0100 when the view rotation differs from the last one sent;
  0x0200 when the weapon float changed; 0x0400 when a tracked vector changed; 0x0800 when either of two counters is
  non-zero; 0x1000 and 0x2000 from two further state tests (no extra field in this variant). The flag word is reset
  to 0 after each send.
- The base-class implementation cshell+0x1fa060 (DISASM) is shorter: the same head; view block 0x0020 = `[yaw_c8:u8]`
  only (no `v`), 0x0040 as above, 0x0080 = `[q:quat128]`; then `[u8]` (an angle compressed by the shell) when
  `pu_flags & 0x0060`; for `pu_flags & 0x0001` three u16 (an offset compressed with scale 100) instead of three
  floats; no 0x0100 / 0x0200 / 0x0400 blocks; its `state:u32` is masked to bit 0x20 unless a caller flag is set; then
  the same trailing block, sent with flags 0. The 2009 client's player object is of the derived class, so this form
  is not sent; a Rust server may still accept it.

**Capture results for 10.3 (PROVEN-CAPTURE; client bytes of tdm1, tdm2, tdm4; meanings in `GAME_MESSAGES_2009_C2S.md` 2.1 and
`MOVEMENT_2009.md` 12.2).** (a) Every one of the real client's unreliable updates parses with the derived layout above to its last bit
(2,338 + 1,361 + 483 + 1,103 messages; 768-1,024 bits): the base-class variant never appears, and the field order is exact. (b) The rate is 15
per second whether the player moves or stands (14.86-14.87 per second, 65-70 ms apart on the trailer clock); one reliable state form
`[0x64][0x0010][1]` is sent right before shell 0x87 (state byte 1); nothing is sent before 0x67 arrives or after 0xbd. (c) `pu_flags` always
contains 0x0001, 0x0020 and 0x1000; 0x0100 appears in 63% of the updates (when the view rotation changed), 0x0400 in 56%, 0x0200 in 21%, 0x2000
during repeat fire and while a throwable is held; 0x0004, 0x0008, 0x0040, 0x0080, 0x0800 never. (d) The view block `v` is `(0, body_yaw, 0)`
(pitch and roll always 0) with the body yaw unwrapped and growing without bound; the six aux words are `[view_pitch:f32][view_yaw_offset:f32]`
(offset within +-pi/4 of the body yaw) then four zero words; `q2 = Ry(body_yaw + offset) * Rx(view_pitch)` (1,472 of 1,472), forward = rotated +Z,
positive pitch looks down. (e) The 0x0001 offset is the first-person eye point minus the body position: (0, 33, 0) standing, (0, 20, 0) crouched
(about 0.2 s transition); the 0x0400 vector is the absolute camera point. (f) `state:u32` equals the mover control word (the identity of 10.3 is
now observed): FWD 0x1, BACK 0x2, CROUCH 0x20, LEFT 0x80, RIGHT 0x100, RUN 0x200, **FIRE 0x400 while the fire key is held (not an aim bit)**, SPRINT
0x400000; JUMP 0x10 never appears in a sent word (the bit lives for one mover frame; jumps travel as shell 0x76). (g) Observed once: a mouse
yaw delta of about 2^31 units drove the unwrapped yaw to 2.4 million radians (tdm2) and froze the client's fine yaw control; a server may repair
it with `GS_VIEW_ANGLES` kind 0 (`GAME_MESSAGES_2009_S2C.md` 4.2, proposed CV2-5).

**10.4 Trailing block** (cshell+0x1fed20, clean decompile, inputs DISASM): exactly
`[a:u8][b: 96 bits][c: 96 bits][t:f32][d:u8][e:u8][f:u64]` (312 bits); **nothing follows**, the function returns
after `f`. Sources (`this` = the object at player+0x10): `a` = byte +0x04; `b` = three f32 from engine client
interface method +0xa0 for the object handle at +0x08 (PLAUSIBLY that object's position; zero when the handle is 0);
`c` = three f32 returned by cshell+0x1fd5b0, or zero when the handle is 0 or either of two game-shell flags
(VA 0x37597e50 object, +0x1a08 / +0x1a09) is set; `t` = engine client interface method +0x6c (engine+0xe520,
DISASM: returns the f32 at client manager +0xa28, PLAUSIBLY the client time in seconds); `d` = byte +0x60; `e` = byte
+0x50; `f` = the 64 bits at +0x48 (low word first). PROVEN-CAPTURE (tdm1) for the meanings: `a` = `move_seq`, the sequence byte of the last
mover teleport (`0xa0` flag 0x0020) echoed (1 in the first updates, 2 after the second teleport); `b` = the body position; `c` = the body velocity
(positions agree with the mean velocity times dt); `t` = the client's own clock in seconds (138.197 at capture time 139.953 while the server's
stream time was about 0; a server must use differences only; the 0x82 `time_ms` uses the same clock); `d` = `on_ground` (1 / 0); `e` = surface
code (71, 10, 20, 23 on the ground, 0 in the air, 200 on a ladder); `f` = the standing ids as two u32: -1 / -1 while airborne, otherwise a
high word constant for the world model (0x5d in tdm1) and a low word varying with the floor polygon. Field widths and order stay as above.

**10.5 Rule for a Rust server.** Treat shell 0x64 as the player's input / view stream: unreliable, may be lost or
arrive out of order, rate about 15 per second, and never trust it for anything the client should not control
(server-side validation is a GAME_RULES matter). The 4-byte reliable form must be accepted as a state change.

## 11. Client -> server: the shell message wrapper and other periodic traffic

**11.1 `C2S_SHELL_MSG`** (id 11). Engine client interface slot +0x158 (engine+0x13f40) is CShell's "send to server":

- Arguments: a finished shell message (a bit string that starts with `shell_id:u8`) and a flag word.
- Refusals: no server connection -> result 0x49; null message -> result 0x3c (nothing sent).
- Wire: `[11][shell message bits]`; sent through engine+0x7d890 with the caller's flags. Flag bit 0 = reliable;
  on a local (same-process) connection the engine always sends unreliable because nothing can be lost
  (engine+0x7bf30). Returns 0 when queued, 1 when the send failed.
- Server: slot 11 (engine+0x922d0, DISASM) returns at once when the client or the server game interface (global
  VA 0x6e1634) is missing; otherwise it wraps the rest of the message (from the current position, just after the id
  11 byte, to the end) in a reader object (engine+0x5cf20) and calls the game interface's method +0x2c with
  `(client, reader)`. A Rust server strips the id 11 byte, reads `shell_id:u8` and dispatches to its own game rules.

Shell ids at CShell send sites (all reliable except 0x64, unreliable, 10.3). The first scan found 29 ids: 0x33, 0x60,
0x64, 0x68, 0x72, 0x76 (with a sub-id byte), 0x7f, 0x82, 0x89, 0x8b, 0x96, 0x98, 0xa8, 0xac, 0xad, 0xae, 0xba, 0xbd,
0xbe, 0xc3, 0xc5, 0xca, 0xcc, 0xcd, 0xce, 0xd1, 0xd2, 0xd5, 0xd8. The re-derived inventory of
`GAME_MESSAGES_2009_C2S.md` 1 (DISASM, 85 send sites) adds 20 ids: 0x4d, 0x59, 0x61, 0x69, 0x6e, 0x71, 0x75, 0x80,
0x81, 0x83, 0x84, 0x85, 0x87, 0x8c, 0x99, 0xa7, 0xa9, 0xb0, 0xb8, 0xdb; with the 29 this gives 49 ids in total. Their payloads are in `GAME_MESSAGES_2009_C2S.md`.

**11.2 Periodic client -> server traffic in a running world.**

| what | how often | block | evidence |
|---|---|---|---|
| shell 0x64 player update | up to `R` per second (default 15) | unreliable | cshell+0x5f7f0, cshell+0x1fa060 |
| `C2S_RATE` | only on rate change or pending sound acknowledgements (checked every frame) | reliable | engine+0x14f90, engine+0x1ae50 |
| transport ack / keep-alive | every `max(200, L)` ms | ctl + empty rel block | transport 11.1 |

No other periodic engine message exists on the client side (the engine's client -> server ids are 5..11 only).

## 12. Reliability and ordering

**12.1 Summary.**

| direction | message | block | evidence |
|---|---|---|---|
| S -> C | 5, 6, 7, 8, 15, 16, 19, 22, 23 | reliable | writer flags 1 (sections 2.4, 3-8) |
| S -> C | 10 (object stream + time) | unreliable | engine+0x8fe60 (flags 0) |
| S -> C | 13 engine-generated (0x00, 0xc1, 0xc2) | reliable | engine+0x90e30 / engine+0x90f10 |
| S -> C | 13 from game code, 17, 20, 21, 14 | UNPROVEN for the 2009 server (writers not decompiled); REFERENCE (tdm1, tdm2): shell messages on both blocks (0xa2 and most 0xa6 unreliable, the rest reliable), id 17 always unreliable; the real client accepts both | |
| S -> C | 4, 12 (part A) | UNPROVEN (writer arguments lost) | engine+0x8e9f0 |
| C -> S | 5, 6, 7, 9, 10 | reliable | client writers (flags 1) |
| C -> S | 11 | caller's flag: shell 0x64 unreliable, almost everything else reliable | 11.1 |

The mapping "flag bit 0 = reliable block" is PROVEN up to the driver and DISASM for the last step: engine+0x7bf30
keeps bit 0 only for non-local connections and passes it to the driver's send method (driver table slot +0x74,
engine+0xb5990), which calls the link's queue method engine+0xb2300: a non-zero flag byte appends the message to the
reliable queue (link+0xa4; count link+0xac; queued bits including the `vlen` header link+0xb0), a zero flag byte to
the unreliable queue (link+0xb4, with its enqueue time at link+0xbc; count link+0xc4; bits link+0xc8). The 2009
writer flags never use other bits.

**12.2 Ordering rules a Rust implementation must keep.**

1. Reliable messages of one link are delivered in send order (transport 8.1). So creates, changes, sound records,
   removals and every other reliable message keep their order across ticks. A change record must never be
   delivered before the create of its object (4.1); sending both on the reliable channel in the right order is
   sufficient.
2. Unreliable messages carry no sequence. An id 10 entry may arrive before the create of its object or after its
   removal; the client ignores it (4.3). Late, duplicated or reordered id 10 messages are harmless because positions
   are absolute and the time entry is monotonic (9.1).
3. Within one datagram the reliable block is parsed before the unreliable block, and both feed the same per-link
   inbox (link+0x80, engine+0xb2c20 appends there; engine+0xb0b20 pops), so reliable entries of a datagram are
   dispatched before its unreliable entries (PLAUSIBLE: the reliable path appends through a lost `this` pointer).
   Across datagrams there is no order between the two blocks: a stream entry may overtake a reliable record that is
   being resent.
4. The server builds the tick's id 8 message before its id 10 message (engine+0x8fe60) and queues both at once, so in
   the normal case they leave in the same datagram with id 8 first.
5. Unreliable messages that wait longer than `2 * max(L, 200)` ms in the sender queue are dropped (transport 11.4);
   the time entry of a dropped id 10 is simply lost.
6. Shell messages inherit the order of the block they travel in. Game code that needs ordering must use the
   reliable flag.

**12.3 Gating on the server.**

- `S2C_OBJ_GROUP` / `S2C_OBJ_STREAM` are built only for clients in state 3 (in world) and are skipped while the
  client's connection reports an ack stall (connection method at slot 0 = engine+0xabd90, DISASM: the link is waiting
  for an ack and `now - wait_start > 90000` ms; engine+0x8fe60 logs and returns).
- Engine-generated reliable broadcasts go to every client (engine+0x90e30) or to in-world clients only
  (engine+0x90f10, used for the statistics messages).
- `S2C_VIEW_OBJECT` is sent to the client and to every client attached to it (spectators / child clients;
  engine+0x90e80 with the "also attached" argument 1).

## 13. UNPROVEN items and the decompiler re-run list

**13.1 Re-read list** (status after the disasm pass; every entry was read in the disassembly):

| module | RVA | status |
|---|---|---|
| Engine | 0xa4ef0 | DISASM: the loop of 3.1 (records on the message reader, `rec_bits` checked, kinds 2 and 4..255 -> 0x2c) |
| Engine | 0xa2960, 0xa3e90, 0xa2a50, 0xa3f80, 0xa44f0, 0xa2c50 | DISASM: ids 5, 6, 7, 14, 16 as in part A; id 23 in 8.7 (B23) |
| Engine | 0x90b40, 0x90b20, 0x922d0 | DISASM (B2, B19) |
| Engine | 0xb5990 (driver slot +0x74) | DISASM: flag byte -> reliable or unreliable queue (B20) |
| Engine | 0xae950 (link slot +0x04) | DISASM: budget in bits (B6) |
| Engine | 0x40c90 .. 0x40cc0, 0x415c0 .. 0x41650, 0x41d70, 0x41db0, 0x5c5a0, 0x5c890 | DISASM: reader slots (B8; +0x04 .. +0x10 are shared with the writer tables and were not followed) |
| Engine | 0xe520 (client interface method +0x6c) | DISASM: returns the f32 at client manager +0xa28 (B18) |
| Engine | message writer method table | located: VA 0x679e48 (base VA 0x677f68); +0x4c = `quat128` (B16) |
| Engine | fallback object at VA 0x6e1ce8, method +0x0c | DISASM: engine+0xcff20 -> engine+0x3e990 on the file announcer (part A 3.3 confirmed) |
| CShell | 0x41c00, 0x41c10, 0x41c40 | DISASM: stage mapping fixed (B10, B11) |
| CShell | 0x41c30 | DISASM: getter of the effect manager, not a consumer (B9) |
| CShell | 0x26b220 | DISASM: returns 0 for every id (B11) |
| CShell | 0x5f7f0, 0x1fa060, 0x23c570 | DISASM: read directly; `decomp_sig` is no longer needed for them (B12, B15-B17) |

**13.2 Open questions (resolve by capture or by the re-run).**

1. id 8: closed for the loop (3.1); kind 2 is not a record kind (the client rejects it with 0x2c).
2. id 14 (bundle, part A 3.4), 17, 20, 21: who sends them and on which block.
3. Reliability of game-code shell messages server -> client (writer not in the ROM).
4. `CF_POS_SNAP` / `CF_ROT_SNAP` versus `CF_POS` / `CF_ROT`: identical fields; whether the client applies them
   differently (snap versus smooth) is not visible.
5. Meanings of the flag words (`flags`, `flags2`, `flags3`, `uflags`), the 2048-bit model masks, `extra` bytes,
   sound `tag`, tracked-sound `tmask` bit 5.
6. Closed: server game time is in seconds (9.1). The 2009 dedicated server's tick rate is not in the ROM; the
   in-process host ticks once per client frame and the client expects 60 updates per second after the CShell's
   first-run setting (9.1).
7. Shell player update: closed for the encodings (`quat128` for writer +0x4c), the trailing block's extent and its field meanings
   (10.3, 10.4; PROVEN-CAPTURE). Open: the 16-bit yaw form, flags 0x0004 / 0x0008 / 0x0040 / 0x0080 / 0x0800 (never set in any capture).
8. Closed: the per-tick budget is in bits (4.3).
9. New: the meaning of player-state values other than 1 (shell 0x67, 7.1) and of the stage-a branch that consumes
   shell 0x66.

**13.3 Validation plan** (black-box, same tap setup as `UDP_TRANSPORT_2009.md` section 18; results below the table).

| id | rule | how to verify | pass condition |
|---|---|---|---|
| V-B1 | 2.4, 3.1 | decode the first in-world reliable entries | entries start with 8; records parse as `[u32 len][record]` to the end |
| V-B2 | 3.2, 4.1 | every object record | `cf` low byte bit 7 iff a second byte follows; no record with `cf = 0` |
| V-B3 | 3.3, 3.4 | creates of the player models | kind byte 1, model block counts within caps, `create_pos` equals the first `pos48` within one cell |
| V-B4 | 4.3, 9.1 | every unreliable entry | starts with 10, entries parse to the end, the last entry is `[0xFFFF][0][f32]`, the f32 never decreases |
| V-B5 | 5.1 | kill a bot / leave a pickup | a kind 3 record lists the id; a later create may reuse it |
| V-B6 | 10.3, 10.4 | client -> server unreliable entries while moving and turning | `[11][0x64][u16]...` at about 15 per second; fit `yaw_c8` / `yaw_c16`, decode the writer +0x4c fields and the trailing block |
| V-B7 | 8.2, 8.3 | fire a weapon, stand near a looping sound | kind 0 / kind 1 records; `dmask = 7` on first use of a sound file, smaller later |
| V-B8 | 6, 7.1 | game events (kill message, round end) | `[13][shell_id]` with ids from table 7.1 |
| V-B9 | 12.2 | tap delays reliable datagrams by 500 ms | stream entries for not-yet-created objects are ignored, no client error |
| V-B10 | 2.3, 4.1 | (Rust server only) send a change record for an unknown id | client stops with error 0x2c (negative test, run once) |

**Results (captures tdm1, tdm2, tdm4; Appendix C).** V-B1, V-B2, V-B4, V-B5: pass, REFERENCE-CONFIRMED (6,893 records, 4,652 streams in tdm1, 0 errors; all 5,194 / 2,732 / 972
object-group messages of tdm2 decode with every `rec_bits` equal to the bits read; removal of a leaver's character and weapon child in one
kind-3 record, and id reuse, seen). V-B3: **fails as written** for the reference stack (`create_pos` is (0, 0, 0) in every one of 230 creates); replacement pass condition: "kind byte 1,
model block counts within caps; `create_pos` is 0 or equal to the first position within one cell; the first `pos48` lies inside the world box";
the client accepts a zero `create_pos`. V-B6: pass, PROVEN-CAPTURE (layout, rate, `yaw_c8` truncation, trailer). V-B7: pass for the acknowledgement
rule in `C2S_RATE` (PROVEN-CAPTURE) and the kind-1 cache (REFERENCE-CONFIRMED); kind 0 undecided. V-B8: pass, REFERENCE (all observed server shell ids are in table 7.1
except 0x9d). V-B9, V-B10: undecided (need delay injection and our own server).

## Appendix S. Glossary rows for `GLOSSARY_UDP.md` section S (part B)

See the rows appended to `GLOSSARY_UDP.md` section S under the heading "S.B" (same names, same evidence).

## Appendix Z: evidence notes of the items folded in from the disasm-pass addendum (2026-10-24)

Each item below was applied into the text above; this list keeps its grade / evidence paragraph so that the
rva citations stay graded. Grades are those of the source addendum, unchanged.

- **B1. Function map 1.1: the client handlers that were labels**
  DISASM; per-handler notes in B4 (id 8) and B23 (id 23), and in the part A addendum A20 (ids 5, 6, 7, 14, 16).

- **B2. Function map 1.2: the server handlers that were labels**
  DISASM: engine+0x90b40 (`len = read(16)`, buffer of `len` bytes, raw read of `len * 8` bits, flags |= 0x80), engine+0x90b20 (`neg al; sbb; and 2; inc` -> 1 or 3 into client+0x70), engine+0x922d0 (B19).

- **B3. Section 2.1: which readers loop to the end**
  PROVEN (clean tree a3070.c has no loop) and DISASM (engine+0xa4ef0).

- **B4. Section 3.1: the client's id 8 loop**
  DISASM: engine+0xa4ef0..0xa5052 (read widths 32, 8, 8, 16; the `jns` on bit 7; the `cmp eax, ecx` of the consumed length against `start + rec_bits`; `mov eax, 0x2c` on mismatch and in the default kind path). PROVEN: the callees' argument lists and loops (clean tree a4a40.c, a3070.c, a2980.c).

- **B5. Section 3.2: undefined special kinds are fatal**
  DISASM.

- **B6. Section 4.3: the per-tick budget**
  DISASM: engine+0xae950..0xaea46 (multipliers 0x51eb851f with shifts 6 = /200, `imul eax, eax, 0xe`, `shl ecx, 4` plus `add ecx, eax` = *17, the f32 5.0 at VA 0x67846c and 1000.0 at VA 0x675270); the queued-bit counters from engine+0xb2300 (reliable: link+0xa4 queue, count +0xac, bits +0xb0; unreliable: link+0xb4 queue, count +0xc4, bits +0xc8).

- **B7. Section 6.2 step 3: where the server address goes**
  DISASM.

- **B8. Section 6.2: the reader slots that were not decompiled**
  DISASM (wrappers listed in the transport addendum item T5).

- **B9. Section 6.3: method +0xd4 is a getter, not a consumer**
  DISASM: cshell+0x1a3d00..0x1a3d4f (the two pushes before `call [edx + 0xd4]` are the stack arguments of the following `call 0x37317760`, whose `this` is the getter's return value), cshell+0x41c30 (`lea eax, [ecx + 0x164e0]; ret`), the shell constructor cshell+0x41820 (member +0x164e0 built by cshell+0x2071b0, table set to VA 0x374cf084).

- **B10. Section 7.1: the stage mapping is resolved**
  DISASM for the getters and the constructor; PE-DATA for the method tables (`+0x24` of VA 0x374d07ac = cshell+0x49a40, `+0x0c` of VA 0x374ce808 = cshell+0x2c430, `+0x24` of VA 0x374d327c = cshell+0x60d90).

- **B11. Section 7.1 table: corrected stage rows**
  PROVEN: clean tree 49a40.c, 1b90e0.c, 2c430.c, 60d90.c, 1f0470.c, 1f97d0.c, 1a86b0.c, 5ac40.c. DISASM: cshell+0x23c570 (byte table VA 0x3733c5d0 and dword table VA 0x3733c5bc), cshell+0x26b220 (`xor eax, eax; ret 8`), cshell+0x61870, cshell+0x1f6fe1.

- **B12. Section 7.1 notes: `decomp_sig` no longer needed for stage d**
  DISASM.

- **B13. Section 7.2: no hidden first consumer**
  DISASM (B9).

- **B14. Section 9.1: time unit, the client's expected rate, tick rate**
  PROVEN (clean tree 9e4e0.c, a12f0.c, a4160.c, 198f0.c); DISASM for engine+0xa2a10 (`comiss` against `srv_conn+0x18`, then `1.0 / U` into `srv_conn+0x24`) and for the CShell console lines (strings at VA 0x374eac30 / 0x374eac40 and the variable name at VA 0x374eac58, not quoted); that the marker persists in the user's configuration is PLAUSIBLE.

- **B15. Section 10.3: who builds the update, and when**
  DISASM: cshell+0x5f80b (`cmp [esi + 0x30], 1; jne end`), cshell+0x5f8b5..0x5f93b (the reliable form: `write(0x64, 8)`, `write(0x10, 16)`, `write([esi + 0x80], 8)`, interface method +0x158 with flags 1), cshell+0x5f96f..0x5f9da (the time test against `1.0 / R`, 1.0 at VA 0x374cd8c0, 15.0 at VA 0x374ce220, only when `[conn_mgr + 4] == 3`), cshell+0x5ff9d..0x5ffcd (flags 0, store of the send time, flag word cleared). PE-DATA: slot +0x100 of both tables.

- **B16. Section 10.3: the unreliable layout with `quat128`**
  DISASM: the writer calls at cshell+0x5fc1c (+0x24, 16 bits), 0x5fc58 / 0x5fc7d (+0x24, 8 bits; +0x2c, 0x60 bits), 0x5fcb3 (+0x24, 16 bits), 0x5fcdc (+0x4c), 0x5fd03..0x5fd85 (six +0x24, 32 bits), 0x5fee6 (+0x2c, 0x60 bits), 0x5ff12 (+0x4c), 0x5ff43 (+0x24, 32 bits), 0x5ff61 (+0x2c, 0x60 bits), 0x5ff75 (+0x24, 32 bits), 0x5ff80 (trailing block). Writer method +0x4c = engine+0x414e0 -> engine+0x5b3d0 (four 32-bit fields x, y, z, w), transport addendum T5.

- **B17. Section 10.3: the base-class form is not what the client sends**
  DISASM (cshell+0x1fa060..0x1fa5ef) and PE-DATA (slot +0x100 of VA 0x374eefc4).

- **B18. Section 10.4: the trailing block is complete**
  PROVEN for the write sequence (clean tree 1fed20.c); DISASM for the stack slots (cshell+0x1fed27..0x1fee22) and for engine+0xe520 (`mov eax, [0x6dd228]; fld dword ptr [eax + 0xa28]; ret`).

- **B19. Section 11.1: the server's id 11 handler**
  DISASM: engine+0x922d0..0x92359.

- **B20. Section 12.1: flag bit 0 -> reliable queue, last step**
  DISASM: engine+0xb59eb..0xb59ee (`call 0x512300` with the flag), engine+0xb2300..0xb23c5.

- **B21. Section 12.3: "connection reports trouble"**
  DISASM.

- **B22. Section 13.1: status of the re-run list**
  Editorial summary.

- **B23. Section 8.7: the id 23 handler**
  DISASM: engine+0xa2c50..0xa2d6c (both error paths jump to the common exit with `xor eax, eax`).

- **B24. Section 13.2: updated open questions**
  Editorial summary of B1-B23.

## Appendix O: reference stack notes (spec-fold-3, 2026-10-24)

Source: `UDP_REFERENCE_FINDINGS_botclient.md` items O1-O13 (the evidence record; runs and captures are cited there). Grade used here:
**REFERENCE-CONFIRMED** = the reference server stack (a second, independent writer or reader, NOT the 2009 server) behaves as the spec text
says when `dc-botclient` talks to it; it is black-box evidence under REFERENCE_TESTING.md section 1, distinct from PROVEN (2009 code or
captures). **Reference stack differs** = the reference stack does something else; the base text and its grade stand and the 2009 evidence decides (a capture
of the real 2009 client). No base grade was changed by this appendix and no reference stack value is a DeCombat rule value. No ROM names are quoted.

| item | spec place | note | state |
|---|---|---|---|
| O2 | 2 id table, 3 object group, 4.3 stream | the engine ids seen in a 60 s run (4, 6, 7, 8, 10, 12, 13, 15, 16, 19, 22, 0x32) arrived with the meanings of the table and were parsed by the bot's reader without a record error; the unreliable stream (id 10) ended every message with the time entry | REFERENCE-CONFIRMED (parse level) |
| O5 | 3 object group, split reliable messages | the initial object group (about 80 create records) arrived split over 12 blocks, reassembled in order and parsed with no record error | REFERENCE-CONFIRMED |
| O12 | 9.1 time entry, B14 | **Reference stack differs / open:** one `S2C_OBJ_STREAM` per tick at about 29.7 per second (1738-1746 time entries in 58.6 s of server time), an object group about 12.7 per second; once per session a time entry repeats the previous value (consistent with the client keeping the larger one, 9.1). The 2009 server tick stays open (B14, 13); DeCombat's proposal is 60 per second (CD-63) | Reference stack differs; tick UNPROVEN |
| O13 | 10.3, B16 | player updates with the `quat128` aim block (`pu_flags` 0x0100, after the aux words) were accepted by the reference stack, reported as 792-bit shell messages (16 + 8 + 96 + 192 + 128 + 32 + 312 bits); the 664-bit form without the block was accepted too, and the walker's object moved on the other bot's screen (stream travel 6306 units in 58 s). Which client condition sets the bit stays UNPROVEN (BOT-9 proposed) | REFERENCE-CONFIRMED (block position and width) |

Count: REFERENCE-CONFIRMED 3 items (O2, O5, O13), reference stack differs 1 (O12).

## Appendix C: capture notes (spec-fold-4, 2026-10-25)

Sources (evidence records, kept in full): `CAPTURE_tdm1_TRANSPORT_VALIDATION.md` section 3 (C-B1 .. C-B9), `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md` sections 1
and 5, `CAPTURE_tdm4_KILL_VALIDATION.md` sections 2.2 and 5, `CAPTURE_tdm4_WEAPON_FORMS.md` section 4, and `SESSION_B_S2C_addendum_dserver3.md` S3-1, S3-3 .. S3-5, S3-8,
S3-9, S3-11, S3-13 (stub since this fold). Server-to-client bytes are the reference stack's (REFERENCE; REFERENCE-CONFIRMED when the real client's next
messages show acceptance); client-to-server bytes are the real client's (PROVEN-CAPTURE).

| item | spec place | observation | grade |
|---|---|---|---|
| C-B1 | 3.1, 3.2, 4.1 | 4,652 id 8 messages, 6,893 records (230 creates, 6,627 changes, 19 tracked-sound records, 17 removals); every `rec_bits` equals the bits read; second `cf` byte present in 1,528 records, exactly when `cf & 0x8F00` is non-zero; no record with `cf` = 0; special kinds 1 and 3 only; the session went on 157 s | REFERENCE-CONFIRMED |
| C-B2 | 3.3, V-B3 | `create_pos` = (0, 0, 0) in all 230 creates (kinds 0 x22, 1 x5, 2 x33, 3 x110, 9 x60); 197 creates use `pos48` + `vec72`, 33 full-precision ones raw floats (`flags & 0x0100`); kind-1 creates respect the caps; the own character: `kind_byte` 0xc1, 580-bit blob, 2 models, create flags with 0x0800 set and 0x0100 clear | REFERENCE-CONFIRMED |
| C-B3 | 4.3, 9.1, V-B4 | 4,652 id 10 messages, all parse; time entry last, non-decreasing (2 repeats), 29.58 per second of server time; 4,650 of 4,652 in the datagram of that tick's id 8 entry (12.2 rule 4) | REFERENCE-CONFIRMED |
| C-B4 | 4.2 suppression | the reference stack streams the client's own object every tick and sends it reliable rotation; the client did not adopt the streamed position | REFERENCE (server), PROVEN-CAPTURE (client) |
| C-B5 | 5.1, V-B5 | 17 kind-3 records (37 ids), each the last record of its message; tracked-sound and object ids removed together; ids reused after removal | REFERENCE-CONFIRMED |
| C-B6 | 8.3, 10.2, V-B7 | see 8.3 and 10.2 | PROVEN-CAPTURE (acknowledgement), REFERENCE-CONFIRMED (cache) |
| C-B7 | 10.3, V-B6 | see 10.3 | PROVEN-CAPTURE |
| C-B8 | 7.1, 12.1 | server shell ids seen: 0x43, 0x44, 0x45, 0x66, 0x67, 0x68, 0x6a, 0x6b, 0x72, 0x74, 0x7d, 0x7f, 0x88, 0x90, 0x9d, 0x9e, 0xa0, 0xa2, 0xa6, 0xb2, 0xb5, 0xca; 283 reliable shell messages, 0xa2 x52 and 0xa6 x2,797 unreliable; id 17 x140 all unreliable (first bytes 0x06, 0x19, 0x10, 0x65: type 0x65 is not in the 7.2 list); 0x9d is in no stage list | REFERENCE |
| C-B9 | V-B9, V-B10 | undecided | |
| P-1 | 3.5 | player character, weapon child and projectile object forms | REFERENCE-CONFIRMED |
| P-2 | 4.3 | remote player stream: `uf` 13 with three trackers | REFERENCE-CONFIRMED |
| P-3 | 10.3 | `yaw_c8` truncates (4,182 + 1,103 updates); derived layout exact; 15 Hz; eye offsets; aim law | PROVEN-CAPTURE |

Not decided by any capture: the client's own `pos48` / `vec72` decoder (the streamed positions of the reference stack were accepted; whether the real client renders a remote
player at the right place is only the user's observation), `UF_ROT` (quat128) streams, kind-0 one-shot sound records, the object records of kinds 4-8 and 10-11, the meaning
of `flags2` 0x0318 / `uflags` 0x00401020 and of the 32 attachment mask pairs.

Count: PROVEN-CAPTURE 3 groups (C-B6 acknowledgement, C-B7/P-3, C-B4 client side); REFERENCE-CONFIRMED 6 (C-B1, C-B2, C-B3, C-B5, P-1, P-2); REFERENCE 2 (C-B4 server, C-B8).

## Appendix F: fold record (spec-fold-5, 2026-10-25)

Sources: `reports/dserver-slice4.md` (DSV-36, DSV-37, live round 3), the ROM-gated test run quoted in 3.5.2, `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md` section 8 (D-ids). Grades
as in the header, plus **D** = DeCombat design (a DC_DEFAULTS row, proposed); TESTED = a crate test asserts it.

| item | place | change | grade |
|---|---|---|---|
| DSV-37 | 3.5.1, new 3.5.2 | world-file zones and flags as DeCombat creates them; ROM-gated bit identity with the reference stack's tdm2 records (4 / 4 zones, 2 / 2 flags, 29 / 31 brush words) | D; forms REFERENCE-CONFIRMED; the brush rule UNPROVEN |
| DSV-36 | new 3.5.3 | pointer to the start and join order | D |
| tdm2 D1-D4, D6, D9, D10 | 3.5, 3.5.1, 4.3, 5.1 | the wire facts of these crate differences live in these sections (`GAME_MESSAGES_2009_S2C.md` Appendix F.4 maps every D-id) | REFERENCE-CONFIRMED |
| render pairing (RENDER_2009 Appendix M, M7) | 3.5.1 animation clip keys | the reference stack pairs tracker 1 with blend set 0 and tracker 2 with blend set 1 for every body; for the 60-node bodies set 1 is almost empty (OBSERVED census), so the pairing is either per body or by name: UNPROVEN | REFERENCE (pairing), OBSERVED (census) |

Not folded: dc-dserver's object ids and file announcement order (crate detail).

## Appendix G: fold record (spec-fold-6, 2026-10-25)

Sources: the captures tdm2 (`20261025_001409_tdm2`) and tdm4 (`20261025_002236_tdm4`) decoded with our own readers (the `dc-botclient` capture reader `capdecode` and a scratch record / stream dump built on the same crate, outside the tree; identifiers and seq numbers only, no reference stack log was read); the reports `reports/{dserver-tidy, stack-verify, bot-respawn, bot-roundend, client-dserver-fix, render-styles}.md`; the addenda `GAME_MESSAGES_2009_S2C_addendum_blast_questions.md` and `RENDER_2009_addendum_styles.md` (both now stubs); `DC_DEFAULTS.md` rows DSV-39, DSV-40, GAIN-1, BOT-18, REN-1..11 (PENDING). Grades as in the header plus **D** (README). No existing grade was lowered; no REFERENCE value became a rule.

| item | place | change | grade |
|---|---|---|---|
| blast questions Q3-Q9 (dserver-tidy) | new 3.5.4; the 3.5 grenade sentence points to it | the projectile's create form, the 0x66 count in its datagram, the flight's change records and stream entries, the blast tick and the two accepted orders, with seq numbers of both captures; DSV-39 (D) and its differences | REFERENCE, ACCEPTED; D for DeCombat |
| render-styles addendum S2 | 8.4 | `extra` = convert-to-ambient and the meaning "global light" EVIDENCE (was UNPROVEN / PLAUSIBLE); the reference stack's id 22 (tdm2, tdm4) equals the DSV-40 / REN-11 rule exactly | EVIDENCE; REFERENCE-CONFIRMED |

Not folded: the reference stack's launch speed, gravity, bounce and fuse (rule values; recorded in 3.5.4 only as forms).

## Appendix H: fold record (spec-fold-7, 2026-10-25)

Sources: `reports/{dserver-visibility, dserver-light, bot-vis13}.md`, the rows DSV-41, the DSV-39 addition and the VIS corrections in `DC_DEFAULTS.md`
(Wave U section, PENDING; master 7a19faf); evidence already in this spec (3.2, 3.5.1, 3.5.4, 4.2 item 6) and `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md` 1.4. No capture was re-read; no new RVA. Grades as in the header
plus **D** (README). No existing grade was lowered; no REFERENCE value became a rule.

| place | change | grade |
|---|---|---|
| 4.2 (after the writer-side suppression) | DeCombat hide / show record of culling: `cf` 0x0202 with item 6 only, park point, refresh, spawn record with the park point; evidence: reader 0x0202 test; the ACCEPTED `CF_POS_SNAP` change records on the real client's own character (spawn records 0x26a8 / 0x22a8, moves 0x0280 / 0x0680; re-read from tdm2 / tdm4); 0x468a only in the bot-only run; the design's seq 753 citation corrected (bot link) | D (VIS-9 proposed), TESTED, RUN (bot only); evidence EVIDENCE / REFERENCE-CONFIRMED (own character only); V-VIS-1 open |
| 4.3 (bullets before the reader rules) | DeCombat culling of the id 10 entries; DSV-41 thrown-object stream behind a switch (default off) and the reference stack form it approximates | D (proposed), TESTED; V-PROJ-1 open |

DSV-41 is the number the dserver-light worker staged; the dserver-visibility worker took no DSV number (its rows are VIS-n), so there is no clash.

## Appendix I: fold record (spec-fold-8, 2026-10-25)

Sources: `UDP_SESSION_2009_B_addendum_round2.md` G2 / G3 (now a stub), `CAPTURE_rust3_RUSTSTACK_VALIDATION.md` 3, 5, 7, 9.5 and 11, the reports
`reports/{aa-triage-grenades, aa-fix-grenades, aa-triage-red-dot, aa-triage-sounds, aa-fix-sounds, aa-triage-world-physics, aa-fix-world-physics, dserver-round2}.md`,
`GAME_MESSAGES_2009_S2C_addendum_blast_questions.md` (stub, Q6 sentence corrected) and the PENDING rows of `DC_DEFAULTS.md` named in the text. No capture was decoded again.

| place | change | grade |
|---|---|---|
| 3.5.1 | the tdm6-world TDM set on the real client's link, start-up removals, collision evidence, sliding doors (G2, V-DOOR-1) | REFERENCE, SIM + PROVEN-CAPTURE; moving door UNPROVEN |
| 3.5.2 | status of DSV-37 `world` and DSV-46 | D |
| 3.5.4 | the tdm6 throws (stream and change-record counts, flight detail); grenade variables; the first observation for V-PROJ-1; DeCombat status | REFERENCE; OBSERVED; D |
| 3.5.5 (new) | death and respawn of a player character: hide record, corpse, death clip, new weapon child, respawn record `cf` 0x2ba8 | REFERENCE, ACCEPTED; PROVEN-CAPTURE (rust3 claims on a standing corpse) |
| 4.2 | 0x0202 hide / show records accepted at parse level on a remote character (rust3) | PROVEN-CAPTURE (part of V-VIS-1 closed) |
| 4.3 | DSV-41 default on; a dead character's stream carries the death clip | D; REFERENCE + PROVEN-CAPTURE (difference) |
| 8.3 | world sounds at world load (11 records, field rules); pain / fall / bounce sounds | REFERENCE, ACCEPTED |
| 8.5 | sky table block = the sky object's outer and inner boxes (formerly UNPROVEN) | REFERENCE + ROM |
| 8.6 | world-settings variables, value text (G3), missing variable reads 0, step height | REFERENCE; EVIDENCE |

New section number: 3.5.5 (nothing was renumbered). No grade was lowered.
