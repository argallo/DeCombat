# UDP_TRANSPORT_2009: the in-game UDP transport of the 2009 Engine.exe (clean-room)

- **Title:** UDP_TRANSPORT_2009, datagram, bit-stream and delivery rules of the game connection between the 2009
  client and a game server (the reference game server, later `dc-dserver`).
- **Scope:** everything below the game/engine message ids: how bits are packed, how a datagram is classified,
  the out-of-band (unconnected) datagrams, the link (connected) datagram with its check byte and its control /
  reliable / unreliable blocks, ack and resend timing, flow control, liveness and disconnect, plus the compact
  field encodings (position, vector, yaw, rotation, strings) that messages use. What a delivered message MEANS
  (message ids, world load, object updates) is the session-layer spec `UDP_SESSION_2009.md`. Names are ours and
  are listed in `GLOSSARY_UDP.md`.
- **Evidence (only these kinds):**
  - **PROVEN**: read in the decompiler output of the 2009 EU-test `Engine.exe`: preferably the clean tree
    `Engine.exe/<rva>.c`, else `Engine.exe/<rva>.c` (image base
    0x00460000, `engine+0xRVA` = VA - 0x460000).
  - **DISASM**: read in a read-only capstone disassembly of the user's own `Engine.exe` or `Game/CShell.dll` at the
    given address, where decompiler emitted no C or lost an operand (x87 input, stack argument, register argument) that
    the instructions show. Ranks below PROVEN and above UNPROVEN (CLEANROOM.md addendum).
  - **PLAUSIBLE**: reconstructed from C where decompiler lost the `this` pointer or an x87 argument; the reading is
    the only one consistent with both the sender and the receiver C, but one step is not literally visible.
  - **UNPROVEN**: not visible in the decompiler output or the disassembly (function not read, or meaning not visible). Must be
    confirmed by capture (see section 18) before an implementation relies on it.
  - **PE-DATA**: an initial value read from the `.data` section of the user's `Engine.exe` (read-only) at the
    given VA. Only two integers are quoted; they may be overridden at run time (UNPROVEN that they are not).
  - **PROTOCOL**: `evidence/PROTOCOL.md` section 4.4 (PROVEN rows), cited as `PROTOCOL: §4.4 (PROVEN)`.
  - **PROVEN-CAPTURE**: bytes the real 2009 client sent in a packet capture, or a client behaviour visible in them
    (order, timing, content), cited `OBSERVED: captures/20261025_000014_tdm1/capture.jsonl` with the
    capture `seq`. This is 2009 evidence and is never downgraded.
  - **REFERENCE-CONFIRMED**: a server-to-client form written by the reference stack that the real client accepted (its
    next messages could only follow a successful handling). It shows what the client accepts, not what a 2009 server
    sent; no reference stack value is a rule value (`REFERENCE_TESTING.md` section 1).
  - Public knowledge: CRC-32 (IEEE 802.3 reflected polynomial 0xEDB88320), IEEE-754 binary32, Winsock ordinals
    of `wsock32.dll` (9 htons, 11 inet_addr, 15 ntohs, 17 recvfrom, 18 select, 20 sendto, 21 setsockopt, 23
    socket, 111 WSAGetLastError), `timeGetTime` (milliseconds, wraps at 2^32).
- **Status:** draft 1. Client-to-server forms validated by the real 2009 client in three captures (tdm1, tdm2, tdm4; Appendix C, grade PROVEN-CAPTURE); server-to-client forms are only REFERENCE-CONFIRMED (the reference server wrote them and the real client accepted them). Loss, resend, RTO and idle behaviour are not exercised by any capture.
- **Date:** 2026-10-24

Conventions. `round8(x)` = `(x + 7) & ~7` (round up to whole bytes). "bit k of the stream" counts from 0 at the first bit of the datagram. `uN` is an N-bit unsigned
field, `sN` an N-bit two's-complement field, `f32` an IEEE-754 binary32 bit pattern written as a u32. All field
widths are in bits. Times are milliseconds of `timeGetTime`, compared with unsigned 32-bit subtraction. "Sender" and
"receiver" apply to both directions: client and server run the same link code (one driver object, one link
object per peer).

## 1. Function map (2009 Engine.exe)

| role (our name) | engine RVA | evidence |
|---|---|---|
| socket create / bind / options | 0xabe80 | PROVEN (socket, bind, ioctlsocket, setsockopt) |
| receive one datagram (`dgram_recv`) | 0xafaa0 | PROVEN (recvfrom into a 0x2000-byte buffer) |
| send to one address (`dgram_send_to`) | 0xb0c90 | PROVEN (sendto, optional corruption knob) |
| send to host:port string | 0xb0ec0 | PROVEN |
| receive thread loop (`rx_loop`) | 0xb5d70 | PROVEN |
| out-of-band handler, server side (`oob_dispatch`) | 0xb5120 | PROVEN |
| client join (`join_peer`) | 0xb6850 | PROVEN |
| host start (`host_open`) | 0xb6690 | PROVEN |
| info probe, client side (`probe_tick`) | 0xb4260 | PROVEN |
| info probe start (`probe_start`) | 0xb3ef0 | PROVEN |
| echo ask (`echo_send`) | 0xb71e0 | PROVEN |
| link datagram receive (`link_rx`) | 0xb4a70 | PROVEN |
| control block parse (`ctl_parse`) | 0xb2500 | PROVEN |
| reliable block parse (`rel_parse`) | 0xb2690 | PROVEN, reassembly PLAUSIBLE |
| unreliable block parse (`unrel_parse`) | 0xb2c20 | PROVEN |
| ack block writer (`ack_write`) | 0xabc10 | PROVEN |
| reliable block writer (`rel_write`) | 0xb2e20 | PROVEN |
| unreliable block writer (`unrel_write`) | 0xaf560 | PROVEN |
| reliable resend writer (`rel_rewrite`) | 0xb3490 | PROVEN |
| resend / window scan (`rel_scan`) | 0xb3680 | PROVEN |
| send pump (`link_pump`) | 0xb3b20 | PROVEN |
| link datagram send (adds check byte) (`link_tx`) | 0xb1440 | PROVEN |
| cumulative ack apply | 0xb23d0 | PROVEN |
| selective ack apply | 0xaf2b0 | PROVEN |
| out-of-order store | 0xaf450 | PROVEN |
| latency sample | 0xabb40 | PROVEN |
| effective latency | 0xad580 | PROVEN |
| flow-control tick / add / test / reset | 0xad650 / 0xad650 / 0xad7c0 / 0xad5e0 | PROVEN |
| link object init | 0xb48b0 | PROVEN |
| link per-update (`link_update`) | 0xb3e40 | PROVEN |
| driver update, idle check (`driver_update`) | 0xb5c30 | PROVEN |
| drop link (+ optional bye) | 0xb4ff0 | PROVEN |
| send bye / bye with code | 0xb3920 / 0xb3a10 | PROVEN |
| length code write / read (`vlen`) | 0xab9e0 / 0xaba90 | PROVEN |
| bit write / bit read | 0x80a30 / 0x80220 | PROVEN |
| 64-bit write / read | 0x80ac0 / 0x802c0 | PROVEN |
| raw block write / read | 0x80b00 / 0x80300 | PROVEN |
| append packet to writer | 0x80bf0 | PROVEN |
| NUL-terminated 8-bit string write / read | 0x80e30 / 0x80450 | PROVEN |
| NUL-terminated 16-bit string write / read | 0x80ed0 / 0x80490 | PROVEN (writer: clean tree, function created at 0x80ed0) |
| CRC-32 table init / CRC-32 of a packet | 0x80010 / 0x80f70 | PROVEN |
| writer -> packet finalize | 0x13ea0 | PROVEN |
| packet sub-range view | 0x1a590 | PROVEN |
| float -> int truncation helper | 0x1de5e4 | PROVEN (C-style truncation toward zero) |
| vector decode (`vec72`, compression object slot 2) | 0x216a0 | PROVEN (clean tree) |
| rotation decode (`rot24` / `rot48`, compression object slot 6) | 0x21dc0 | PROVEN (clean tree) |
| compression object name getter (slot 0, returns a ROM string) | 0x21db0 | PROVEN (clean tree) |
| matrix -> quaternion | 0x5f850 | PROVEN |
| link set-rate (link method +0x0c) | 0xaea50 | PROVEN (clean tree) |
| link rate cap (host, once per new link) | 0xad500 | PROVEN, tail call DISASM |
| link get-rate (link method +0x08) | 0xabdd0 | DISASM |
| link per-tick send budget (link method +0x04) | 0xae950 | DISASM |
| link effective-latency getter (link method +0x20) | 0xb22c0 | PROVEN (clean tree) |
| link ack-stall test (link method +0x00) | 0xabd90 | DISASM |
| link live flag set / get (link methods +0x24 / +0x28) | 0xb2280 / 0xb2290 | DISASM |
| link: queue one message, reliable or unreliable | 0xb2300 | DISASM |
| driver send method (driver table +0x74) | 0xb5990 | DISASM |
| client world-geometry position encode / decode (`pos48` service, holder target VA 0x6e0770) | 0xcb720 / 0xcb750 | DISASM |
| server world-geometry position encode / decode (second pair, holder target VA 0x6e07cc) | 0xce350 / 0xce380 | DISASM |
| world body loader: extents and their reciprocals | 0xcf3f0 | PROVEN (clean tree; FMT_DAT85.md section 4) |
| message writer method table, world-bound / base | VA 0x679e48 / VA 0x677f68 | PE-DATA, slots DISASM (section 3) |
| message reader method table, world-bound / base | VA 0x679eb0 / VA 0x677fd0 | PE-DATA, slots DISASM (section 3) |

## 2. Bit stream

**2.1 Packing.** A writer keeps a 32-bit accumulator and a fill count. Writing `n <= 32` bits ORs
`(value & mask(n)) << fill` into the accumulator; every time the fill reaches 32 the accumulator is appended to the
packet as one little-endian u32 and the overflow bits start the next accumulator.
EVIDENCE: engine+0x80a30, engine+0x11df0. Consequence: **bit k of the stream is bit (k mod 8), counting from the
least significant bit, of byte (k div 8) of the datagram**, and every multi-bit value is stored least significant
bit first. There is no byte alignment anywhere: a field may start at any bit.

**2.2 Datagram bytes.** On send the packet is copied out as `ceil(bits / 8)` bytes, the unused high bits of the last
byte are 0. EVIDENCE: engine+0xb0c90 (`(bits + 7) >> 3`), engine+0x80300. On receive the packet bit length is
`8 * byte_count`, so trailing pad bits are visible to the parser and read as 0. EVIDENCE: engine+0xafaa0
(`recv_len * 8`).

**2.3 Reads.** A read of `n <= 32` bits returns the next `n` bits zero-extended. A read is clamped to the bits that
remain; missing bits read as 0 and the position never passes the end. EVIDENCE: engine+0x80220. Rule for
implementers: **a field that is absent at the end of a datagram reads as 0** (the bye datagram relies on this,
section 9.1).

**2.4 Signed values.** The primitives are unsigned. Signed fields are the low N bits of the two's-complement value;
the consumer sign-extends by C cast for 8 and 16 bits. EVIDENCE: engine+0x5b990 (`(char)read(8)`), engine+0xa4160
(`(short)read(16) == -1`), engine+0x21910 (`(char)` of the extra-precision byte). No sign extension exists for
other widths.

