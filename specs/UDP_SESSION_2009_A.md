# UDP_SESSION_2009_A: game-session start-up of the 2009 client (part A, clean-room)

- **Title:** UDP_SESSION_2009_A, the session layer of the in-game connection from the lobby's game-start
  message to a loaded world with the player's own client object.
- **Scope:** (1) CShell: how the lobby push `match_address_push` (PROTOCOL_2009.md §7) becomes the engine
  connect call; (2) Engine.exe: the session message dispatcher that runs on top of the link of
  `UDP_TRANSPORT_2009.md`, and every message the client handles before it is in a playable world; (3) the
  client -> server messages of this phase. Part B (`UDP_SESSION_2009_B.md`, other author) covers the playing
  phase (object updates, game messages, leave). Transport framing, join ask / reply, acks and disconnect blocks are
  in `UDP_TRANSPORT_2009.md` and are referenced, not repeated.
- **Evidence sources:** decompiler output of the 2009 EU-test binaries only: `Engine.exe/<rva>.c`
  (`engine+0xRVA`, image base 0x00460000, RVA = VA - 0x460000) and `CShell.dll/<rva>.c`
  (`cshell+0xRVA`, image base 0x37100000), the `*.funcs.orig.tsv` tables; sibling clean specs
  `PROTOCOL_2009.md`, `UDP_TRANSPORT_2009.md`, `GLOSSARY_UDP.md`.
  Evidence grades as in `UDP_TRANSPORT_2009.md`: **PROVEN** (read in the decompiler output), **PLAUSIBLE** (one step
  reconstructed), **UNPROVEN** (not visible; needs capture or a decompiler re-run). One extra grade is used here:
  **DISASM**: the function exists in the 2009 binary but decompiler did not create it (it shows only as a `LAB_`
  label or is missing from `*.funcs.orig.tsv`); the rule was read from a read-only x86 disassembly of the
  user's own `Engine.exe` / `CShell.dll` at that address (same 2009 binary, same evidence class as the decompiler
  C, but not reviewable in `decompiler/decomp`). The coordinator may want a decompiler re-run that creates these
  functions (list in section 10) before the grade is lifted to PROVEN. **PE-DATA**: a method-table entry or a
  format string read from the user's binary at the given VA (addresses and widths only; no ROM text is quoted).
- **Status:** DRAFT. The client-to-server messages of this phase are validated by the real 2009 client (captures tdm1, tdm2, tdm4; Appendix C, grade PROVEN-CAPTURE: bytes the client sent, or a client behaviour visible in them); the server-to-client forms are REFERENCE-CONFIRMED only (the reference server wrote them and the client accepted them; not 2009 server evidence). Not implemented.
- **Date:** 2026-10-24

Conventions: as `UDP_TRANSPORT_2009.md` (bit order LSB first, `uN` = N-bit unsigned field, field widths in bits).
"S->C" is game server to client. Names are ours and are listed in `GLOSSARY_UDP.md` section S.

## 1. Function map

Names in the "role" column are ours. `cs+` = `cshell+`.

### 1.1 CShell.dll (game shell, image base 0x37100000)

| role (our name) | RVA | grade |
|---|---|---|
| lobby handler of `match_address_push` 0x3043 (`addr_push_rx`) | cs+0x165650 | PROVEN |
| game-connect sequence (`match_connect`) | cs+0x1630a0 | PROVEN |
| connection manager object (`conn_mgr`), global at VA 0x3759e22c, constructed by cs+0x2f7a0 (base part cs+0x23c7e0), method table VA 0x374cedc4 (PE-DATA) | cs+0x2f7a0 | PROVEN |
| `conn_mgr` method +0x10: fill the engine start request for a client join (`join_request_fill`) | cs+0x2fac0 | PROVEN |
| `conn_mgr` methods +0x48 / +0x4c: attach the hello blob and call the engine start-game slot (`join_start`; +0x48 serves request kind 0, +0x4c request kind 3 = remote join) | cs+0x2f930 / cs+0x2f9e0 | DISASM |
| `conn_mgr` method +0x44: local (offline) world start, request kind 4 | cs+0x2f8d0 | DISASM |
| `conn_mgr` method +0x24: connection-level shell messages 0x72, 0x73, 0x9e, 0xac (contents: part B) | cs+0x23c570 | DISASM |
| `conn_mgr` method +0x2c: start by request kind (`conn_mgr+4`): 0 -> +0x48, 3 -> +0x4c, 4 -> +0x44, other -> fail | cs+0x23c730 | DISASM |
| `conn_mgr` method +0x28: network-driver check before the start (enumerate, pick, initialise; meaning PLAUSIBLE) | cs+0x23c6b0 | DISASM |
| `conn_mgr` server-address store (IPv4 bytes at +0x11e1, port at +0x11e6), called by the derived shell's game-message entry | cs+0x23c4c0 | DISASM |
| mission object, global VA 0x3759e120, method +0x1c "start as client" (`mission_start_client`) | cs+0x22f8a0 | PROVEN |
| mission method +0x48 "enter loading" (`mission_enter_loading`) | cs+0x22fa50 | DISASM |
| mission method +0x2c "continue start": mission state (+0x94) 1 -> method +0x34, 2 -> method +0x38, 3 -> done (1), other -> fail (0) | cs+0x22f750 | DISASM |
| mission method +0x38 "connect": mission state := 3, `conn_mgr` method +0x2c; on success a game-shell field (object at VA 0x37597e50, +0xc3b0) := 2 | cs+0x22f910 | DISASM |
| loading-screen state update (`loading_tick`): connect-result check, "in world" check, timeout box | cs+0x1bb910 | PROVEN |
| "in world" test (`in_world_test`) | cs+0x1f3950 | PROVEN |
| client shell event method (slot +0x1c of the client shell tables VA 0x374d026c / 0x374eaf84) | cs+0x1a8470 | DISASM |
| interface event handler (disconnect, shutdown, focus) | cs+0x1bbfb0 | PROVEN |
| disconnect reason -> message box (`disconnect_box`) | cs+0x1bb2f0 | PROVEN |
| client shell "entered world" callback (slot +0x4c of VA 0x374eaf84; derived slot points to cs+0x32e10) | cs+0x1a5590 | PROVEN |
| shell message dispatcher (part B) and its switching-worlds state handler (shell id 0x88) | cs+0x1a86b0 / cs+0x1a58f0 | PROVEN |

### 1.2 Engine.exe, client side (image base 0x00460000)

| role (our name) | RVA | grade |
|---|---|---|
| engine client interface: function-pointer slots filled at start-up (slot +0x80 start game, +0x84 own client id, +0x88 end game, +0x8c is-connected, +0x7c request kind, +0x20c own view object) | 0x14270 | PROVEN |
| slot +0x80 stub: forwards the request to the client manager (global VA 0x6dd228) | 0xe980 | DISASM |
| start game (`start_game`): request kinds 0, 2, 3, 4; kind 3 = join a remote server | 0x18d00 | PROVEN |
| server-connection object (`srv_conn`, 0x80 bytes) constructor | 0x19e80 | PROVEN |
| client message handler table init, 256 slots at VA 0x6e18e0 (`dn_table_init`) | 0xa5230 | PROVEN |
| client message dispatcher (`dn_dispatch`) / receive loop | 0xa52f0 / 0xa53e0 | PROVEN |
| per-frame connection update: runs the receive loop, turns a handler error into a disconnect | 0x198f0 | PROVEN |
| error processing: disconnect-class errors become shell event 1 | 0x24d40 | PROVEN |
| `S2C_VERSION` handler (id 4) | 0xa2a70 | PROVEN |
| `S2C_LEAVE_WORLD` handler (id 5) -> world unload | 0xa2960 -> 0x1a2c0 | DISASM -> PROVEN |
| `S2C_LOAD_WORLD` handler (id 6) -> world load (`world_load`) | 0xa3e90 -> 0x1a770 | DISASM -> PROVEN |
| `S2C_VIEW_OBJECT` handler (id 7) | 0xa2a50 | DISASM |
| `S2C_CLIENT_ID` handler (id 12) | 0xa2910 | PROVEN |
| `S2C_BUNDLE` handler (id 14) | 0xa3f80 | DISASM |
| `S2C_SKY_TABLE` handler (id 16) | 0xa44f0 | DISASM |
| `S2C_PRELOAD` handler (id 19) | 0xa45b0 | PROVEN |
| own view object lookup (slot +0x20c target) | 0xa5530 | PROVEN |
| client file service (method table VA 0x680208, PE-DATA): unknown-id hook +0x0c, lookup by reference +0x24, register server file +0x38 | 0xcff20 / 0xd09e0 / 0xd0c40 | DISASM |
| file-list receiver (`file_list_rx`), answers with `C2S_FILE_ACK` | 0x3e990 | PROVEN |
| server-file table lookup by id (hash, id mod 100) / local resource lookup by name | 0xcfee0 / 0xcfe60 | PROVEN |

### 1.3 Engine.exe, server side (the same binary can host; its writers fix the exact bits the client expects)

| role (our name) | RVA | grade |
|---|---|---|
| new client on a new link (`host_new_client`): sends `S2C_VERSION`, `S2C_CLIENT_ID`, `S2C_FILE_LIST` | 0x8e9f0 | PROVEN |
| server message handler table init, 256 slots at VA 0x6e1230 / dispatcher | 0x925e0 / 0x923b0 | PROVEN |
| `C2S_HELLO` handler (id 5) | 0x90b40 | DISASM |
| `C2S_BYE` handler (id 6) | 0x90ae0 | PROVEN |
| `C2S_RATE` handler (id 7) | 0x90a80 | PROVEN |
| `C2S_LOAD_STATE` handler (id 9) | 0x90b20 | DISASM |
| per-client tick (`host_client_tick`) | 0x8e1b0 | PROVEN |
| ready-to-load test | 0x8b2e0 | PROVEN |
| send world (`host_send_world`): `S2C_EVAR` per variable, then `S2C_LOAD_WORLD` | 0x8ddd0 | PROVEN |
| loading-phase step (`host_loading_step`): preload, sky table, enter-world callback, `S2C_VIEW_OBJECT` | 0x8db60 | PROVEN |
| preload lists + preload end | 0x8ccd0, 0x8cb60 | PROVEN |
| sky table writer | 0x8d5a0 | PROVEN |
| leave-world notice writer | 0x8ceb0 | PROVEN |
| view-object writers (normal / attach a viewer) | 0x8db60, 0x8d0c0, 0x8e000 | PROVEN |
| phase setter | 0x8e160 | PROVEN |
| boot player (bye code 6) / disconnect a link with a code | 0x9bb10 / 0x79d30 | PROVEN |
| file announcer object: create / add entry / send pending list (`S2C_FILE_LIST`) / receive client answers | 0x3f240 / 0x3f360 / 0x3f3f0 / 0x3f740 | PROVEN |
| server used-file registry (name -> id, id counter) | 0x95730 | PROVEN |
| send to one client with flags (bit 0 = reliable; part B) | 0x90e80 | PROVEN |

## 2. CShell: from match_address_push to the engine connect call

The lobby side ends with `match_address_push` 0x3043 (PROTOCOL_2009.md §7): `+6 wstr[17]` game server IPv4 text,
`+0x28 u32` UDP port, `+0x2c u32` record count N, then N ranking records. There is no key, ticket or world name in
it (SPEC: PROTOCOL_2009.md §7, EVIDENCE: cs+0x165650). Everything the game server learns about the player arrives
over UDP (section 5.2).

**2.1 `addr_push_rx`** (EVIDENCE: cs+0x165650, PROVEN):

1. Store the IPv4 text (UTF-16) in the room-screen object (+0x5880).
2. Format the port as decimal text (format string at VA 0x374d4b38 is a plain decimal conversion, PE-DATA) and
   store it (+0x589c). The port is therefore only used as text.
3. Copy the N ranking records into the score panel (not network relevant; send N = 0, PROTOCOL_2009.md §7).
4. Call `match_connect` (cs+0x1630a0).

**2.2 `match_connect`** (EVIDENCE: cs+0x1630a0, PROVEN except where marked):

1. Join the stored IP text and port text as `<ip>:<port>` (UTF-16; the format string at VA 0x374e5c94 joins two
   strings with a colon, PE-DATA), log it, and convert it to an 8-bit string.
2. Convert a second UTF-16 string member of the room object (+0x5530) to 8 bits. Its meaning is UNPROVEN
   (candidates: room password, empty). It is stored in the request but the remote-join path of the engine does
   not read it (section 2.4).
3. Call `conn_mgr` method +0x10 = `join_request_fill` (cs+0x2fac0) with (address8, second8, 0). It returns 0 when
   the address is empty, else 1.
