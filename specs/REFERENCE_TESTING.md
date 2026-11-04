# REFERENCE_TESTING

Scope: how the DeCombat Rust servers (and later client) are tested against two black boxes, the
reference server stack and the real 2009 client, using only observed wire behaviour. Covers the tool
`evidence/captap.py`, the capture and allowlist formats, the roles and the gates. Does not cover server rule
values or any wire layout (those live in PROTOCOL_2009, UDP_TRANSPORT_2009 and the DeCombat defaults).
Evidence sources: `evidence/captap.py` and `evidence/test_captap.py` (original harness code),
`captures/README.md`, `CLEANROOM.md` section 2 (observed behaviour row),
`specs/PROTOCOL_2009.md` section 1 (framing).
Status: DRAFT

## 1. Principle

Two things are treated as sealed boxes: the reference stack (auth, lobby, clan, game) under `evidence/`
and the 2009 client. For reference stack testing we never read inside the first and we never learn rules from it. We only
watch bytes on loopback.

| item | allowed use | forbidden use |
|---|---|---|
| captures (`captures/*.jsonl`) | evidence for wire format and client behaviour: which frames the client sends in which order, which replies it accepts. Cite as `// OBSERVED: captures/<file>` | learning server rule values (rewards, odds, limits, prices, item choices) |
| the reference stack as a replay target | checking that a capture is self-consistent; baseline for diffs | copying its logic or data into Rust |
| the real client | driving captures during a user-approved run | any automated launch without the user's GO |

Server rule VALUES are never taken from the reference stack. Where a reply carries a value the reference stack chose, the Rust server
uses the documented DeCombat default (CD-nn rows) and the allowlist marks that field as varying. A capture can prove
that the client accepts a layout; it cannot prove that a number is right.

## 2. Roles

- Spec authors write from decompiler evidence, spec files and captures. Implementers write Rust from the spec only.
  They are different agents (specs/README.md, two-person rule). An implementer never edits the allowlist to make a
  test pass; an allowlist change goes through the spec author with a spec section reference.
- An auditor (never an implementer) may compare the reference stack's behaviour with the spec and file discrepancies
  back to the author.

## 3. Tool

`python evidence/captap.py {tap|replay|shadow} --help`. Python 3.12, standard library only; unit tests:
`python -m unittest test_captap` in the evidence archive (synthetic loopback sockets, no game, no DServer). Formats are in
`captures/README.md` (capture format 1, allowlist version 1). TCP traffic is cut into frames
with the 8-byte frame header (i32 id, i32 body length; PROTOCOL_2009.md section 1.2); UDP datagrams are whole
records keyed by their first N bytes (UNPROVEN what those bytes mean; set with `--udp-key-bytes`).

| mode | purpose | how |
|---|---|---|
| `tap` | record a real session | transparent proxy and relay between client and the reference stack, JSONL capture |
| `replay` | regression test without a client | acts as the client against a server under test and expects the recorded server frames |
| `shadow` | compare during real play | forwards client traffic to primary and shadow; the client sees only the primary |

Limits that every report must be read with:
- Replay sends recorded client bytes unchanged. If a client frame embeds a value the server issued earlier (for
  example a session key echoed back), a server that issues a different value receives a stale one. The scenario
  then fails honestly, or passes where the server does not validate it. The fix is a deterministic test mode in
  the server for issued values, never a rewrite inside the tool.
- Frame matching is by key and arrival order, so a reordering shows as extra plus missing.
- In shadow mode the client's frames are reactions to the primary's replies. If the shadow's state diverges, its
  replies are judged against traffic it did not cause. Treat shadow diffs as leads and replay as the gate.
- A pass proves byte equality on the recorded scenario only.

## 4. Workflow

1. Capture (needs the user's GO; procedure in captures/README.md section 4). One file per scenario.
2. Prove the capture by replaying it against the reference stack itself:
   `python evidence/captap.py replay captures/RUN.jsonl --target lobby=127.0.0.1:PORT --report diff.json`.
   Every field that differs run to run goes into the allowlist with a spec reference or a DeCombat default id.
3. Replay against the Rust server in CI with no game window, loopback only. The test starts the Rust server on
   ephemeral ports and stops it afterwards; a non-zero exit fails the build:
   `python evidence/captap.py replay captures/RUN.jsonl --target lobby=127.0.0.1:PORT --allow captures/allow.json`.
4. During user play sessions (with GO) run `shadow` with the Rust server as the shadow:
   `python evidence/captap.py shadow --tcp lobby=27002:HOST:PORT --shadow lobby=127.0.0.1:PORT --allow allow.json --report shadow.json`.
   Reports are leads; every confirmed difference becomes a spec fix or an implementation fix with a replayable
   capture attached.

## 5. Capture rules

Before a capture-derived fact enters a spec or a test fixture:
- the file lives under `captures/` and is cited as OBSERVED;
- no ROM strings or item tables are pasted; specs refer to frames by key, sizes and offsets only, fixtures by
  capture file and `seq`;
- the spec passes the identifier check with zero hits;
- values chosen by the reference stack are replaced by DeCombat defaults, and the allowlist entry says so.
Fixtures committed to the public tree are synthetic frames written by us from the spec, not slices of captures.

## 6. Unproven

- UNPROVEN: that the first UDP byte is a useful matching key; the UDP spec decides the real key and the tool takes
  it as a parameter.
- UNPROVEN: which fields vary per run; the first proving replay (section 4 step 2) produces that list.