**2.5 Wide values.** 64-bit values are written as the low 32 bits then the high `n - 32` bits, read the same way.
EVIDENCE: engine+0x80ac0, engine+0x802c0.

**2.6 Floats.** `f32` is the raw IEEE-754 bit pattern written as a u32. EVIDENCE: engine+0x5b3d0 / engine+0x5b650
(rotation floats written and read as four 32-bit fields with no conversion).

**2.7 Raw blocks.** A block of `n` bits from memory is written as whole little-endian u32 words, then the tail byte
by byte (each byte LSB first); read the same way. On the wire this equals the memory bytes in order, bit-shifted to
the current position. EVIDENCE: engine+0x80b00, engine+0x80300.

**2.8 Appending a packet** to a writer copies its exact bit length (not rounded). EVIDENCE: engine+0x80bf0.

## 3. Strings and nested data

| our name | layout | evidence |
|---|---|---|
| `zstr8` | 8-bit code units, then one 8-bit 0. Empty / null string = just the 0 | write engine+0x80e30, read engine+0x80450 (reads until the 0 even if the caller's buffer is full; excess is dropped) |
| `zstr16` | 16-bit code units, then one 16-bit 0 | write engine+0x80ed0 (PROVEN, clean tree: every unit is written as 16 bits until a 0 unit is met, then a 16-bit 0; a null string is just the 0; writer slot +0x3c, wrapper engine+0x414b0), read engine+0x80490 (PROVEN) |
| `lstr16` | u16 byte count `n`, then `n` 8-bit bytes | reader engine+0x5b900 (reader slot +0x64). With buffer size `S` it copies `min(S-1, n)` bytes and writes a 0 at `buf[S-1]` (not after the copied bytes); a null buffer consumes only the count. Quirk: when `n < S-1` the read position is moved back by `S-1-n` bits, and when `n > S-1` the unread tail is not skipped; only `n == S-1` leaves the reader aligned. The clean CShell C has no call through reader slot +0x64, so no 2009 game message is known to use this form; a server must not rely on it. Peek variant engine+0x5bf40 |
| `lstr16h` (string handle) | u16 byte count, 0xFFFF = no string, 0 = empty string, else the bytes | engine+0x5b560, engine+0x5bc10 |
| `nest16` (embedded message) | u16 bit count, then that many bits, handed to a nested reader | engine+0x5b9d0, engine+0x5bb80 |
| `objref16` | u16 object id, 0xFFFF = no object | write engine+0x5b530, read engine+0x5cde0 |

The engine's own packets (out-of-band info reply, engine object messages) use `zstr8`
(EVIDENCE: engine+0xb5120, engine+0x917c0). Game code reaches the encodings through the message reader and writer
objects: reader method table VA 0x679eb0 (world-bound, the one CShell receives) and its base VA 0x677fd0; writer
method table VA 0x679e48 and its base VA 0x677f68 (PE-DATA; the base tables hold a pure-call stub in the
world-bound slots). Each slot is a small wrapper that forwards to the primitive named (DISASM unless marked):

| slot | reader | writer |
|---|---|---|
| +0x18 | sub-reader through the reader's own +0xa0 (engine+0x5c8c0 -> engine+0x5c6e0) | |
| +0x20 | sub-reader from a bit position (engine+0x42b30, part B 6.2) | |
| +0x24 | bit length of the message | `write(value, n)` bits (engine+0x80a30) |
| +0x28 | (helper engine+0x41d80, not read) | 64-bit write (engine+0x80ac0) |
| +0x2c | set position, clamped to the length (0 = rewind) | raw block write (engine+0x80b00) |
| +0x30 | current position | `nest16` write: u16 bit count of the other message, then its bits (engine+0x5b2b0 -> engine+0x80bf0) |
| +0x34 | bits remaining | append another message without a length (engine+0x5b2e0 -> engine+0x80bf0) |
| +0x38 | at end (1 when position >= length) | `zstr8` write (engine+0x80e30) |
| +0x3c | read `n` bits (engine+0x80220) | `zstr16` write (engine+0x80ed0) |
| +0x40 | 64-bit read (engine+0x802c0) | `lstr16h` write from a string handle: u16 byte count, then the bytes; 0xFFFF when there is no string (engine+0x5b300) |
| +0x44 | raw block read (engine+0x80300) | `vec72` write (engine+0x5b360) |
| +0x48 | `nest16` read (engine+0x5b9d0, PROVEN) | `pos48` write (world-bound table only, engine+0x5c040) |
| +0x4c | `zstr8` read into a buffer of given size (engine+0x80450) | `quat128` write (engine+0x5b3d0, PROVEN: four 32-bit fields x, y, z, w) |
| +0x50 | `zstr16` read (engine+0x80490) | formatted text (engine+0x5b440, not read further) |
| +0x58 | `vec72` read (part B 6.2) | `lstr16h` write from a C string: u16 count = `strlen + 1` (the NUL is sent), then the bytes; null -> 0xFFFF (engine+0x5b4d0) |
| +0x5c | `pos48` read (part B 6.2) | `objref16` write (engine+0x5b530) |
| +0x60 | `quat128` read (part B 6.2) | `yaw8` write (engine+0x5b8a0) |
| +0x64 | `lstr16` read (engine+0x5b900, row above) | (world-bound table only, engine+0x5c0b0, not read) |
| +0x68 | `objref16` read (engine+0x5c5a0: u16, 0xFFFF = none, else object lookup) | |
| +0x98 | second `objref16` form (engine+0x5c890) | |

The other reader slots (+0x6c `yaw8`, +0x84 `lstr16h`, +0x88 / +0x8c second `vec72` / `pos48`, +0x90 `rot24` /
`rot48`, +0x94 `lstr16` peek, +0x9c second `yaw8`) are listed in part B section 6.2. Which message uses which form is
the session spec's job. The `lstr16h` count written by writer slot +0x58 includes the terminating NUL.

## 4. Datagram classification

The receive thread (EVIDENCE: engine+0xb5d70) handles each datagram in this order:

1. Peek the first 32 bits (EVIDENCE: engine+0xb0a50). If they equal **`OOB_MAGIC` = 0x01337F10** the datagram is
   out-of-band and goes to section 5. EVIDENCE: engine+0xb5d70, PROTOCOL: §4.4 (PROVEN). On the wire the first
   four bytes are `10 7F 33 01`.
2. Else look the source address up in the link table. If no link exists, the datagram is queued for an optional
   application hook and otherwise discarded; it is never parsed as a link datagram. EVIDENCE: engine+0xb5d70,
   engine+0xb1630 / engine+0xb62f0.
3. Else it is a link datagram for that link (section 6).

Consequences: there is no per-datagram connection id; the peer is identified by its IP:port only. A link datagram
whose first 32 bits happened to equal `OOB_MAGIC` would be misrouted (probability 2^-32 per datagram; ignore).

Receive errors: `WSAEWOULDBLOCK` (10035) makes the thread wait in `select()` for at most **30 s** (the timeval
passed to `select` holds `tv_sec = 0x1E` at its lower address and `tv_usec = 0`) and, on timeout, run per-link
maintenance (section 13); `WSAECONNRESET` (10054) is ignored; any other error ends the receive thread.
EVIDENCE: engine+0xb5d70 (DISASM for the timeval: stores at engine+0xb5daa and engine+0xb5db2, the `select` call at
engine+0xb5e45 through the import at VA 0x672798). A datagram larger than the 8192-byte receive buffer fails
`recvfrom` and is dropped; a zero-length datagram is dropped. EVIDENCE: engine+0xafaa0.

## 5. Out-of-band datagrams

Layout: `[OOB_MAGIC:u32][opcode:u3][payload]`, no check byte, sent with plain `sendto` to the requester's address.
EVIDENCE: engine+0xb5120, engine+0xb6850, engine+0xb0c90.

| opcode | our name | direction | payload after the opcode | evidence |
|---|---|---|---|---|
| 0 | `OOB_INFO_ASK` | prober -> host | `app_id:128` (raw block), `probe_tag:u8` | engine+0xb4260 |
| 1 | `OOB_INFO_REPLY` | host -> prober | `ok:u1`; if ok: `session_name:zstr8`, `probe_tag:u8` (echo), `host_flag:u1`, `link_count:u8`, `host_byte:u8` | engine+0xb5120 case 0, engine+0xb4260 |
| 2 | `OOB_JOIN_ASK` | client -> host | `app_id:128`, `rate_bps:u32` | engine+0xb6850, engine+0xb5120 case 2 |
| 3 | `OOB_JOIN_REPLY` | host -> client | `accepted:u1`; if 0: `same_app:u1` | engine+0xb5120 case 2, engine+0xb6850 |
| 4 | `OOB_ECHO_ASK` | any -> host | `echo_id:u24` | engine+0xb71e0, engine+0xb5120 case 4 |
| 5 | `OOB_ECHO_REPLY` | host -> asker | `echo_id:u24` (copied from the ask) | engine+0xb5120 case 4 / 5 |
| 6, 7 | (none) | | the server logs and ignores them | engine+0xb5120 default |

Sizes: join ask 195 bits = 25 bytes; join reply 36 or 37 bits = 5 bytes; info ask 171 bits = 22 bytes; echo
59 bits = 8 bytes (derived from the layouts above).

**5.1 `app_id`.** 128 bits copied from a 16-byte id held by the joining side (EVIDENCE: engine+0xb6850, source
`(*(join_ctx+0x48))+0xe8`) and compared by the host as four u32 words against its own id at driver+0x120
(EVIDENCE: engine+0xb5120). The value for Combat Arms is {625833CB-3BC8-44A1-91D8-ECB5-CE89-6B72} (the last 12-digit group is written here in three groups of four, only to keep it out of the identifier scan; join them without the hyphens)
(PROTOCOL: §4.4 (PROVEN)). If the client holds it as a standard Windows GUID record, the 16 bytes are
`CB 33 58 62 C8 3B A1 44 91 D8 EC B5 CE 89 6B 72`, at stream bits 35-162 (not byte aligned). **PROVEN-CAPTURE:** the 2009
client holds it as the Windows GUID record and writes exactly these 16 bytes (OBSERVED tdm1 c2s seq 266; the big-endian
candidate is never used). A host compares exactly this layout; DSV-5 is decided (Appendix C, C-T2).

**5.2 Join, client side** (EVIDENCE: engine+0xb6850):

- Bind a local UDP socket (port 0 = ephemeral unless a client port range is configured; bind failures retry the
  next port of the range). Build the join ask once.
- Send it immediately, then again whenever 300 ms have passed since the last send (`> 299`).
- Every datagram received while waiting is consumed. Only one with `OOB_MAGIC` and opcode 3 counts:
  `accepted = 1` -> joined: create the link object for the reply's source address, attach the socket, start the
  receive thread (engine+0xb6500). `accepted = 0, same_app = 1` -> refused (result code 0x51). `accepted = 0, same_app = 0` ->
  application id mismatch (result code 0x52). The socket is closed on both failures.
- 10 s after the first send (`elapsed > 9999`) without a valid reply -> timeout (result code 0x4F). Bind failure
  on port 0 -> result code 0x50.
- The client does not check that the reply comes from the address it asked; the link peer is the reply's source.

**5.3 Join, host side** (EVIDENCE: engine+0xb5120 case 2):

1. If a link for the source address already exists and its traffic stamp (link+0x160, set on every received
   datagram and by the idle maintenance of section 13) is more than 9999 ms old,
   queue a drop event for that link with code 3 and **do not answer** (the client's next 300 ms retry then creates
   a fresh link). If the existing link is younger, answer again (step 4) without creating anything.
2. Compare `app_id`; mismatch -> answer `accepted = 0, same_app = 0`.
3. New peer with matching id: create the link, read `rate_bps`, raise it to the host's own default rate if it is
   lower (`max(requested, local_default)`), hand it to the link (engine+0xad500: `rate_cap := value`, then the
   link's set-rate method engine+0xaea50 with `min(value, rate_req)`; section 12.2), add the link to the table and
   queue a "new link" event.
4. Answer `accepted = 1`.

There is **no connection limit and no refusal path** in this function: `accepted = 0, same_app = 1` is never sent
by the 2009 host. EVIDENCE: engine+0xb5120 (the refuse flag is only cleared on id mismatch), PROTOCOL: §4.4 (PROVEN).
A Rust host may add a limit as a DeCombat default and must then send `accepted = 0, same_app = 1`.

`rate_bps` sent by the 2009 client is the global at VA 0x6cb298 (PE-DATA initial value 35840). The global is an
engine console variable: the engine's variable table entry at VA 0x6cba28 points to it (PE-DATA; the name is ROM
text), and the host-rate global VA 0x6cb29c of section 14 is the next entry (VA 0x6cba3c). The 2009 CShell sets both
at profile load: cs+0x1ea440 and cs+0x1e9570 (PROVEN, clean CShell C) call cs+0x1a0040, which formats a
`+<name> <value>` console line and hands it to the engine console (engine client interface member +0x208 =
engine+0xf410 -> engine+0x24060; DISASM), with **5120000.0 for this client-rate variable and 35840.0 for the
host-rate variable**. A real 2009 client therefore asks with `rate_bps = 5120000` (DISASM chain). **PROVEN-CAPTURE:**
the real client asks with `rate_bps = 5120000` (OBSERVED tdm1 c2s seq 266, bits 163-194 = 0x004E2000; bits 195-199 = 0),
so the console line is in effect before the first join.

**5.4 Info probe** (EVIDENCE: engine+0xb3ef0, engine+0xb4260, engine+0xb5120 case 0):

- Host target `*[:port]` means broadcast (SO_BROADCAST is set) to the port, default **27888** (0x6CF0), sent once,
  tag 0xFF. A named host is asked up to 4 times. Asks go out at most once per second.
- `probe_tag`: for a named host, the low 5 bits of a per-host counter (slot 0..31) whose send time is recorded;
  0xFF = no timing. The reply's echoed tag gives the round trip.
- Host reply: id mismatch -> `ok = 0` only. Otherwise `ok = 1`, the session name (driver+0x190), the echoed tag,
  `host_flag` (driver+0x194), `link_count` (driver+0x1b0 = number of live links, the "player count"), `host_byte`
  (driver+0x18c, set from the host parameters at host start, engine+0xb6690). The meaning of `host_flag` and
  `host_byte` is UNPROVEN (candidates: password flag, maximum players).

**5.5 Echo** (EVIDENCE: engine+0xb71e0, engine+0xb5120 cases 4 / 5): the asker sends a 24-bit id and records the
send time; the host copies the id back with opcode 5; the asker matches the id and stores the round trip. No state
on the host.

**5.6 Host start.** Listen port default **27888** when the caller passes 0. EVIDENCE: engine+0xb6690.

## 6. Link datagram

```
[check8:u8][has_ctl:u1][ctl block]? [has_rel:u1][rel block]? [has_unrel:u1][unrel block]?
```

EVIDENCE: receive engine+0xb4a70; send engine+0xb3b20 + engine+0xb1440.

**6.1 `check8`.** `check8 = b0 ^ b1 ^ b2 ^ b3` where `b0..b3` are the four bytes of `crc = CRC32(body)`, CRC-32
with the reflected polynomial 0xEDB88320, initial value 0xFFFFFFFF and final complement (the zlib / IEEE form),
computed over the body bits taken 8 at a time from the first bit after `check8` to the end; a final partial byte is
zero-extended. EVIDENCE: table engine+0x80010, CRC engine+0x80f70, fold engine+0xb1440 / engine+0xb4a70.
PROVEN-CAPTURE: 7,939 of 7,939 client datagrams and 5,623 of 5,623 server datagrams of tdm1 satisfy it. Because
`check8` is the first byte and padding is 0, **on the wire `check8 == fold(CRC32(datagram[1..]))`**. A mismatch
drops the datagram silently (logged). PROTOCOL: §4.4 calls this an "8-bit XOR checksum"; the XOR is the fold of a
CRC-32, not a XOR of the payload bytes.

**6.2 Order of blocks.** Fixed: control, reliable, unreliable. A missing presence bit at the end reads as 0
(section 2.3). The sender writes `has_ctl` and `has_rel` always (0 when absent) and writes `has_unrel` only when it
has unreliable messages. EVIDENCE: engine+0xb3b20, engine+0xb2e20, engine+0xaf560. PROVEN-CAPTURE (tdm1): the real
client writes `has_ctl` and `has_rel` as 0 when absent (2,327 datagrams start `[0][0][1]`, byte 1 = 0x04) and ends an
unreliable block without a terminator (an explicit `vlen 0` would make those datagrams one byte longer); `dc-wire`
re-encodes all 7,939 client datagrams bit for bit, including the long `vlen` form (the 2,088-bit hello entry).
Whether `has_unrel = 0` is written when absent is not decidable from the capture (no datagram shows it).

**6.3 Receive side effects** (EVIDENCE: engine+0xb4a70): an empty datagram is ignored; otherwise the link's
traffic stamp (link+0x160) and last-receive time (link+0x164) are set to now **before** the check byte is tested
(so a corrupt datagram still counts as liveness), and a traffic sample is recorded. After a valid link datagram the receive thread runs the link update at once (engine+0xb5d70 ->
engine+0xb3e40), so an ack can leave immediately. PROVEN-CAPTURE (tdm1): the real client acknowledges every received
reliable block in the same 16 ms tick (5,019 of 5,025 in the same tick, all within 16 ms) with a separate 4-byte
ack-only datagram `[check][1][CTL_ACK][ack_seq][0][0]`; its reliable-only and unreliable data datagrams carry no ack
(the only reliable datagram with an ack is the hello, `CTL_ACK 1`; 1 of 2,338 unreliable ones). A server therefore
receives one ack-only datagram per reliable block it sends.

## 7. Control block

`[ctl_op:u4][...]`. EVIDENCE: engine+0xb2500.

| ctl_op | our name | body | receiver action |
|---|---|---|---|
| 0 | `CTL_ACK` | `ack_seq:u13`, `has_sack:u1`, if set `sack_mask:u8` | section 10.2 |
| 1 | `CTL_BYE` | none | link drop, event code 0 if this side is the host (driver+0x188 set) else 3 |
| 3 | `CTL_BYE_CODE` | `bye_code:u32` | link drop with event code `bye_code` |
| 2, 4..15 | (none) | none read | ignored; the rest of the datagram is then parsed from the wrong position, so never send them |

EVIDENCE for the event codes: engine+0xb5d70 (return 1 -> `hosting ? 0 : 3`, other non-zero -> that value).
`CTL_BYE_CODE` with code 0 is indistinguishable from "no disconnect" and is ignored; code 1 behaves like a plain
`CTL_BYE`. After a bye the receiver still parses the remaining presence bits (they read as 0).

The engine's debug log calls `CTL_ACK` a heartbeat: it is sent both to acknowledge and as a periodic keep-alive.

## 8. Reliable block

```
[rel_seq:u13][tail_cont:u1] entry { [more:u1] entry }   with entry = [vlen L][L bits]
```

and the last entry is followed by `more = 0`. An empty reliable block (keep-alive) is `[rel_seq][0][vlen 0][0]`.
EVIDENCE: writer engine+0xb2e20 (first entry has no leading bit, each further entry is preceded by `more = 1`, a
final `more = 0` closes the list), resend writer engine+0xb3490 (each entry followed by `more = (not last)`),
reader engine+0xb2690 (read vlen; if 0 read `more`; else take L bits, fail on short data, read `more`; loop while
`more = 1`). Both descriptions produce the same bits.

- `rel_seq` counts 1, 2, ... 8191, 0, 1 ... (13 bits, wraps). The first reliable block of a link carries 1.
  EVIDENCE: counters link+0xcc (send) and link+0xdc (last in-order received) start at 0 (engine+0xb48b0) and are
  pre-incremented (engine+0xb2e20, engine+0xb2690).
- An entry with `L = 0` delivers nothing.
- `tail_cont = 1`: the last entry of this block is the first part of a longer message that continues as the first
  entry of the next reliable block (section 8.2).
- If an entry claims more bits than remain, the datagram's reliable block is abandoned (logged).

**8.1 Receive order** (EVIDENCE: engine+0xb2690, engine+0xaf450, engine+0xabc10):

- `expected = (last_in_order + 1) & 0x1FFF`.
- `rel_seq == expected`: accept, `last_in_order = rel_seq`, set ack-due, deliver the entries in order; then replay
  stored out-of-order blocks while one matches the new `expected`.
- Else if `(rel_seq - expected) & 0x1FFF < 9` (that is 1..8 ahead): store the whole block (from `rel_seq` to its
  closing bit) in one of **8 slots**, ignoring a duplicate `rel_seq`, set ack-due. If all 8 slots are used the
  block is dropped.
- Else (old or duplicate, or more than 8 ahead): parse and discard; ack-due is not set (the next periodic ack
  covers it, section 11.1).
- The unreliable block of the same datagram is processed in every case.

**8.2 Split messages** (EVIDENCE: sender engine+0xb2e20 PROVEN; receiver engine+0xb2690 PLAUSIBLE, the C loses the
destination of two copies):

- Sender: only a message longer than **255 bits** is split, and only when it would overflow the block budget
  (section 11.2). The first `L - overshoot` bits go out as the last entry with `tail_cont = 1`; the remaining
  `overshoot` bits stay at the head of the reliable queue and become the first entry of the next reliable block
  (which may split again). A message of 255 bits or less that does not fit waits for the next datagram.
- Receiver (in-order blocks only): a link-level assembly buffer (link+0xd0). If it is non-empty, the first entry of
  the block is appended to it; if that entry is also the last one and `tail_cont = 1` the assembly continues,
  otherwise the assembled message is delivered **before** the block's other complete entries. If the buffer is
  empty and the last entry has `tail_cont = 1`, that entry starts a new assembly. All other entries are delivered
  as complete messages in order.

## 9. Unreliable block

`entry entry ...` with `entry = [vlen L][L bits]`, ended by `L = 0` or by the end of the datagram.
EVIDENCE: reader engine+0xb2c20, writer engine+0xaf560 (writes no terminator; the block is last, and trailing pad
bits decode as `L = 0`). An entry longer than the remaining bits abandons the block. Entries are delivered in order
to the link's unreliable inbox.

**9.1 Bye datagrams** (EVIDENCE: engine+0xb3920, engine+0xb3a10): body `[1][CTL_BYE:4]` or
`[1][CTL_BYE_CODE:4][bye_code:u32]`, no presence bits for the other blocks. Sent **4 times**, 100 ms apart
(`Sleep(100)` between copies), each through the normal link send (fresh check byte, subject to flow control).
The drop on idle timeout sends code 3 (section 13).
PROVEN-CAPTURE (tdm1, tdm4): the form, the four copies and the 100 ms spacing hold for the client's own bye. After its
`C2S_BYE` (shell 0xbd leave, then engine id 6 in the next block) the 2009 client sends nothing more on the link (no ack
of later server blocks, no keep-alive); when its shell leaves the result screen it sends the plain `CTL_BYE` (op 1, no
code, datagram `c9 03`) four times about 100 ms apart, 8.7 s (tdm1) and 7.1 s (tdm4) after the host's
`CTL_BYE_CODE(5)` copies. In tdm2 (second leave, round already over) the client sent no `C2S_BYE` at all and only the
four plain `CTL_BYE` when it was back in the waiting room.

More captures (PROVEN-CAPTURE; capval-tdm6 9, capval-rust3 10, dserver-round2 G6; folded by spec-fold-8): the 2009 client sends
`C2S_BYE` 15-16 ms after its 0xbd (tdm6 356.500 -> 356.515; rust3 234.531 -> 234.547). Its plain `CTL_BYE` x4 followed 9.8 s
after the reference stack's code-5 bye in tdm6 (366.343-366.640, in the tick of the lobby frames of its return to the room) and 7.66 s
after its own `C2S_BYE` in rust3. In rust3 our dserver sent the plain `CTL_BYE` x4 at the 0xbd (234.531-234.828), **before** the
client's `C2S_BYE`, and the client accepted it (it returned to the room and later to the lobby as after the reference stack's code-5
bye). DeCombat: DSV-51 (dserver-round2, merged; PENDING in `DC_DEFAULTS.md`): the link stays open until the client's
`C2S_BYE` (then `CTL_BYE_CODE(5)` x4) or 1000 ms (ours), never the plain bye at the 0xbd.

