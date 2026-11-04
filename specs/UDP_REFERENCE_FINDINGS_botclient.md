# botclient reference stack findings: `dc-botclient` against the reference lobby and its game server (staging, addendum style)

- **Targets:** `specs/UDP_TRANSPORT_2009.md`, `UDP_SESSION_2009_A.md`, `UDP_SESSION_2009_B.md` (each with its
  `*_addendum_disasm.md`, which overrides the base text), `GAME_MESSAGES_2009_S2C.md`, `GAME_MESSAGES_2009_C2S.md`.
- **Evidence class:** OBSERVED, black box (REFERENCE_TESTING.md §1): wire bytes between our bot and the reference stack on
  loopback. The reference lobby was only run (never read); its game server is the reference stack, **not** the 2009
  server, so every item below is evidence about the reference stack. Where it agrees with a spec item derived from the 2009
  Engine.exe host code, the item gains a second, independent confirmation; where it differs, the spec text stands
  until a capture of the real 2009 client decides (REFERENCE_TESTING.md §1: the reference stack never decides rule values).
- **Runs added after the first draft:** `20261024_233658` (60 s clean, player updates with the aim block, O13).
- **Captures** (captap format 1, merged from the two bots, `blocks` = decoded link blocks; never commit them):
  `captures/botclient_reference_<ts>.jsonl`; run folders (bot reports, event logs, the lobby log
  with the game server's console) `botclient-tests/<ts>/`. Runs cited below:
  - `20261024_232007`: 60 s, two bots, clean loopback (the reference run);
  - `20261024_230809`: 60 s, clean, with a 125 ms receive-loop stall in the bot (since fixed in the bot);
  - `20261024_232131`: 60 s, 10 % datagram loss each way on the bot side (same seed for both bots);
  - `20261024_232839`: 60 s, 20 % loss each way, per-bot seeds;
  - `20261024_232423`: 20 s, clean, records the server's bye copies.
- **ROM content:** none quoted. File names, world names and asset names that the session carries are replaced by
  `<name>`; the game server's advertised LAN address is not reproduced.

- **Folded 2026-10-24 (spec-fold-3):** each item is noted, as REFERENCE-CONFIRMED or "Reference stack differs", in Appendix O of `UDP_TRANSPORT_2009.md` (O1, O3, O4, O5, O8, O11), `UDP_SESSION_2009_A.md` (O1, O2, O6, O7, O9, O11), `UDP_SESSION_2009_B.md` (O2, O5, O12, O13) and `GAME_MESSAGES_2009_S2C.md` (O2, O6, O9, O10). This file stays the evidence record; base grades are unchanged.

Summary: **no mismatch between our transport (`dc-wire` + `dc-net-engine`) and the reference stack's that breaks or slows the
session was found.** Every server datagram passed `check8`, no reliable or unreliable block was abandoned, every split
message reassembled, both reliable channels stayed in order for 60 s clean and under 10 % and 20 % loss, and the
reference stack accepted the bot's join ask, hello, file acks, load states, rate report, player info 0x72, client options 0x85,
world ready 0x87 and player update 0x64 (with and without the `quat128` aim block). Items O1-O5, O11 and O13 confirm
spec text; O6-O10 and O12 are differences, our own deviations or open points.

## O1. Join: application id layout and rate (transport §5.1, §5.3, addendum T8; V3)

The first candidate layout (Windows GUID record, `dc_wire::udp::TODO_GAME_APP_ID_GUID_LAYOUT`) was accepted at the
first ask, every run, both bots:

```
c2s OOB_JOIN_ASK   (25 bytes) 10 7f 33 01 5a 9e c1 12 43 de 09 25 8a c4 66 af 75 4e 5c 93 03 00 71 02 00
s2c OOB_JOIN_REPLY ( 5 bytes) 10 7f 33 01 0b                      (opcode 3, accepted = 1)
```

The ask carries `rate_bps = 5120000` (T8's value for a real client). The reference stack's host rate in `S2C_VERSION` is
5120000 (`04 07 00 00 00 00 20 4e 00`, O2) and its console reports a bandwidth target of 5120000.
**Proposed spec note:** V3 has an reference stack confirmation of the GUID-record layout; the real-client capture is still
the deciding evidence. `DSV-5` (accept both layouts) can stay until then.

## O2. Start-up messages and gates (session A §5.7, §8.2, §9.2 S2-S11; addendum A7-A18)

Entries in arrival order (reliable unless marked; `bits` include the id byte):

| step | entry bytes (head) | bits | note |
|---|---|---|---|
| S1 c2s | `05 02 01` + 258 blob bytes | 2088 | first reliable entry (`rel_seq` 1) |
| s2c | `0d 90 01` | 17 | shell 0x90, 1-bit body = 1 (see O6) |
| s2c | `0d 88 02` | 24 | shell 0x88 state 2 **before** S2 (see O6) |
| S2 s2c | `04 07 00 00 00 00 20 4e 00` | 72 | version 7, rate 5120000 |
| S3 s2c | `0c 00 00 00` / `0c 01 00 00` | 32 | client ids 0 and 1, `local_host` 0 |
| S4 s2c | `32 00 00` `<size:u32>` `<name>` `00` ... | 1992 | one batched list, world first (id 0) |
| S4a c2s | `36 00 80 06 80 03 80 04 80 ...` | 136 | every entry with bit 15 |
| S5 s2c | `0f <name> 00 <value> 00` x 108 | | one variable per message, all before S6 |
| S6 s2c | `06 00 00 00 00 00 00` | 56 | `srv_clock` 0.0, world file id 0 |
| S6a c2s | `09 00` | 16 | after the (simulated) load |
| A14 c2s | `07 80 02` | 24 | `rate_8k` 640 |
| S7 s2c | `13 00`, `13 02 04 00`, `13 01`, `13 05 13 01` | | one `13 01` only (see O7) |
| S8a c2s | `09 01` | 16 | |
| S8 s2c | `10` + 48 bytes + `1e 00` + 30 ids | 888 | sky table, count 30 |
| s2c | `0d 72` | 16 | identify request (S2C §5.1), right before the view object |
| c2s | `0b 72 01 00 00 00 ea 03 00 00 06 00 ...` | 643 | `UP_PLAYER_INFO` (C2S §5.1), accepted |
| c2s | `0b 85 00 00` | 25 | `UP_CLIENT_OPTIONS` |
| S10 s2c | `07 ff 00` / `07 fc 00` | 24 | view objects 255 and 252 |
| S9 s2c | id 8 records | | the own object's create came in a split group (O5) |
| S12 s2c | `0d 67 01` | 24 | player state 1 directly (see O9) |
| c2s | `0b 87` | 16 | `UP_WORLD_READY` once the in-world test passed |
| s2c | `0d 88 03` | 24 | switching-worlds state 3 after 0x87 |

The S2C ids first seen, in order: shell 0x90, shell 0x88, 4, 12, 0x32, 15, 6, 19, 16, shell 0xa6, shell 0x72, shell 0x9e,
shell 0x9d, shell 0x7d, 7, shell 0x6b, shell 0x6a, shell 0x67, shell 0x66, shell 0xb2, shell 0x7f, shell 0xa0, shell 0x43,
shell 0x44, shell 0xb5, shell 0x45, 8, 10, 22, shell 0xa2. Counts over the 60 s reference run (bot 1): id 4 x1, 6 x1, 7
x1, 8 x748, 10 x1738, 12 x1, 13 x211, 15 x108, 16 x1, 19 x4, 22 x1, 0x32 x15; shell ids 0x43, 0x44, 0x45, 0x67, 0x6a, 0x72,
0x7d, 0x9d, 0x9e, 0xb5 x1, 0x88 x2, 0x90 x2, 0x6b x4, 0xa0 x6, 0x7f x6, 0xb2 x8, 0x66 x13, 0xa2 x18, 0xa6 x142.
**Confirms:** the host gates of A §5.7 (S6 only after S1 and S4a; preload only after S6a; the player object only after
S8a), `S2C_FILE_LIST` batched first and single-entry later (A §4.4: 1 + 14 lists), `S2C_EVAR` list right before
`S2C_LOAD_WORLD` (A §4.5), sky count 30 (A §4.8), `srv_clock` 0.0 at world start (addendum A10), C2S_RATE 640 (A14).

## O3. Transport rhythm (transport §10.2, §11.1, addenda T9-T11)

- The reference stack acknowledges each client reliable block at once with a 4-byte ack-only datagram, for example
  `ed 61 01 00` = `[check][1][CTL_ACK][ack_seq 11][sack 0]`.
- Its keep-alive is `[ack][rel: one zero-length entry]` every ~200 ms (`3.0680`, `3.2693`, `3.4802` ... in run
  `232007`), carrying a stale cumulative ack of our newest block (`ack 2` repeated while we sent nothing newer). That is
  the §10.2 "stale cumulative ack" case; our link ignores it as specified.
- Our keep-alive has the same form (7 bytes, `[ack][rel n][0-length entry]`) every ~200 ms; the reference stack acks it at once.
- The reference stack sent a selective-ack mask only when one of our few reliable blocks was lost ahead of a later one (2 masks
  in the 20 % run, none otherwise); our link sent 2 / 0 (clean), 33 / 38 (10 %) and 69 / 74 (20 %) selective masks,
  and the reference stack resent only the gaps they left (no burst of needless resends seen).
**Confirms** transport §11.1 (heartbeat `max(200, L_ms)` with `L_ms ~ 0` on loopback) on both sides.

## O4. Resend timer (transport §11.2, addendum T11) and the client's receive thread (session A §4.6)

Run `230809`: the bot's loop stalled 125 ms while it parsed the world (a bot defect, fixed: the world is now parsed on
a worker thread). During the stall the reference stack resent its three newest blocks four more times each, about every 25 ms:

```
2.8961 s2c rel 2 (1063 bytes)  2.8968 s2c rel 3 (1070)  2.8970 s2c rel 4 (12: 06 00 00 00 00 00 00 = S6)
3.0215 c2s ack 2, ack 3, ack 4                                    (the bot is back)
3.0219 .. 3.0252 s2c rel 2, 3, 4 x4 (queued in the socket meanwhile)
3.0409 .. 3.0413 c2s ack 4 x12                                     (see O8)
```

That matches `RTO = clamp(L_ms, 17, 1000)` with `L_ms = trunc(rtt_mean) ~ 0` on loopback (plus the reference stack's frame
pacing). It also shows why the 2009 client's separate receive thread matters (A §4.6: "link acks keep flowing"): a
client that stops acknowledging during a world load gets its start-up blocks resent at frame rate. After the fix
(runs `231243` onward) the clean runs show zero duplicated blocks in either direction.

## O5. Split reliable messages (transport §8.2 receiver, open item 19.7)

The reference stack splits long reliable messages with `tail_cont`: in every clean run 12 continued blocks per client, for
example blocks 13-25 at `4.141`-`4.208` (run `232007`): block 14 ends with a 3761-bit part, blocks 15-20 carry one
8536-bit part each, 21-23 one 8544-bit part each, block 25 starts with the 8256-bit tail. The whole group (the
initial object group with about 80 create records) reassembled in order and parsed with no record error. **Confirms**
our reading of §8.2 (the last entry of a `tail_cont` block continues as the first entry of the next block) against a
second, independent writer. Part sizes stay under the 8800-bit body budget (§11.5).

## O6. Shell 0x90 and 0x88 before `S2C_VERSION` (session A §8.2, §9.2 S11; GAME_MESSAGES_2009_S2C.md §7.2)

At link creation the reference stack queues shell 0x90 (`0d 90 01`, a 1-bit body) and shell 0x88 state 2 (`0d 88 02`) ahead of
`S2C_VERSION` in the first reliable block, and sends both again to all clients after its world starts. Spec A §8.2
lists `S2C_VERSION`, `S2C_CLIENT_ID`, `S2C_FILE_LIST` as the host's new-link messages and 0x88 as step S11. The client
accepts shell messages in every linked state (A §8.1), so the order is harmless; it means in-world test condition (b)
holds from the first block on. **Proposed spec note:** record that 0x88 may arrive before S2 (reference stack behaviour, game
layer); the meaning of shell 0x90's single bit is UNPROVEN.

## O7. One `S2C_PRELOAD` sub 1 (session A §4.7)

The reference stack sends `13 00`, `13 02 04 00` (one model id), `13 01`, then `13 05 13 01` (sub 5, one id) and **no second
`13 01`**. Spec A §4.7 says the 2009 host sends `[19][1]` twice and the client answers `C2S_LOAD_STATE(1)` twice; the
bot (like the 2009 client) answers each `[19][1]`, here once. **Proposed spec note:** "the host acts on the first
`C2S_LOAD_STATE(1)`; a server may send `[19][1]` once" (no client-side effect either way).

## O8. Our acks of duplicate blocks (DSV-4 deviation, transport §8.1)

`dc-net-engine` acknowledges old / duplicate blocks (`ACK_DISCARDED_BLOCKS`, DSV-4); the 2009 receiver sets no
ack-due for them (§8.1). On the wire this showed as 12 ack-only datagrams `ack 4` for the 12 duplicates of O4, and
82-127 extra acks per 60 s under loss. The reference stack tolerated every one (no reaction, no extra resend). Not a mismatch
that matters; noted because a capture comparison against the real client will show this difference.

## O9. Player state and spawn set (S2C §4.1, C2S Appendix A A4-A6; DC-C2S-10 proposed)

The reference stack sends `0d 67 01` (state 1) directly, without a state 3 first, then the mover update twice (`0d a0 0d 00 ...`
body + containers + friction, then `0d a0 20 00 01 <pos:3 x f32> <dims:3 x f32>` teleport with seq 1 and again with
seq 2 at the same position), facing `0d 6a 00 <pitch> <yaw> <roll>`, weapons 0xb2 / 0x7f, status 0x66. The bot echoed
the teleport's `move_seq` and the 0x6a yaw in its updates. Game layer, not transport; recorded because DC-C2S-10
proposes 3 then 1 (both pass the client's test).

## O10. Game messages on the unreliable channel (S2C §1.1, PD-S2C-1 proposed)

The reference stack sends some shell messages in the **unreliable** block: shell 0xa2 (`0d a2 01 00 0a 00 00 00 0a 00 ff ff`,
ping table, every ~3 s) and part of the shell 0xa6 traffic (heads `0d a6 18 ...`). PD-S2C-1 proposes "every server ->
client game message reliable"; the client does not show the server's choice (S2C §1.1). Informational: a DeCombat
server may keep PD-S2C-1.

## O11. The bye exchange (session A §5.6, transport §9.1)

After our `C2S_BYE` (engine message `06`, 8 bits, in the reliable block `rel_seq` 11) the reference stack acknowledges at once and then
sends `CTL_BYE_CODE(5)` four times, 100 ms apart (run `232423`):

```
22.7864 c2s rel 11 [06]            22.7865 s2c ack 11
22.8053 s2c 0e a7 00 00 00 00      22.9059 s2c 0e a7 00 00 00 00
23.0060 s2c 0e a7 00 00 00 00      23.1065 s2c 0e a7 00 00 00 00
```

`0e a7 00 00 00 00` = `[check][1][CTL_BYE_CODE][code 5]`, byte for byte what `dc_wire::udp::encode_bye_datagram(Some(5))`
writes. The first copy came 18 ms (one bot) and 320 ms (the other) after the ack: the reference stack drops the client at a
later frame. **Confirms** A §5.6 (bye code 5) and transport §9.1 (4 copies, 100 ms) on the reference stack. The bot now waits up
to 1000 ms for this bye before it sends its own (BOT-7 proposed); with the 10 % loss run one bot missed all copies and
closed by itself after the wait, as designed.

## O12. Server tick and time entries (session B §9.1, addendum B14; CD-63)

The reference stack sends one `S2C_OBJ_STREAM` per tick at about 29.7 per second (1738-1746 time entries over 58.6 s of server
time, time entry `[ff ff][0:4][f32]` last in each message) and an object group about 12.7 times per second. Once per
session a time entry repeats the previous value (the client keeps the larger one, B §9.1). The 2009 rate stays open
(B14); DeCombat's proposal is 60 per second.

## O13. Player update with the `quat128` aim block (C2S §2.1, addendum B16)

Run `20261024_233658` (60 s, clean): the bots add `pu_flags` 0x0100 with `[aim_q:quat128]` after the aux words whenever
the view rotation changed (BOT-9 proposed). The reference stack's console reports the walker's updates as 792-bit shell
messages (shell id + 784 = 16 + 8 + 96 + 192 + 128 + 32 + 312) and the stander's as 792 bits once, then 664; both
forms were accepted and the walker's object moved on the other bot's screen (stream travel 6306 units in 58 s).
Head of a walker update: `0b 64 20 01 <yaw8c> <pitch:f32> <yaw:f32> <roll:f32> <aux: 6 x u32> <q: 4 x f32> <control:u32> ...`.
**Confirms** that a server built from C2S §2.1 parses the `0x0100` block as 128 raw bits in that position (reference stack
evidence; which client condition sets it stays UNPROVEN).

## Loss runs (in-sync check of both reliable channels)

| run | loss each way | bot | dropped out / in | our resends | server duplicates seen | our selective acks | in order at end | outstanding at end | steps missing |
|---|---|---|---|---|---|---|---|---|---|
| `232007` | 0 | 1 / 2 | 0 / 0 | 0 / 0 | 0 / 0 | 2 / 0 | 972 / 959 | 0 / 0 | none |
| `232131` | 10 % (same seed) | 1 / 2 | 181 / 237, 184 / 229 | 0 / 0 | 82 / 85 | 33 / 38 | 872 / 856 | 0 / 0 | none |
| `232839` | 20 % (own seeds) | 1 / 2 | 322 / 432, 346 / 434 | 4 / 2 | 127 / 111 | 69 / 74 | 740 / 737 | 0 / 0 | none |

No trickle mode, no pause, at most two outstanding blocks, no idle drop; the walking bot's object moved for the other
bot over the whole minute (stream travel 6295 units clean, 6147 at 10 %, 5662 at 20 %; the standing bot 0).

## Open (for the coordinator)

- A capture of the **real 2009 client** against the reference stack (or against `dc-dserver`) is still needed to turn O1, O6, O7
  and O10 into 2009 evidence; the bot's results are reference stack evidence only.
- The meaning of shell 0x90's 1-bit body and of shell 0x9d (`0d 9d 00 00 00 00`, 48 bits incl. ids) is not in the S2C
  catalogue's stage lists as read by the bot.