4. On 1: call the mission object's method +0x1c `mission_start_client` (cs+0x22f8a0): sets loading flags, calls
   its methods +0x4c and +0x48 (`mission_enter_loading`, cs+0x22fa50, DISASM: clears the game shell's
   "entered world" flag at +0x1efc, and when the connection request kind is 3 switches the interface to the
   loading screen, interface state 3, cs+0x1bb0b0); then mission state = 2. On failure it logs and returns 0.
5. Independent UI work follows (options, HUD values, loading-screen music; none of it touches the network). Then
   `match_connect` calls the mission object's method **+0x2c** (cs+0x1630a0, PROVEN); this is the actual start.
   Mission +0x2c (cs+0x22f750, DISASM) sees mission state 2 and tail-calls mission +0x38 (cs+0x22f910, DISASM:
   mission state := 3, a log line, then `conn_mgr` method +0x2c), which dispatches on the request kind at
   `conn_mgr+4` (cs+0x23c730, DISASM) and, for kind 3, tail-calls `conn_mgr` method +0x4c = cs+0x2f9e0 (section
   2.4). The join therefore runs synchronously inside `match_connect`. On success mission +0x38 sets a game-shell
   field (object at VA 0x37597e50, +0xc3b0) to 2 and returns 1. When mission +0x2c returns 0 (with method +0x4c
   only for a join timeout, result 0x4F), `match_connect` sets the game-start-failed flags +0x198 and +0x199 of
   the character-info record (cs+0x1b1a10).

**2.3 `join_request_fill`** (EVIDENCE: cs+0x2fac0, PROVEN). The engine start request (`start_request`) is the
`conn_mgr` object from offset +4 on; offsets below are relative to `start_request` (= `conn_mgr` offset - 4):

| request offset | content | evidence |
|---|---|---|
| +0x0 | request kind = **3** (join a remote server) | cs+0x2fac0 (`conn_mgr+4 = 3`) |
| +0x68 | address text `<ip>:<port>`, 8-bit, at most 100 bytes | cs+0x2fac0 (strncpy 100 to `conn_mgr+0x6c`) |
| +0x1a4 | second string, 8-bit, at most 0xfff bytes, only if non-empty | cs+0x2fac0 |
| +0x11cc | third argument (0 here; non-zero would be converted by cs+0x2fa70) | cs+0x2fac0 |
| +0x1214 .. +0x1315 | **hello blob**, 0x102 = 258 bytes, zero-filled first | cs+0x2fac0 |

Hello blob layout (offsets inside the 258 bytes; EVIDENCE: cs+0x2fac0, PROVEN for offsets and copy limits):