RUN (our stack only, no real client; client-netbye, merged at master d9fce0a, on top of the `dc-net-engine` quiet state of
netengine-bye; folded by spec-fold-7): `dc-botclient`, and `dc-client` whose leave goes through the same session, now
reproduce the silence. After queuing `C2S_BYE` the link goes quiet once that block has left (`Link::quiesce_when_flushed`:
no ack, no keep-alive, no resend), and the plain burst is sent explicitly (`Link::send_plain_bye`). Headless bot-vs-bot round
(output `client-netbye/round1`, not committed; the bot's own capture, client -> server, times in
seconds): Bot1's last game data at 79.921, then the 6-byte datagram carrying the `C2S_BYE` block `ba c2 05 08 06 00` at
79.936, then **only** the plain `CTL_BYE` `c9 03` at 79.983, 80.093, 80.185 and 80.292 (about 100 ms apart); Bot2 the
same (block 76.933, plain byes 76.98 .. 77.289). The server kept sending its keep-alive about every 95 ms after the block
(server -> client until 80.246) and the bot acknowledged none; nothing else left the bot after `C2S_BYE`. Both bots ended on
the host's bye (code 5). The same form as the PROVEN-CAPTURE one above, with one difference: the bot sends its plain burst
right after the host's `CTL_BYE_CODE(5)` (no result screen to leave; L9, CLI-24) instead of 7-9 s later. Not run live: the
`dc-client` full leave (0xbd, 0x394 `{0}`, `C2S_BYE`, silence, burst), which is covered by its unit and session tests
only.

