# Spec questions for GAME_MESSAGES_2009_S2C.md / UDP_SESSION_2009_B.md: the thrown object and its blast

Status: STUB (answered by spec-fold-6, 2026-10-25, from the captures tdm2 and tdm4; questions staged by the dserver-tidy worker for DSV-39). Answers, all REFERENCE (forms the real
client accepted) with capture seq numbers, are in `UDP_SESSION_2009_B.md` 3.5.4 and `GAME_MESSAGES_2009_S2C.md` 8.2 (rows 0x13 and the blast row, plus the DeCombat paragraph):
Q1 blast `a` 71, `b` 0x59f0 / 0x5ff0 (differs per blast), `c` 1, `d..g` 00 ff ff ff (meanings UNPROVEN, V-PROJ-2); Q2 `normal` exactly (0, 1, 0) in both; Q3 `from` = the release
0x82's `origin`, not `weapon_pos`; Q4 `flags3` 0; Q5 one main tracker (`anim` 0, time 0), the create rotation is not the thrower's yaw (source UNPROVEN, V-PROJ-3); Q6 the reference stack
streamed the projectile (`uf` 4 in every stream message) and sent change records during the flight (corrected and extended by spec-fold-8 from aa-triage-grenades: the
reference stack DID stream every thrown object, 64-74 entries per throw over tdm2, tdm4 and tdm6, with 1, 15, 24 and 19 change records per flight; the rust4 playtest report is the first
observation that the client does not move the object without them, V-PROJ-1; B 3.5.4); Q7 blob widths above
255 UNPROVEN (V-PROJ-4); Q8 both orders of removal and blast in one tick were accepted; Q9 the reference stack sent 0x66 kind 1 `[7][9][0.0][3][1]` to the thrower in the datagram of the
create. The crate differences that follow are in `staging/spec-fold-6_CRATE_FOLLOWUPS.md` (S-3a..S-3h).
