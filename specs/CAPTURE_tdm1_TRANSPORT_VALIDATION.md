# capval-transport: tdm1 capture validation of the UDP transport and session specs (staging, addendum style)

- **Targets:** `specs/UDP_TRANSPORT_2009.md` (Appendix Z items T0-T20 already folded), `UDP_SESSION_2009_A.md`
  (A1-A20 folded), `UDP_SESSION_2009_B.md` (B1-B24 folded), `UDP_REFERENCE_FINDINGS_botclient.md` (O1-O13), and the
  DC_DEFAULTS rows they lean on.
- **Evidence:** `// OBSERVED: captures/20261025_000014_tdm1/capture.jsonl` (captap format 1).
  Connection 5 is the UDP game link of the **real 2009 client** (local port ephemeral) to the reference game server
  through the tool's relay: 7,940 c2s and 5,624 s2c datagrams, t = 133.312 s to 306.546 s. Lobby connection 4 (the
  same client) is used only to align two moments: the start push 0x3043 (t = 133.312, the same tick as the join ask)
  and the return from the result screen (t = 306.234). The bot (connections 2 / 3) never joined the game.
  "seq" below is the capture's event number; bit positions count from bit 0 of the datagram (transport section 2.1).
- **Grades:**
  - **PROVEN-CAPTURE**: bytes the real 2009 client sent, or a client behaviour visible in them (order, timing,
    content). This is 2009 evidence.
  - **REFERENCE**: bytes the reference server sent. They show what the reference stack does, not what a 2009 server did.
  - **REFERENCE-CONFIRMED**: an reference stack form whose acceptance by the 2009 client is shown by the client's next messages
    (a later session answer such as `C2S_FILE_ACK`, `C2S_LOAD_STATE`, shell 0x87, or the session going on for
    157 s without a client error or bye). A transport ack alone proves only transport acceptance.
- **Limits that every number here must be read with:**
  1. The tap's clock has a resolution of **15.625 ms** (every `t` is a multiple of 1/64 s; 64 distinct sub-second
     values in the file). Intervals are +-16 ms.
  2. Loopback, **zero loss**: no reliable block was resent in either direction, none arrived out of order, no
     selective-ack mask was sent. Resend, RTO, trickle, pause and out-of-order rules are not exercised.
  3. One player: the only player object is the client's own (id 0x198); no other client's stream.
  4. The server is the reference stack: every server-to-client value is REFERENCE at best.
- **Decoders (both scratch, outside the tree, nothing in `rust/` edited):** (a) a Python decoder written from
  transport sections 2, 5-10 and part B sections 3-4 (bit reader, `check8`, blocks, `vlen`, in-order reassembly with
  `tail_cont`, id 8 record reader, id 10 reader, the C2S shell 0x64 reader of `GAME_MESSAGES_2009_C2S.md` 2.1);
  (b) a Rust binary on our crates: `dc_wire::udp::{decode_link_datagram, encode_link_datagram, OobDatagram}`,
  `dc_botclient::objects::ObjectTable`, `dc_botclient::romworld::RomWorlds` (world box read from the user's ROM at run
  time, never written down here). Both agree on every count below.
- **ROM content:** none quoted. File, world and variable names are described by form only; the player's name and the
  hello's second string are not reproduced.

## 0. Summary

| item | spec | verdict | grade |
|---|---|---|---|
| V1 / V2 join ask and reply bits | transport 5, 18 | CONFIRMED | PROVEN-CAPTURE / REFERENCE-CONFIRMED |
| V3 app id layout, join rate | transport 5.1, 5.3 | CONFIRMED: Windows GUID record layout, `rate_bps = 5120000` (**decides DSV-5**) | PROVEN-CAPTURE |
| V4 join retry | transport 5.2 | UNDECIDED (one ask, answered in the same tick) | |
| V5 `check8` | transport 6.1 | CONFIRMED on 13,562 of 13,562 link datagrams | PROVEN-CAPTURE (c2s) / REFERENCE (s2c) |
| V6 framing, presence bits, no unreliable terminator, `vlen` | transport 6.2, 9, 10.1 | CONFIRMED; `dc-wire` re-encodes every datagram byte for byte | PROVEN-CAPTURE |
| V7 keep-alive period | transport 11.1 | CONFIRMED (201.5 ms mean on loopback; none while acks are recent) | PROVEN-CAPTURE |
| V8 sequence and cumulative ack | transport 8, 10.2 | CONFIRMED (1..584 c2s, 1..5026 s2c, no gap) | PROVEN-CAPTURE |
| client ack timing and form | transport 6.3, 10.2 | CONFIRMED + new text (ack-only datagram per block, same tick) | PROVEN-CAPTURE |
| V9 / V10 resend, out-of-order, RTO | transport 8.1, 11.3 | UNDECIDED (no loss) | |
| V11 split messages | transport 8.2 | CONFIRMED for the receiver rule (3 split messages, 26 parts) | REFERENCE-CONFIRMED |
| V12 unreliable block | transport 9 | CONFIRMED | PROVEN-CAPTURE |
| V13 bye | transport 9.1, A 5.6 | CONFIRMED form and repetition; DIFFERENT detail: plain `CTL_BYE` 8.7 s after the server's bye code, client silent meanwhile | PROVEN-CAPTURE |
| V14 idle drop | transport 13 | UNDECIDED | |
| V15 / V16 info probe, echo | transport 5.4, 5.5 | UNDECIDED (the client sent neither) | |
| V17 `pos48` box | transport 15.1 | CONFIRMED for the reference stack's encoder; client decoder UNDECIDED | REFERENCE |
| V18 scales | transport 15.2, 15.3, B 10.3 | `vec72` 262143 CONFIRMED (REFERENCE); engine `yaw8` truncation CONFIRMED (REFERENCE); **shell `yaw_c8` is truncated, not rounded: DIFFERENT** | PROVEN-CAPTURE for the shell |
| V19 / V20 sizes, flow | transport 11.4-11.6, 12 | CONFIRMED | PROVEN-CAPTURE / REFERENCE |
| open items 19.3, 19.8 | transport 19 | CLOSED | PROVEN-CAPTURE |
| V-A1 hello | A 2.3, 5.2 | CONFIRMED; second string observed as a 36-character GUID text | PROVEN-CAPTURE |
| V-A2 / V-A3 first server entries, gates | A 4, 5.7 | CONFIRMED; name form and `file_info` recorded | REFERENCE-CONFIRMED |
| V-A4 in-world test | A 4.11, 8.3 | CONFIRMED sufficient set (0x88 = 2, 0x67 = 1 directly, view object, create); 0x87 at t = 139.953 | PROVEN-CAPTURE (timing) |
| S1-S13 as the client sent them | A 9.2 | CONFIRMED with two new facts: 6.08 s session-level silence after the join, 219 ms world load | PROVEN-CAPTURE |
| V-A5 .. V-A9 | A 9.3 | UNDECIDED (need our own server) | |
| V-B1 / V-B2 / V-B4 / V-B5 | B 3-5, 9.1 | CONFIRMED (6,893 records, 4,652 streams, 0 errors) | REFERENCE-CONFIRMED |
| V-B3 create record | B 3.3 | DIFFERENT: `create_pos` is (0, 0, 0) in all 230 creates | REFERENCE-CONFIRMED (accepted) |
| V-B6 player update | B 10.3 | CONFIRMED rate 15/s, unreliable, layout parses exactly; yaw rounding DIFFERENT (see V18) | PROVEN-CAPTURE |
| V-B7 sounds | B 8.3, 10.2 | CONFIRMED tracked sounds and their acknowledgement in `C2S_RATE`; one-shot kind 0 UNDECIDED | PROVEN-CAPTURE / REFERENCE |
| V-B8 shell ids | B 7.1 | CONFIRMED except shell 0x9d (in no stage list; client showed no reaction) | REFERENCE |
| V-B9 / V-B10 | B 12.2, 4.1 | UNDECIDED | |
| O1-O13 (bot findings) | findings file | real client agrees with O1-O7, O9, O11-O13; O8 UNDECIDED; O10 shown accepted | see section 4 |

## 1. Transport (`UDP_TRANSPORT_2009.md`)

### C-T1. V1 / V2: join ask and join reply bits