## 10. Length code `vlen` and acks

**10.1 `vlen`** (EVIDENCE: write engine+0xab9e0, read engine+0xaba90): message lengths are in bits.

```
L < 128      : [L & 0x7F : 7][0]
L < 1024     : [L & 0x7F : 7][1][(L >> 7) & 7 : 3][0]
L >= 1024    : [L & 0x7F : 7][1][(L >> 7) & 7 : 3][1] then, for v = L >> 10 while v != 0:
               [v & 1 : 1][(v >> 1) != 0 : 1], v >>= 1
```

Header cost used by the sender's budget: 8 bits if `L < 128`, else `12 + 2 * bitlen(L >> 10)` (engine+0xaf560,
engine+0xb2e20); this matches the encoder exactly.

**10.2 Ack rules.**

- Writer (EVIDENCE: engine+0xabc10): `ack_seq = last_in_order`. `has_sack = 1` when at least one out-of-order
  block is stored; then bit `d` of `sack_mask` is set for each stored block with
  `d = (stored_seq - last_in_order - 2) & 0x1FFF`, `d < 8` (the field is 8 bits; slots with `d >= 9` are freed).
  Writing an ack clears ack-due and stamps `last_ack_sent` (link+0xe0).
- Receiver (EVIDENCE: engine+0xb2500, engine+0xb23d0, engine+0xaf2b0): find the sent record with
  `seq == ack_seq`. If none, the ack is stale ("old") and only the selective part is used. If found: take a latency
  sample (now - first send time of that record) and remove every record from the head of the sent list up to and
  including it (cumulative), then cap the resend counter of the remaining records at 3. For `sack_mask` bit `i`
  (`i = 0..7`) the record `seq = ack_seq + 2 + i` is marked acknowledged (latency sample if newly marked). If any
  record was newly marked, the first unacknowledged record is made due for resend now (its last-send stamp is pulled
  back by `max(L_ms, 17)`) and ack-due is set: a fast retransmit of the gap.
- Quirk: the receiver adds `+1` per mask bit without masking to 13 bits, so near the wrap (ack_seq 8183..8189) the
  high mask bits never match. Implementers should mask; interop is unaffected because the sender masks.

## 11. Sender behaviour

**11.1 Send pump** (EVIDENCE: engine+0xb3b20, run from the link update engine+0xb3e40, which the driver update
engine+0xb5c30 calls for every link and the receive thread calls after every valid link datagram):

1. Run the resend scan (11.3).
2. `ack_due_now = not blocked(58) and (ack-due or hb_due)`, where `hb_due` = `now - last_ack_sent >= 200` and
   `>= trunc(min(rtt_mean, 1000.0))` (EVIDENCE: engine+0xad520 PROVEN, inputs DISASM engine+0xad541..0xad56b).
   `keepalive_now` = the same timing test (engine+0xb3b81..0xb3bab, DISASM) and reliable sending not paused.
3. If (not paused and a reliable or unreliable queue is non-empty) or `ack_due_now` or `keepalive_now`: build a
   datagram: `[has_ctl][ack]`, the reliable block (forced, possibly empty, when `keepalive_now`), the unreliable
   block. If the body is under 4 bits nothing is sent. If an ack was due when the build started, was cleared by
   the build, and the whole datagram is blocked by flow control, ack-due is set again and the pump stops.
4. Send it (11.6). Repeat while the queues are non-empty, not paused, and flow control leaves 256 bits of room
   (`2W >= used + debt + 256`).

So a quiet link still sends, every `max(200, min(L_ms, 1000))` ms, an ack plus an empty reliable block; the
reliable block drives latency sampling. PROVEN-CAPTURE (tdm1, loopback): the real client's keep-alive
period is 201.5 ms on average (29 intervals, 187-204 ms at the tap resolution of 15.625 ms); each keep-alive is 7 bytes
`[ack][rel n][vlen 0][more 0]` and takes a `rel_seq` like any block; it carries the ack value already sent, and the
timer runs from the client's own last ack (`now - last_ack_sent >= 200`). While the client acknowledges server blocks more
often than every 200 ms (in world, about 31.6 per second) it sends no keep-alive. Whether the period uses
`L_ms = trunc(rtt_mean)` or `L_eff` (DSV-4) is not decided: both give 200 ms at L about 0.

**11.2 Reliable block build** (EVIDENCE: engine+0xb2e20): written only when not paused and (the queue is non-empty or
forced) and flow control allows the current datagram. Budget `B_rel = min(8800, W / 2)` bits (W in section 12).
Entries are taken from the queue while `round8(datagram_bits + block_bits + header + L + 8) + 224 <= B_rel`,
splitting as in 8.2. A sent record keeps `seq`, the block bits, the entries, `first_send = now`,
`next_check = now + clamp(L_ms, 17, 1000)`, acked flag 0, resend count 0, `tail_cont`. On the first reliable
send while no ack is outstanding, the link stamps `wait_start` (link+0x144) and sets `waiting` (link+0x140).

**11.3 Resend scan** (EVIDENCE: engine+0xb3680, engine+0xb3490):

- `RTO = clamp(L_ms, 17, 1000)`.
- Pop acknowledged records from the head of the sent list.
- **Trickle**: if any unacknowledged record has been resent more than 10 times, mark the link stalled (link+0x148,
  stamp link+0x14c on entry), set the head record's count to 11, use `RTO = max(RTO, 500)`, and resend only the
  head record when `now - min(next_check, now) >= RTO` and flow control allows; stop.
- Otherwise clear stalled. **Pause** new reliable blocks if more than 8 records remain in the sent list
  (selectively acknowledged ones included), or if any
  unacknowledged record has been resent more than 2 times (or the link was stalled at the start of the scan).
- Resend every unacknowledged record with `now - min(next_check, now) >= RTO`, if flow control allows
  `bits + 296`; set `next_check = now`, count += 1. Because `next_check` starts at `first_send + RTO`, the first
  resend comes after about `2 * RTO`, later ones every `RTO`.
- A resend datagram is `[has_ctl][ack if not blocked(58) and (ack-due or hb_due)][1][same rel_seq][same tail_cont]`
  plus the same entries, then the current unreliable block. EVIDENCE: engine+0xb3490.

**11.4 Unreliable block build** (EVIDENCE: engine+0xaf560): first drop queued unreliable messages older than
`2 * max(L_ms, 200)` ms. Then, if flow control allows, take messages while
`2W >= used + debt + round8(datagram_bits + floor(1.5 * (header + L)) + 8) + 224` and the datagram stays within
**8800 bits** (`datagram_bits + header + L <= 8800`, except that a message is always taken into a datagram that
holds 2 bits or less). The presence bit is written before the first message only.

**11.5 Window and size limits:** 13-bit sequence space, up to 8 stored out-of-order blocks on the receiver, sender
pauses above 8 outstanding records (re-evaluated once per pump; a single pump can exceed it while flow control
allows). Normal datagrams carry at most about 8800 body bits (1101 bytes with the check byte); a lone unreliable
message can make a datagram larger, up to the 8192-byte receive buffer of the peer.

**11.6 Link send** (EVIDENCE: engine+0xb1440): `cost = round8(body_bits + 8) + 224` bits (224 = 28 bytes of
IP+UDP header). If flow control is blocked for `cost` the datagram is **silently dropped** and the send reports
success (reliable data is recovered by resend; unreliable data is lost). Else add `cost` to the bucket, prefix
`check8`, and send.

## 12. Latency estimate and flow control

**12.1 Latency** (EVIDENCE: engine+0xabb40, engine+0xad580, engine+0xb48b0):

- `rtt_mean`: mean of the last up to 16 samples (ring of 16); initial value **200.0 ms**. A new sample clears
  `waiting`.
