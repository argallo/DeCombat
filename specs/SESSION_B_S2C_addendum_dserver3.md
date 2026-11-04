# SESSION_B_S2C addendum dserver3: APPLIED (stub)

Applied on 2026-10-25 (spec-fold-4) into `UDP_SESSION_2009_B.md` and `GAME_MESSAGES_2009_S2C.md`, items S3-1 .. S3-13. The full text stays in git history
(master 4a691aa, `specs/SESSION_B_S2C_addendum_dserver3.md`). Each item's content is now part of a base spec; grades were not changed (REFERENCE items stayed REFERENCE or became
REFERENCE-CONFIRMED where a real-client capture shows the same form accepted; no reference stack value became a rule).

| item | what | applied in |
|---|---|---|
| S3-1 | player character create record (kind 1, type 0x18 blob, model block, animation block, flags, attachments, dims, files) | `UDP_SESSION_2009_B.md` 3.5 and 3.5.1 |
| S3-2 | type 0x18 character record (572 bits) | `GAME_MESSAGES_2009_S2C.md` 8.2.1 |
| S3-3 | weapon model object attached at socket 2 | `UDP_SESSION_2009_B.md` 3.5, 3.5.1 |
| S3-4 | spawn change records and visibility | `UDP_SESSION_2009_B.md` 3.5, 3.5.1 |
| S3-5 | characters in the object stream, three trackers, clip indices | `UDP_SESSION_2009_B.md` 4.3, 3.5.1 |
| S3-6 | shell 0xa6 notes to the type 0x18 object | `GAME_MESSAGES_2009_S2C.md` 8.3 |
| S3-7 | start order and spawn set as the reference stack sent it | `GAME_MESSAGES_2009_S2C.md` 10.1 (and 3.2.2, 4.1.1, 4.4.1) |
| S3-8 | file table during the session | `UDP_SESSION_2009_B.md` 3.5, 3.5.1 |
| S3-9 | the player box | `UDP_SESSION_2009_B.md` 3.5, 3.5.1 |
| S3-10 | no relay in the bot-only run (U5 open) | `UDP_SESSION_2009_B.md` 3.5.1; U5 decided for the reference stack form by tdm2 (`GAME_MESSAGES_2009_S2C.md` 8.2) |
| S3-11 | world objects of the initial group (not implemented by `dc-dserver`) | `UDP_SESSION_2009_B.md` 3.5.1 |
| S3-12 | `GS_WEAPON_SWITCH` command numbers | `GAME_MESSAGES_2009_S2C.md` 4.4.1 |
| S3-13 | agreement with the real-client captures (8 points) | points 1-4: `UDP_SESSION_2009_B.md` 3.5; point 5 (relays): S2C 8.2; point 6 (0xca): S2C 3.2; point 7 (runaway yaw): `GAME_MESSAGES_2009_C2S.md` 2.1.1; point 8 (respawn): S2C 4.1.1 |

Not applied: the "Status of the implementation" paragraph and the `dc-dserver` column of the S3-13 table (they describe a crate, not the wire). The evidence files
`CAPTURE_tdm1_TRANSPORT_VALIDATION.md`, `CAPTURE_tdm1_GAME_VALIDATION.md`, `CAPTURE_tdm2_TWOPLAYER_VALIDATION.md`, `CAPTURE_tdm4_KILL_VALIDATION.md`, `CAPTURE_tdm4_WEAPON_FORMS.md` and
`REFERENCE_FINDINGS_botduel.md` are kept in full as the evidence record.
