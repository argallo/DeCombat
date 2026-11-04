# WEAPON_FIRE_2009 addendum: the view recoil and the hit jar of the 2009 client (clean-room)

- **Status:** addendum, written 2026-10-26 (spec-fold-10) from the worker report `reports/client-recoil.md` (branch `client-recoil`, merged into the Rust master).
  It replaces the "recoil functions not read" entries of `WEAPON_FIRE_2009.md` (scope line, section 9). Correction marker: **CORRECTION 2026-10-26 (client-recoil)**.
- **Evidence:** the clean decompiler output of CShell.dll, read-only disassembly of the user's CShell.dll where the clean C hides x87 arguments or key names, the user's ROM tables read at run time,
  and the sweep (20261025) and rust6 captures of the real 2009 client. Grades as in `WEAPON_FIRE_2009.md`; PROVEN-CAPTURE for the replay against the sweep. No ROM value is copied here:
  row keys are named, their numbers are read from the ROM; the few numbers below are binary constants (code defaults, console defaults).

## R1. Where the state lives

The recoil state is on the **view controller, not on the weapon**: one state for every weapon, reset at spawn, with the side flag kept. Fields (controller offsets): `a` pending pitch (+0x54),
`w` pending yaw (+0x58), side flag (+0x5c), tick of the last kick (+0x60), kick-back distance (+0x64). The camera update (cshell+0x1f8b40) runs every frame: first the drain
(cshell+0x1f3070), then the fire check; when a shot fired, the pitch kick (cshell+0x1f5150) and, when `a` exceeds the row's `MaxRandomPitch`, the yaw kick (cshell+0x1f2f60).

## R2. Row keys

Weapon-row keys (read order and record offsets from the loader, cshell+0x17dd12..0x17def9; code defaults, used for a row without the key, from cshell+0x17f7b0), with the record offset and the
code default in brackets: `MaxCamRecoilPitch` (+0x2f0, 10), `BaseCamRecoilPitch` (+0x2f4, 1.2), `BaseCamRecoilAccelPitch` (+0x2f8, 0), `MaxCamRecoilYaw` / `BaseCamRecoilYaw` (+0x2fc / +0x300, 0;
no reader in the camera code), `MaxCamRecoilWag` (+0x304, 30), `BaseCamRecoilWagR` (+0x308, 1.0), `BaseCamRecoilWagL` (+0x30c, -1.0), `MaxRandomPitch` (+0x310, 0.032), `MaxRandomWag` (+0x314,
0.015), `TripleShotMaxCamRecoilPitch` / `TripleShotBaseCamRecoilPitch` (+0x320 / +0x324), `BaseCamBackJitter` (+0x328, 0.5), `BackJitterRecover` (+0x32c, 3.0). The ammunition row adds
`JitterDecDelay`, `JitterWaitRecover`, `JitterRecover`. Observed in the ROM: the two wag sides carry opposite signs (L positive, R negative, for example the rifles 1.2 / -1.2 with
`MaxCamRecoilWag` 15..30), `MaxRandomPitch` 0.02..0.065, `MaxRandomWag` 0.01..0.065.

## R3. The kick (per shot)

Angles are in radians, `rad(x) = x * pi / 180` with the client's f32 constants.

1. **Pitch.** `k = 0.5 * (rad(Base) * (1 - a / m) + Accel * a)` (the accel term only while the first part is positive), `m = rad(MaxCamRecoilPitch)`; `k` is capped so that `a + k <= m`; then
   `a += k` and the view pitch `-= 2k` (up). In triple-shot mode, when both TripleShot keys are set, that pair replaces the normal pair.
2. **Kick tick and kick-back.** The kick tick := `GetTickCount`; `back := max(back, 0) + BaseCamBackJitter`.
3. **Yaw kick**, only when `a > MaxRandomPitch` (a raw row value compared with radians, as the code does): `f = rad(WagL or WagR, chosen by the side flag) * (1 - |w| / rad(MaxCamRecoilWag))`;
   `w += f / 2`; the view yaw `-= f`. Then, when `|w| > MaxRandomWag` (raw) and **`GetTickCount() & 1` is 1, the side flips**. That low clock bit is the only random source: there is no random
   magnitude. The yaw convention is the 0x82 one (`atan2(x, z)` of the forward axis); the positive L wag turns the view toward negative yaw first.

## R4. The drain (per frame)

