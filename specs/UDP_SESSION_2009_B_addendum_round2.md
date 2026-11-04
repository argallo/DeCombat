# UDP_SESSION_2009_B addendum: dserver-round2 findings: APPLIED (stub)

Applied on 2026-10-25 by spec-fold-8 into the base specs; the full staged text stays in git history (master c2c114c, `specs/UDP_SESSION_2009_B_addendum_round2.md`). Grades
unchanged. G1 (door object type 0x39) -> `GAME_MESSAGES_2009_S2C.md` 8.2 row 0x39 (`OBJ_DOOR`); G2 (doors in the initial group, V-DOOR-1) -> `UDP_SESSION_2009_B.md` 3.5.1
("Sliding doors"); G3 (world variable value text, the 16 client names in CShell.dll) -> `UDP_SESSION_2009_B.md` 8.6; G4 (0x45 intro field sources) ->
`GAME_MESSAGES_2009_S2C.md` 2.1.1 step 5 ("Field sources"); G5 (spawn teleport height) -> `MOVEMENT_2009.md` 12.4 ("Spawn teleport height"); G6 (`C2S_BYE` 15-16 ms after the
0xbd) -> `UDP_TRANSPORT_2009.md` 9.1 and `GAME_MESSAGES_2009_C2S.md` 5.7; G7 (open items): V-DOOR-1 and the door open rule in B 3.5.1, V-FALL-1 (a capture with a death in a
kill volume) and the spawn-point choice of the 2009 server stay open (listed in the spec-fold-8 report; the selection rule is a game rule and stays ours).