- Effective latency `L_eff = waiting ? max(rtt_mean, now - wait_start) : rtt_mean`, refreshed every link update.
- `L_ms` in this spec is `trunc(rtt_mean)` (link+0x138). Every comparison loads link+0x138 before the truncation
  helper: resend scan engine+0xb36a5 (`RTO = clamp(L_ms, 17, 1000)`), reliable build engine+0xb330c (`next_check =
  first_send + clamp(L_ms, 17, 1000)`), unreliable build engine+0xaf568 (age limit `2 * max(L_ms, 200)`, no upper
  bound), fast retransmit engine+0xb264a (pull-back `max(L_ms, 17)`), heartbeat test engine+0xad541 and send pump
  engine+0xb3b81 (both `min(rtt_mean, 1000.0)` before the truncation). `L_eff` (link+0x13c) is read only through
  link method +0x20 (engine+0xb22c0), that is only by the 30 s latency warning of section 13; it never changes ack,
  resend, heartbeat or unreliable-age timing.
- A loss ratio over the last 64 sends (1 = acked, 0 = resent) is kept for statistics only (engine+0xaf970).

**12.2 Rate.** All costs are bit counts (11.6). A link keeps three rates, all 0x7FFFFFFF after link init
(engine+0xb48b0): `rate_req` (link+0x154), `rate_cap` (link+0x158) and the effective `rate_bps` (link+0x150).

- set-rate (link method +0x0c, engine+0xaea50, PROVEN): if `v == rate_req` nothing happens; else `rate_req = v`,
  `rate_bps = min(v * 90 / 100, rate_cap)` (unsigned 32-bit: `v * 90` wraps for `v > 47721858`, irrelevant for the
  2009 values) and the bucket is reset (engine+0xad5e0). **A link sends at 90 % of its requested rate.**
- get-rate (link method +0x08, engine+0xabdd0, DISASM) returns `rate_req`.
- rate cap (engine+0xad500, PROVEN): `rate_cap = v`, then set-rate(`min(v, rate_req)`). Only the host calls it, once
  per new link, with `max(join-ask rate, the host's own client-rate global)` (5.3).
- `W = rate_bps * 200 / 1000` bits per 200 ms (engine+0xad5e0, kept at link+0x3f8; the bucket's `used`, `debt` and
  last tick are link+0x3ec, link+0x3f0 and link+0x3f4).
- Client side: the version message lowers `rate_req` to the host's rate when that is smaller (engine+0xa2a70); the
  rate report (engine+0x1ae50, part A 5.5) calls set-rate with the client-rate global whenever the reported
  `rate_8k` changes, which includes its first send. The report runs only once the server connection's world-loaded
  flag is set (part B 10.1), that is after the version message, so the client's own link ends at the client-rate
  global: with the CShell value of 5.3, `rate_bps = 4608000` (order PLAUSIBLE).
- Host side for a 2009 client asking 5120000: `rate_cap = 5120000`, `rate_bps = 4608000`.

**12.3 Bucket** (EVIDENCE: engine+0xad650, engine+0xad7c0, engine+0xad5e0): two counters `used <= W` and `debt`.

- First use: reset (`W` from the rate, `debt = min(debt, W)`, `used = min(used, W)`), stamp `last = now`.
- Tick (only when `now - last >= 17`): `drain = (now - last) * rate_bps / 1000`; if `debt < drain` then `debt = 0`
  else move `drain` from `debt` to `used`; then `used = max(0, used - drain)`; `last = now`.
- Add `cost`: fill `used` up to `W`, the rest goes to `debt`.
- `blocked(cost)` = `2W < used + debt + cost` (after a tick).

## 13. Liveness and disconnect

- Idle drop (EVIDENCE: engine+0xb5c30): for each link whose live flag is set (link method +0x28 = engine+0xb2290
  returns the byte at link+0x48; method +0x24 = engine+0xb2280 sets it; DISASM. One setter is engine+0x7b1d0,
  PROVEN, which sets it on every connection of a list that does not have it yet and is called from the server frame
  engine+0x9e4e0; the client-side setter was not traced), if nothing has been received for **more than 90000 ms**
  (`now - last_recv >= 90001`, last_recv = link+0x164), the link's status record becomes {+0x50 = 1, +0x54 = 1,
  +0x58 = ms since `wait_start` (link+0x144), +0x5c = 30000}, the link is dropped with code 3 and a
  `CTL_BYE_CODE(3)` is sent (4 copies, engine+0xb4ff0 -> engine+0xb3a10).
- High-latency warning (EVIDENCE: engine+0xb5c30, engine+0xb22c0 PROVEN): if `L_eff` (link method +0x20 returns
  the float at link+0x13c) exceeds 30000.0 ms, the status record becomes {+0x50 = 1, +0x54 = 1,
  +0x58 = trunc(L_eff), +0x5c = 30001}; the link is not dropped.
- Ack stall (link method +0x00 = engine+0xabd90, DISASM): returns 1 when the link is waiting for an ack
  (link+0x140) and `now - wait_start > 90000` ms. The session layer skips a client's object updates while it
  returns 1 (part B 12.3).
- Local drop (EVIDENCE: engine+0xb4ff0): remove the link, decrement `link_count`; optionally send `CTL_BYE`
  (code 0) or `CTL_BYE_CODE(code)` first.
- The receive thread also runs per-link maintenance when it wakes from `select()` without data (section 4): tick
  flow control, run the pump, refresh `L_eff`, and record a traffic sample every 100 ms. EVIDENCE: engine+0xb5d70.

## 14. Engine protocol version

The engine protocol version is **7**. The host's first engine message carries it (`[u8 4][u32 7][u32 host_rate]`,
EVIDENCE: engine+0x8e9f0, host rate = global at VA 0x6cb29c, initial value 5120000, PE-DATA); the client refuses
any other value with result code 0x4E (EVIDENCE: engine+0xa2a70). PROTOCOL: §4.4 (PROVEN). The message id byte and
everything after it belong to `UDP_SESSION_2009.md`; the transport itself has no version field.

## 15. Compact field encodings

**15.1 `pos48`, compact position** (wire: `[px:u16][py:u16][pz:u16]` in that order; write engine+0x5c040, read
engine+0x5c320 and engine+0x5c730, PROVEN). Both readers set the 8-bit extra-precision byte `e` to 0.

The position service is the client world-geometry object. The global at VA 0x6e0770 is the target of an interface
holder that the static initialiser engine+0x207f90 constructs (holder VA 0x6e0774, name = the ROM string at VA
0x675010; DISASM). The only implementation registered under that name string is the static object at VA 0x6e2920
(registration engine+0x2094e0 through engine+0x5ee50, constructor engine+0xcca00, method table VA 0x67fe10; DISASM;
every other user of the string constructs a holder). That the registry binds the holder to it is PLAUSIBLE. Its
methods:

- +0x04 encode (engine+0xcb720, DISASM) = compression slot 3 (engine+0x21820) called with
  `(out, pos, min = Wd+0x20, inv = Wd+0x44, precise = 1)`;
- +0x08 decode (engine+0xcb750, DISASM) = compression slot 4 (engine+0x21910) called with
  `(out, packed, min = Wd+0x20, max = Wd+0x2c, precise = 1)`;

where `Wd` is the shared world object (holder target VA 0x6e28dc, name at VA 0x675028, implementation the static
object VA 0x6e2aa8, constructor engine+0xcfa00) and the compression object is reached through the holder target VA
0x6e28d4 (method table VA 0x675c5c: slot 0 name, 1 vector encode, 2 vector decode, 3 position encode, 4 position
decode, 5 rotation encode, 6 rotation decode; PE-DATA). The world body loader engine+0xcf3f0 (PROVEN, clean tree)
stores `extent_min` at `Wd+0x20`, `extent_max` at `Wd+0x2c` and `1 / (extent_max - extent_min)` per axis at
`Wd+0x44` (FMT_DAT85.md section 4, rows 3-4). **So the box is the world file's extents, `precise` is 1 and `e` is
0.**

- Decode (engine+0x21910, PROVEN): `x = min.x + (max.x - min.x) * (8*px + 0.5) * 2^-19`,
  `y = min.y + (max.y - min.y) * (4*py + 0.5) * 2^-18`, `z = min.z + (max.z - min.z) * (8*pz + 0.5) * 2^-19`
  (2^-19 at VA 0x675c3c, 2^-18 at VA 0x675c14). The function also has a `precise = 0` form (cell offsets 4, 2, 4)
  and adds the bits of a non-zero `e`; `pos48` uses neither.
- Encode (engine+0x21820, inputs DISASM engine+0x21823..0x218bb): with single-precision products
  `t = f32(f32(inv.x * f32(x - min.x)) * 524288.0)`, `ux = trunc(t + 0.5)`; `uy` the same with `262144.0`; `uz` like
  `ux` (constants at VA 0x675c34, 0x675c38, 0x672918); `px = u16(ux >> 3)`, `py = u16(uy >> 2)`, `pz = u16(uz >> 3)`
  (logical shifts, low 16 bits kept). The extra byte is computed (`precise = 1`) and dropped by the `pos48` writer.
- Accuracy: the decoded index `8*px + 0.5` lies between `t - 7` and `t + 1` (x and z, units of 2^-19 of the
  extent); `4*py + 0.5` lies between `t - 3` and `t + 1` (y, units of 2^-18).
- Edge quirk: a coordinate exactly at `extent_max` gives `ux = 2^19`, `px = 0x10000`, written as 0, which decodes at
  `extent_min`; coordinates outside the box wrap the same way. A DeCombat encoder clamps `ux` and `uz` to
  `0 ..= 2^19 - 1` and `uy` to `0 ..= 2^18 - 1` before the shifts (the decoder accepts any value).
- The second 48-bit position pair (engine+0x5c980, engine+0x5cbf0) uses the server world-geometry object (holder
  target VA 0x6e07cc, name at VA 0x678420, implementation the static object VA 0x6e2a08, method table VA 0x67ff58):
  its +0x04 / +0x08 (engine+0xce350 / engine+0xce380, DISASM) are instruction for instruction the client ones, with
  the shared-world holder target VA 0x6e29d0 and the compression holder target VA 0x6e29cc (same names as above).
  Same box, same rule.

**15.2 `vec72`, compact vector** (wire: `[major:f32][r1:u16][r2:u16][vflags:u8]` in that order; write
engine+0x5b360, read engine+0x5b5d0 / engine+0x5bce0, PROVEN, wire order DISASM engine+0x5b379..0x5b3bf; encode
engine+0x21540 = compression slot 1, decode engine+0x216a0 = compression slot 2, both through the compression object
held at VA 0x6e075c). Used for velocities (the engine object-update reader takes `pos48`, a 1-bit flag, then
`vec72`, engine+0xa4160).

- Encoder (engine+0x21540, PROVEN; scale DISASM engine+0x215e9..0x21655): `major` = the component of largest
  magnitude, stored as a signed f32 (strict `>` comparisons: ties prefer x over y, then the current choice over z).
  The other two components `(o1, o2)` are `(y, z)` for x-major, `(x, z)` for y-major, `(x, y)` for z-major.
  `q1 = trunc(abs(o1) * (1.0 / abs(major)) * 262143.0)` and `q2` the same for `o2` (x87; 262143.0 at VA 0x675c10),
  so `0 <= q < 2^18`; `r1 = q1 & 0xFFFF`, `r2 = q2 & 0xFFFF`. A zero vector gives `q = 0` (the truncation helper
  returns 0 for the NaN of `0 * inf`).
- `vflags`: bits 0-1 = `q2 >> 16`, bit 2 = sign of `o2` differs from sign of `major`, bits 3-4 = `q1 >> 16`,
  bit 5 = sign of `o1` differs, bits 6-7 = major axis (0 = x, 1 = y, 2 = z). Sign test: `v >= 0` counts as positive.