- **Spec now (section 18, V1 / V2):** "first 4 bytes `10 7F 33 01`, opcode bits 32-34 = 2" and "25 bytes; reply 5 bytes
  with bit 35 = 1"; section 5: "Sizes: join ask 195 bits = 25 bytes; join reply 36 or 37 bits = 5 bytes".
- **Capture:** c2s seq 266, t = 133.312, 25 bytes, head `10 7f 33 01 5a 9e c1 12 43 de 09 25 8a c4 66 af 75 4e 5c 93 03 00
  71 02` (25th byte `00`): bits 0-31 = 0x01337F10, bits 32-34 = 2. s2c seq 267, same tick: `10 7f 33 01 0b`, bits
  32-34 = 3, bit 35 = 1 (accepted), bits 36-39 = 0. The ask is the client's first datagram on the port; there was
  exactly one ask (the reply came inside the same 15.6 ms tick). `dc_wire::udp::OobDatagram` decodes both and
  re-encodes both byte for byte.
- **Verdict:** CONFIRMED. Ask: PROVEN-CAPTURE. Reply: REFERENCE-CONFIRMED (the client's next datagram, seq 269, is a
  link datagram to the replying address).
- **Replacement text:** none needed; append to V1 / V2 "OBSERVED tdm1: pass (c2s seq 266, s2c seq 267)".

### C-T2. V3: application id layout and join rate (decides DSV-5)