`rate` = `JitterWaitRecover` while `GetTickCount - kick tick < JitterDecDelay`, else `JitterRecover`. Pitch: `r = dt * rate * (1 + a / m)`, `r <= a`, then `a -= r / 2` and the view pitch `+= r`. Yaw
the same with `|w|` and `rad(MaxCamRecoilWag)`, signed by `w`. Kick-back: `back -= dt * BackJitterRecover * 5` while positive; the eye sits `10 * back` units behind its place along the view
(cshell+0x1f9c40). Because the kick moves the angle by twice what it adds to `a` and the drain by twice what it takes away, the view's offset is always `-2a` / `-2w`: after a burst the view returns exactly
to where it was (PROVEN-CAPTURE: the yaw comes back to the burst's first value to 0.001 degree).

## R5. Recoil and the cone

No shared state. The cone (`WEAPON_FIRE_2009.md` 2, `_addendum_cone.md`) is drawn around the camera object's rotation (cshell+0x234d50; the rotation is built by cshell+0x1fafa0 from the view
angles plus the camera-effect offsets). The recoil moves the view, so the next shots follow it, and the 0x82 `aim` (the view's rotation) carries the recoil too. The one indirect coupling: the motion
spread's view-turn term sees every change of the view, the recoil's included.

## R6. Check against the real client (sweep 20261025, every swept weapon)

The replay used each burst's own 0x82 times. Unknown from the capture are the tick clock's phase and the clock's low bit at each kick, so at each kick where the rule allows a side change the replay
took the choice that fits the next two shots, and never changed side where the rule forbids it. Largest error over the first 30 shots of every burst, degrees: weapon 145 yaw 0.08 / pitch 0.14;
weapon 164 0.02 / 0.02; five more weapons at most 0.02; three at 0.05-0.08 / 0.12-0.19; one outlier at 0.40 / 0.44 (the sweep's odd capture, longer client frames); the rest show no yaw (slow fire
drains the kick, or a zero wag) and a pitch error of at most 0.13. The first yaw kick came at the rule's shot in every burst. Over the weapons with visible yaw the side changed on 276 of 601
allowed kicks (46 %): a coin, as the clock's low bit is. (PROVEN in the code, PROVEN-CAPTURE against the sweep.)

## R7. The damage jar behind the "Reality" option (S2C 0x74; default OFF)

0x74 is handled in cshell+0x1f7d20. After the reads: if the console value **"Reality"** (registered with default 0 at cshell+0x2b001b; an options-menu entry whose text sits beside "Tracer Bullet" and
"Blood Effect") is above 0 and `|from_dir| > 0`: `d` = the normalised `from_dir`; the body rotation is built from the controller's angles (controller +0x68..+0x70: pitch 0, the yaw, the roll); `F` is its forward
axis (cshell+0x11b8a0, third column), `R` its right axis (cshell+0x11b800, first column); the **sector** is `k = ftol((atan2(d . F, d . R) + pi) * 1.9098593)` (**12 sectors**, k = 0..12);
the damage indicator of sector `k` gains `|from_dir|` (capped at 1). Then the jar, only if `damage` != 0, `k` != 12, the damage type has the **jar flag** (`JarCamera`) set, the camera is in its normal
mode and the console value `CamDamage` is above 0. The damage type is the byte before `mode` in the 0x74 body (the byte our servers fill with the ammunition's damage type); the type rows
(cshell+0x282c20) are filled by name from the ROM's damage-type file; in the user's ROM the Bullet and Choke types carry the flag.

- A parameter block of six wave channels (pitch, yaw, roll, x, y, z; cleared by cshell+0x1f1750) is built.
- `CamDamagePitch` > 0: channel 0 (pitch) gets `+rad(CamDamageMinPitchVal)` for sectors 8..10 (shot from behind: view down) and `-rad(...)` for sectors 2..4 (from the front: view up), rise time
  `CamDamageTime1`, fall time `CamDamageTime2`, curve 2 for both; written into the camera-effect slot when the slot's amplitudes sum below 2.0 (cshell+0x2b1530).
- `CamDamageRoll` > 0: channel 2 (roll) gets `+rad(CamDamageMinRollVal)` for sectors 5..7 (from the left) and `-` for sectors 0, 1, 11 (from the right); written when the sum is below 1.0 (cshell+0x2b15a0).
- Console defaults (cshell+0x1f7322..0x1f7416): `CamDamage` 1, `CamDamagePitch` 1, `CamDamageRoll` 1, `CamDamageTime1` **0.1 s**, `CamDamageTime2` 0.25 s, `CamDamageMinPitchVal` **2 (degrees)**,
  `CamDamageMaxPitchVal` 20 (unused here), `CamDamageMinRollVal` 2, `CamDamageMaxRollVal` 20 (unused).
- The channel (cshell+0x2b0f40): rise `amp * c(t / t1)`, fall `amp - amp * c(t / t2)`; curve 2 (cshell+0x19f010) is `c(x) = x` below 0.5, else `(1 - cos(pi x)) / 2`. The slots are summed into pitch /
  yaw / roll offsets (cshell+0x2b17e0), clamped by `CamMax*Offset` (default 15, raw: never bites) and added to the view angles when the camera rotation is built (cshell+0x1fafa0).
- **The magnitude does not depend on the damage**; the direction only picks pitch or roll and the sign. A hit is 2 degrees at 0.1 s and back to 0 at 0.35 s (wave values: 0.5 of the amplitude at 25 ms, full
  at 100 ms, 1.0 of the amplitude at 225 ms on the way down, 0 after 350 ms).

The camera rotation built there is also the shot rotation (cshell+0x234d50), so with the option on the shots after a hit leave up to 2 degrees off the reported aim; the 0x64 / 0x82 `aim` stay the
view's. **Observed:** the real 2009 client of rust6 (52 bullet hits from our server, damage-type byte 2) fired 6 shots 47..188 ms after front hits where the jar would have been 0.94..1.98 degrees up; they
left 0.007..0.38 degrees off their aim (within the cone, at most 0.23 of the jar); its option was off. The arena reference stack's flinch (1.816 degrees on the first shot after three hits) is consistent
with the jar but is the reference stack's behaviour with the option effectively on, not the 2009 default. DeCombat default: off (the 2009 registration default); `--reality` turns it on (CLI-313, project decision
welcome). HUD note: in 2009 the damage indicator is fed inside the same "Reality" gate and its sector is `atan2(d . F, d . R) + pi` of `from_dir` itself; which image the HUD draws per sector was not traced.

## R8. Not covered (open)

The drawn camera's smoothing of pitch / yaw (cshell+0x1fafa0, 1 / 1.5 per frame, 500 ms snap; it also lags the shots); the weapon mods' recoil terms (mod +0x238 / +0x23c / +0x258 / +0x25c in the kick,
+0x254 in the yaw threshold); the first-person weapon model's kick offset; the other camera waves (landing dip / roll / pitch, `CamLand*`); the body yaw writer (+0x6c).