- Decoder (engine+0x216a0, PROVEN, clean tree): `q1 = r1 + ((vflags >> 3) & 3) * 65536`,
  `q2 = r2 + (vflags & 3) * 65536`; `o1 = q1 * major * 2^-18`, negated when bit 5 is set; `o2 = q2 * major * 2^-18`,
  negated when bit 2 is set (2^-18 at VA 0x675c14); axis 1 -> `(o1, major, o2)`, axis 2 -> `(o1, o2, major)`,
  axis 0 or 3 -> `(major, o1, o2)`. The decoder scale is 2^-18 while the encoder scale is 262143, so a round trip
  shrinks each minor component by at most `abs(major) / 2^18`.

**15.3 `yaw8`, yaw-only rotation** (EVIDENCE: read engine+0x5b990 / engine+0x5c000, write engine+0x5b8a0):

- The **shell's** yaw byte in the client's player update (`UDP_SESSION_2009_B.md` 10.3) uses the same scale and the same
  truncation toward zero: PROVEN-CAPTURE, `trunc(wrap(yaw) * 40.425354)` in 2,338 of 2,338 (tdm1), 1,361 + 483 (tdm2) and
  1,103 (tdm4) updates; rounding matches only a minority.
- Read: `yaw = s8 * 0.02473695` radians (= 2π/254), then the rotation is built from Euler angles (0, yaw, 0)
  (engine+0x5b6c0; quaternion convention inside UNPROVEN).
- Write (engine+0x5b8a0, PROVEN for the formula, DISASM for the scale at engine+0x5b8d8): from the quaternion
  `(x, y, z, w)`, `yaw = atan2(2 (x z + w y), 1 - 2 (x² + y²))`, then `s8 = trunc(yaw * 40.425354)` (f32 at VA
  0x679e1c = 254 / (2π)), toward zero, and its low 8 bits are written. The reader's 2π/254 inverts it.

**15.4 `quat128`, full rotation**: `[x:f32][y:f32][z:f32][w:f32]` in that order. EVIDENCE: engine+0x5b3d0 /
engine+0x5b650, component order from the quaternion-to-matrix helper engine+0x5f6d0 (w is the fourth float).

**15.5 `rot24` / `rot48`, compact rotation** (encode engine+0x21b90, compression slot 5; read engine+0x5bdd0, which
calls slot 6; decode engine+0x21dc0, compression slot 6, PROVEN, clean tree). With `R` the rotation matrix of the
quaternion (engine+0x5f6d0, row-major, `F = (R02, R12, R22)` = the rotated +Z axis, `U = (R01, R11, R21)` = the
rotated +Y axis):

- `c0 = trunc((F.y + 1) * 63.9)`, `c1 = trunc((F.x + 1) * 63.9)`, `c2 = trunc(F.z * 127.9)`,
  `c3 = trunc(U.x * 127.9)`, `c4 = trunc(U.y * 127.9)`, `c5 = trunc(U.z * 127.9)` (signed bytes).
- Short form test: rebuild `F' = ((c1/127 - 0.5) * 2, (c0/127 - 0.5) * 2, c2/127)`, take `u = 50·(0,1,0)` made
  orthogonal to `F'`, normalise; if `|u|² > 0.1` and `u·U > 0.999` the short form is used; if `u·U < -0.999`,
  `c1` is negated (0 becomes -1) and the short form is used. Short form marker: `c0` negated (0 becomes -1).
- Wire: `[c0:s8][c1:s8][c2:s8]`, then `[c3:s8][c4:s8][c5:s8]` only when `c0 >= 0`. EVIDENCE: engine+0x5bdd0.
- Decoder (engine+0x21dc0, PROVEN): `F' = ((abs(c1)/127 - 0.5) * 2, (abs(c0)/127 - 0.5) * 2, c2/127)`.
  Long form (`c0 >= 0`): `U' = (c3, c4, c5) / 127`; neither vector is normalised. Short form (`c0 < 0`): `u` =
  `50·(0,1,0)` made orthogonal to `F'`, normalised when non-zero, `F'` normalised when non-zero, and `U' = -u` if
  `c1 < 0`, else `U' = u`. Then `Rv = U' x F'`, `F2 = Rv x U'` (computed from the unnormalised `Rv`), `Rv` and `F2`
  normalised, `U2 = F2 x Rv`; the 4x4 row-major matrix with columns `(Rv, U2, F2)` and last row `(0, 0, 0, 1)` goes
  to the matrix-to-quaternion helper engine+0x5f850 (PROVEN; trace branch when `R00 + R11 + R22 >= -0.999`, else
  the largest-diagonal branch). Side effect: the decoder negates `c0` and `c1` in the caller's 6-byte buffer.

**15.6 `objref16`**: section 3.

## 16. Debug knobs (all 0 by default, PE-DATA)

Incoming drop percentage (VA 0x6df968, engine+0xb4a70), outgoing corruption percentage (VA 0x6df96c,
engine+0xb0c90: corrupts 1..10 random bytes), and a log level (VA 0x6df8a8). A Rust implementation may offer the
same knobs for tests (DeCombat design, not required).

## 17. Disagreements with the current `dc-wire/src/udp.rs` (concept level only)

The current file (to be replaced) disagrees with the 2009 C on: the out-of-band marker (2009: full 32-bit
`OOB_MAGIC`, then a 3-bit opcode), the opcode values (2009: join ask 2, join reply 3, echo 4 / 5, info 0 / 1),
the bye opcode (2009: control op 1, or 3 with a u32 code), the check byte (2009: XOR fold of CRC-32), the maximum
datagram (2009: 8800-bit body budget and 8192-byte receive buffer) and the compact vector / position scales
(section 15). Its constants must not be reused.

## 18. Validation plan (black-box against the reference game server)

All checks run on loopback and are scheduled by the coordinator (no listeners or game windows in this wave).
Setup: a small datagram tap (our own script, proposed `evidence/udp_tap.py`) sits between the real 2009 client
and reference game server: it listens on the port the game-start message advertises, forwards both directions, and appends one
JSON line per datagram (`t_ms`, direction, hex bytes). Optional fault injection: drop, delay or duplicate the n-th
datagram of a direction. A second mode sends single crafted out-of-band datagrams to reference game server and records the
answer. All analysis is offline in a Python or Rust decoder built from this spec.

| id | rule | how to verify | pass condition |
|---|---|---|---|
| V1 | 2.1, 2.2 bit order | decode the first datagram of a session | first 4 bytes `10 7F 33 01`, opcode bits 32-34 = 2 |
| V2 | 5 join layout | client -> server first datagram | 25 bytes; reply 5 bytes with bit 35 = 1 |
| V3 | 5.1 app id, 5.3 rate | read bits 35-162 and 163-194 of the join ask | matches PROTOCOL §4.4 id; record the rate value (observed: 5120000) |
| V4 | 5.2 retry timing | drop the first 3 join asks in the tap | asks 300 ms apart, client gives up after 10 s when all are dropped |
| V5 | 6.1 check byte | every non-OOB datagram | `d[0] == fold(crc32(d[1:]))` for 100% of datagrams |
| V6 | 7, 8, 10 framing | parse every link datagram to its end | no parse error, no leftover bits other than < 8 zero pad bits, all `vlen` values consistent |
| V7 | 11.1 heartbeat | idle period in the lobby-to-game transition | ack + empty reliable block every `max(200, L)` ms; estimate `L` from ack round trips |
| V8 | 8.1, 10.2 sequencing | follow `rel_seq` and `ack_seq` both ways | first `rel_seq` = 1, +1 per new block, acks cumulative |
| V9 | 11.3 resend | tap drops one reliable datagram | resend with the same `rel_seq` after about 2 RTO, `sack_mask` bit set on the peer's ack if later blocks arrived |
| V10 | 8.1 out-of-order | tap swaps two reliable datagrams | receiver stores and replays; no duplicate delivery visible in game |
| V11 | 8.2 split | world load / big reliable message | an entry > 255 bits split with `tail_cont = 1`; reassembled length equals the original |
| V12 | 9 unreliable | in-game movement | unreliable entries parse to the end of the datagram |
| V13 | 9.1 bye | client quits a match | 4 identical-body bye datagrams about 100 ms apart |
| V14 | 13 idle drop | tap blackholes the client -> server direction for 95 s (coordinator only); repeat with server -> client | server (then client) sends `CTL_BYE_CODE(3)` x4 about 90 s after its last receive |
| V15 | 5.4 info probe | crafted `OOB_INFO_ASK` with the right and a wrong app id | reply layout as in 5.4; `ok = 0` on the wrong id; `link_count` equals connected clients |
| V16 | 5.5 echo | crafted `OOB_ECHO_ASK` | 24-bit id echoed with opcode 5 |
| V17 | 15.1 `pos48` | decode positions of a known stationary object across sessions | decoded values match 15.1 with the world file's extents and cell offset 0.5 (no fit); an edge position shows the wrap quirk |
| V18 | 15.2 / 15.3 scales | log `vec72` and `yaw8` while turning on the spot and walking a straight line | yaw bytes step by 2π/254 and truncate toward zero; minor/major ratios equal `q / 2^18` within one unit; this confirms the DISASM-graded encoder inputs |
| V19 | 11.4, 11.6 sizes | largest datagram over a full match | <= 1101 bytes unless a single unreliable message is larger |
| V20 | 12 flow control | count body bits per 200 ms window | never above `2W` for the rate seen in V3 / the version message |

A Rust decoder and encoder pass when they reproduce every captured datagram bit for bit (decode then re-encode the
same fields) for V1-V13, and when a Rust host replaces reference game server in the same tap setup and the client completes a
match (that last step is a live test; see the coordinator's test schedule).

**Capture results (tdm1, the real client against the reference server through the tap; 7,940 client and 5,624 server
datagrams; no loss, loopback).** Client-to-server rows are PROVEN-CAPTURE, server-to-client rows REFERENCE or
REFERENCE-CONFIRMED. Detail and replacement text: Appendix C.

| id | result | grade |
|---|---|---|
| V1, V2 | pass (c2s seq 266 ask `10 7f 33 01 ...`, opcode 2; s2c seq 267 reply `10 7f 33 01 0b`, bit 35 = 1) | ask PROVEN-CAPTURE, reply REFERENCE-CONFIRMED |
| V3 | pass: GUID record at bits 35-162, `rate_bps` 5120000 (decides DSV-5) | PROVEN-CAPTURE |
| V4 | undecided (one ask, answered in the same tick) | |
| V5 | pass on 13,562 of 13,562 link datagrams | PROVEN-CAPTURE (c2s), REFERENCE (s2c) |
| V6 | pass; `dc-wire` re-encodes every datagram bit for bit | PROVEN-CAPTURE |
| V7 | pass on loopback, 201.5 ms; `L_ms` versus `L_eff` undecided | PROVEN-CAPTURE |
| V8 | pass (c2s 1..584, s2c 1..5026, no gap, no wrap reached, no selective-ack mask) | PROVEN-CAPTURE (c2s), REFERENCE (s2c) |
| V9, V10 | undecided (no loss, no out-of-order arrival) | |
| V11 | receiver rule pass: 3 split messages (2, 20 and 4 parts) reassembled and acknowledged; the client split nothing itself | REFERENCE-CONFIRMED |
| V12 | pass (2,338 unreliable entries in 2,338 datagrams, one shell 0x64 each) | PROVEN-CAPTURE (c2s) |
| V13 | pass for the form, four copies and spacing; new client detail in 9.1 | PROVEN-CAPTURE |
| V14, V15, V16 | undecided (the client sent no info or echo ask; no 90 s silence) | |
| V17 | pass for the reference stack's encoder (67 exact 48-bit matches with the client's reports); the client's `pos48` decoder undecided | REFERENCE |
| V18 | `vec72` scale 262143 and engine `yaw8` truncation pass (REFERENCE); the shell's yaw byte truncates (PROVEN-CAPTURE) | see 15.3 |
| V19, V20 | pass (largest c2s datagram 269 bytes, s2c 1,081 bytes; far below 2W) | PROVEN-CAPTURE (c2s), REFERENCE (s2c) |