- **Spec now (5.1):** "If the client holds it as a standard Windows GUID record, the 16 bytes are `CB 33 58 62 C8 3B
  A1 44 91 D8 EC B5 CE 89 6B 72`, starting at stream bit 35 (not byte aligned). The in-memory layout is UNPROVEN;
  confirm with V3 (section 18)."
- **Spec now (5.3):** "A real 2009 client therefore asks with `rate_bps = 5120000` (DISASM chain; that the console line
  updates the global through the variable table, and that the profile is loaded before the first join, are
  PLAUSIBLE; V3 confirms)."
- **Capture (seq 266):** bits 35-162 = `cb 33 58 62 c8 3b a1 44 91 d8 ec b5 ce 89 6b 72` (the GUID record form of the
  PROVEN id); bits 163-194 = 0x004E2000 = **5120000**; bits 195-199 = 0. The datagram is byte-identical to the bot's
  ask in findings O1.
- **Verdict:** CONFIRMED, PROVEN-CAPTURE. The big-endian candidate is never used by the 2009 client.
- **Replacement text (5.1, last two sentences):** "The 2009 client holds it as a Windows GUID record: the 16 bytes
  `CB 33 58 62 C8 3B A1 44 91 D8 EC B5 CE 89 6B 72` at stream bits 35-162 (not byte aligned). OBSERVED tdm1 (c2s seq
  266); a host compares exactly this layout."
- **Replacement text (5.3, last sentence):** "A real 2009 client asks with `rate_bps = 5120000` (OBSERVED tdm1, c2s seq
  266; the CShell console line of the DISASM chain is therefore in effect before the first join)."
- **Section 19 item 3:** closed by the same observation.

### C-T3. V4: join retry and timeout

- **Spec now (5.2):** "Send it immediately, then again whenever 300 ms have passed since the last send (`> 299`)."
- **Capture:** one ask, answered in the same tick; no retry could happen.
- **Verdict:** UNDECIDED. Needs the tap's drop injection on the first asks (no client restart needed beyond a normal
  match start).

### C-T4. V5: `check8`

- **Spec now (6.1):** "**on the wire `check8 == fold(CRC32(datagram[1..]))`**".
- **Capture:** 7,939 of 7,939 c2s link datagrams and 5,623 of 5,623 s2c link datagrams satisfy it (both decoders).
- **Verdict:** CONFIRMED. PROVEN-CAPTURE for the client's own datagrams; REFERENCE for the server's, and the client
  accepted every one of them (it acknowledged all 5,025 s2c reliable blocks that came before its bye).

### C-T5. V6: framing, presence bits, missing terminator, `vlen`

- **Spec now (6.2):** "The sender writes `has_ctl` and `has_rel` always (0 when absent) and writes `has_unrel` only
  when it has unreliable messages." **(9):** "writer engine+0xaf560 (writes no terminator; the block is last, and
  trailing pad bits decode as `L = 0`)".
- **Capture:**
  - every link datagram of both directions parses to its end; no entry overruns; the bits after the last field are
    0 to 6 bits, always zero;
  - `encode_link_datagram(decode_link_datagram(d)) == d` for 7,939 of 7,939 c2s and 5,623 of 5,623 s2c datagrams,
    `OobDatagram` likewise for both OOB datagrams. So `dc-wire`'s writer (presence bits as above, no unreliable
    terminator, minimal `vlen`) produces exactly the 2009 client's bytes, including the long `vlen` form (the
    2,088-bit hello entry);
  - 2,327 c2s datagrams carry only an unreliable entry and start with `[has_ctl 0][has_rel 0][has_unrel 1]` (byte 1 =
    0x04, e.g. head `05 04 b4 05 b2 10`), so `has_rel = 0` is written; 543 c2s reliable-only datagrams start with
    `has_ctl = 0`;
  - an explicit `vlen 0` after the last unreliable entry would make those datagrams one byte longer than the
    re-encoding; none is;
  - `has_unrel = 0` when absent cannot be told apart here: no datagram without unreliable entries ends on a byte
    boundary right before that bit.
- **Verdict:** CONFIRMED, PROVEN-CAPTURE (has_ctl / has_rel always written, no terminator, `vlen` encoder);
  `has_unrel` when absent: compatible, UNDECIDED in the strict sense.
- **Replacement text (append to 6.2):** "OBSERVED tdm1: the 2009 client writes `has_ctl` and `has_rel` as 0 when
  absent and ends an unreliable block without a terminator; `dc-wire` reproduces all 7,939 client datagrams bit for
  bit."

### C-T6. V7: keep-alive and heartbeat period

- **Spec now (11.1):** "So a quiet link still sends, every `max(200, min(L_ms, 1000))` ms, an ack plus an empty
  reliable block; the reliable block drives latency sampling."
- **Capture:**
  - From t = 133.4 to 139.4 the client's session layer was silent (C-A3). Its link sent 30 keep-alives, each 7 bytes
    `[ack][rel n][vlen 0][more 0]` (e.g. seq 272 `bd 21 00 28 00 00 00`), `rel_seq` 2..31, and 30 ack-only
    datagrams for the server's own keep-alives. Mean keep-alive interval **201.5 ms** (29 intervals, 187-204 ms at the
    tap resolution).
  - Each client keep-alive carries the ack value the client had already sent (no new server block in between) and
    leaves in the same tick as the server's keep-alive, before the client's ack of it (seq 272 ack 1, then seq 273
    ack 2): the client's timer runs from its own last ack, as in 11.1 (`now - last_ack_sent >= 200`).
  - In world (t = 139.95 .. 297.2) the client sent **no** keep-alive: it acknowledged the server's reliable blocks
    (about 31.6 per second) at once, so its last ack was never 200 ms old. The reference stack, which got few client blocks,
    kept sending keep-alives (342 in total, mostly 203 ms apart; REFERENCE).
- **Verdict:** CONFIRMED, PROVEN-CAPTURE, for loopback (L about 0). Whether the period uses `L_ms = trunc(rtt_mean)`
  or `L_eff` (DSV-4) is not decided: both readings give 200 ms here.
- **Replacement text (append to 11.1):** "OBSERVED tdm1: the 2009 client's keep-alive period on loopback is 200 ms
  (201.5 ms mean); it sends none while it acknowledges server blocks more often than every 200 ms."

### C-T7. V8: sequence numbers and cumulative acks

- **Spec now (8):** "`rel_seq` counts 1, 2, ... 8191, 0, 1 ... (13 bits, wraps). The first reliable block of a link
  carries 1." **(V8):** "first `rel_seq` = 1, +1 per new block, acks cumulative".
- **Capture:** c2s blocks 1..584, s2c blocks 1..5026, every step +1; keep-alive blocks take numbers like any other
  (c2s 2..31 were keep-alives, data resumed at 32). Every c2s ack equals the newest s2c block already forwarded by the
  tap (4,992) or lags it by 1-4 because a new block crossed it (63). No selective-ack mask in either direction. No
  wrap (largest 5026).
- **Verdict:** CONFIRMED, PROVEN-CAPTURE (client) / REFERENCE (server). The 13-bit wrap: UNDECIDED (not reached; at the reference stack's
  31.6 blocks per second the server side wraps after about 4.3 minutes in world).

### C-T8. The client's ack timing and ack datagram form (new text)

- **Spec now (6.3):** "After a valid link datagram the receive thread runs the link update at once (engine+0xb5d70 ->
  engine+0xb3e40), so an ack can leave immediately."
- **Capture:** of the 5,025 s2c reliable blocks acknowledged, the client's covering ack came in the same tick for
  5,019 and within 16 ms for all. 5,023 of these acks are 4-byte ack-only datagrams (`[check][1][CTL_ACK][ack_seq]
  [0][0]`, e.g. seq 273 `4c 41 00 00`). The client never piggybacks an ack on its reliable data: its 543 reliable-only
  datagrams carry none, the only reliable datagram with an ack is the hello (seq 269, ack 1), and 1 of 2,338
  unreliable datagrams carries one. Its player updates travel alone, one entry per datagram.
- **Verdict:** CONFIRMED, PROVEN-CAPTURE.
- **Replacement text (append to 6.3):** "OBSERVED tdm1: the 2009 client acknowledges every received reliable block
  within the same 16 ms in a separate 4-byte ack-only datagram; its reliable and unreliable data datagrams carry no
  ack. A server receives one ack-only datagram per reliable block it sends."

### C-T9. V9 / V10: resend, out-of-order store, selective acks, RTO

- **Spec now (11.3):** "`RTO = clamp(L_ms, 17, 1000)`" ... "the first resend comes after about `2 * RTO`, later ones
  every `RTO`".
- **Capture:** no duplicate `rel_seq` in either direction, no out-of-order arrival, no `sack_mask`.
- **Verdict:** UNDECIDED. Needs a run with drop / delay injection in the tap (the tap records and forwards unchanged
  today).

### C-T10. V11: split messages

- **Spec now (8.2):** "Sender: only a message longer than **255 bits** is split, and only when it would overflow the
  block budget (section 11.2)." Receiver: "(EVIDENCE: ... receiver engine+0xb2690 PLAUSIBLE, the C loses the
  destination of two copies)".
- **Capture:** 23 s2c blocks with `tail_cont = 1` carry 3 split messages: a shell message of 2,490 bits (2 parts,
  t = 139.906), an id 8 group of 163,019 bits (20 parts, middle parts 8,536 and 8,544 bits, completed in seq 448) and
  an id 8 group of 31,088 bits (4 parts). Reassembled by the 8.2 rule, each parses exactly (the 163,019-bit group:
  every record's `rec_bits` equals the bits read, 0 errors). The client acknowledged every part and sent shell 0x87
  47 ms after the 20-part group (C-A6): a wrong reassembly would have ended the session with 0x2c. The client split
  nothing itself (its largest entry, the 2,088-bit hello, fit one block).
- **Verdict:** receiver rule CONFIRMED, REFERENCE-CONFIRMED. The 2009 client's own sender split: UNDECIDED.
- **Replacement text (19, item 7):** "7. The split-message reassembly order (8.2) agrees with the reference stack's writer and
  the 2009 client accepted three split messages of 2, 20 and 4 parts (OBSERVED tdm1); the client's own split path is
  not exercised (its messages stayed under the block budget)."

### C-T11. V12: unreliable block

- **Capture:** c2s 2,338 unreliable entries in 2,338 datagrams (one shell 0x64 update each, 768-1,024 bits); s2c
  7,641 entries in 4,660 datagrams; all parse to the end of the datagram.
- **Verdict:** CONFIRMED, PROVEN-CAPTURE (c2s) / REFERENCE (s2c).

### C-T12. V13: bye datagrams and the client's leave

- **Spec now (9.1):** "body `[1][CTL_BYE:4]` or `[1][CTL_BYE_CODE:4][bye_code:u32]`, no presence bits for the other
  blocks. Sent **4 times**, 100 ms apart". **(V13):** "4 identical-body bye datagrams about 100 ms apart".
- **Capture:**
  - t = 297.218, seq 13820: the client sends shell 0xbd (`0b bd`, rel 583); t = 297.234, seq 13826: `C2S_BYE` `06`
    alone in rel 584 (datagram `c5 22 09 08 06 00`). Its last player update was seq 13818 and its last ack (5025)
    seq 13817.
  - Server (REFERENCE): ack 584 at once (seq 13828), its block 5026 (seq 13827), then `CTL_BYE_CODE(5)` four times,
    `0e a7 00 00 00 00`, at 297.265 / .359 / .453 / .562.
  - The client never acknowledged block 5026 and sent **nothing** for 8.69 s (no ack, no keep-alive, no update).
  - t = 306.250 .. 306.546, seq 13836-13839: the client sends the plain `CTL_BYE` (control op 1, no code) four times,
    `c9 03`, 93 / 94 / 109 ms apart at the tap resolution. Its lobby connection reports the return from the result
    screen 16 ms before the first copy (conn 4, t = 306.234).
- **Verdict:** CONFIRMED for the datagram form, the four copies and the 100 ms spacing (PROVEN-CAPTURE). New: the
  2009 client goes silent on the link right after `C2S_BYE` (it does not even acknowledge a block that arrives in the
  same tick) and sends its own plain `CTL_BYE` x4 only when the shell leaves the game screens.
- **Replacement text (append to 9.1):** "OBSERVED tdm1: after its `C2S_BYE` the 2009 client sends nothing more on the
  link (no ack of later server blocks, no keep-alive); when its shell leaves the result screen it sends `CTL_BYE`
  (op 1, no code) four times about 100 ms apart, here 8.7 s after the host's `CTL_BYE_CODE(5)` copies."

### C-T13. V14 idle drop, V15 info probe, V16 echo

- **Capture:** no silence of 90 s on the link; the client sent no `OOB_INFO_ASK` and no `OOB_ECHO_ASK` (one OOB
  datagram per direction in the whole session).
- **Verdict:** UNDECIDED (V14 needs the coordinator's blackhole run; V15 / V16 need crafted asks, no client needed).

### C-T14. V17: `pos48` over the world box

- **Spec now (15.1):** "**So the box is the world file's extents, `precise` is 1 and `e` is 0.**" **(V17):** "decoded
  values match 15.1 with the world file's extents and cell offset 0.5 (no fit); an edge position shows the wrap
  quirk".
- **Capture:** the client's own object (id 0x198, `S2C_VIEW_OBJECT` `07 98 01`, seq 406) is in every id 10 message
  (4,652 entries, `uf = 0xD`: `UF_POS | UF_YAW | UF_ANIM`) and in 1,560 reliable records. The client reports its own
  body position as raw floats in every shell 0x64 update (movement trailer, `GAME_MESSAGES_2009_C2S.md` 2.1). With
  the box read at run time from the world file the client loaded (`dc-botclient` `RomWorlds`, `CA_ROM`):
  - 67 stream positions equal, in all 48 bits, the 15.1 encoding (single-precision products, +0.5, shifts 3 / 2 / 3)
    of a position the client had sent 0-1 updates earlier;
  - moving the box by one unit gives 0 such matches; moving it by a quarter unit (below one cell, 0.27 / 0.5 / 0.29
    units) gives 76, so the match fixes the box to within a cell, not better;
  - the other entries differ from the client's report by up to about 40 cells, constant while the client stands
    still (for example 161.28-168.48 s: the client reports one position 103 times, the stream carries one position
    18 / -3 / -16 cells away 205 times): the reference stack moves its server object itself.
  - no position at the box edge (wrap quirk not seen).
- **Verdict:** CONFIRMED for the reference stack's encoder (box = world extents, REFERENCE). The 2009 client's `pos48` decoder:
  UNDECIDED (in a solo round nothing the client sends depends on it). One client behaviour is visible: during the
  standing runs the client did not move its body to the streamed position of its own object (its reports stayed
  constant, PROVEN-CAPTURE), which is what DSV-21 / CD-67 assume.
- **Replacement text (V17 row):** "pass condition as written; OBSERVED tdm1: the reference stack's stream positions of the
  client's own object equal the 15.1 encoding over the world extents (67 exact 48-bit matches with the client's
  reported positions); the client-side decode needs a second player (tdm2)."

### C-T15. V18: `vec72`, engine `yaw8` and the shell's yaw byte

- **Spec now (15.2):** "`q1 = trunc(abs(o1) * (1.0 / abs(major)) * 262143.0)`". **(15.3):** "`s8 = trunc(yaw *
  40.425354)` ... toward zero". **(B 10.3):** "`yaw_c8 = round(yaw(q) * 40.425354)` ... Note the shell rounds where
  the engine's `yaw8` writer truncates."
- **Capture, `vec72` (REFERENCE):** of the client's own object's 2,700 stream velocities, 1,223 have a major component
  equal (as f32) to the client's last reported velocity. For those, `q` computed with 262143 reproduces both minor
  fields exactly in 859 cases, `2^18` in 601; 262 match only with 262143, 4 only with `2^18` (both within 1.5 units).
  The major axis is the largest component in 2,700 of 2,700.
- **Capture, engine `yaw8` (REFERENCE):** the stream `yaw8` of the own object equals `trunc` of the client's last reported
  yaw x 40.425354 in 4,182 of 4,651 entries (54 match only rounding; the rest lag the client's turning).
- **Capture, shell yaw byte (PROVEN-CAPTURE):** in all 2,338 player updates the byte after `pu_flags` equals
  **`trunc(yaw(q) * 40.425354)`** of the body angles sent in the same message (`q` from Euler (0, yaw, 0)): 2,338 of
  2,338 match truncation toward zero; in the 1,381 messages where rounding and truncation differ, rounding never
  matches; negative angles truncate toward zero (895 cases where floor would differ also match truncation).
- **Verdict:** `vec72` encoder scale CONFIRMED (REFERENCE); engine `yaw8` writer CONFIRMED (REFERENCE); **B 10.3 DIFFERENT**:
  the 2009 shell truncates its yaw byte.
- **Replacement text (B 10.3 bullet 2):** "`yaw_c8 = trunc(yaw(q) * 40.425354)` (= 254 / 2pi, the `yaw8` scale,
  toward zero; OBSERVED tdm1, 2,338 of 2,338 updates) and `yaw_c16` (scale 5215.03; not sent in tdm1, its rounding is UNPROVEN, probably the same truncation),
  with `yaw(q) = atan2(2(z x + w y), 1 - 2(x^2 + y^2))`." Delete "Note the shell rounds where the engine's `yaw8`
  writer truncates." The same correction applies to `GAME_MESSAGES_2009_C2S.md` 2.1 (`yaw8c`); left to capval-game.

### C-T16. V19 / V20: datagram sizes and flow control

- **Capture:** largest c2s datagram 269 bytes (the hello), largest s2c 1,081 bytes (body 8,640 bits), none above
  1,101; largest bits per 200 ms window (datagram bits + 224 per datagram): c2s 86,184, s2c 190,272 (the initial
  object group), far below `2W` = 1,843,200 for 4,608,000 bps.
- **Verdict:** CONFIRMED (PROVEN-CAPTURE c2s, REFERENCE s2c).

### C-T17. Section 19 item 8: version message, then the first rate report

- **Spec now (19, item 8):** "the order "version message, then the first rate report" that fixes the client's final
  link rate (12.2) is PLAUSIBLE; a capture of the client's first `C2S_RATE` (`rate_8k = 640` expected, part A 5.5)
  confirms."
- **Capture:** `S2C_VERSION` arrived at 133.359 (seq 268); the client's first `C2S_RATE` is `07 80 02` (`rate_8k` =
  640) at 139.687 (seq 400), in the same reliable block right after `C2S_LOAD_STATE(0)` `09 00`.
- **Verdict:** CONFIRMED, PROVEN-CAPTURE. Item 8 closed: "OBSERVED tdm1: first `C2S_RATE` = 640, sent right after
  `C2S_LOAD_STATE(0)`, 6.3 s after `S2C_VERSION`."

## 2. Session start-up (`UDP_SESSION_2009_A.md`)

Timeline of the start-up as the tap saw it (t in s; R = reliable entry, U = unreliable; "rel" is the block's
`rel_seq`; every server entry is REFERENCE, every client entry PROVEN-CAPTURE):

| t | seq | dir | rel | entries (heads, at most 24 bytes; names omitted) |
|---|---|---|---|---|
| 133.312 | 266 / 267 | c2s / s2c | | `OOB_JOIN_ASK` / `OOB_JOIN_REPLY` accepted (C-T1) |
| 133.359 | 268 | s2c | 1 | `0d 90 01` (shell 0x90, 17 bits), `0d 88 02`, `04 07 00 00 00 00 20 4e 00`, `0c 00 00 00`, `32 ...` (10 entries, 2,704 bits) |
| 133.359 | 269 | c2s | 1 | `C2S_HELLO` `05 02 01` + 258 blob bytes (2,088 bits); the datagram also carries `CTL_ACK 1` |
| 133.56 .. 139.41 | 271 .. 391 | both | 2 .. 31 | keep-alives and acks only (C-T6) |
| 139.437 | 392 | c2s | 32 | `C2S_FILE_ACK` `36 00 80 09 80 03 80 04 80 05 80 08 80 06 80 07 80 02 80 01 80` |
| 139.468 | 394 / 395 | s2c | 32 / 33 | 86 x `S2C_EVAR`, then `S2C_LOAD_WORLD` `06 00 00 00 00 00 00` |
| 139.687 | 400 | c2s | 33 | `C2S_LOAD_STATE(0)` `09 00`, `C2S_RATE` `07 80 02` |
| 139.734 | 402 | s2c | 35 | `13 00`, `13 02 04 00`, `13 01`, `13 05 13 01`, `S2C_SKY_TABLE` (888 bits: 48-byte block, count 30, 30 ids) |
| 139.734 | 404 | c2s | 34 | `C2S_LOAD_STATE(1)` `09 01` |
| 139.812 | 406 | s2c | 36 | 10 single-entry `S2C_FILE_LIST`, `S2C_EVAR`s, shell 0xa6 x3, shell 0x72 `0d 72`, 0x9e, 0x9d, 0x7d, `S2C_VIEW_OBJECT` `07 98 01` |
| 139.812 | 408 | c2s | 35 | 10 single-entry `C2S_FILE_ACK` (`36 0a 80` .. `36 13 80`), shell 0x72 (643 bits), shell 0x85 `0b 85 02 01` |
| 139.906 | 410 | s2c | 37 | shells 0x6b, 0x6a, 0xa6, two file lists, **`0d 67 01`**, 0x66 x4, 0xb2 / 0x7f pairs, 0xa0 x4, 0x43, 0x44, 0xb5, ... |
| 139.906 | 411 .. 448 | s2c | 38 .. 57 | shell 0x45 (2,490 bits, split), then the 20-part id 8 group (163,019 bits) with the create of object 0x198 |
| 139.906 | 437 / 441 | c2s | 36 / 37 | `C2S_FILE_ACK` `36 14 80`, `36 15 80` |
| 139.953 | 457 | c2s | 38 | shell 0x76 / 0x7f x4, two file acks, shell 0x64 reliable form `0b 64 10 00 01`, **shell 0x87 `0b 87`**; the same datagram carries the first unreliable 0x64 update |
| 140.000 | 461 .. 472 | s2c | 59 .. 65 | `0d 90 00`, `0d 88 03`, id 8 group (31,088 bits, 4 parts), shells, `S2C_DIR_LIGHT` (id 22, 232 bits) |

### C-A1. S1 / V-A1: `C2S_HELLO` and the hello blob

- **Spec now (V-A1):** "id 5, `blob_len = 258`, UTF-16 name at blob 0x82 equals the 8-bit name at 0x00".
  **(2.3, row 0x40):** "8-bit string from another shell object (global VA 0x3759b9d8, std::string at +0x5c), at most
  63 bytes + NUL | PROVEN copy; meaning UNPROVEN". **(5.2):** "It is usually the **first reliable message on the
  link** (rel_seq 1 from the client), sent before the client has seen anything from the server."
- **Capture (seq 269, rel 1, 47 ms after the join reply):** entry of 2,088 bits = `05`, `02 01` (258), 258 bytes:
  - 0x00: 8-bit name, 5 ASCII characters + NUL, zero padding to 0x3f;
  - 0x40: a **36-character upper-case GUID text** (8-4-4-4-12 hex digits) + NUL, zero padding. It appears in no TCP
    frame of the capture and in no client file of the deployed client folder (byte search, read-only, 8-bit and
    UTF-16; only the folder's copies of the reference stack's logs contain it, as the reference stack prints what it received);
  - 0x80, 0x81: 0;
  - 0x82: the same name in UTF-16, NUL-terminated; every other byte 0;
  - the UTF-16 name equals the login name the client sent in auth 0x200 (conn 1) and the name the lobby echoes, so
    matching by it (CD-60) works for the real client.
  - The datagram already acknowledges the server's block 1 (seq 268, same tick): the transport had received the
    server's first block, the session layer had not processed it (its first answer came 6.08 s later, C-A3).
- **Verdict:** V-A1 CONFIRMED, PROVEN-CAPTURE. The 5.2 sentence is DIFFERENT in detail.
- **Replacement text (2.3, row 0x40, grade column):** "PROVEN copy; OBSERVED tdm1: a 36-character upper-case GUID text
  (8-4-4-4-12 hex digits) that the lobby traffic does not carry; meaning and lifetime UNPROVEN (tdm2 compares two
  sessions)".
- **Replacement text (5.2, bullet 1, last sentence):** "It is the **first reliable message on the link** (rel_seq 1
  from the client), built before the client's session layer has handled anything from the server; its datagram may
  already acknowledge the server's first block (OBSERVED tdm1: `CTL_ACK 1` in the hello datagram)."

### C-A2. S2-S4 / V-A2: first server entries (REFERENCE) and what the client accepted

- **Spec now (V-A2):** "ids 4 (value 7), 12 (`local_host = 0`), 0x32; record the world's announced name form (closes
  10.1 #3) and the rate value".
- **Capture (seq 268, block 1, REFERENCE):** shell 0x90 (`0d 90 01`, a 1-bit body), shell 0x88 state 2, `S2C_VERSION`
  (7, host rate 5120000), `S2C_CLIENT_ID` (client id 0, `local_host` 0), `S2C_FILE_LIST` with 10 entries, the world
  first with file id **0**. Name forms (described, not quoted): the world is "mixed-case directory, backslash,
  upper-case base name, lower-case `.dat`"; the other 31 files of the session are upper case with backslashes
  (extensions LTB 13, DTX 11, WAV 5, SPR 2). `file_info` is non-zero in all 32 entries; for the world it equals the
  size in bytes of the world file that `dc-rez` reads from the user's ROM (value not quoted).
- **Client reaction:** `C2S_FILE_ACK` with all 10 ids (C-A4), `C2S_LOAD_STATE(0)` 219 ms after `S2C_LOAD_WORLD` (the
  world name resolved: no 0x29), no 0x4E (version accepted).
- **Verdict:** CONFIRMED, REFERENCE-CONFIRMED: the client accepts a world file id 0, this mixed-case name form, a 7 / 5120000
  version message and client id 0. 10.1 #3: the reference stack's spelling is recorded; the 2009 server's own spelling stays
  unknown (not needed: the rule of 4.4 is what the client accepted). 10.1 #2: the reference stack's file ids start at 0.
- **Replacement text (append to 4.4, "Rule for our server" bullet):** "OBSERVED tdm1: the client resolved the world
  announced as mixed-case directory + backslash + upper-case base + lower-case `.dat`, file id 0, `file_info` = the
  file size."

### C-A3. The client's session-level silence after the join (new)

- **Spec now (9.2, notes):** "S2, S3 and S4 may be sent in any order and before S1 arrives; the 2009 host sends them on
  link creation (8.2)." **(7.1):** "No session-level timeout was found for the start-up sequence itself".
- **Capture:** the server's block 1 (S2-S4) arrived at 133.359. The client's next session message, `C2S_FILE_ACK`,
  left at 139.437: **6.078 s** later. In between its link sent only 30 keep-alives and 30 ack-only datagrams (C-T6).
  The world load itself was then fast: `S2C_LOAD_WORLD` at 139.468, `C2S_LOAD_STATE(0)` at 139.687 (**219 ms**).
  The cause of the 6 s is not visible on the wire (UNPROVEN; candidates: the loading screen's own set-up, or loading
  done before the session messages are dispatched).
- **Verdict:** new PROVEN-CAPTURE fact.
- **Replacement text (append to 9.2 notes):** "- OBSERVED tdm1: the 2009 client handled no session message for 6.1 s
  after its join (its first `C2S_FILE_ACK` came 6.08 s after S4) while its transport kept the 200 ms keep-alive; the
  world load after S6 took 0.22 s. A server must not treat several seconds without session answers after the join
  as a failure (CD-61's 120 s bound covers it)."

### C-A4. S4a / V-A3: `C2S_FILE_ACK` and the host gates

- **Spec now (5.4):** "One message per `S2C_FILE_LIST`, one `u16` per announced entry, in the order received
  (EVIDENCE: engine+0x3e990, PROVEN). The 2009 client always sets bit 15 (section 4.4)." **(V-A3):** "0x36 answers
  every 0x32 entry with bit 15 set; the server's id 6 comes after the client's 5 and 0x36; `srv_clock` value recorded".
- **Capture:** 23 `S2C_FILE_LIST` (1 batched with 10 entries, 22 single), 23 `C2S_FILE_ACK`, one per list, entries
  in list order, every entry `id | 0x8000` (the batched answer: `36 00 80 09 80 03 80 04 80 05 80 08 80 06 80 07 80 02
  80 01 80`). Single-entry lists were answered in the same tick (20 of 22) or 47 ms later (2). The reference stack sent `S2C_LOAD_WORLD` 31 ms after the
  batched ack and after the hello (gate of 5.7, REFERENCE); `srv_clock` = 0.0.
- **Verdict:** CONFIRMED, PROVEN-CAPTURE (client) / REFERENCE (gate).

### C-A5. S6a, S8a, `C2S_RATE`: load states and the rate report

- **Spec now (5.3):** "`state = 1`: sent when the client receives `[19][1]` ... It does not depend on the earlier
  preload lists: the client answers `[19][1]` whenever it arrives." **(5.5):** "a real client is expected to send
  `rate_8k = 640`, PLAUSIBLE until V3". **(9.2 note):** "The client sends `C2S_RATE` once its world is loaded (5.5):
  `07 80 02` (640)".
- **Capture:** `09 00` and `07 80 02` in one block (seq 400, 139.687); the reference stack sent one `13 01` (seq 402) and the
  client answered with exactly one `09 01` (seq 404, same tick). Later `C2S_RATE` messages: `07 80 02 9e 01`
  (209.484) and `07 80 02 9d 01` (224.968), each carrying one tracked-sound id (C-B6).
- **Verdict:** CONFIRMED, PROVEN-CAPTURE. 5.5 "PLAUSIBLE until V3" is now OBSERVED.
- **Replacement text (5.5, sentence 1):** "`rate_8k = client_rate_bps / 8000`, clamped to 0..65535; the client's rate is
  the global at VA 0x6cb298 (PE-DATA initial 35840 -> `rate_8k = 4`; the 2009 CShell sets it to 5120000 at profile
  load, transport 5.3; OBSERVED tdm1: the real client sends `rate_8k = 640`, first right after `C2S_LOAD_STATE(0)` in
  the same block)."

### C-A6. S9-S12 / V-A4: the in-world test and the 0x87 moment

- **Spec now (8.3):** "All four conditions must hold" (a: view object resolves; b: switching-worlds state 2 or 3 from
  shell 0x88; c: entered-world flag after `S2C_LOAD_WORLD`; d: player state non-zero from shell 0x67). **(V-A4):**
  "record the shell messages between the client's `[9][1]` and the end of the loading screen (0x88 value; confirm that
  the player-state message for 8.3 d is shell 0x67 `[state:u8]` ...)".
- **Capture:** (b) `0d 88 02` arrived at 133.359 in block 1, before `S2C_VERSION`; (c) world loaded by 139.687;
  (a) `07 98 01` at 139.812, **before** the create of 0x198, which completed with the 20-part group at seq 448
  (139.906); (d) `0d 67 01` at 139.906 (seq 410), with no 0x67 state 3 before it. The client's shell 0x87 left at
  **139.953** (seq 457), 47 ms after the last of these, together with the reliable 0x64 state form `0b 64 10 00 01`
  and the first unreliable 0x64 update. No 0x64 of either form was sent before 0x67 arrived. The reference stack sent
  `0d 88 03` after 0x87 (140.000).
- **Verdict:** CONFIRMED as a sufficient set, PROVEN-CAPTURE for the moment and order. That each condition is
  necessary is not shown (needs our own server, V-A9). 4.9 "Order is free: the id may arrive before or after the
  create record" is CONFIRMED (id first here). The 0x67 state 1 "directly" case of findings O9 holds for the real
  client.
- **Replacement text (8.3, after the table):** "OBSERVED tdm1: with 0x88 state 2 (sent at link creation), the
  view object id before the create record, and 0x67 state 1 without an earlier state, the 2009 client sent shell 0x87
  47 ms after the last of them and started its 0x64 updates in the same frame."

### C-A7. V-A5 .. V-A9 and the remaining A items

- V-A5 (stuck start-up), V-A6 (kick reasons), V-A7 (version 8), V-A8 (missing world), V-A9 (our slice): UNDECIDED; they
  need our own server in front of the real client.
- 6.2 "Kicks by the server": the reference stack's `CTL_BYE_CODE(5)` arrived after the client's own `C2S_BYE`, so it shows
  nothing about which box a code selects. UNDECIDED.
- 4.5 / 10.1 #7 (`S2C_EVAR` required?): the reference stack sent 86 before S6 and 29 later (REFERENCE); not decidable which the
  client needs. UNDECIDED.
- 4.8 / 10.1 #6 (sky block): the reference stack sent count 30 with a 48-byte block (REFERENCE-CONFIRMED that count 30 is
  accepted); block layout UNDECIDED.

## 3. In-world session (`UDP_SESSION_2009_B.md`)

Counts (both decoders agree; `dc-botclient` `ObjectTable` reports 0 would-be-fatal errors): s2c reliable ids 4 x1,
6 x1, 7 x1, 8 x4,652, 12 x1, 13 x283, 15 x115, 16 x1, 19 x4, 22 x1, 0x32 x23; s2c unreliable ids 10 x4,652, 13 x2,849,
17 x140; c2s reliable ids 5 x1, 6 x1, 7 x3, 9 x2, 11 x836, 0x36 x23; c2s unreliable id 11 (shell 0x64) x2,338. Not
seen: s2c 5, 14, 20, 21, 23; c2s 8, 10.

### C-B1. V-B1 / V-B2: `S2C_OBJ_GROUP` framing and the change-mask head

- **Spec now (V-B1):** "entries start with 8; records parse as `[u32 len][record]` to the end". **(V-B2):** "`cf` low
  byte bit 7 iff a second byte follows; no record with `cf = 0`". **(3.1):** "the position must equal the remembered
  position plus `rec_bits`; a mismatch returns 0x2c."
- **Capture (REFERENCE):** 4,652 id 8 messages (one per server tick), 6,893 records: 230 creates, 6,627 changes, 19
  tracked-sound records, 17 removal records. Every record's `rec_bits` equals the bits its reader consumed (B 3.3,
  4.2, 4.4, 3.4, 8.3, 5.1 readers). The second `cf` byte is present in 1,528 records and absent in 5,365, always
  exactly when `cf & 0x8F00 != 0`; no object record has `cf = 0`; special kinds seen: 1 and 3 only.
- **Client reaction:** none of these would end the session, and the session went on for 157 s: REFERENCE-CONFIRMED.
- **Verdict:** CONFIRMED.

### C-B2. V-B3: the create record (`create_pos` is zero)

- **Spec now (3.3):** "`[create_pos: 3 x f32]               x, y, z (raw floats, always present)`". **(V-B3):** "kind
  byte 1, model block counts within caps, `create_pos` equals the first `pos48` within one cell".
- **Capture (REFERENCE):** `create_pos` is **(0.0, 0.0, 0.0) in all 230 create records** (kinds 0 x22, 1 x5, 2 x33, 3
  x110, 9 x60). The position always arrives in the forced `CF_POS` change part: `pos48` + `vec72` for 197 creates,
  raw floats for the 33 full-precision ones (`flags & 0x0100`; kinds 2 and 0). The 5 kind-1 creates respect the
  caps (model count 1-2, sub-models <= 2, textures <= 5, styles <= 3). The client's own object 0x198: kind byte 0xC1
  (kind 1, blob present, 16-bit blob length), a 580-bit blob, 2 models, create flags word with 0x0800 (yaw only) set
  and 0x0100 clear.
- **Client reaction:** accepted (no 0x2c; the own object resolved for the in-world test, C-A6).
- **Verdict:** DIFFERENT from the V-B3 pass condition on the reference stack; REFERENCE-CONFIRMED that a zero `create_pos` with
  the position in `CF_POS` is accepted. What the 2009 client does with a non-zero `create_pos` is not shown.
- **Replacement text (3.3, the `create_pos` line comment and V-B3):** "`[create_pos: 3 x f32]` x, y, z (raw floats,
  always present; OBSERVED tdm1: the reference stack writes 0, 0, 0 in every create and the client accepts it, since the create
  always carries `CF_POS`)"; V-B3 pass condition: "kind byte 1, model block counts within caps; `create_pos` either 0
  or equal to the first position within one cell".

### C-B3. V-B4: `S2C_OBJ_STREAM` and the time entry

- **Spec now (V-B4):** "starts with 10, entries parse to the end, the last entry is `[0xFFFF][0][f32]`, the f32 never
  decreases". **(9.1):** "the server sends one id 10 message to every in-world client on every server tick".
- **Capture (REFERENCE):** 4,652 id 10 messages, every one parses to its last bit, the time entry is last in all, its value
  never decreases (2 repeats), first 0.0 at 139.906, last 157.248 at 297.234: **29.58 messages per second of server
  time** (29.57 per wall second). 4,579 messages hold the own object + the time entry, 73 also one other object.
  4,650 of the 4,652 travel in the same datagram as that tick's id 8 entry (B 12.2 rule 4).
- **Verdict:** CONFIRMED, REFERENCE-CONFIRMED (accepted for 157 s). The rate is the reference stack's (CD-63 is DeCombat's).

### C-B4. The client's own object in the stream (DSV-21, CD-67)

- **Spec now (4.2, writer-side suppression):** "The client's own view object ... gets no `CF_ROT` / `CF_ROT_SNAP`
  unless the client has option bit 0x08". DSV-21: "the client's own object is never streamed to it".
- **Capture (REFERENCE):** the reference stack streams 0x198 to its own client in every tick (4,652 entries, `UF_POS | UF_YAW |
  UF_ANIM`) and sends 1,293 reliable records with a rotation for it (`cf` 0x0480 etc.).
- **Client reaction (PROVEN-CAPTURE):** while it stood still the client kept reporting its own unchanged position
  although the stream carried a position up to 40 cells away (C-T14): it did not adopt the streamed position for its
  body.
- **Verdict:** the reference stack does what B 4.2 says the 2009 writer avoids, and the client tolerates it. Neither DSV-21
  nor CD-67 is contradicted; their premise "the client owns its body" is supported.

### C-B5. V-B5: removals and id reuse

- **Capture (REFERENCE):** 17 kind 3 records (37 ids), each the last record of its message; the removal list covers
  objects and tracked sounds alike; ids are reused after removal (tracked sound 0x19d: removed at 169.812, started
  again at 170.359).
- **Verdict:** CONFIRMED, REFERENCE-CONFIRMED (accepted).

### C-B6. V-B7: tracked sounds and their acknowledgement in `C2S_RATE`

- **Spec now (10.2):** "Sent only when `rate_8k` differs from the last value sent, or when the client has
  acknowledgements pending" ... "PLAUSIBLE meaning: "these tracked sounds have finished on this client"". **(V-B7):**
  "kind 0 / kind 1 records; `dmask = 7` on first use of a sound file, smaller later".
- **Capture:** 19 tracked-sound records (kind 1, `tmask` 3), `dmask` 7 on the first use of a sound file, 0, 4 or 5
  on later uses (REFERENCE). Two of them were not removed by the server before they ended: tracked sound 0x19e started
  at 208.812, the client sent `07 80 02 9e 01` at 209.484 (672 ms later); 0x19d started at 224.296, the client sent
  `07 80 02 9d 01` at 224.968 (672 ms later); the reference stack removed each 31-47 ms after the client's message. The sounds
  the server removed earlier were never acknowledged, nor were the 11 tracked sounds started at 139.906 (other
  flag words, never removed; PLAUSIBLY looping). No one-shot (kind 0) record in the whole session.
- **Verdict:** CONFIRMED for the client's acknowledgement rule (PROVEN-CAPTURE: `C2S_RATE` re-sent with an unchanged
  rate when a tracked sound finished, one id per finished sound); kind 1 delta cache REFERENCE-CONFIRMED; kind 0
  UNDECIDED.
- **Replacement text (10.2, last bullet, final sentence):** "Meaning: "these tracked sounds have finished on this
  client" (OBSERVED tdm1: `[7][640][id]` sent about 0.67 s after the start of a tracked sound that the server had not
  removed; sounds removed earlier are not acknowledged)."

### C-B7. V-B6: the client's player update (shell 0x64)

- **Spec now (10.3):** "For a remote server (kind 3) the update is therefore limited to `R` per second" with `R`
  "defaults to **15.0** when the variable is missing"; the unreliable layout of 10.3 / `GAME_MESSAGES_2009_C2S.md`
  2.1; "a separate **reliable** form `[0x64][pu_flags:u16 = 0x0010][state:u8]` is sent first (flags 1)".
- **Capture (PROVEN-CAPTURE):** 2,338 updates, all unreliable, 139.953 .. 297.218: **14.87 per second**, the time
  field steps by 0.0670 s (median, minimum 0.06699): `R = 15`. Each parses exactly to its end with the 10.3 layout
  (field widths), sizes 768-1,024 bits; `pu_flags` always contains 0x0021 and 0x1000, plus 0x0100 in 1,472, 0x0400 in
  1,302, 0x0200 in 491, 0x2000 in 95 updates; 0x0040 / 0x0080 never. The reliable form `0b 64 10 00 01` was sent once,
  right before 0x87. One gap of 0.44 s (161.546-161.984); no stop while the player state stayed 1 (the reference stack sent
  0x67 only once). The yaw byte is truncated (C-T15).
- **Verdict:** CONFIRMED (rate, block, reliable form, layout widths); DIFFERENT for the yaw byte rounding. Field
  meanings belong to capval-game.

### C-B8. V-B8 and the shell-message channel (findings O10)

- **Spec now (7.1 / 6.2 step 5):** "An id nobody handles is dropped silently." **(12.1):** "S -> C | 13 from game code,
  17, 20, 21, 14 | UNPROVEN (writers not decompiled)".
- **Capture (REFERENCE):** s2c shell ids 0x43, 0x44, 0x45, 0x66, 0x67, 0x68, 0x6a, 0x6b, 0x72, 0x74, 0x7d, 0x7f, 0x88, 0x90,
  0x9d, 0x9e, 0xa0, 0xa2, 0xa6, 0xb2, 0xb5, 0xca. All are in table 7.1 except **0x9d** (`0d 9d 00 00 00 00`, 48 bits,
  once at 139.812), which no stage lists; the client showed no reaction (consistent with silent drop). Reliable: 283
  shell messages; unreliable: 0xa2 x52 and 0xa6 x2,797. `S2C_SHELL_OBJ_MSG` (id 17) x140, all unreliable, first bytes
  0x06 x83, 0x19 x50, 0x10 x3, 0x65 x4 (0x65 is not in the 7.2 effect list).
- **Verdict:** CONFIRMED (REFERENCE); the client tolerates game messages and id 17 on the unreliable channel
  (REFERENCE-CONFIRMED in the sense that the session went on; whether the client acted on each is not visible).
- **Replacement text (12.1, row "S -> C | 13 from game code, 17, 20, 21, 14"):** "UNPROVEN for the 2009 server; OBSERVED
  tdm1 (reference stack): shell messages on both blocks (0xa2 and most 0xa6 unreliable), id 17 unreliable; the client accepts
  both".

### C-B9. V-B9, V-B10 and other B items

- V-B9 (delayed reliable datagrams) and V-B10 (change record for an unknown id): UNDECIDED (need delay injection and our
  own server).
- 8.4 `S2C_DIR_LIGHT` once at 140.000 (232 bits = id + 7 floats; REFERENCE-CONFIRMED accepted); 8.5 sky table once (C-A7).
- 9.4 console: no `C2S_CONSOLE` and no shell 0x00 text in the session.

## 4. `UDP_REFERENCE_FINDINGS_botclient.md`: does the real client behave like the bot?

| item | bot / reference stack finding | real 2009 client in tdm1 | verdict |
|---|---|---|---|
| O1 | GUID-record join ask accepted, rate 5120000 | identical 25-byte ask (C-T2) | CONFIRMED, PROVEN-CAPTURE: the bot's layout is the client's |
| O2 | start-up order S1 .. 0x87 | same order and gates (section 2 timeline). Differences: the client is silent for 6.08 s before S4a (bot: at once), loads the world in 0.22 s (bot: 1,000 ms by BOT-6), answers each later single-entry file list with its own `C2S_FILE_ACK`, sends shell 0x76 / 0x7f pairs and the reliable 0x64 state form before 0x87, and its 0x85 body is `02 01` (bot `00 00`); its 0x72 answer has the bot's length (643 bits) | CONFIRMED; differences are game layer (capval-game) or timing |
| O3 | ack per block at once, keep-alive about 200 ms | 4-byte ack-only datagram per block in the same tick; 201.5 ms keep-alive when idle (C-T6, C-T8) | CONFIRMED, PROVEN-CAPTURE |
| O4 | a stalled receive loop makes the server resend at frame rate | no resend at all: the client kept acknowledging through its 6 s session silence and its world load | CONFIRMED (A 4.6 "link acks keep flowing" PROVEN-CAPTURE) |
| O5 | split groups reassemble | 3 split messages accepted (C-T10) | CONFIRMED, REFERENCE-CONFIRMED |
| O6 | 0x90 / 0x88 before `S2C_VERSION` | same block 1; the client used the 0x88 state 2 for its in-world test (C-A6) | CONFIRMED, REFERENCE-CONFIRMED |
| O7 | one `[19][1]` | one `[19][1]`, one `09 01` (C-A5) | CONFIRMED: a server may send `[19][1]` once |
| O8 | our acks of duplicate blocks (DSV-4) | no duplicate block reached the client, so nothing to compare | UNDECIDED |
| O9 | 0x67 state 1 directly | accepted: 0x87 47 ms later, 0x64 updates start (C-A6) | CONFIRMED, PROVEN-CAPTURE |
| O10 | game messages on the unreliable block | same reference stack behaviour; the client went on (C-B8) | REFERENCE-CONFIRMED (tolerated) |
| O11 | bye code 5 x4 after `C2S_BYE` | same reference stack bytes; the client then went silent and sent plain `CTL_BYE` x4 8.7 s later (C-T12) | CONFIRMED; new client detail |
| O12 | about 29.7 stream ticks per second | 29.58 per second (C-B3) | CONFIRMED (REFERENCE) |
| O13 | 0x0100 aim block accepted | the real client sets 0x0100 in 1,472 of 2,338 updates and always sets 0x0001 and 0x1000, which the bot does not (C-B7) | CONFIRMED that the block exists in real traffic; bot differences go to capval-game |

## 5. Server-to-client start forms the real client accepted (REFERENCE-CONFIRMED)

Each row is an reference stack form followed by a client message that could only come after a successful handling:

| reference stack form | client reaction that proves acceptance |
|---|---|
| `OOB_JOIN_REPLY` `10 7f 33 01 0b` | link datagrams to that address (seq 269 on) |
| block 1: `0d 90 01`, `0d 88 02`, `04 07 00 00 00 00 20 4e 00`, `0c 00 00 00`, `S2C_FILE_LIST` (10 entries, world id 0, sizes) | `C2S_FILE_ACK` with all ten ids (no 0x4E drop) |
| 86 `S2C_EVAR`, `06 00 00 00 00 00 00` | `C2S_LOAD_STATE(0)` 219 ms later (world found and loaded, no 0x29 / 0x2A / 0x2B / 1) |
| `13 00`, `13 02 04 00`, `13 01`, `13 05 13 01`, sky table count 30 | `C2S_LOAD_STATE(1)` (no result 1 from the sky count) |
| 10 + 12 single-entry file lists | one `C2S_FILE_ACK` each |
| shell 0x72 identify request | shell 0x72 answer (643 bits) |
| `07 98 01`, the 20-part id 8 group with the create of 0x198, `0d 67 01` | shell 0x87 and the start of the 0x64 stream |
| id 8 + id 10 per tick for 157 s, id 22, id 17, unreliable shell messages | no client error, bye only on the user's exit |
| `CTL_BYE_CODE(5)` x4 after `C2S_BYE` | none needed (the client had already left) |

## 6. Proposed DC_DEFAULTS changes

Rows whose proposal or premise the capture contradicts or decides (approval is the user's; nothing is changed in
`DC_DEFAULTS.md` by this file):

| row | now | capture | proposal |
|---|---|---|---|
| DSV-5 | "accept both candidate in-memory layouts ... until V3 fixes the layout; then keep only the proven one" | V3 decided: the 2009 client sends the Windows GUID record layout (C-T2) | **resolve**: accept only the GUID record layout; a mismatch answers `accepted = 0, same_app = 0`. Keep the big-endian form only behind an operator switch if wanted |
| BOT-5 | GUID record first, big-endian after a 0x52 | the first layout is the client's | drop the big-endian fallback (or keep it, harmless); cite OBSERVED tdm1 |
| BOT-8 | hello second string = the bot's account id text | the real client sends a 36-character upper-case GUID text that the lobby never sees (C-A1) | send a GUID-form text (for example one derived from the account number) so servers see the real form; meaning stays UNPROVEN |
| DC-C2S-10 | "before 0x87: 0x67 state 3 and 0x88 state 2" | the real client passed its in-world test with 0x88 state 2 and 0x67 state 1 directly, no state 3 (C-A6) | keep as a compatibility default, but note "state 3 first is not required by the 2009 client (OBSERVED tdm1)" |
| BOT-6 | 1,000 ms simulated world load | real client: 219 ms load, but 6.08 s session silence after the join (C-A3) | optional bot mode that holds the first server block for about 6 s, so servers are tested against the real start-up gap; load time 250 ms |
| BOT-7 | after `C2S_BYE` wait <= 1,000 ms for the server's close, then send the transport bye x4 | the real client stops all link traffic right after `C2S_BYE` (no acks) and sends plain `CTL_BYE` x4 only when its shell leaves the result screen (8.7 s later) (C-T12) | keep the 1,000 ms wait as the bot's choice; make sure the bot's bye is the plain form (op 1) and that it stops acknowledging after `C2S_BYE`, like the client |
| DSV-4 | ack old / duplicate blocks; "Re-evaluate after V7" | V7 passed on loopback but cannot tell `L_ms` from `L_eff` (L about 0) and no duplicate reached the client (O8) | no change; re-evaluate with a delay / loss injection run |
| CD-63 | 10 ticks per second until live tests | the real client stayed in world for 157 s against a 29.6-per-second stream from the reference stack (C-B3) | data point for the planned test: 30 per second is accepted on the wire; smoothness was not judged |

Rows checked and consistent (no change): CD-58 (host rate 5120000 = the reference stack's value; the client did not drop),
CD-59 (`file_info` = size: the reference stack sends the size; accepted), CD-60 (the UTF-16 hello name equals the login name),
CD-61 (start-up took 6.6 s from join to 0x87), CD-66 / DSV-3 (ids reused after removal; the world file id 0 and object
ids from 0x9b up were accepted), DSV-21 / CD-67 (the client did not follow the streamed position of its own object),
PD-S2C-1 (the reference stack mixes channels and the client tolerates it; reliable-only stays a valid DeCombat choice).

## 7. Items that still need the two-player capture (tdm2: user + Bot1, reference server)

1. **V17 / V18 client side:** the real client's decoding of another object's `pos48`, `vec72` and `yaw8` (the bot's
   object): compare the bot's own reported positions with what the real client aims at or reports hitting (shell hit
   messages), and with its view of the bot's yaw.
2. **Remote player object:** the create record of another player's object (kind 1 with blob), its stream entries,
   its removal when the bot leaves (V-B5 for a player), and the client's reaction (no 0x2c, the bot visible).
3. **V-B7 kind 0:** one-shot sound records (the bot firing near the user); `dmask` on first and later use.
4. **V-B8:** two-player game events (kill, death of the user by the bot, scores) and their shell ids.
5. **Hello second string (C-A1):** the same machine in a second session: same GUID text (per install) or a new one
   (per session or per process).
6. **Start-up silence (C-A3):** whether the 6 s gap after the join repeats (systematic) and whether it depends on the
   loading screen.
7. **Bye path:** whether the client again goes silent after `C2S_BYE` and sends plain `CTL_BYE` x4 only on leaving the
   result screen; and the case where the server ends the match first (bye code or level exit) while the client is in
   world.
8. **Stream rate:** the same 29.6-per-second stream with a moving remote player, for the CD-63 smoothness judgement.

Not tdm2 (these need fault injection in the tap or our own server; listed for the coordinator): V4 join retry; V9 /
V10 resend, out-of-order, selective acks, RTO, trickle and pause; O8 / DSV-4 duplicate acks; V14 idle drop; V15 / V16
crafted info / echo asks; V-A5 .. V-A9 and V-B9 / V-B10 negative tests; 13-bit sequence wrap (about 4.3 minutes in
world on the server side; a longer tdm2 round would show it).

## 8. What could not be decoded or decided

- The **meaning and origin of the hello's second string** (a GUID text): not in the lobby traffic, not in the client
  folder's files.
- The **cause of the 6.08 s** session-level silence after the join.
- Shell 0x90's 1-bit body, shell 0x9d's 32-bit body (zero), the 48-byte sky block, and `S2C_SHELL_OBJ_MSG` first byte
  0x65 (not in the 7.2 list): carried, not interpreted.
- Whether `has_unrel = 0` is written when absent (no datagram distinguishes it).
- All loss-dependent transport behaviour (resend timing, RTO, `sack_mask`, out-of-order store, trickle, pause) and the
  `L_ms` / `L_eff` question: no loss, loopback.
- The 2009 client's own `pos48` / `vec72` decoding: in a solo round nothing it sends depends on it.
- The exact rounding of the reference stack's `pos48` encoder (round versus floor differ by about 2 % of matches); irrelevant
  for 2009 evidence.
- Field meanings of shell 0x64's trailer and of the other game messages: capval-game's scope.

## 9. Notes for the spec owner (how to apply)

- Apply C-T2, C-T5, C-T6, C-T8, C-T10, C-T12, C-T14, C-T15, C-T17 to `UDP_TRANSPORT_2009.md` (sections 5.1, 5.3, 6.2,
  6.3, 9.1, 11.1, 18 V17, 19 items 3 / 7 / 8) and C-T15's yaw correction to `UDP_SESSION_2009_B.md` 10.3 (and, through
  capval-game, `GAME_MESSAGES_2009_C2S.md` 2.1).
- Apply C-A1 .. C-A6 to `UDP_SESSION_2009_A.md` (2.3, 4.4, 5.2, 5.5, 8.3, 9.2 notes); C-B2, C-B6, C-B8 to part B (3.3,
  10.2, 12.1, V-B3).
- Grades: an item marked PROVEN-CAPTURE may be cited as `// OBSERVED: captures/20261025_000014_tdm1/capture.jsonl`
  with the seq numbers given; REFERENCE items must not become rules for our server beyond "the client accepts this form".
