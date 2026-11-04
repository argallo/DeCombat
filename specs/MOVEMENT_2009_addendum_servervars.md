# MOVEMENT_2009 addendum: server variables vs the mover, per-frame rules, fixed-step recipe (dserver-servervars)

Status: promoted verbatim from the Wave AC worker's staged text (2026-10-25); to be folded into MOVEMENT_2009.md sections 4, 8.1 and 12 and UDP_SESSION_2009_B.md 8.6 by the next spec-fold. project decision: no frame-rate cap; dc-client implements the fixed 1/60 s step (CLI-MOVE-1). Grades as stated per item.

For the spec-fold worker and the dc-client worker. Our words; evidence by RVA.

## A. Server variables and the mover (new subsection for section 2)

- The client keeps server variables (`S2C_EVAR`) in their own table (engine client + 0x4a0). Writer engine+0xa50c0 ->
  engine+0x22c30; created / cleared by engine+0x15980 / engine+0x159d0 with no bindings (binding count 0, so no engine
  global is set from it) and no change callbacks. The only reader is engine+0xd760 (client slot +0x134): it writes 0
  to the output, then returns not-found for a missing name (EVIDENCE).
- CShell reads through slot +0x134 only at: cshell+0x1a55f2 (step height, at world entry, pushed into the physics
  unconditionally), cshell+0x1fe738 (step height, every physics step, pushed when not 30.0), cshell+0x1a9c00 /
  +0x1a9c18 / +0x1a9c30 (wind x / y / z), cshell+0x1a687a inside cshell+0x1a6840 (called by cshell+0x1a6900 and
  cshell+0x1a9b00: the world-settings names, the world-north angle and three more world names, each copied into a
  client console value with `"<name> %f"`). ClientFx.fxd has no slot +0x134 call (DISASM scan of every
  `call [reg+0x134]` with the string pushed before it).
- Consequence: **the only server variable that reaches the mover is `STEP_HEIGHT`**. `GRAVITY`, `WALK_SPEED`,
  `CROUCH_SPEED`, `JUMP_SPEED` and the other section-2 tunables are client console variables (slots +0x104 /
  +0x5c); the server cannot set them through `S2C_EVAR`. The 2009 server's own speed / jump / swim / ladder / leash
  variables are not read by the client (their names are not in the client binaries).
- cshell+0x1a9b00 also runs the console command "Prediction 1" or "Prediction 0" at world entry, depending on a
  shell state word (`[0x3759e22c]+4 == 3`); not a server variable (noted for section 12).

## B. Frame-rate dependence of the 2009 mover (for section 15; project decision: no frame cap, frame-rate independence)

Per-frame rules that make slow motion depend on the frame time (sections 4, 8.1, 8.3):

1. `frame_dt` is a whole number of milliseconds (engine+0x17460); above about 1000 frames/s many frames have
   `frame_dt` 0 (no motion at all) or 1 ms.
2. 8.1 step 5: a frame whose displacement `|out|` is <= 0.1 units does not move the body (the velocity is kept).
   With `dt` = 1 ms every speed <= 100 units/s is dropped: crouch (cap 50) moves only in frames of 3 ms or more,
   walk (cap 70) only in frames of 2 ms or more (observed: crouch frozen in 67 of 68 updates, walk at about a tenth
   of its speed, aa-triage-world-physics item 8), and the last part of a jump before the apex (vertical speed < 100) does not rise, which lowers
   the apex by up to about 100^2 / (2 x 1100) = 4.5 units. At 60 frames/s (`dt` 16 or 17 ms) the dead zone is
   6 units/s and invisible.
3. 8.3: `|v|^2 < 0.1` and `|a|^2 < 0.1` dead zones are per velocity, not per frame (frame-rate independent).
4. 8.3 quarter `dt^2` term and the clamp of `dt` at 0.1 s are per-frame.

dc-client rule (proposed, CLI-MOVE-1): run the mover at a fixed step of 1/60 s (accumulate real time; zero, one or
more mover steps per rendered frame; render interpolated between the last two states) and apply rules 2 and 4 to
that fixed step only. This keeps the 2009 numbers tuned for 60 frames/s at every display rate without capping the
renderer. Equivalent alternative: keep variable `dt` but evaluate rule 2 as a speed threshold, `|out| / dt <= 6`
units/s, never per frame. The dserver needs no change for either (the client is authoritative for its own
movement; the server checks with a tolerance band, section 15 item 3).

The 2009 binary itself keeps rule 2; on a fast machine it freezes crouch-walking against any server (observed with
the reference stack in tdm1 and with ours in rust4).