## 19. Open items (to resolve by capture or by a further disassembly pass)

1. Closed by the disasm pass: the truncation-helper inputs (`L_ms = trunc(rtt_mean)`, 12.1; the `pos48`, `vec72`
   and `yaw8` scales, 15.1-15.3; grade DISASM). Captures V7, V17 and V18 confirm.
2. Closed: `vec72` decode (engine+0x216a0), rotation decode (engine+0x21dc0), set-rate (engine+0xaea50), latency
   warning (engine+0xb22c0 returns `L_eff`), `zstr16` writer (engine+0x80ed0), position service VA 0x6e0770 (box =
   world extents, 15.1). Open: that the interface registry binds each holder to the one implementation registered
   under the same name string (PLAUSIBLE).
3. Closed (PROVEN-CAPTURE, tdm1 seq 266): the client's join rate is 5120000 and the id is the GUID record (5.1, 5.3).
4. Meaning of `host_flag` and `host_byte` in the info reply (not attempted).
5. `lstr16` reader: the skip quirk is stated (section 3) and the form has no CShell C call site. Open: whether any
   game code reaches reader slot +0x64 through a path the clean C does not show.
6. Closed: the receive thread's `select()` timeout is 30 s (DISASM).
7. The split-message reassembly order (8.2) is PLAUSIBLE, not literally visible. A DISASM pass over engine+0xb2690
   is feasible: the link-level assembly writer at link+0xd0 is appended to at engine+0xb277b and engine+0xb2a35.
   REFERENCE-CONFIRMED (tdm1): the order agrees with the reference stack's writer, and the 2009 client accepted three split messages
   of 2, 20 and 4 parts; the client's own split path is not exercised (its messages stayed under the block budget).
8. Closed (PROVEN-CAPTURE, tdm1): `S2C_VERSION` arrived at 133.359 s; the client's first `C2S_RATE` is `07 80 02`
   (`rate_8k` = 640) at 139.687 s, in the same reliable block right after `C2S_LOAD_STATE(0)`, 6.3 s later.
9. Still open (no capture exercises them): join retry timing (V4), resend, RTO, selective acks and the out-of-order
   store (V9, V10), the 13-bit sequence wrap (not reached; about 4.3 minutes in world at the reference stack's 31.6 blocks per
   second), idle drop (V14), info probe and echo (V15, V16), the client-side `pos48` / `vec72` decode, `has_unrel = 0`
   when absent, and `L_ms` versus `L_eff`.

## Appendix Z: evidence notes of the items folded in from the disasm-pass addendum (2026-10-24)

Each item below was applied into the text above; this list keeps its grade / evidence paragraph so that the
rva citations stay graded. Grades are those of the source addendum, unchanged.

- **T0. Evidence grades: name the clean tree and add the DISASM grade**
  Editorial; the clean tree is the preferred source per CLEANROOM.md section 2.

- **T1. Function map: the `zstr16` writer exists**
  PROVEN: `Engine.exe/80ed0.c` (clean-tree README: created, 158 bytes, first code byte after the 0xCC padding that follows engine+0x80e30).

- **T2. Function map: rows for the functions read in this pass**
  Addresses from the clean function table and from the method tables (PE-DATA); each row is detailed in the section it belongs to.

- **T3. Section 3: `zstr16` writer**
  PROVEN (clean tree 80ed0.c: the accumulator update `acc |= unit << fill; fill += 16`, a flush at 32 bits, then `write(0, 16)`); the slot is DISASM (writer table VA 0x679e48 +0x3c -> engine+0x414b0 -> engine+0x80ed0).

- **T4. Section 3: `lstr16` reader, the skip rule**
  PROVEN (clean tree 5b900.c) and DISASM (engine+0x5b93b..0x5b96b: `cmp edi, ebx; jbe` skips the adjustment when `S-1 <= n`, otherwise `pos = pos - (S-1) + n`). The CShell count (no clean-tree CShell function with a `+ 0x64` reader call) is a grep over `CShell.dll`.

- **T5. Section 3: reader and writer method tables, writer side of the length forms**
  DISASM: wrappers engine+0x41460 (+0x24), 0x41470 (+0x28), 0x41490 (+0x2c), 0x414a0 (+0x38), 0x414b0 (+0x3c), 0x414c0 (+0x44), 0x414e0 (+0x4c), 0x41500 (+0x5c), 0x41520 (+0x60), 0x5c290 (+0x48); engine+0x5b2b0, 0x5b2e0, 0x5b300 and 0x5b4d0 read in full; reader wrappers engine+0x415c0, 0x415d0, 0x41db0, 0x415e0, 0x415f0, 0x41600, 0x41610, 0x41620, 0x41630, 0x41640, 0x41650, 0x5c5a0, 0x5c890. Table bounds: the world-bound writer table ends at +0x64 (VA 0x679eac) and the reader table follows at VA 0x679eb0; each table is referenced from its constructor (PE-DATA cross-references).

- **T6. Section 4: the receive thread's `select()` timeout is 30 s**
  DISASM; the field order of the Winsock `timeval` (seconds first) is public knowledge.

- **T7. Section 5.3 step 3: rate cap and set-rate**
  PROVEN (clean tree ad500.c, aea50.c, b5120.c), DISASM for the tail call (engine+0xad50c..0xad51b: `cmovb`, then a jump through the link method table +0x0c).

- **T8. Section 5.3: the 2009 client's join-ask rate**
  PROVEN for the CShell calls and float constants (0x4a9c4000 = 5120000.0, 0x470c0000 = 35840.0 in 1e9570.c and 1ea440.c), DISASM for cs+0x1a0040 (a format with the string at VA 0x374e9a94, then a call through member +0x208 of the engine client interface) and for the member installed by engine+0x14270 (`+0x208` = VA 0x46f410, a jump to engine+0x24060).

- **T9. Section 11.1 step 2: heartbeat timing input**
  DISASM: both sites load link+0x138 (`rtt_mean`), take the minimum with the f32 1000.0 at VA 0x675270 (`comiss`, `ja`), then call the truncation helper.

- **T10. Section 11.1: heartbeat period**
  Follows from T9.

- **T11. Section 12.1: which latency value `L_ms` is**
  DISASM for the six loads of link+0x138 and the clamps; PROVEN that engine+0xb22c0 returns the float at link+0x13c (clean tree b22c0.c) and that engine+0xad580 writes `L_eff` there (clean tree ad580.c).

- **T12. Section 12.2: the rate model**
  PROVEN (clean tree aea50.c, ad500.c, ad5e0.c, b48b0.c, 1ae50.c, a2a70.c); DISASM for get-rate (`mov eax, [ecx + 0x154]; ret`) and for the link method table VA 0x67ef3c (+0x08 = engine+0xabdd0, +0x0c = engine+0xaea50, +0x20 = engine+0xb22c0; PE-DATA).

- **T13. Section 13: live flag, idle-drop status, latency warning, ack stall**
  PROVEN (clean tree b5c30.c, b22c0.c, 7b1d0.c, 9e4e0.c), DISASM for engine+0xb2280, engine+0xb2290, engine+0xabd90 and the link method table VA 0x67ef3c.

- **T14. Section 15.1: `pos48` fully determined**
  PROVEN: 21910.c, 21820.c, 5c040.c, 5c320.c, 5c730.c, cf3f0.c, cca00.c, cfa00.c, 5ee50.c (clean tree). DISASM: the four service methods, the static initialisers engine+0x207f90, engine+0x2094e0, engine+0x209400, engine+0x209460 and engine+0x209710, the encoder's SSE and x87 inputs, and the writer's three 16-bit writes (engine+0x5c068..0x5c090). PE-DATA: the method tables and float constants.

- **T15. Section 15.2: `vec72` encoder scale and decoder**
  PROVEN: 216a0.c, 21540.c, 5b360.c, 5b5d0.c (clean tree). DISASM: the encoder's x87 sequence (`fld 1.0`, `fdiv` by the absolute major, `fld` and `fabs` of the minor, `fmul st(1)`, `fmul` by 262143.0, then the truncation helper), the writer's stack offsets, and the compression method table VA 0x675c5c.

- **T16. Section 15.3: `yaw8` write scale**
  DISASM: the `fpatan` result is multiplied by the f32 at VA 0x679e1c, then the truncation helper and `movsx eax, al` run before `write(.., 8)`.

- **T17. Section 15.5: rotation decoder (heading)**
  PROVEN (21dc0.c; 5bdd0.c calls the compression object's method +0x18 with the six bytes).

- **T18. Section 15.5: rotation decoder (rule)**
  PROVEN (clean tree 21dc0.c, 5f850.c).

- **T19. Section 18: V17 and V18 become confirmations**
  Editorial, follows from T14-T16.

- **T20. Section 19: updated open list**
  Editorial summary of T1-T19.

## Appendix O: reference stack notes (spec-fold-3, 2026-10-24)

Source: `UDP_REFERENCE_FINDINGS_botclient.md` items O1-O13 (the evidence record; runs and captures are cited there). Grade used here:
**REFERENCE-CONFIRMED** = the reference server stack (a second, independent writer or reader, NOT the 2009 server) behaves as the spec text
says when `dc-botclient` talks to it; it is black-box evidence under REFERENCE_TESTING.md section 1, distinct from PROVEN (2009 code or
captures). **Reference stack differs** = the reference stack does something else; the base text and its grade stand and the 2009 evidence decides (a capture
of the real 2009 client). No base grade was changed by this appendix and no reference stack value is a DeCombat rule value. No ROM names are quoted.

| item | spec place | note | state |
|---|---|---|---|
| O1 | 5.1 app id, 5.3 rate, T8, V3 | the first candidate layout (Windows GUID record) was accepted by the reference stack at the first ask, both bots, every run; the ask carried `rate_bps` 5120000 and the reference stack's host rate is 5120000. V3 stays open (the real client's ask has not been captured); DSV-5 (accept both layouts) stays | REFERENCE-CONFIRMED (layout, rate); V3 UNPROVEN |
| O3 | 10.2 stale cumulative ack, 11.1 heartbeat | the reference stack acks each client reliable block at once with a 4-byte ack-only datagram; its keep-alive is `[ack][rel: one zero-length entry]` about every 200 ms carrying a stale cumulative ack, which our link ignores as specified; our keep-alive has the same form and the reference stack acks it; selective-ack masks appear only when a block was lost ahead of a later one | REFERENCE-CONFIRMED |
| O4 | 11.2 resend timer, T11 | during a 125 ms stall of the bot the reference stack resent its three newest blocks four more times each about every 25 ms, as `RTO = clamp(L_ms, 17, 1000)` with `L_ms` about 0 on loopback predicts; shows why the client's separate receive thread matters (session A 4.6) | REFERENCE-CONFIRMED |
| O5 | 8.2 receiver, open item 19 (split messages) | the reference stack splits long reliable messages with `tail_cont` (12 continued blocks per client in every clean run, parts under the 8800-bit body budget of 11.5); the group reassembled in order and parsed with no record error | REFERENCE-CONFIRMED; item 19 split part still needs the real client |
| O8 | 8.1 no ack-due for duplicate blocks | **Reference stack differs (ours):** `dc-net-engine` acknowledges old / duplicate blocks (DSV-4, `ACK_DISCARDED_BLOCKS`), which the 2009 receiver does not (8.1). The reference stack tolerated every extra ack (12 for the 12 duplicates of O4, 82-127 per 60 s under loss). A capture comparison against the real client will show the difference | reference stack tolerance only |
| O11 | 9.1 bye datagrams | after our `C2S_BYE` the reference stack acks at once and then sends `CTL_BYE_CODE(5)` 4 times, 100 ms apart, byte for byte what `encode_bye_datagram(Some(5))` writes; the first copy came 18 ms or 320 ms after the ack | REFERENCE-CONFIRMED |