| blob offset | size | content | grade |
|---|---|---|---|
| 0x00 | 64 | 8-bit copy of a UTF-16 name string held by the shell's character-info record (singleton returned by cs+0x1b1a10, wide string at its +0x138, copied by cs+0x28d80, PROVEN clean tree), at most 63 bytes + NUL. The wide-to-8-bit conversion cs+0x20d60 (PROVEN, clean tree) calls the system conversion with the code page its caller passes, and cs+0x2fac0 passes 0, the system ANSI code page (DISASM push at cs+0x2fb5e); the bytes therefore depend on the player's Windows locale | PROVEN copy; meaning PLAUSIBLE (the player's nickname); code page = system ANSI (DISASM) |
| 0x40 | 64 | 8-bit string from another shell object (global VA 0x3759b9d8, std::string at +0x5c), at most 63 bytes + NUL | PROVEN copy; OBSERVED tdm1 (PROVEN-CAPTURE): a 36-character upper-case GUID text (8-4-4-4-12 hex digits) + NUL, zero padding, not carried by any lobby frame; meaning and lifetime UNPROVEN |
| 0x80 | 1 | 0 | PROVEN |
| 0x81 | 1 | 0 (not written) | PROVEN (zero fill) |
| 0x82 | 128 | the same UTF-16 name string as offset 0x00, NUL-terminated (unbounded copy; a name longer than 63 units would overrun, the lobby limits names to 16 units, PROTOCOL_2009.md §3.7b) | PROVEN copy, meaning PLAUSIBLE |

**2.4 Engine call.** For request kind 3 the start method is `conn_mgr` method **+0x4c** (cs+0x2f9e0, DISASM;
method +0x48 = cs+0x2f930 serves kind 0 and also stores a 48-byte block at +0x11ac / +0x11b0, which the remote-join
path does not send). Method +0x4c:

1. calls the engine client interface member +0x2b8 with (0, 0) (engine+0xff80, DISASM: it destroys a previous
   server-connection object held by the client manager, resets the client manager's network part and always returns
   0; the result is stored in `conn_mgr+0x11dc`, and a non-zero value would abort);
2. calls `conn_mgr` method +0x28 (cs+0x23c6b0, DISASM: walks the list returned by interface member +0x2bc
   (engine+0x7b0d0) for the first entry with flag bit 0, frees the list (+0x2c0) and initialises the entry (+0x2c4);
   meaning PLAUSIBLE: select the network driver); a failure aborts with 0;
3. stores the hello blob pointer (`conn_mgr+0x1218` = request +0x1214) and length 0x102 at request +0x11b4 /
   +0x11b8;
4. calls the engine client interface member **+0x80** with the request (`conn_mgr+4`), stores the result in
   `conn_mgr+0x11dc` (the "last connection result" that `loading_tick` tests, section 6.3) and logs it;
5. returns 1 unless the result is 0x4F (join timeout).

The caller chain is `match_connect` -> mission +0x2c -> mission +0x38 -> `conn_mgr` +0x2c -> `conn_mgr` +0x4c
(section 2.2 step 5; DISASM). The end-to-end effect is OBSERVED: the real client contacts the advertised UDP port
after 0x3043 (PROTOCOL_2009.md §7, LIVE).

Engine slot +0x80 is the stub engine+0xe980 (DISASM; installed by engine+0x14270, PROVEN), which calls
`start_game` (engine+0x18d00, PROVEN) on the client manager (global VA 0x6dd228). For request kind 3:

1. Find the network driver registered under the internet driver name (engine+0x7b180 lookup; failure = result 0x49).
2. Call the driver's connect method (table offset +0x58 = engine+0xb7150, PE-DATA table at VA 0x67f1c0). It
   parses the address text (engine+0xad820, PROVEN: split at the first `:`, port = decimal conversion of the
   rest, host = dotted quad if it starts with a digit, else a host-name lookup; failure = result 0x50) and then
   runs `join_peer` (engine+0xb6850). The join is synchronous: the call returns only after the join reply or the 10 s join timeout (UDP_TRANSPORT_2009.md §5.2;
   results 0x4F timeout, 0x50 no socket, 0x51 refused, 0x52 wrong app). A non-zero result returns at once.
3. Create the server-connection object `srv_conn` (engine+0x19e80): own client id (+0x38) = 0xFFFF, view
   object id (+0x64) = 0xFFFF, world-loaded flag (+0x6c) = 0, request kind copied to +0x74.
4. Send **`C2S_HELLO`** (id 5) reliably (section 5.2).
5. Return 0. The CShell then shows the loading screen and waits for the server (section 8).

The 8-bit address text is the only server identification the engine uses; the join goes to that IP:port and the
link peer is whatever address answers (UDP_TRANSPORT_2009.md §5.2).

## 3. Engine: session message framing and dispatcher

**3.1 Message = transport entry.** A session message is one entry of a reliable or unreliable block after
split-message reassembly (UDP_TRANSPORT_2009.md §8, §9). Its first 8 bits are the message id (`msg_id:u8`), the
rest is the body, read with the bit primitives of UDP_TRANSPORT_2009.md §2-3 (LSB-first, no alignment).
EVIDENCE: engine+0xa52f0 (client), engine+0x923b0 (server). Every message in this part is sent **reliably** by
the 2009 host (send flag bit 0 = 1 on every writer listed in section 1.3; EVIDENCE: engine+0x90e80 callers
engine+0x8ddd0, engine+0x8db60, engine+0x8ccd0, engine+0x8d5a0, and engine+0x3f3f0 for the file list).

**3.2 Id spaces are per direction.** The client and the server each have a 256-slot handler table; the same
number means different things per direction (id 5 S->C is leave-world, id 5 C->S is hello).

| direction | handled ids | table init | evidence |
|---|---|---|---|
| server -> client | 4, 5, 6, 7, 8, 10, 12, 13, 14, 15, 16, 17, 19, 20, 21, 22, 23 | engine+0xa5230 (VA 0x6e18e0) | PROVEN |
| server -> client, no table slot | 0x32 (file list), handled by the fallback (3.3) | engine+0xa52f0 -> engine+0xcff20 -> engine+0x3e990 | PROVEN / DISASM |
| client -> server | 5, 6, 7, 8, 9, 10, 11 | engine+0x925e0 (VA 0x6e1230) | PROVEN |
| client -> server, no table slot | 0x36, 0x38 (file-list answers), handled by the per-client file announcer | engine+0x923b0 -> engine+0x3f740 | PROVEN |

Ids 8, 10, 11, 13, 17, 20, 21, 22, 23 (and the C->S ids 8, 10, 11) belong to the playing phase and are
specified in part B; this part gives only what the start-up needs from them.

**3.3 Client dispatcher** (`dn_dispatch`, EVIDENCE: engine+0xa52f0 PROVEN, argument passing DISASM):

1. Wrap the message in a reader, read `msg_id:u8`.
2. If the slot is set, call `handler(srv_conn, reader)` (cdecl, two arguments; DISASM 0x505346..0x50534c).
3. If the slot is empty, pass the whole message to the client file service hook (method +0x0c of the object at
   VA 0x6e1ce8 = engine+0xcff20, DISASM), which hands it to `file_list_rx` (engine+0x3e990, PROVEN). That
   function handles only id 0x32 and ignores everything else. So **an unknown S->C id is silently ignored**.
4. A handler returns 0 for success or an engine result code. The receive loop (engine+0xa53e0, PROVEN) stops at
   the first non-zero result; the per-frame connection update (engine+0x198f0, PROVEN) returns
   `result | 0x2000000`, and error processing (engine+0x24d40, PROVEN) then destroys `srv_conn` and raises the
   shell's disconnect event with code `result & 0xF9FFFFFF` (section 6). **A handler error is fatal for the
   session.**

**3.4 Bundle, `S2C_BUNDLE` (id 14)** (EVIDENCE: engine+0xa3f80, DISASM): body = repeated `[len:u8][len bits]`,
ended by `len = 0` or by the end of the message. Each inner bit string is a complete message (its own `msg_id:u8`
first) dispatched through the same table with the same `srv_conn`. An inner id without a slot is skipped (not
offered to the file service). `len` larger than the remaining bits -> result 0x2c (invalid server message). An
inner handler error is returned. A server may use the bundle to pack short start-up messages; nothing in this
part requires it.

**3.5 Server dispatcher** (EVIDENCE: engine+0x923b0, PROVEN): reads `msg_id:u8`, calls
`handler(reader, client)`; an id without a slot goes to the client's file announcer (engine+0x3f740). The
Rust server needs no table of its own beyond the ids in section 5.

## 4. Server -> client messages before the world is playable

Field lists start after `msg_id:u8`. Widths are the reader calls' bit counts. "Host" is the 2009 Engine.exe
acting as server (section 1.3); it is the reference for order and values, not a rule for our server unless
the client depends on it.

### 4.1 Summary

| our name | id | dir | body (after `msg_id:u8`) | client handler | host writer |
|---|---|---|---|---|---|
| `S2C_VERSION` | 4 | S->C | `net_version:u32`, `host_rate_bps:u32` | engine+0xa2a70 PROVEN | engine+0x8e9f0 PROVEN |
| `S2C_LEAVE_WORLD` | 5 | S->C | (none) | engine+0xa2960 DISASM | engine+0x8ceb0 PROVEN |
| `S2C_LOAD_WORLD` | 6 | S->C | `srv_clock:f32`, `world_file:u16` | engine+0xa3e90 DISASM, engine+0x1a770 PROVEN | engine+0x8ddd0 PROVEN |
| `S2C_VIEW_OBJECT` | 7 | S->C | `object_id:u16` | engine+0xa2a50 DISASM | engine+0x8db60, 0x8d0c0, 0x8e000 PROVEN |
| `S2C_CLIENT_ID` | 12 | S->C | `client_id:u16`, `local_host:u8` | engine+0xa2910 PROVEN | engine+0x8e9f0 PROVEN |
| `S2C_BUNDLE` | 14 | S->C | `{len:u8, len bits}` until `len = 0` / end | engine+0xa3f80 DISASM | (not used by the host in this phase) |
| `S2C_EVAR` | 15 | S->C | `name:zstr8`, `value:zstr8` | engine+0xa50c0 PROVEN | engine+0x8ddd0, 0x90830 PROVEN |
| `S2C_SKY_TABLE` | 16 | S->C | `sky_block:384 bits`, `count:u16` (<= 30), `count x sky_object:u16` | engine+0xa44f0 DISASM | engine+0x8d5a0 PROVEN |
| `S2C_PRELOAD` | 19 | S->C | `sub:u8`, then sub-specific `u16` list to the end of the message | engine+0xa45b0 PROVEN | engine+0x8ccd0, 0x8cb60 PROVEN |
| `S2C_FILE_LIST` | 0x32 | S->C | `{file_id:u16, file_info:u32, name:zstr8}` to the end of the message | engine+0x3e990 PROVEN (via engine+0xcff20 DISASM) | engine+0x3f3f0, 0x3f150 PROVEN |
| `S2C_OBJ_GROUP` (part B) | 8 | S->C | object create / change / remove records | part B §3-5 | engine+0x8fe60 |
| (part B) `S2C_SHELL_MSG` | 13 | S->C | shell message bits to the end (first `shell_id:u8`) | engine+0xa5060 PROVEN | part B §6 |

### 4.2 `S2C_VERSION` (id 4)

- Client (EVIDENCE: engine+0xa2a70, PROVEN): `net_version` must be **7**. Otherwise the client records an error
  text and returns result **0x4E**, which ends the session (section 3.3, 6). Then, if `host_rate_bps` is lower
  than the link's current rate (link method +0x08), the link rate is set to `host_rate_bps` (link method +0x0c).
  A higher value is ignored.
- Host (EVIDENCE: engine+0x8e9f0, PROVEN): first message on a new client link: `7`, then the host's rate global
  (VA 0x6cb29c, initial value 5120000, PE-DATA per UDP_TRANSPORT_2009.md §14; the 2009 CShell sets this console
  variable to 35840 at profile load, transport 5.3). Sent reliably (DISASM: the send helper engine+0x90e80 is called
  with flags 1 and "not to attached clients", engine+0x8eb87..0x8eb91).
- Effect on the client: the lowered rate holds only until the client's first rate report after the world load,
  which sets the link back to the client-rate global (transport 12.2), so this field limits the client's upload
  rate during the start-up only.
- Our server: send `7` and a rate in bits per second (DC-DEFAULT proposal CDS-A1: 5120000, i.e. no
  throttling below the client's own default).

### 4.3 `S2C_CLIENT_ID` (id 12)

- Client (EVIDENCE: engine+0xa2910, PROVEN): `srv_conn+0x38 = client_id`, `srv_conn+0x3a = (local_host != 0)`,
  debug log. The engine interface slot +0x84 then returns `client_id` (result 0x3F while it is 0xFFFF;
  EVIDENCE: engine+0xe9d0, DISASM).
- `local_host` must be **0** for a remote server: a non-zero value makes `world_load` take the "share the
  in-process server's world" branch instead of loading the file (EVIDENCE: engine+0x1a770, PROVEN), which fails
  without a local server (result 1).
- Host (EVIDENCE: engine+0x8e9f0, PROVEN): `client_id` = the lowest id in 0..29999 not used by another client
  (client+0x68); the third field is the new-client function's second argument (DISASM engine+0x8ebde), the same
  byte that sets client flag 2 = local client (engine+0x8ea23..0x8ea38, DISASM): the local-connection flag. Sent
  reliably (DISASM: engine+0x90e80 with flags 1, engine+0x8ebfd..0x8ec07).
- Our server: unique `client_id` per connected client, `local_host = 0`.

### 4.4 `S2C_FILE_LIST` (id 0x32) and the file table

This message is the **resource or file table** of the session. Every file reference the server sends later
(`world_file` in `S2C_LOAD_WORLD`, model and texture ids in object records and preload lists) is a 16-bit
**server file id** that the client resolves through this table.

- Body: entries `[file_id:u16][file_info:u32][name:zstr8]` until the reader reaches the end of the message (no
  count field). EVIDENCE: reader engine+0x3e990 (loop while bits remain; name read into a 0x104-byte buffer,
  longer names are truncated by the `zstr8` reader, UDP_TRANSPORT_2009.md §3), writers engine+0x3f3f0 (batched)
  and engine+0x3f150 (single entry), PROVEN.
- Client, per entry (EVIDENCE: engine+0xd0c40, DISASM): look `name` up in the client's own resource trees
  (engine+0xcfe60, PROVEN: tries every mounted resource tree in turn). Not found: log "unable to find" and keep no
  record. Found and `file_id` not yet known: store a record `{file_id -> local file}` in a 100-bucket hash
  (bucket = `file_id mod 100`, lookup engine+0xcfee0, PROVEN). An id that is already known keeps its first
  record. `file_info` is **not used** by the client (DISASM: the register method ignores that argument).
- The registration method always returns 0, so the client answers **every** entry as accepted, even a name it
  could not find (EVIDENCE: engine+0x3e990 sets bit 15 when the result is 0; engine+0xd0c40 returns 0 on both
  paths, DISASM). A wrong name therefore does not fail here; it fails later, when the id is used (missing world
  file -> result 0x29).
- Answer: one `C2S_FILE_ACK` (id 0x36) per received `S2C_FILE_LIST`, sent reliably (section 5.4).
- Resolution of a file reference (EVIDENCE: engine+0xd09e0, DISASM): a reference of kind 2 carries a server file
  id; unknown id -> no file.
- `name` is a path relative to the root of each mounted resource tree, resolved by engine+0xcfe60, which tries every
  tree in turn through engine+0x317e0 (PROVEN, clean tree):
  - a directory tree is searched with the OS file-find on `<root>\<name>` (Windows: case-insensitive, `\` and `/`);
  - an archive tree uses engine+0x1c0170 (PROVEN): a leading separator is skipped; the name is split at the last
    separator into a directory path (walked component by component, engine+0x1c0000) and a file name; the file
    name is split into base name and extension (`_splitpath`), the extension (up to 4 characters) is upper-cased
    and packed into the entry type, and the base name is matched in that type's table (engine+0x1bf820). A name
    without an extension gets type 0 and does not find a world file. Separators are the characters of a
    configured set, or, when none is set, every character outside `0x20..0x2e`, `0-9`, `A-Z`, `a-z` (so `\` and `/`
    both work). Names are compared with a case-insensitive compare unless an archive flag (+0x68 of the archive
    configuration) is set (engine+0x1c2570 / engine+0x1c26c0; the flag's run-time value is UNPROVEN, 0 = ignore
    case).
  - The client then keeps an upper-cased copy of the name in its file record (engine+0xd0c40 -> engine+0xec940).
  - The 2009 host registers the world as the world name plus `.dat` when the name has no extension of its own,
    with the "required" flag 1 (engine+0xa12f0 -> engine+0x95730, PROVEN); the engine's own fallback resource
    names that it registers (engine+0x94b90, engine+0x94c70) use `\` separators and their extension.
  - **Rule for our server:** announce the world file's path inside the ROM's world archive exactly as `dc-rez`
    lists it, with `\` separators and the `.dat` extension; letter case does not matter. A wrong path still fails
    only at `S2C_LOAD_WORLD` with 0x29 (V-A2 confirms the 2009 server's spelling). REFERENCE-CONFIRMED (tdm1): the client
  resolved a world announced as mixed-case directory + backslash + upper-case base name + lower-case `.dat`, with file id
  0 and `file_info` = the world file's size in bytes (the other 31 files of the session were upper case with
  backslashes); the 2009 server's own spelling stays unknown and is not needed.
- `file_info` on the host is the second field of the server's used-file entry (engine+0x95730), filled from the
  resource lookup's result (engine+0x954b0 -> engine+0x317e0): the found file's size (for a directory tree the
  file-find size field; for an archive tree the entry's size field +0x10) (PROVEN; that the archive field +0x10 is
  the size is PLAUSIBLE). The client ignores it (4.4 above). Our server sends the file size in bytes from `dc-rez`
  (DC-DEFAULT proposal CDS-A2).
- Host behaviour (EVIDENCE: engine+0x8e9f0, engine+0x3f360, engine+0x3f3f0, engine+0x95730, PROVEN): on a new
  client it queues every file the server currently uses (flag "batched") and sends them all in one
  `S2C_FILE_LIST`; a file the server starts using later is sent at once to every client as a single-entry
  `S2C_FILE_LIST`. Ids are a server-wide counter (first id = the counter's start value, UNPROVEN; ids are just
  keys, any unique u16 works). The host does not send `S2C_LOAD_WORLD` until the client has answered every
  announced entry (section 7.2, ready test).

### 4.5 `S2C_EVAR` (id 15)

`[name:zstr8][value:zstr8]`, one variable per message (EVIDENCE: client engine+0xa50c0 reads two strings and
stores them through engine+0x22c30, PROVEN; host engine+0x8ddd0 sends one message per entry of the server's
variable list right before `S2C_LOAD_WORLD`, PROVEN). Which variables the 2009 server sent is server
configuration, not client evidence; **none is required** by the client code read here (UNPROVEN that the game
shell needs any; check by capture). Part B owns the playing-phase use. *Correction (spec-fold-8, from aa-triage-world-physics item 10):* one is required in
effect: a variable the server does not send reads 0 (the engine's float read engine+0xd760 stores 0 before the lookup, EVIDENCE), so without the step-height variable the
client's step height is 0 and it cannot climb stairs without jumping (PROVEN-CAPTURE rust4 vs tdm4; `MOVEMENT_2009.md` 2, part B 8.6). The reference stack sent its variables right
before `S2C_LOAD_WORLD` as described (tdm6 seq 676-678, tdm4 seq 746-750).

### 4.6 `S2C_LOAD_WORLD` (id 6): load-world

Body: `[srv_clock:f32][world_file:u16]`.

Client (handler engine+0xa3e90 DISASM, `world_load` engine+0x1a770 PROVEN):

1. View object id (`srv_conn+0x64`) := 0xFFFF.
2. If a world is loaded (`srv_conn+0x6c`), unload it (engine+0x17360); clear per-world client state.
3. `srv_clock`: the 32 bits are stored unchanged as the client's previous and current server time
   (`srv_conn+0x14`, `+0x18`); frame delta := 0, local stamp := now, tick length := 1/30. The value is a binary32
   number of **seconds** of server game time: the host's game-time global (server+0x4fc) is advanced each server
   frame by `clamp(frame_ms * 0.001, 0, 0.2) * time_scale` (engine+0x9e4e0, PROVEN; `time_scale` = the f32 at VA
   0x6cb258, PE-DATA 1.0) and reset to 0.0 when a world starts (engine+0xa12f0, PROVEN). 0.0 is therefore what a
   2009 host itself has at world start; the client accepts any value (it only stores it).
4. `world_file` is a file reference of kind 2 (server file id, section 4.4). The client resolves it; if it is a
   different file than the previous world, the model cache is flushed (engine+0x250d0). The world stream is
   opened; **no stream -> result 0x29** (world file missing: unknown id, or a name the client could not find).
5. With `local_host = 0` (section 4.3): load the world from the stream. Load failure -> **0x2a** (invalid world
   file), bind failure -> **0x2b**, visibility-container failure -> **1**.
6. Success: remember the world file (`srv_conn+0x68`), call the client shell's **entered-world callback**
   (engine+0x14c80 -> client shell slot +0x4c, which is cs+0x1a5590 for the 2009 shell: sets the game shell's
   entered-world flag +0x1efc = 1 and starts in-world setup), set world-loaded (`srv_conn+0x6c = 1`), return 0.
7. The handler then sends **`C2S_LOAD_STATE(0)`** (`[9][0]`, reliable; DISASM 0x503ed8..0x503f1b). On an error
   nothing is sent and the session ends with that code (section 3.3).

The load runs inside the message handler on the client's main loop, so no other session message is processed
until it returns (PROVEN: it is called from the receive loop engine+0xa53e0). Link acks keep flowing because the
UDP receive thread is separate (UDP_TRANSPORT_2009.md §4, §11; PLAUSIBLE that a multi-second load never trips the
90 s idle drop). The server must expect a gap of several seconds before `C2S_LOAD_STATE(0)`.

Host (EVIDENCE: engine+0x8ddd0, PROVEN): `srv_clock` = the server's game-time global (seconds since the world
started, step 3), `world_file` = the id of the current world in the used-file registry; the `S2C_EVAR` list goes
first; then the client moves to phase 2 (section 8.2).

What the client does with the path: the file name came with `S2C_FILE_LIST`; the client opens the file from its
own resource archives (section 4.4). The server never sends world geometry. The world must be the same file on
both sides (the client checks nothing; a different world on the server would only desynchronise objects).

### 4.7 `S2C_PRELOAD` (id 19)

`[sub:u8]` then, for the list kinds, `u16` values until the end of the message (EVIDENCE: engine+0xa45b0,
PROVEN):

| sub | meaning (our reading) | client action | grade |
|---|---|---|---|
| 0 | preload start: reset the client's preload state | engine+0x25090, engine+0xd8e20, engine+0xd9150 | PROVEN calls, meaning PLAUSIBLE (the host sends it first, engine+0x9ea50) |
| 1 | **preload end, answer requested** | sends `C2S_LOAD_STATE(1)` = `[9][1]` reliably | PROVEN |
| 2 | model file ids to load now | per id: load model (engine+0x15450); a missing file only logs | PROVEN |
| 3 | file ids to load and mark (kind 2 references) | engine+0x259d0, sets a cache flag on the result | PROVEN calls, meaning UNPROVEN |
| 4 | file ids handed to engine+0xa1970 | | PROVEN call, meaning UNPROVEN |
| 5 | file references of kind 4 handed to the file service and the sound layer (engine+0xd8e20 / engine+0xdabf0) | | PROVEN calls, meaning UNPROVEN (plausibly sounds) |
| 6 | cached model ids to load (engine+0x17870) | | PROVEN |
| other | ignored, result 0 | | PROVEN |

Host (EVIDENCE: engine+0x8ccd0, engine+0x9ea50, engine+0x8cb60, PROVEN), after it receives
`C2S_LOAD_STATE(0)`: `[19][0]`, one model message per server model (callback engine+0x9e930, DISASM; the same
`[19][2 or 6][file_id:u16]` form as the later single-model writer engine+0x9e140, PROVEN), `[19][1]`, then the sub
4, 3 and 5 lists from the server's precache table (each only when non-empty), a sub 5 list of object-bound files,
and **`[19][1]`** again. The client therefore answers `C2S_LOAD_STATE(1)` twice; the host acts on the first one
(section 8.2). All preload lists are optional for correctness (a missing preload only means loading on first
use, PLAUSIBLE); `[19][1]` is what makes the client report `C2S_LOAD_STATE(1)`.

### 4.8 `S2C_SKY_TABLE` (id 16)

`[sky_block:384 bits][count:u16][count x sky_object:u16]` (EVIDENCE: client engine+0xa44f0 DISASM, host
engine+0x8d5a0 PROVEN). Client: copies the 48-byte block to the client manager (+0x9cc), fills the 30-entry
object table (+0x98c) with 0xFFFF, then stores `count` ids. **`count > 30` -> result 1** (session ends with
reason code 1, section 6). Host: always `count = 30` (its whole table), sent right after the preload lists. The
block's layout: part B 8.5 (REFERENCE + ROM since spec-fold-8: the outer and inner boxes of one sky world-model object; formerly UNPROVEN). The reference stack sent it after the
preload lists and before `S2C_VIEW_OBJECT` (tdm6 seq 701, tdm4 seq 788). Name note: part B
calls this message the "world slot table"; it is the same message.

**CORRECTION 2026-10-26 (report client-hudgaps; DISASM engine+0xa44f0, engine+0x188c0, engine+0x193ea0, engine+0xfb8c0, engine+0x1b4d50; read in the clean tree):** the clouds and the sunset glare are not special sky effects but
**sky-box world models**, and this table is what selects them. The client keeps the 48-byte block and up to 30 object ids; each scene resolves the ids to objects (engine+0x188c0); the sky pass
runs only when the scene has sky objects (engine+0x193ea0); the sky eye is placed in the block's view box (floats 6..11 of the block, engine+0xfb8c0); and only objects with the visible bit
draw (engine+0x1b4d50). So the table names which of a world's sky world models are drawn, and the view box places the eye inside them. A client that ignores the table and uses another eye (for
example the centre of all the sky boxes of a world) draws the boxes as floating quads over the clear colour. A world with two sky boxes (one ROM world has two) shows only the one the table names. The
draw order inside the sky pass is bucket order, not table order. See `RENDER_2009.md` 4.4 / 5.3.

### 4.9 `S2C_VIEW_OBJECT` (id 7): client-object-id assignment

`[object_id:u16]` -> `srv_conn+0x64` (EVIDENCE: engine+0xa2a50, DISASM). The engine's "own object" accessor
(client interface slot +0x20c, engine+0xa5530, PROVEN) returns an object only when the client is connected,
`srv_conn+0x64 != 0xFFFF`, and the client object map holds an object (map entry kind 1) at that id
(engine+0x25370, PROVEN). The object itself is created by an object record in the id 8 message (part B §3).
Order is free: the id may arrive before or after the create record; the accessor returns nothing until both
exist (PROVEN by the accessor logic).

Host (EVIDENCE: engine+0x8db60, PROVEN): after the server game code has created the player object (callback
with the hello blob, section 8.2), `[7][object id]`, reliable, also copied to clients attached as viewers.
engine+0x8e000 / engine+0x8d0c0 resend it when a client is attached to (spectates) another client's object or
detached; then the id is the viewed client's object. Reset to 0xFFFF on the client by `S2C_LOAD_WORLD` and
`S2C_LEAVE_WORLD`.

### 4.10 `S2C_LEAVE_WORLD` (id 5): unload / change world

No body. Client (EVIDENCE: engine+0xa2960 DISASM -> engine+0x1a2c0 PROVEN): view object id := 0xFFFF, unload the
world if one is loaded (engine+0x17360), drop the per-world object lists, sound and effect state. The link stays
up. Host (EVIDENCE: engine+0x8ceb0, PROVEN): calls the server game code's exit-world callback, detaches the
client's object, sends `[5]` reliably; it is used when a client leaves phase 3 (phase setter engine+0x8e160),
that is on a world change. **Change-world** = `S2C_LEAVE_WORLD`, then the load sequence of section 9 from step
S5 (`S2C_EVAR`..., `S2C_LOAD_WORLD`) with the new world file (announced first with `S2C_FILE_LIST` if it is new).
`S2C_LOAD_WORLD` alone also works on the client because it unloads first (step 2 of 4.6).

### 4.11 Game-layer messages needed before the world is playable (owned by part B)

The engine is in a world after 4.6 and has a client object after 4.9 plus the create record, but the CShell
leaves its loading screen only when its own test passes (section 8.3). Two game-layer items are known to be
involved:

- shell message **0x88** (inside `S2C_SHELL_MSG`, id 13): `[shell_id:u8 = 0x88][state:u8]` sets the game
  shell's switching-worlds state (shell+0x1a0c; EVIDENCE: cs+0x1a86b0 case 0x88 -> cs+0x1a58f0, PROVEN). The
  in-world test requires **state 2 or 3** (EVIDENCE: cs+0x1f3950, PROVEN). Every reader of shell+0x1a0c found in
  the disassembly (cs+0x1f1680, cs+0x1f3950, cs+0x1f95d0) tests only "2 or 3", and its only other writers store 0
  (shell constructor cs+0x1aa910, shell event method cs+0x1a8470), so **2 and 3 are equivalent on the client**;
  which one the 2009 server sent stays UNPROVEN and does not matter. The six handlers ahead of the fallback switch
  do not consume 0x88: stage a cs+0x49a40, stage b cs+0x2c430 and stage c cs+0x60d90 / cs+0x1f0470 have no such
  case (PROVEN, clean tree), stage d cs+0x23c570 handles only 0x72, 0x73, 0x9e, 0xac (DISASM jump table), stage e
  cs+0x5ac40 only 0x97, 0x99, 0xc6, 0xc9 (part B 7.1), and stage f cs+0x26b220 returns 0 for every id (DISASM:
  `xor eax, eax; ret 8`).
- shell message **0x72** (server -> client) makes the connection manager answer with a long client -> server
  shell message 0x72 carrying character data (EVIDENCE: cs+0x23c570 -> cs+0x23ccb0, DISASM: the answer starts
  `[0x72][u32][u32][u16][u32]...` read from the character-info record). This is very likely the game server's
  "who are you / loadout" request; its layout belongs to part B and is not needed for the engine to load the
  world.

## 5. Client -> server messages of this phase

### 5.1 Summary

| our name | id | dir | body (after `msg_id:u8`) | client writer | host handler |
|---|---|---|---|---|---|
| `C2S_HELLO` | 5 | C->S | `blob_len:u16` (bytes), `blob_len x 8` bits of hello blob | engine+0x18d00 PROVEN | engine+0x90b40 DISASM |
| `C2S_BYE` | 6 | C->S | (none) | engine+0xa48d0 PROVEN | engine+0x90ae0 PROVEN |
| `C2S_RATE` | 7 | C->S | `rate_8k:u16`, then `u16` ids to the end (part B) | engine+0x1ae50 PROVEN | engine+0x90a80 PROVEN |
| `C2S_LOAD_STATE` | 9 | C->S | `state:u8` (0 = world loaded, 1 = preload done) | engine+0xa3e90 DISASM (0), engine+0xa45b0 PROVEN (1) | engine+0x90b20 DISASM |
| `C2S_FILE_ACK` | 0x36 | C->S | `{entry:u16}` to the end; `entry = file_id \| 0x8000` when accepted | engine+0x3e990 PROVEN | engine+0x3f740 PROVEN |
| (part B) | 8, 10, 11 | C->S | tracked-sound acknowledgements (`C2S_SOUND_ACK`), console command, shell message | part B | engine+0x90a10, 0x92270, engine+0x922d0 (DISASM) |

All of them are sent reliably (send flag 1 at every writer listed; PROVEN in the C, DISASM for the `state = 0` writer).

### 5.2 `C2S_HELLO` (id 5): the connect message

- Sent exactly once, by `start_game` right after the join succeeded and `srv_conn` exists (EVIDENCE:
  engine+0x18d00 lines writing `5`, `(u16) request+0x11b8`, then `request+0x11b8 * 8` bits from the pointer at
  `request+0x11b4`, PROVEN). It is the **first reliable message on the link** (rel_seq 1 from the
  client), built before the client's session layer has handled anything from the server; its datagram may already
  acknowledge the server's first block (OBSERVED tdm1: `CTL_ACK 1` in the hello datagram, 47 ms after the join reply).
  PROVEN-CAPTURE (tdm1 c2s seq 269, tdm2 conns 6 and 7): the entry is 2,088 bits = `05`, `02 01` (258), 258 blob bytes:
  offset 0x00 the 8-bit name (5 ASCII characters + NUL, zero padding to 0x3f); 0x40 the 36-character GUID text (above);
  0x80 and 0x81 zero; 0x82 the same name in UTF-16, NUL-terminated; every other byte 0. The UTF-16 name equals the login
  name of the auth message and the name the lobby echoes, so matching by it (CDS-A3 / CD-60) works for the real client.
  On a re-join the hello is byte-identical to the first one (tdm2 conn 7 against conn 6).
- `blob_len` = 0x102 (258) and the blob is the hello blob of section 2.3 (PROVEN via cs+0x2fac0 and the DISASM
  of cs+0x2f930 / cs+0x2f9e0, which store length 0x102).
- Host (EVIDENCE: engine+0x90b40, DISASM): `blob_len` (u16) is stored, a buffer of that size is allocated and the
  bytes are read into it, and the client's "hello received" flag (bit 0x80 of the client flags) is set. The host
  keeps the blob and gives it, with its length, to the server game code when the client enters the world
  (engine+0x8db60, PROVEN: callback `(client, blob, blob_len)`). A zero `blob_len` is accepted (no buffer).
- Our server: this message is the only player identification on the UDP side (SPEC: PROTOCOL_2009.md §7). Parse
  the name at blob offset 0x82 (UTF-16, NUL-terminated) or offset 0x00 (8-bit) and match it against the lobby's
  room member list for this game server (DC-DEFAULT proposal CDS-A3: match by the UTF-16 name, case-sensitive;
  refuse with a bye code when there is no match). The second string at offset 0x40 is a GUID text (OBSERVED tdm1, 2009
  client); its meaning and whether it changes per session are UNPROVEN; log it.

### 5.3 `C2S_LOAD_STATE` (id 9): the "world loaded" and "ready" reports

- `state = 0`: sent by the load-world handler after `world_load` succeeded (EVIDENCE: engine+0xa3e90, DISASM).
  This is the **world loaded** report.
- `state = 1`: sent when the client receives `[19][1]` (EVIDENCE: engine+0xa45b0 case 1, PROVEN). This is the
  **preload done / ready** report. It does not depend on the earlier preload lists: the client answers
  `[19][1]` whenever it arrives. PROVEN-CAPTURE (tdm1): the reference stack sent one `[19][1]` and the client answered with
  exactly one `09 01` in the same tick; `09 00` came 219 ms after `S2C_LOAD_WORLD` in the same block as the first
  `C2S_RATE`. A server may send `[19][1]` once.
- Host (EVIDENCE: engine+0x90b20, DISASM): `state = 0` -> load sub-state 1, any other value -> 3 (section 8.2).

### 5.4 `C2S_FILE_ACK` (id 0x36)

One message per `S2C_FILE_LIST`, one `u16` per announced entry, in the order received (EVIDENCE: engine+0x3e990,
PROVEN). The 2009 client always sets bit 15 (section 4.4). Host (EVIDENCE: engine+0x3f740, PROVEN): an entry with
bit 15 = 1 completes that file (pending counters decremented); bit 15 = 0 re-flags it for announcement. An id the
host does not know is ignored. PROVEN-CAPTURE (tdm1): 23 `S2C_FILE_LIST` messages (one batched with 10 entries, 22
single-entry) were answered by 23 `C2S_FILE_ACK`, one per list, entries in list order, every id with bit 15 set (the
batched answer `36 00 80 09 80 03 80 ...`); single-entry lists were answered in the same tick (20 of 22) or 47 ms later.

### 5.5 `C2S_RATE` (id 7)

`rate_8k = client_rate_bps / 8000`, clamped to 0..65535; the client's rate is the global at VA 0x6cb298 (PE-DATA
initial 35840 -> `rate_8k = 4`; the 2009 CShell sets it to 5120000 at profile load, transport 5.3, so a real client
sends `rate_8k = 640`: PROVEN-CAPTURE, tdm1 c2s seq 400, `07 80 02`, first right after `C2S_LOAD_STATE(0)` in the same
block). Sent from the per-frame client update (engine+0x14f90 ->
engine+0x1ae50, PROVEN) only while the server connection exists and its world-loaded flag (`srv_conn+0x6c`) is set
(PROVEN, engine+0x14f90), whenever `rate_8k` differs from the last value sent (initially 0, engine+0x16f00, so the
first frame after the world load always sends it) or the tracked-sound acknowledgement list is non-empty (part B).
When `rate_8k` changed the client also calls set-rate on its own link with the client-rate global (transport 12.2).
Host (EVIDENCE: engine+0x90a80, PROVEN): client rate := `rate_8k * 8000` bits per second, then the remaining `u16`
values are handled like C->S id 8 (`C2S_SOUND_ACK`, part B).

### 5.6 `C2S_BYE` (id 6)

No body, reliable (EVIDENCE: engine+0xa48d0, PROVEN; sent only while the link exists). Host (EVIDENCE:
engine+0x90ae0, PROVEN): disconnects the client's link with bye code **5**.

### 5.7 What the server must have seen before each step

The 2009 client itself enforces no order beyond reliable delivery; it handles each message when it arrives. The
gates below are the 2009 host's (EVIDENCE: engine+0x8b2e0, engine+0x8e1b0, engine+0x8db60, PROVEN) and are the
safe order for our server:

| before sending | the host requires | why it matters to the client |
|---|---|---|
| anything | the join (transport `OOB_JOIN_REPLY`, accepted = 1) | the client only parses link datagrams of a joined peer (UDP_TRANSPORT_2009.md §4) |
| `S2C_LOAD_WORLD` | `C2S_HELLO` received and every announced file answered in `C2S_FILE_ACK` | the world file id must already be in the client's file table (4.4); the hello identifies the player |
| preload lists, `[19][1]`, `S2C_SKY_TABLE` | `C2S_LOAD_STATE(0)` | the world must be loaded before world-bound tables are useful (PLAUSIBLE) |
| player object creation, `S2C_VIEW_OBJECT` | `C2S_LOAD_STATE(1)` | the object records arrive after the world exists (the host never sends them earlier) |
| `S2C_DIR_LIGHT` (id 22, part B 8.4) | a loaded world (the 2009 host sends it with the other world-bound tables when a global light object exists, option bit 0x200; `RENDER_2009.md` 6.3.1, EVIDENCE engine+0x8d6c0) | DeCombat send point (D, DSV-40 details (proposed, PENDING); `dc-dserver` `worldlight.rs`, merged at master 9126fb2; TESTED `session_sim`: sky table < id 22 < view object, none with `-globallight off` or without the object; a binary run on a ROM world got exactly one): reliable, once per client, **after the preload lists and the optional sky table, before `S2C_VIEW_OBJECT`**. The reference stack sent one per link during the join and the real client accepted it (tdm2 / tdm4, REFERENCE-CONFIRMED, RENDER 6.3.1); no 2009 server capture. The reference stack sent it after `S2C_VIEW_OBJECT` (tdm6 seq 722, tdm4 seq 807 after 799) and our rust3 server before it with the same seven floats (difference below 2e-7): the client accepted both orders (capval-rust3 9.3; spec-fold-8) |

## 6. Disconnect and failure reasons shown to the player

### 6.1 How a session ends with a reason

1. A client message handler returns a non-zero result (section 3.3). The per-frame update returns
   `result | 0x2000000` (EVIDENCE: engine+0x198f0, PROVEN).
2. Error processing (EVIDENCE: engine+0x24d40, PROVEN) destroys `srv_conn` and calls the client shell's event
   method (slot +0x1c of the shell object, which is cs+0x1a8470, DISASM) with event **1** (disconnect) and
   `code = result & 0xF9FFFFFF`.
3. The shell clears its in-world state (entered-world flag, switching-worlds state) and forwards the event to
   the interface manager (its method +0x28, which is cs+0x1bbfb0 in both method tables VA 0x374ec3dc / 0x374d07ac,
   PE-DATA) and to `conn_mgr` method +0x20 (DISASM cs+0x1a8470). The interface event handler (EVIDENCE:
   cs+0x1bbfb0 case 1, PROVEN) calls `disconnect_box(code)` (cs+0x1bb2f0) when the connection request kind is 3
   (remote join).

Session-ending results produced by the start-up handlers of this part:

| code | produced by | evidence |
|---|---|---|
| 0x4E | `S2C_VERSION` with `net_version != 7` | engine+0xa2a70 PROVEN |
| 0x29 | `S2C_LOAD_WORLD`: world file id unknown or its file not found in the client resources | engine+0x1a770 PROVEN |
| 0x2A | `S2C_LOAD_WORLD`: world file failed to load | engine+0x1a770 PROVEN |
| 0x2B | `S2C_LOAD_WORLD`: world failed to bind | engine+0x1a770 PROVEN |
| 1 | `S2C_LOAD_WORLD`: visibility containers failed (or the local-server branch failed); `S2C_SKY_TABLE` with `count > 30` | engine+0x1a770 PROVEN, engine+0xa44f0 DISASM |
| 0x2C | `S2C_BUNDLE` with an inner length past the end; malformed object group records (part B) | engine+0xa3f80 DISASM, engine+0xa4ef0 DISASM |

### 6.2 Reason codes and the text the client shows

`disconnect_box` (EVIDENCE: cs+0x1bb2f0, PROVEN) loads a message text from the ROM's CRes string table by id
(texts stay in the ROM; only the ids are given):

| code | text id | extra | grade |
|---|---|---|---|
| 1 | `res 0x3ef` | the shell's log line names an application-id mismatch | PROVEN id, meaning PLAUSIBLE |
| 2 | `res 0x3f2` | log line names a wrong password | PROVEN id, meaning PLAUSIBLE |
| 3 | `res 0x178c` | codes other than 1 and 2 also set the interface's return-to-room flag (+0x6a6d) | PROVEN |
| 6 | `res 0x5bb` | | PROVEN |
| 7 | `res 0x432` | | PROVEN |
| 8 | `res 0x462` | formatted with two values from a shell global (VA 0x37560cfc, fields +0x1c and a table entry selected by +0x18) | PROVEN |
| 9 | `res 0xbbe` | | PROVEN |
| 10 | `res 0x58c` | | PROVEN |
| 11 | `res 0x58c` | through the shut-down-with-message path (cs+0x38290): the client closes after the box | PROVEN |
| any other (0, 4, 5, 0x29, 0x4E ...) | `res 0x482` | the code is only written to the log | PROVEN |

After the box: the game-start-failed flags of the character-info record are set, and the interface goes back to
the waiting room (screen 9); if the interface was in the loading state (3), `loading_tick` performs the return
(EVIDENCE: cs+0x1bb2f0 tail, PROVEN). The lobby TCP connection stays up (SPEC: PROTOCOL_2009.md §7).

**Kicks by the server.** The 2009 host disconnects a client with a transport bye code (`CTL_BYE_CODE`,
UDP_TRANSPORT_2009.md §7): code 5 after `C2S_BYE`, code 6 for a boot (engine+0x9bb10 -> engine+0x79d30, PROVEN),
code 3 on idle timeout. Whether the received bye code reaches `disconnect_box` unchanged is **UNPROVEN**: the
client's network handler ignores the drop notification (method +0x08 of the table at VA 0x6755dc is an empty
function, DISASM engine+0x16ed0) and the path from the dropped link to shell event 1 was not found in the C. Test
V-A6 (section 9.3) sends bye codes 1..11 and records the box.

### 6.3 Connection-timeout box

The box shown when the game connection cannot be made is not a reason code. `loading_tick` (EVIDENCE:
cs+0x1bb910, PROVEN) checks the connection manager's last start result (`conn_mgr+0x11dc`, written by the
engine start call, section 2.4): when it is non-zero, the client marks the load as failed, shows **`res 0xbd0`**
(OBSERVED as the "Timeout" box, SPEC: PROTOCOL_2009.md §7 LIVE), sets the game-start-failed flags and returns to
the waiting room, after which the lobby sees `match_exit` 0x394 (formerly `match_join_failed`; PROTOCOL_2009.md §3.7). Every non-zero
start result gives the same box: 0x4F join timeout (the usual case), 0x50 no socket / bad address, 0x51 refused,
0x52 wrong application id, 0x49 driver missing (UDP_TRANSPORT_2009.md §5.2, engine+0x18d00).

## 7. Timers

### 7.1 Client timers in this phase

| timer | value | effect | evidence |
|---|---|---|---|
| join retry | 300 ms | resend `OOB_JOIN_ASK` | UDP_TRANSPORT_2009.md §5.2 (engine+0xb6850) |
| **join timeout** | 10000 ms after the first ask | start result 0x4F -> **`res 0xbd0` box** (6.3) | engine+0xb6850, cs+0x1bb910 PROVEN |
| link idle drop | 90000 ms without any received datagram | link dropped with code 3 | UDP_TRANSPORT_2009.md §13 (engine+0xb5c30) |
| keep-alive | ack + empty reliable block every `max(200, L)` ms | keeps the server's idle timer quiet during a long load | UDP_TRANSPORT_2009.md §11.1 |
| tick length after load | 1/30 s (stored by `world_load`) | client-side interpolation base until updates arrive (part B) | engine+0x1a770 PROVEN |

The join runs synchronously inside the engine start call (section 2.4), so the client's main loop does not run
during those up to 10 s (PLAUSIBLE: `start_game` calls `join_peer` directly and `join_peer` polls the socket
until reply or timeout, engine+0x18d00 / engine+0xb6850).

No session-level timeout was found for the start-up sequence itself: neither `world_load`, the handlers of
section 4, nor `loading_tick` (cs+0x1bb910) has a timer (PROVEN for those functions; UNPROVEN that no other
code watches the loading screen). A server that completes the join but never sends `S2C_LOAD_WORLD` leaves the
client on the loading screen for as long as the link stays alive (PLAUSIBLE; check with V-A5).

### 7.2 Host timing (reference for our server)

- The per-client tick (EVIDENCE: engine+0x8e1b0, PROVEN) runs every server frame: in phase 1 it sends the world
  as soon as the ready test passes; in phase 2 it runs the loading step; then it ticks the file announcer with
  the server time in seconds (`time_ms * 0.001`).
- Ready test (EVIDENCE: engine+0x8b2e0, PROVEN): no world change pending, hello received (flag 0x80), and the
  file announcer has no unanswered "required" entry (flag 1 entries; for a client with flag 1 set, no unanswered
  entry at all). A local client (flag 2) is always ready.
- There is no host timer on the start-up steps either: a client that never answers stays in its phase until the
  transport idle drop (90 s) removes the link (PROVEN for the functions listed; the drop is transport behaviour).
- DC-DEFAULT proposal CDS-A4: our server drops a client that has not reached phase 3 within 120 s of the join
  (bye code 3), so a stuck client does not hold a player slot.
- Host tick (EVIDENCE: engine+0x198f0 -> engine+0x9e4e0, PROVEN): the in-process 2009 host runs one server frame
  per client frame, with the client frame time in ms; the frame delta is clamped to 0.2 s and scaled by the time
  scale (4.6 step 3). There is no fixed tick rate in the 2009 Engine.exe; the dedicated 2009 server is not part of
  the ROM. The client's own expectation of the stream period is `1 / U` (part B 9.1), and the 2009 CShell sets `U`
  to 60 on a fresh profile (part B addendum B12). DeCombat chooses its tick rate (proposal in part B).

## 8. Client state machine

### 8.1 Client states (our names)

| state | entered by | leaves on | evidence |
|---|---|---|---|
| `ST_LOBBY` | lobby login | `match_address_push` 0x3043 -> `ST_JOINING` | cs+0x165650 |
| `ST_JOINING` | `match_connect` -> engine start call | join accepted -> `C2S_HELLO` sent -> `ST_LINKED`; any start error -> `ST_FAILED` | cs+0x1630a0, engine+0x18d00 |
| `ST_LINKED` | join done; loading screen up (interface state 3) | `S2C_LOAD_WORLD` -> `ST_LOADING_WORLD`; handler error or lost link -> `ST_DROPPED` | engine+0xa52f0 |
| `ST_LOADING_WORLD` | inside the `S2C_LOAD_WORLD` handler | success: `C2S_LOAD_STATE(0)`, entered-world callback -> `ST_WORLD_LOADED`; error code -> `ST_DROPPED` | engine+0xa3e90, engine+0x1a770 |
| `ST_WORLD_LOADED` | world loaded, shell entered-world flag set | in-world test passes -> `ST_IN_WORLD`; `S2C_LEAVE_WORLD` -> `ST_LINKED`; `S2C_LOAD_WORLD` -> `ST_LOADING_WORLD` | cs+0x1bb910, cs+0x1f3950 |
| `ST_IN_WORLD` | loading screen closed, game state 8 | part B; `S2C_LEAVE_WORLD` -> `ST_LINKED`; `S2C_LOAD_WORLD` -> `ST_LOADING_WORLD` | cs+0x1bb910 |
| `ST_FAILED` | non-zero engine start result | box `res 0xbd0`, waiting room, lobby gets 0x394 | cs+0x1bb910 |
| `ST_DROPPED` | session-ending result or disconnect | box by reason code (6.2), waiting room | engine+0x24d40, cs+0x1bb2f0 |

Messages accepted in each linked state (the client never rejects a message for arriving "early"; it just
applies it): `S2C_VERSION`, `S2C_CLIENT_ID`, `S2C_FILE_LIST`, `S2C_EVAR`, `S2C_SHELL_MSG`, `S2C_PRELOAD`,
`S2C_SKY_TABLE`, `S2C_VIEW_OBJECT` in any linked state (PROVEN: the handlers test no state). Object records
(id 8) need a loaded world to be useful (part B).

```
 ST_LOBBY --0x3043--> ST_JOINING --join ok, C2S_HELLO--> ST_LINKED --S2C_LOAD_WORLD--> ST_LOADING_WORLD
     ^                    | start error                     ^   |  error                    | ok: C2S_LOAD_STATE(0)
     |                    v                                 |   v                           v
     +---- waiting room <- ST_FAILED (res 0xbd0)            |  ST_DROPPED (6.2) <---- ST_WORLD_LOADED
     +---- waiting room <------------------------------------+-----------------------   | in-world test
                                             S2C_LEAVE_WORLD |                           v
                                                             +----------------------- ST_IN_WORLD (part B)
```

### 8.2 Host reference states (2009 Engine.exe as server)

Per client: phase (`client+0x6c`), load sub-state (`client+0x70`), flags (`client+0x74`; 0x80 = hello
received, 0x02 = local client). EVIDENCE: engine+0x8e9f0, engine+0x8e1b0, engine+0x8ddd0, engine+0x8db60,
engine+0x8e160, engine+0x90b20 (DISASM), engine+0x90b40 (DISASM).

| phase / sub | host does | moves on |
|---|---|---|
| new link | `S2C_VERSION`, `S2C_CLIENT_ID`, one `S2C_FILE_LIST` with all used files; phase 1 | at once |
| 1 | waits | ready test (7.2) passes -> `S2C_EVAR` per variable, `S2C_LOAD_WORLD` -> phase 2, sub 0 |
| 2 / 0 | waits | `C2S_LOAD_STATE(0)` -> sub 1 |
| 2 / 1 | preload messages (`[19][0]`, models, `[19][1]`, lists, `[19][1]`), `S2C_SKY_TABLE` | sub 2 at once |
| 2 / 2 | waits | `C2S_LOAD_STATE(1)` -> sub 3 |
| 2 / 3 | game code's enter-world callback with the hello blob creates the player object; `S2C_VIEW_OBJECT`; per-client object tables reset | phase 3 |
| 3 | in world: object groups each tick (part B) | phase change away from 3 -> `S2C_LEAVE_WORLD`; to load again the host re-runs phase 1 |
| any | `C2S_BYE` -> drop with bye code 5; boot -> bye code 6 | link gone |

A local client (flag 2) skips the waits in phase 2 (PROVEN, engine+0x8db60). Our server has no local clients.

### 8.3 CShell "in world" test

`loading_tick` (cs+0x1bb910) closes the loading screen and switches the game to its playing state (game shell
state 8) when `in_world_test` (cs+0x1f3950, PROVEN) is true. All four conditions must hold:

| # | condition | what makes it true | grade |
|---|---|---|---|
| a | engine client interface slot +0x20c returns an object | `S2C_VIEW_OBJECT` with the id of an object that exists in the client object map (create record, part B §3) | PROVEN (engine+0xa5530) |
| b | game shell switching-worlds state is 2 or 3 | shell message 0x88 inside `S2C_SHELL_MSG` (4.11) | PROVEN |
| c | game shell entered-world flag (+0x1efc) is set | the entered-world callback after a successful `S2C_LOAD_WORLD` (cs+0x1a5590 sets it; cs+0x22fa50 clears it when loading starts; disconnect clears it) | PROVEN (cs+0x1a5590), DISASM (cs+0x22fa50, cs+0x1a8470) |
| d | field +0x30 (the local player's state) of the player object at VA 0x3759bf60 is non-zero | shell message **0x67** inside `S2C_SHELL_MSG`: `[0x67][state:u8]`, nothing else. Chain: dispatcher stage c (getter +0xb0 = cs+0x41c10 returns the shell member at +0x16054, the only instance of the player class; its derived handler cs+0x60d90 always ends in the base handler cs+0x1f0470), case 0x67 -> method +0xb8 = cs+0x61870 -> cs+0x1f97d0, which reads `state:u8` into +0x30 and runs the state's setup (cases 1 to 4). The global VA 0x3759bf60 is set to this object by cs+0x1f6fb0. The longer reader cs+0x1f4ab0 (method +0x10) also writes +0x30 but has no caller visible statically | PROVEN reader (clean tree 1f97d0.c, 1f0470.c), DISASM chain. Carrier 0x67 RESOLVED statically (GAME_MESSAGES_2009_S2C.md 4.1) and PROVEN-CAPTURE by the client's reaction (tdm1: `0d 67 01` at seq 410, then shell 0x87 47 ms later with no 0x67 state 3 before it; the reference stack's form is state 1 directly) |

**Observed (PROVEN-CAPTURE for the moment and order, tdm1).** With 0x88 state 2 (sent at link creation, in the first block, before
`S2C_VERSION`), the view object id (`07 98 01`, seq 406) named **before** the create record of that object (the create completed with
the 20-part object group at seq 448), and 0x67 state 1 without an earlier state, the 2009 client sent shell 0x87 47 ms after the last of
them, together with the reliable 0x64 state form `0b 64 10 00 01` (state byte 1) and the first unreliable 0x64 update. No 0x64 of either form
was sent before 0x67 arrived. This is a sufficient set; that each condition is necessary is not shown. 4.9 "order is free: the id may arrive
before or after the create record" is confirmed (id first; the test waits for the record). The reference stack sent 0x88 state 3 and 0x90 `[0]` after the 0x87.
tdm2 adds the same for two more links (one 0x87 per link, 63-109 ms after 0x67 state 1).

## 9. First vertical slice

The minimal sequence a `dc-dserver` must run to take the real 2009 client from the join reply to a loaded
SnowValley world with its own client object. Every message is one reliable entry (UDP_TRANSPORT_2009.md §8)
unless marked. Byte strings below are the message bits read 8 at a time from the entry's first bit (all fields
of these messages are whole bytes, so the entry equals this byte string; inside the datagram it is not byte
aligned). Grades: each step inherits the grade of the section it cites.

### 9.1 Preconditions

- P1. The lobby has sent `match_address_push` 0x3043 with this server's IPv4 text and UDP port (SPEC:
  PROTOCOL_2009.md §7) and has told the game server which characters belong to the room (internal, DeCombat design).
- P2. The server knows the client resource path of the room's world file. For SnowValley, take it from the ROM at
  run time (`dc-bute` world mapping for the room's map and mode, SPEC: LOBBY_HARNESS.md row 15; the path as
  `dc-rez` lists it in the world archive). Do not hard-code it (ROM data).
- P3. The server owns a file-id table: W = world file id, plus one id per model / texture file the player
  object's create record will reference (part B §3). Ids are any unique u16 values (section 4.4); suggestion:
  W = 1, then 2, 3, ...
- P4. The transport of UDP_TRANSPORT_2009.md runs underneath (acks, keep-alive, resend, split). The server answers
  `OOB_JOIN_ASK` with `OOB_JOIN_REPLY accepted = 1` (UDP_TRANSPORT_2009.md §5.3) and creates the link.

### 9.2 Steps

| step | dir | message | exact content | wait for / check | ref |
|---|---|---|---|---|---|
| S1 | C->S | `C2S_HELLO` | `05`, `02 01` (258), 258 blob bytes | first reliable entry from the client; it can arrive before S2. Parse the UTF-16 name at blob offset 0x82; unknown name -> bye code (CDS-A3) | 5.2, 2.3 |
| S2 | S->C | `S2C_VERSION` | `04`, `07 00 00 00`, `00 20 4E 00` (5120000) | none; send right after the join reply | 4.2 |
| S3 | S->C | `S2C_CLIENT_ID` | `0C`, `cid:u16` (e.g. `01 00`), `00` | none | 4.3 |
| S4 | S->C | `S2C_FILE_LIST` | `32`, then per file: `id:u16`, `info:u32` (size or 0), name bytes, `00`; first entry W with the world path, then the model / texture files of the player object | none | 4.4 |
| S4a | C->S | `C2S_FILE_ACK` | `36`, then one `u16` per entry, each `id \| 0x8000` | wait for it **and** for S1 before S5 | 5.4, 5.7 |
| S5 | S->C | (optional) `S2C_EVAR` x n | `0F`, name, `00`, value, `00` | none required (4.5) | 4.5 |
| S6 | S->C | `S2C_LOAD_WORLD` | `06`, `00 00 00 00` (`srv_clock` 0.0), `W:u16` (e.g. `01 00`) | the client now loads the world (seconds) | 4.6 |
| S6a | C->S | `C2S_LOAD_STATE(0)` | `09 00` | world loaded. If instead the client drops with reason 0x29, the name sent in S4 did not resolve (10.1 #3) | 5.3, 6.1 |
| S7 | S->C | `S2C_PRELOAD` start (optional) and end | `13 00` (optional), `13 01` | none | 4.7 |
| S8 | S->C | (optional) `S2C_SKY_TABLE` | `10`, 48 bytes `00`, `00 00` (count 0) | none; content: part B 8.5 (the sky object's boxes; spec-fold-8), DeCombat DSV-45 | 4.8 |
| S8b | S->C | (optional) `S2C_DIR_LIGHT` | `16`, `dir:3 x f32`, `color:3 x f32`, `extra:f32` from the world's global light object; nothing when the world has none | none; DeCombat order (5.7 row, D DSV-40): after S7 / S8, before S10 | part B 8.4 |
| S8a | C->S | `C2S_LOAD_STATE(1)` | `09 01` | ready; the server may now create the player object | 5.3 |
| S9 | S->C | `S2C_OBJ_GROUP` (id 8) with one create record | object id O (unique u16), model / texture ids from S4, position inside the SnowValley world | part B §3 | part B |
| S10 | S->C | `S2C_VIEW_OBJECT` | `07`, `O:u16` | the engine now resolves its own object (8.3 a) | 4.9 |
| S11 | S->C | `S2C_SHELL_MSG` with shell 0x88 | `0D`, `88`, `02` (2 and 3 are equivalent on the client, 4.11) | 8.3 b | 4.11 |
| S12 | S->C | `S2C_SHELL_MSG` with shell 0x67 (player state) | `0D`, `67`, `01` (state 1: any non-zero value passes 8.3 d; state 1 is the one in which the client sends its player updates, part B 10.3). Carrier RESOLVED statically (GAME_MESSAGES_2009_S2C.md 4.1, DISASM), UNPROVEN by capture | 8.3 d | 8.3 |
| S13 | both | steady state: object groups / streams each server tick, shell messages, `C2S_RATE` etc. | part B | the client closes its loading screen when 8.3 a-d hold | part B |

Notes:

- The client sends `C2S_RATE` once its world is loaded (5.5): `07 80 02` (640) with the CShell rate, `07 04 00`
  only if the engine default is in effect; it may send it again; accept it at any step (5.5).
- S2, S3 and S4 may be sent in any order and before S1 arrives; the 2009 host sends them on link creation
  (8.2). They may also be packed into one `S2C_BUNDLE` (3.4); not needed.
- After S6 the client processes nothing until the world is loaded; do not treat the silence as a failure (the
  transport keeps the link alive). CDS-A4 bounds the whole start-up at 120 s.
- OBSERVED tdm1 (PROVEN-CAPTURE): the 2009 client handled no session message for 6.1 s after its join (its first
  `C2S_FILE_ACK` came 6.08 s after the server's first block) while its transport kept the 200 ms keep-alive and
  acknowledged every block; the world load after S6 then took 0.22 s. The cause is not visible on the wire (UNPROVEN;
  candidates: the loading screen's own set-up, or loading done before session messages are dispatched). tdm2 shows the
  same (5.7 s from the room start push to the world load). A server must not treat several seconds without session
  answers after the join as a failure (CDS-A4's 120 s bound covers it).
- Join of a running world (OBSERVED tdm2, REFERENCE form accepted): the first block of a link to a world that is already running
  starts with shell 0x90 `[0]` and 0x88 `[3]` (a starting world sends `[1]` and `[2]` first, and `[0]` / `[3]` to every link when the
  world starts). The client answers a re-join exactly as a first join (same hello, same 0x72, one 0x87).
- A "loaded world with its own client object" in engine terms is reached after S10 (`world_load` done +
  `srv_conn+0x64 = O` + object O created). The CShell's visible "in game" needs S11 and S12 as well.
- Change of world later: `S2C_LEAVE_WORLD` (`05`), then S4 (new files only), S5, S6 ... as above (4.10).

### 9.3 Validation (black box; the coordinator schedules any live run)

Same tap as UDP_TRANSPORT_2009.md §18 between the real 2009 client and the reference game server, decoded
offline with a decoder built from this spec.

| id | checks | pass condition |
|---|---|---|
| V-A1 | first client reliable entry | id 5, `blob_len = 258`, UTF-16 name at blob 0x82 equals the 8-bit name at 0x00 |
| V-A2 | first server entries | ids 4 (value 7), 12 (`local_host = 0`), 0x32; record the world's announced name form (closes 10.1 #3) and the rate value |
| V-A3 | order and gates | 0x36 answers every 0x32 entry with bit 15 set; the server's id 6 comes after the client's 5 and 0x36; `srv_clock` value recorded |
| V-A4 | game layer | record the shell messages between the client's `[9][1]` and the end of the loading screen (0x88 value; confirm that the player-state message for 8.3 d is shell 0x67 `[state:u8]`; confirmed, tdm1) |
| V-A5 | stuck start-up | our test server stops after S4: the client stays on the loading screen; measure until something happens |
| V-A6 | kick reasons | our test server sends `CTL_BYE_CODE(n)` for n = 1..11 after S10: record the box text id shown (closes 6.2 "kicks") |
| V-A7 | version | our test server sends `S2C_VERSION` with 8: the client drops with the generic box (`res 0x482`) |
| V-A8 | missing world | `S2C_LOAD_WORLD` with an id never announced: drop with the generic box (code 0x29) |
| V-A9 | slice | our server runs S1-S13 against the real client: the client reaches the playing state in SnowValley (user-confirmed live test) |

**Capture results (tdm1; Appendix C has the detail).** V-A1 pass (PROVEN-CAPTURE; the second string is a 36-character GUID text); V-A2 pass
(REFERENCE-CONFIRMED: ids 4 (value 7), 12 (`local_host` 0), 0x32 accepted; world file id 0; `srv_clock` 0.0); V-A3 pass (PROVEN-CAPTURE for
the client's answers; `C2S_FILE_ACK` bit 15 on every entry; the reference stack sent `S2C_LOAD_WORLD` 31 ms after the batched ack and after the hello);
V-A4 pass as a sufficient set (PROVEN-CAPTURE, 8.3); V-A5 to V-A9 undecided (they need our own server in front of the real client). 6.2 "Kicks by the
server" stays open: the reference stack's `CTL_BYE_CODE(5)` arrived after the client's own `C2S_BYE`, so it shows nothing about which box a code selects.

## 10. UNPROVEN items

### 10.1 Open facts

1. CShell: closed for the caller chain (2.2 step 5, 2.4) and the code page (2.3, system ANSI). Open: the meaning of
   the second request string (room object +0x5530) and of the hello blob's second 8-bit string (offset 0x40; its form is
   a 36-character GUID text, OBSERVED tdm1, PROVEN-CAPTURE).
2. Engine: `file_info` is the file size (4.4). Open: the first id value of the host's file-id counter (any unique
   u16 works).
3. The world name in `S2C_FILE_LIST`: rule in 4.4 (relative path, `\` or `/`, case-insensitive, `.dat` required).
   Open only the 2009 server's own spelling (V-A2) and the archive case flag's run-time value.
4. `srv_clock` is binary32 seconds (PROVEN, 4.6); the client accepts any value. V-A3 records the value.
5. Closed: the third field of `S2C_CLIENT_ID` is the host's local-connection flag (DISASM, 4.3); a remote server
   sends 0.
6. The 48-byte block of `S2C_SKY_TABLE` (answered: part B 8.5, spec-fold-8) and whether the client needs the message at all (4.8; rust3 without it: the sky "very
   oddly" drawn, OBSERVED, cause PLAUSIBLE).
7. Whether any `S2C_EVAR` variable is required by the game shell (4.5); V-A4. Answered in effect (spec-fold-8): a missing variable reads 0, the step height and the
   world-settings set are needed (4.5, part B 8.6).
8. Closed: shell 0x88 values 2 and 3 are equivalent (4.11); the 8.3 d player state is carried by shell 0x67 (8.3; RESOLVED statically, DISASM / PROVEN reader, and PROVEN-CAPTURE by the client's reaction, tdm1).
   Open: the meaning of the player-state values other than 1.
9. Whether a received transport bye code reaches `disconnect_box` unchanged (6.2); V-A6.
10. Whether anything other than the transport idle drop ends a stuck start-up on the client (7.1); V-A5.
11. Closed: `S2C_VERSION` and `S2C_CLIENT_ID` are sent reliably (DISASM, 4.2, 4.3).

### 10.2 Re-read list (status after the disasm pass)

All entries were re-read in the read-only disassembly; three now have clean-tree C.

- Engine.exe, DISASM confirmed: 0xe980 (start-game stub: `start_game(clientmgr, request)`), 0xe9d0 (own client id:
  0 and the id, or 0x3F), 0xff80 (member +0x2b8; the master text's "0x6ff80" is a wrong RVA), 0xa2960
  (`S2C_LEAVE_WORLD`: view id := 0xFFFF, unload, return 0), 0xa3e90 (`S2C_LOAD_WORLD`: then `[9][0]` reliable),
  0xa2a50 (`S2C_VIEW_OBJECT`), 0xa3f80 (`S2C_BUNDLE`: `len` in bits, `len > remaining` -> 0x2c), 0xa44f0
  (`S2C_SKY_TABLE`: 384 raw bits, `count > 30` -> 1), 0xa4ef0 (`S2C_OBJ_GROUP`, part B addendum B4), 0xcff20
  (file service +0x0c -> engine+0x3e990 on the announcer at +0xe84), 0xd0c40 (register: lookup, upper-cased record,
  returns 0), 0x90b40 (`C2S_HELLO` host), 0x90b20 (`C2S_LOAD_STATE` host: `state != 0` -> 3, else 1), 0x922d0
  (C->S id 11 host, part B addendum B15), 0x16ed0 (`ret 8`, empty). Not re-read: 0xd09e0, 0x9e930.
- CShell.dll, DISASM confirmed: 0x2f8d0, 0x2f930, 0x2f9e0 (2.4), 0x23c570 (jump table: 0x72, 0x73, 0x9e, 0xac),
  0x22fa50 / 0x22f780 (called by cs+0x22f8a0, not re-read in detail), 0x1a8470 (writes shell+0x1a0c := 0).
  Now PROVEN (clean tree C exists): 0x23ccb0 (shell 0x72 answer writer), 0x28d80 (name getter), 0x20d60
  (wide-to-8-bit conversion with the caller's code page).

### 10.3 Names registered

All names of this part are listed in `GLOSSARY_UDP.md` section S.A. Part B's names for shared messages
(`S2C_VERSION`, `C2S_HELLO`, `S2C_CLIENT_ID`, `S2C_EVAR`, `S2C_PRELOAD`, `C2S_RATE`, `C2S_BYE`, `S2C_SHELL_MSG`,
and `C2S_PRELOAD_ACK` = `C2S_LOAD_STATE(1)`) are used unchanged where part B coined them first; part B in turn
uses this part's names for ids 5, 6, 7, 9, 14, 16, 0x32 and 0x36, and names id 8 `S2C_OBJ_GROUP`.

## Appendix Z: evidence notes of the items folded in from the disasm-pass addendum (2026-10-24)

Each item below was applied into the text above; this list keeps its grade / evidence paragraph so that the
rva citations stay graded. Grades are those of the source addendum, unchanged.

- **A1. Function map: which start method serves which request kind**
  DISASM: the dispatch in cs+0x23c730 (A2).

- **A2. Function map: start dispatch and driver check**
  DISASM: cs+0x23c730 (`sub eax, 0` / `sub eax, 3` / `dec eax` on `[ecx + 4]`, then tail jumps through the method table +0x48 / +0x4c / +0x44), cs+0x23c6b0, cs+0x23c4c0; method table VA 0x374cedc4 (+0x24 = cs+0x23c570, +0x28 = cs+0x23c6b0, +0x2c = cs+0x23c730, +0x44 / +0x48 / +0x4c = cs+0x2f8d0 / cs+0x2f930 / cs+0x2f9e0; PE-DATA).

- **A3. Function map: the mission methods of the start path**
  DISASM; mission method table VA 0x374d3194 (+0x2c = cs+0x22f750, +0x38 = cs+0x22f910; PE-DATA). cs+0x22f910 lies between two functions of the clean function table (not created there).

- **A4. Section 2.2 step 5: the engine start happens here**
  PROVEN for the call order in cs+0x1630a0 (clean tree: `join_request_fill`, then mission +0x1c, then mission +0x2c); DISASM for cs+0x22f750, cs+0x22f910, cs+0x23c730 and cs+0x2f9e0.

- **A5. Section 2.3: code page of the name's 8-bit copy**
  PROVEN: clean tree 20d60.c (`WideCharToMultiByte(param_3, 0, ...)`), 28d80.c. DISASM: cs+0x2fb5e `push 0` as the third argument of the call at cs+0x2fb79. Rule for our server: identify the player by the UTF-16 copy at blob offset 0x82 (5.2), never by the 8-bit copy.

- **A6. Section 2.4: the start method, fully**
  DISASM: cs+0x2f9e0..0x2fa64 (members +0x2b8 and +0x80 called through the interface pointer at VA 0x37560ea8, `setne al` on `result != 0x4f`), engine+0xff80..0xffb7, cs+0x23c6b0. Correction: the master text's "engine label 0x6ff80" is a wrong RVA; the member +0x2b8 installed by engine+0x14270 is VA 0x46ff80 = engine+0xff80.

- **A7. Section 4.2: `S2C_VERSION` is reliable; what its rate does**
  DISASM for the flags; the effect follows from transport T12 (PLAUSIBLE order).

- **A8. Section 4.3: `S2C_CLIENT_ID` is reliable**
  DISASM.

- **A9. Section 4.4: the world-name form and `file_info`**
  PROVEN: clean tree cfe60.c, 317e0.c, 1c0170.c, 1c0000.c, 1bf820.c, 1bee90.c, 1c2570.c, 1c26c0.c, 1bf010.c, ec940.c, 954b0.c, 95730.c, a12f0.c, 94b90.c, 94c70.c. DISASM: engine+0xd0c40 (the register path, `call 0x54c940` before the record is stored).

- **A10. Section 4.6 step 3: `srv_clock` is seconds**
  PROVEN (clean tree 9e4e0.c, a12f0.c); a debug line of engine+0x9e4e0 also prints the global with two decimals.

- **A11. Section 4.6: host source of `srv_clock`**
  PROVEN.

- **A12. Section 4.11: shell 0x88 values and its consumers**
  PROVEN (clean tree 1a58f0.c, 1a86b0.c, 1f3950.c, 49a40.c, 2c430.c, 60d90.c, 1f0470.c); DISASM for the readers and writers of shell+0x1a0c (full-text search of the CShell disassembly) and for stages d and f.

- **A13. Section 5.1: C->S id 8 name**
  PROVEN for the id 8 handler (clean tree 90a10.c, a6f30.c); DISASM for engine+0x922d0 (part B addendum B15).

- **A14. Section 5.5: `C2S_RATE` value and timing**
  PROVEN (clean tree 14f90.c lines testing `clientmgr+0xa44` and `+0x6c`, 1ae50.c, 90a80.c).

- **A15. Section 7.2: host tick**
  PROVEN (clean tree 198f0.c calls engine+0x9e4e0 with `*(clientmgr + 0xa1c)`; 9e4e0.c); the 60 is DISASM (part B addendum).

- **A16. Section 8.3 row d: the carrier of the player state**
  PROVEN: clean tree 1f97d0.c (`in_ECX[0xc] = read(8)`), 1f0470.c (case 0x67 -> vtable +0xb8), 60d90.c (tail call of the base handler). DISASM: cs+0x41c10, cs+0x61870 (`call 0x372f97d0` first), the shell constructor cs+0x41820 (member at +0x16054 built by cs+0x5e550, the only caller of that constructor), cs+0x1f6fe1 (`mov [0x3759bf60], esi`). The player update (part B 10.3) is only sent while this state is 1.

- **A17. Section 9.2 S11 / S12: concrete bytes**
  Follows from A12 and A16; the meaning of state values other than 1 is UNPROVEN.

- **A18. Section 9.2 notes: when `C2S_RATE` arrives**
  Follows from A14.

- **A19. Section 10.1: updated open facts**
  Editorial summary of A1-A18.

- **A20. Section 10.2: re-run list status**
  DISASM listing per function; clean-tree presence checked by file name.

## Appendix O: reference stack notes (spec-fold-3, 2026-10-24)

Source: `UDP_REFERENCE_FINDINGS_botclient.md` items O1-O13 (the evidence record; runs and captures are cited there). Grade used here:
**REFERENCE-CONFIRMED** = the reference server stack (a second, independent writer or reader, NOT the 2009 server) behaves as the spec text
says when `dc-botclient` talks to it; it is black-box evidence under REFERENCE_TESTING.md section 1, distinct from PROVEN (2009 code or
captures). **Reference stack differs** = the reference stack does something else; the base text and its grade stand and the 2009 evidence decides (a capture
of the real 2009 client). No base grade was changed by this appendix and no reference stack value is a DeCombat rule value. No ROM names are quoted.

| item | spec place | note | state |
|---|---|---|---|
| O1 | 9.2 S1 (join), transport 5.1 / 5.3 | join ask accepted at the first ask with the GUID-record application id; rate 5120000 | REFERENCE-CONFIRMED (V3 stays open, see the transport appendix) |
| O2 | 5.7 gates, 4.4 file list, 4.5 evars, 4.6 load world, 4.8 sky table, 5.5 rate, addenda A7-A18 | the whole start-up ran to a loaded world with both bots: `S2C_FILE_LIST` batched first (world first) and single-entry later, the 108 `S2C_EVAR` messages right before `S2C_LOAD_WORLD`, `srv_clock` 0.0 at world start (A10), sky count 30, `C2S_RATE` 640 (A14); the host gates held (S6 only after S1 and the file acks, preload only after S6a, the player object only after S8a); the client's identify request 0x72 and options 0x85 / world ready 0x87 were accepted | REFERENCE-CONFIRMED |
| O6 | 8.2 host new-link messages, 9.2 S11 | **Reference stack differs:** shell 0x90 (1-bit body) and shell 0x88 state 2 arrive in the first reliable block ahead of `S2C_VERSION`, and again after the world starts; 8.2 lists version, client id, file list as the new-link messages and 0x88 as step S11. Harmless (shell messages are accepted in every linked state, 8.1); in-world test condition (b) then holds from the first block on. The meaning of 0x90's bit stays UNPROVEN | Reference stack differs; 2009 order unchanged |
| O7 | 4.7 preload | **Reference stack differs:** the reference stack sends `[19][0]`, one model message, `[19][1]`, then sub 5 and no second `[19][1]`; 4.7 says the 2009 host sends `[19][1]` twice and the host acts on the first answer. The bot answers each `[19][1]` once. A DeCombat server may send it once (no client-side effect either way) | Reference stack differs; 4.7 stands for 2009 |
| O9 | 9.2 S12 player state | **Reference stack differs:** the reference stack sends shell 0x67 state 1 directly, without a state 3 first (DC-C2S-10 (proposed) has 3 then 1); both pass the client's in-world test. It then sends the mover update twice (body + containers + friction; teleport with `move_seq` 1 and again 2 at the same position), facing 0x6a, weapons 0xb2 / 0x7f, status 0x66; the bot echoed the teleport's `move_seq` and the 0x6a yaw | Reference stack differs (game layer) |
| O11 | 5.6 `C2S_BYE`, 6.1 | the bye the reference stack sends back carries code 5 | REFERENCE-CONFIRMED |

Open from the findings: a capture of the real 2009 client is still needed to turn O1, O6, O7 into 2009 evidence.
Count: REFERENCE-CONFIRMED 3 items (O1, O2, O11), reference stack differs 3 (O6, O7, O9).

## Appendix C: capture notes (spec-fold-4, 2026-10-25)

Source: `CAPTURE_tdm1_TRANSPORT_VALIDATION.md` section 2 (C-A1 .. C-A7), plus the join / re-join rows of `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md`
section 2 (evidence records, kept in full). `OBSERVED: captures/20261025_000014_tdm1/capture.jsonl` (seq numbers as cited) and
`20261025_001409_tdm2`. Grades: PROVEN-CAPTURE = the real client's own bytes or reaction; REFERENCE-CONFIRMED = a server form the client accepted
(its next message could only follow a successful handling); REFERENCE = what the reference server did.

Start-up timeline of tdm1 (t in seconds of the tap clock; client entries PROVEN-CAPTURE, server entries REFERENCE):

| t | seq | dir | what |
|---|---|---|---|
| 133.312 | 266 / 267 | c2s / s2c | `OOB_JOIN_ASK` / accepted reply |
| 133.359 | 268 / 269 | s2c / c2s | block 1: shell 0x90 `[1]`, shell 0x88 `[2]`, `S2C_VERSION` (7, 5120000), `S2C_CLIENT_ID` (0, local_host 0), `S2C_FILE_LIST` (10 entries, world id 0) / `C2S_HELLO` (2,088 bits) with `CTL_ACK 1` |
| 133.56 .. 139.41 | 271 .. 391 | both | keep-alives and acks only: 6.08 s of session-level silence |
| 139.437 | 392 | c2s | `C2S_FILE_ACK` for all ten ids |
| 139.468 | 394 / 395 | s2c | 86 `S2C_EVAR`, then `S2C_LOAD_WORLD` |
| 139.687 | 400 | c2s | `C2S_LOAD_STATE(0)` (219 ms later) and `C2S_RATE` `07 80 02` |
| 139.734 | 402 / 404 | s2c / c2s | `S2C_PRELOAD` forms `13 00`, `13 02 04 00`, `13 01`, `13 05 13 01` and the sky table (count 30) / `C2S_LOAD_STATE(1)` |
| 139.812 | 406 / 408 | s2c / c2s | 10 single-entry file lists, `S2C_EVAR`s, shells 0xa6, 0x72 (identify request), 0x9e, 0x9d, 0x7d, `S2C_VIEW_OBJECT` / ten `C2S_FILE_ACK`, shell 0x72 (643 bits), shell 0x85 `0b 85 02 01` |
| 139.906 | 410 .. 448 | s2c | spawn set incl. `0d 67 01` (seq 410), shell 0x45 (split), then the 20-part object group with the create of object 0x198 |
| 139.953 | 457 | c2s | shells 0x76 / 0x7f x4, file acks, reliable 0x64 state form, **shell 0x87**, first unreliable 0x64 |

| item | spec place | verdict | grade |
|---|---|---|---|
| C-A1 | 2.3 row 0x40, 5.2, V-A1 | hello confirmed; second string = 36-character GUID text; hello datagram carries `CTL_ACK 1` | PROVEN-CAPTURE |
| C-A2 | 4.4, V-A2 | the client accepted world file id 0, the mixed-case spelling of 4.4, version 7 / 5120000, client id 0 | REFERENCE-CONFIRMED |
| C-A3 | 9.2 notes | 6.08 s session silence after the join, 219 ms world load | PROVEN-CAPTURE |
| C-A4 | 5.4, V-A3 | one `C2S_FILE_ACK` per list, bit 15 on every id; the reference stack held `S2C_LOAD_WORLD` until the acks and the hello (gate of 5.7) | PROVEN-CAPTURE (client), REFERENCE (gate) |
| C-A5 | 5.3, 5.5 | `09 00` + `07 80 02` in one block; one `09 01` per `[19][1]`; later `C2S_RATE` carry one extra u16 tracked-sound id each (`07 80 02 9e 01`, tdm1 seq 6721 and 7985) | PROVEN-CAPTURE |
| C-A6 | 8.3, S12, V-A4 | sufficient in-world set, order and moment (0x87 47 ms after 0x67 state 1) | PROVEN-CAPTURE |
| C-A7 | V-A5 .. V-A9, 6.2, 4.5, 4.8 | undecided: stuck start-up, kick reasons, version 8, missing world, our slice; whether `S2C_EVAR` are needed (the reference stack sent 86 before S6 and 29 later); the 48-byte sky block layout (count 30 with a 48-byte block was accepted) | |
| J-1 | 9.2 notes | running-world join starts with 0x90 `[0]` + 0x88 `[3]`; re-join hello and 0x72 are byte-identical to the first join | REFERENCE (order), PROVEN-CAPTURE (client messages) |

Proposed DC_DEFAULTS changes (approval is the user's, nothing edited here): DC-C2S-10 keep "state 3 before 0x87" only as a compatibility default and
note that state 3 first is not required by the 2009 client (OBSERVED tdm1; DC-C2S-10 is rewritten as DC-C2S-10' by spec-fold-5, `GAME_MESSAGES_2009_C2S.md` 12.3, Appendix F); BOT-6 optional bot mode that holds the first server block for about
6 s and loads in 250 ms; BOT-8 send a GUID-form text as the hello's second string.

Count: PROVEN-CAPTURE 6 (C-A1, C-A3, C-A5, C-A6, the client side of C-A4, J-1 client messages), REFERENCE-CONFIRMED 1 (C-A2), undecided 1 group (C-A7).

## Appendix F: fold record (spec-fold-5, 2026-10-25)

Sources: `reports/client-reference stack.md` section 4 (the Rust client `dc-client` joined the reference stack four times as the room host), `dserver-slice4.md` (DSV-36). Grades as in the
header plus **D** = DeCombat design (DC_DEFAULTS row, proposed) and RUN = a loopback run of our own stack.

| item | place | finding | grade |
|---|---|---|---|
| second reader of the join | 4, 5.2, 8.3 | a client written from this spec (`dc-client`) passed S1-S13 against the reference stack within 0.5 s in four runs; its join ask (25 bytes) is byte-identical to the real client's (tdm1, tdm4), its hello 2,088 bits, 0x72 643 bits, 0x85 25 bits, 0x87 16 bits, the reliable state form `0b 64 10 00 01` once right before 0x87, all equal to the real client's forms | PROVEN-CAPTURE (the real forms) + RUN (our client reproduces them) |
| start-up order | 9.2 | equal to the real client's order except the weapon selects, which `dc-client` does not send yet | RUN |
| DC-C2S-10 | Appendix C note above | rewritten as DC-C2S-10' (the spawn set with state 1 before 0x87 is the tested path) | D |
| DSV-36 | 9.2 notes, J-1 | `dc-dserver` sends 0x90 `[1]` / 0x88 `[2]` at the link and 0x90 `[0]` / 0x88 `[3]` after 0x87, and both already at the link for a joiner of a running round (`GAME_MESSAGES_2009_S2C.md` 10.1) | D, TESTED, RUN |

## Appendix H: fold record (spec-fold-7, 2026-10-25)

Source: `reports/dserver-light.md`, `DC_DEFAULTS.md` DSV-40 (details, Wave U section, PENDING), `RENDER_2009.md` 6.3.1. No new 2009 evidence.

| item | place | finding | grade |
|---|---|---|---|
| `S2C_DIR_LIGHT` send point | 5.7 (new row), 9.2 (new step S8b) | DeCombat sends id 22 after the preload lists and the optional sky table, before the view object, once per client; the reference stack's id 22 at the join was accepted | D, TESTED, RUN (binary); REFERENCE-CONFIRMED (the form) |

## Appendix I: fold record (spec-fold-8, 2026-10-25)

Sources: `CAPTURE_rust3_RUSTSTACK_VALIDATION.md` 3.1, 3.2, 9.1, 9.3 and 11; `reports/aa-triage-world-physics.md` item 10. No capture was decoded again.

| place | change | grade |
|---|---|---|
| 4.5, open item 7 | a missing `S2C_EVAR` reads 0: the step height (and the world-settings set) are needed in effect | EVIDENCE + PROVEN-CAPTURE |
| 4.8, S8 row, open item 6 | the sky table block is answered in part B 8.5; the reference stack's send point | REFERENCE + ROM |
| 5.7 `S2C_DIR_LIGHT` row | the reference stack's id 22 after the view object, ours before: both accepted | REFERENCE + PROVEN-CAPTURE |