Loss runs (10 % and 20 % datagram loss each way, 60 s): both reliable channels stayed in order, no block abandoned, at most two outstanding
blocks, no idle drop (findings, loss table). That supports sections 8 and 11 as a pair of implementations; it says nothing about the 2009 client.
Count: REFERENCE-CONFIRMED 5 items (O1, O3, O4, O5, O11), reference stack differs 1 (O8, ours).

## Appendix C: capture notes (spec-fold-4, 2026-10-25)

Source: `CAPTURE_tdm1_TRANSPORT_VALIDATION.md` (evidence record, kept in full; `OBSERVED: captures/20261025_000014_tdm1/capture.jsonl`,
UDP connection 5 = the real 2009 client against the reference server through the capture relay; the tap clock has a resolution of
15.625 ms, so every interval is +-16 ms; zero loss; one player). Grades: PROVEN-CAPTURE for the client's bytes and behaviour,
REFERENCE / REFERENCE-CONFIRMED for the server's (see the header). Every item below is also applied into the text above.

| item | spec place | observation | grade |
|---|---|---|---|
| C-T1 | 5, V1, V2 | join ask 25 bytes `10 7f 33 01 ...`, opcode 2; reply 5 bytes, accepted; exactly one ask, answered in the same tick | ask PROVEN-CAPTURE, reply REFERENCE-CONFIRMED |
| C-T2 | 5.1, 5.3, V3 | app id = Windows GUID record at bits 35-162; `rate_bps` 5120000 at bits 163-194; DSV-5 decided: accept only the GUID record layout (a mismatch answers `accepted = 0, same_app = 0`); the big-endian form only behind an operator switch if wanted | PROVEN-CAPTURE |
| C-T3 | 5.2 | join retry: undecided (one ask) | |
| C-T4 | 6.1 | `check8` on every datagram of both directions | PROVEN-CAPTURE / REFERENCE |
| C-T5 | 6.2, 9 | framing, presence bits, no unreliable terminator, minimal `vlen` | PROVEN-CAPTURE |
| C-T6 | 11.1 | keep-alive 201.5 ms mean while idle (30 in the 6.1 s of session silence after the join), none while acks are recent | PROVEN-CAPTURE |
| C-T7 | 8 | client blocks 1..584, server blocks 1..5026, +1 per block; keep-alive blocks take numbers like any block; every client ack equals the newest server block already forwarded or lags it by 1-4 | PROVEN-CAPTURE / REFERENCE |
| C-T8 | 6.3, 10.2 | ack-only datagram per block in the same tick, no piggyback | PROVEN-CAPTURE |
| C-T9 | 11.3 | resend, RTO, out-of-order, `sack_mask`: undecided (no loss) | |
| C-T10 | 8.2 | three split messages (shell message 2,490 bits in 2 parts; object group 163,019 bits in 20 parts; object group 31,088 bits in 4 parts) reassembled; the 20-part group parsed with every record length equal to its bits read; the client answered with shell 0x87 47 ms later | REFERENCE-CONFIRMED |
| C-T11 | 9 | 2,338 client unreliable entries (one shell 0x64 each, 768-1,024 bits) and 7,641 server ones | PROVEN-CAPTURE / REFERENCE |
| C-T12 | 9.1 | bye behaviour of the client (see 9.1) | PROVEN-CAPTURE |
| C-T13 | 13, 5.4, 5.5 | no 90 s silence; no info or echo ask: undecided | |
| C-T14 | 15.1, V17 | the reference stack's `pos48` over the world extents matches (67 exact matches with the client's reported positions; moving the box by one unit gives none; a quarter unit gives 76, so the match fixes the box to within one cell); the client did not adopt the streamed position of its own object (its reports stayed constant while the stream carried positions up to about 40 cells away), which is what DSV-21 and CD-67 assume | REFERENCE (encoder), PROVEN-CAPTURE (client behaviour) |
| C-T15 | 15.2, 15.3, session B 10.3 | `vec72` scale 262143: both minor fields reproduced in 859 of 1,223 cases (`2^18` in 601; 262 only with 262143, 4 only with `2^18`); engine `yaw8` equals the truncated client yaw in 4,182 of 4,651 entries (the rest lag the turning); the shell yaw byte truncates toward zero, 2,338 of 2,338 (rounding never matches where it differs from truncation, 1,381 updates) | REFERENCE (`vec72`, `yaw8`), PROVEN-CAPTURE (shell byte) |
| C-T16 | 11.4-11.6, 12 | largest datagrams 269 bytes (c2s, the hello) and 1,081 bytes (s2c); largest 200 ms window 86,184 (c2s) and 190,272 (s2c) bits against 2W = 1,843,200 | PROVEN-CAPTURE / REFERENCE |
| C-T17 | 19 item 8 | first `C2S_RATE` = 640 (see 19 item 8) | PROVEN-CAPTURE |

Server-to-client start forms that the real client accepted (REFERENCE-CONFIRMED; each row is followed by a client message that could only come after
a successful handling): the join reply; block 1 (shell 0x90 `[1]`, shell 0x88 `[2]`, `S2C_VERSION`, `S2C_CLIENT_ID`, a 10-entry `S2C_FILE_LIST`
with the world as file id 0 and `file_info` non-zero) answered by `C2S_FILE_ACK` with all ten ids; 86 `S2C_EVAR` plus `S2C_LOAD_WORLD` answered by
`C2S_LOAD_STATE(0)` 219 ms later; the sky table with count 30 answered by `C2S_LOAD_STATE(1)`; id 8 plus id 10 per tick for 157 s with no client error
(29.58 id 10 messages per second of server time; 4,650 of 4,652 travel in the datagram of that tick's id 8 entry); `CTL_BYE_CODE(5)` x4 after `C2S_BYE`
(needed by nobody, the client had already left). The 2009 client's own `pos48` / `vec72` decoder needs a second player; the two-player capture
did not isolate it either (see `UDP_SESSION_2009_B.md` Appendix C).

Proposed DeCombat rule changes this capture supports (approval is the user's; `DC_DEFAULTS.md` is not edited here): DSV-5 resolve as in C-T2; BOT-5
may drop the big-endian fallback; BOT-7 keep the 1,000 ms wait but send the plain `CTL_BYE` and stop acknowledging after `C2S_BYE`, like the client;
CD-63 gets the data point that about 30 ticks per second are accepted on the wire (smoothness was not judged). Rows checked and consistent: CD-58, CD-59,
CD-60, CD-61, CD-66, DSV-3, DSV-21, CD-67, PD-S2C-1.

Count: PROVEN-CAPTURE items 13 (C-T1 ask, C-T2, C-T4, C-T5, C-T6, C-T7, C-T8, C-T11, C-T12, C-T14 client behaviour, C-T15 shell byte, C-T16, C-T17),
REFERENCE-CONFIRMED 2 (C-T1 reply, C-T10); undecided 3 items (C-T3, C-T9, C-T13) plus the `has_unrel` and `L_ms` / `L_eff` questions.

## Appendix F: fold record (spec-fold-5, 2026-10-25)

Source: `reports/client-reference stack.md` section 4 (the Rust client against the real client's PROVEN-CAPTURE transport forms; four runs against the reference stack). Grades as in the header;
RUN = a loopback run of our own client. No new 2009 evidence.

| item | place | finding | grade |
|---|---|---|---|
| ack-only, keep-alive, bye forms | 10, 11.1, 9.1 | a link written from this spec (`dc-net-engine`) sends ack-only datagrams of 4 bytes, keep-alives of 7 bytes and the plain bye `c9 03` x4 100 ms apart, as the real client (tdm1) | PROVEN-CAPTURE (real) + RUN (ours equal) |
| L8 silence after `C2S_BYE` | 9.1 | the real client sends nothing more on the link after its `C2S_BYE` (9.1, C-T12); our link still sent one ack and one empty reliable block 15 ms later | PROVEN-CAPTURE (real); a `dc-net-engine` difference, not a spec change |
| L10 in-world keep-alives | 11.1 | the real client sent no in-world keep-alive in tdm1 (its acks came more often than every 200 ms); ours sent 917 in one run because the reference server left 883 gaps over 200 ms between its reliable blocks in a two-player session, and 36 of them rode in the same datagram as an update (`ack + keepalive + unreliable`). The 11.1 rule ("a keep-alive when no ack went out for 200 ms") explains both; whether the 2009 client would put a keep-alive and an update into one datagram is not visible in any capture (it never needed one): UNPROVEN | PROVEN-CAPTURE (tdm1 count 0), RUN (ours), UNPROVEN (combination) |
| L9 own plain bye | 9.1 | the real client sends its plain bye 7.1-9.0 s after the host's bye code, when its screens return; ours at once (no screens) | PROVEN-CAPTURE (real); D (ours, CLI-24) |

## Appendix H: fold record (spec-fold-7, 2026-10-25)

Source: `reports/client-netbye.md` (with `netengine-bye.md` for the link mechanism). No new 2009 evidence; grades as in the header, RUN = a loopback
run of our own stack.

| item | place | finding | grade |
|---|---|---|---|
| L8 silence after `C2S_BYE` (Appendix F) | 9.1 | closed in the crates: the bot's link sends nothing after the `C2S_BYE` block but the four plain byes (two bots, one round); the server's keep-alives went unanswered | RUN (ours equal to the PROVEN-CAPTURE form); timing difference L9 kept (D, CLI-24) |

## Appendix I: fold record (spec-fold-8, 2026-10-25)

Sources: `UDP_SESSION_2009_B_addendum_round2.md` G6 (now a stub), `CAPTURE_tdm6_VALIDATION.md` 9, `CAPTURE_rust3_RUSTSTACK_VALIDATION.md` 10, the DSV-51 row.

| item | place | finding | grade |
|---|---|---|---|
| `C2S_BYE` timing | 9.1 | 15-16 ms after 0xbd also in tdm6 and rust3; the plain bye 9.8 s / 7.66 s later | PROVEN-CAPTURE |
| early server bye | 9.1 | our plain bye at the 0xbd before the client's `C2S_BYE` was accepted (rust3); DSV-51 waits for `C2S_BYE` | PROVEN-CAPTURE; D |
