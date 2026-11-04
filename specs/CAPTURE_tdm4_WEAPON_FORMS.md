# bot-shooter: the real 2009 client's weapon forms (tdm4, client -> server)

- **Source:** `// OBSERVED: captures/20261025_002236_tdm4/capture.jsonl`, UDP connection 6 (the real
  client, peer port 55725; Bot1 is connection 5, port 55723). Decoded with dc-botclient's own code (branch
  `bot-shooter`): `examples/capdecode.rs` (each direction's datagrams replayed into a receiving `dc_net_engine::Link`:
  check byte, reliable order, resends dropped, split messages joined; engine bundles unpacked) and
  `examples/weaponforms.rs` (field readers of `src/weapon.rs`, written from `GAME_MESSAGES_2009_C2S.md` §2.1, §4.1,
  §4.4, §4.5). Cross-check: every client 0x64 and 0x82 parses with 0 bits left over.
- **Grade:** **PROVEN-CAPTURE** for every row below (bytes the real 2009 client sent). Server -> client rows are marked
  REFERENCE. The parallel validator capval-kill (`staging/capval-kill_CAPTURE_VALIDATION.md`) decoded the same messages
  independently; its numbers agree with these in every field compared (sections 2.1-2.6 there).
- **Offsets:** bit N counts from bit 0 of the delivered engine message (bits 0-7 engine id 11, bits 8-15 shell id,
  fields from bit 16, LSB-first); "byte" = bit / 8 where the field is byte aligned. Hex heads are the first <= 24 bytes
  of the delivered message. No ROM names are reproduced; table keys only.

## 1. Message counts on the client link (t = 56.109 .. 144.469 s)

| shell | count | reliability | note |
|---|---|---|---|
| 0x64 | 1,104 | 1 R (state form `10 00 01`), 1,103 U | section 5 |
| 0x76 | 72 | R | [1][1] x 26, [1][3] x 29, [1][2] x 3, [3][2] x 14 |
| 0x7f | 14 | R | always right after `76 03 02` in the same instant |
| 0x80 | 6 | R | select / hold animation sounds, never per shot |
| 0x82 | 5 | R | 4 rifle hits, 1 grenade release |
| 0x4d | 18 | R | grenade hold, 62-78 ms apart |
| 0xca | 50 | R | sprint |
| 0x83 | 0 | - | no reload happened (UNDECIDED from this capture) |
| 0x89 / 0xc5 / 0xd2 | 0 | - | the client never died |

## 2. Weapon select: `UP_WEAPON_EVENT` 0x76 [3][2] + `UP_WEAPON_CHANGE` 0x7f

```
76: [11][0x76][category:u8 = 3 @byte 2][event:u8 = 2 @byte 3]                 32 bits   0b760302
7f: [11][0x7f][slot:u8 @byte 2][weapon_id:u16 @byte 3][aux:u16 @byte 5]       56 bits   0b7f0091009100
```

| slot | weapon_id | aux | count | server grant for that slot (REFERENCE 0xb2 `[weapon][ammo_kind][ammo:f32][class_pos][slot][4 x i8]`) |
|---|---|---|---|---|
| 0 | 145 | 145 | 8 (4 at t = 62.828: one per doubled spawn grant) | `0db2910091000000f0420000ffffffff` |
| 2 | 0 | 0 | 4 | `0db20000000000007a440002ffffffff` |
| 3 | 7 | 9 | 2 | `0db2070009000000803f0003ffffffff` |

- `aux` = the ammunition id of the new weapon (the 0xb2 `ammo_kind`; 3 of 3 weapons). The lobby loadout 0x323 +0x12
  listed item numbers 145, 164, 0, 7 for equipment slots 0..3 (backpack 7001 in slot 9): equipment slot i = weapon
  slot i, item number = weapon number for weapons; the ammunition id is the ROM row named by `[Weapon<n>] AmmoName0`.
- The primary pick: at the spawn the client selected slot 0 (the lowest granted slot).

## 3. Fire and hit: `UP_FIRE` 0x82, one message per pellet

Rifle hit (`has_extra` = 1, `has_hit` = 1), 887 bits; grenade release (no extra, no hit), 623 bits.

| field (C2S §4.1) | bit | byte | rifle shots 1..4 | grenade |
|---|---|---|---|---|
| `wkind:u8` | 16 | 2 | 0 | 0 |
| `weapon_id:u16` | 24 | 3 | 145 | 7 |
| `ammo_id:u16` | 40 | 5 | 145 | 9 |
| `weapon_pos:vec96` | 56 | 7 | origin + (10.01, -4.27, 0.36) = 4.69 right, 8.85 forward, 4.27 down at yaw 1.0465 | origin + (-2.44, -12.08, -2.07) |
| `origin:vec96` | 152 | 19 | (-1468.163, 704.979, 1104.178) = 0x64 trailer position + (0, 32.98, 0) | |
| `aim:raw128` | 248 | 31 | (0.0059, 0.4997, -0.0034, 0.8662): yaw 1.0466 about +Y then pitch 0.0136 about +X (equal to the 0x64 `aim_q` of the same frame) | (-0.0002, 0.8400, -0.5426, -0.0003) |
| `yaw:f32` | 376 | 47 | 1.0465 .. 1.0477 (controller yaw, not wrapped) | 9.4255 |
| `dir:vec96` | 408 | 51 | unit vector, within 0.004 of the aim forward (spread) | (-0.001, -0.911, -0.412) |
| `seed:u8` | 504 | 63 | 119, 104, 102, 201 (2..255) | 22 |
| `moving`, `wflag`, `mode1:bool1` | 512, 513, 514 | - | 0, 0, 0 | 0, 0, 0 |
| `frac:u8` | 515 | - | 0 | 0 |
| `shot_no:u32` | 523 | - | 0x3F800000 every time (the float 1.0) | 0x3F800000 |
| `time_ms:u32` | 555 | - | 75401, 75696, 76025, 76369 (client time x 1000; 295 / 329 / 344 ms apart) | 107484 |
| `has_extra:bool1` | 587 | - | 1 | 0 |
| `extra_obj:objref16` | 588 | - | 408 (= `hit_obj`, Bot1's character object) | - |
| `extra_kind:u8` | 604 | - | 24, 24, 11, 11 (UNPROVEN; PLAUSIBLY a model part) | - |
| `extra_a:vec96` | 612 | - | the hit point, = `origin + hit_dist * dir` exactly | - |
| `extra_b:vec96` | 708 | - | (-1, 0, 0) in all four (CORRECTION 2026-10-26, client-hitnodes: the entered face's normal of the doubled object box, not a constant; C2S 4.1.1) | - |
| `flag_c:bool1` | 804 (588) | - | 0 | 0 |
| `last:bool1` | 805 (589) | - | 1 (one vector per round: `[Weapon145] VectorsPerRound` read at run time) | 1 |
| `has_hit:bool1` | 806 (590) | - | 1 | 0 |
| `hit_dist:f32` | 807 | - | 1329.886, 1328.119, 1326.353, 1327.387 | - |
| `hit_obj:objref16` | 839 | - | 408 | - |
| `stat:u32` | 855 (591) | - | 2, 4, 6, 8 (+2 per 0x82) | 10 |

Hex heads: shot 1 `0b820091009100f744b6c4632d2f4423118a443885b7c4b0`, shots 2-4
`0b8200910091001645b6c4c02c2f44b2108a443885b7c4b0`, grenade `0b82000700090075011ac5dc211544e2fe034562da19c5b0`.

Geometry: Bot1's own trailer position was (-289.0, 661.4, 1775.0) with the reference stack's teleport box 15 / 50 / 15; the
four hit points lie 30 units in front of that centre on the shooter's side and 24-27 units above it: the client's
hit volume is the model, not the box.

## 4. What the client sends around a shot (0x64 while firing)

`pu_flags` and the control word of the unreliable 0x64 (layout C2S §2.1; for flags 0x3321: `pu_flags` bit 16,
`yaw8c` bit 32, body angles bits 40-135, aux 136-327, `eye_offset` 328-423, `aim_q` 424-551, `weapon_f` 552-583,
control word bit 584, trailer from bit 616; 928 bits, head `0b6421332b00000000d7708a3f00000000202a5e3c9d7b0f`):

| t | pu_flags | control | note |
|---|---|---|---|
| 75.922 | 0x1321 | 0x200 | standing, `RUN` only |
| 76.000 / 76.062 | 0x3321 / 0x3221 | 0x600 | fire key down (0x400) and `pu_flags` 0x2000; shot 1 at 76.078 |
| 76.125 | 0x1321 | 0x200 | `weapon_f` 0.0488 (recoil), then 0.0263, then ~0 |
| 76.406, 76.734, 77.078 | 0x3321 | 0x600 | right after shots 2, 3, 4 |

- Over the whole stream 0x400 appears only in the 5 updates around the shots and in the 2.2 s grenade hold; it never
  coincides with `SPRINT` 0x400000. `pu_flags` 0x2000 is set in the same frames.
- No fire event exists in 0x76 and there is no extra 0x64 per shot: shots travel only as 0x82.
- `yaw8c` is the truncation (toward zero) of `wrap(yaw) x 40.425354` in 1,103 of 1,103 updates (rounding matches
  527): `shell::yaw8c` now truncates.

## 5. What the bot sends now (`dc-botclient --shooter`)

| step | message | form |
|---|---|---|
| spawn (0x67 state 1) + weapon grants (0xb2 / 0xb3; else 0x323 slot 0 + the ROM row) | `76 03 02`, then `7f [slot][weapon][ammo]` | as section 2 |
| every `--fire-interval` (150 ms) with a clear line of sight | one 0x82 per pellet (`[Weapon<n>] VectorsPerRound`) | the 887-bit form of section 3: `wkind` 0, ids of the held weapon, `weapon_pos` = origin + the observed offset turned by the yaw, `origin` = position + 0.66 x box half height (the observed 33 / 50), `aim` = yaw / pitch toward the target's centre + 0.5 x half height, `yaw` the same yaw, `dir` its forward vector, `seed` 2..255, `moving` / `wflag` / `mode1` / `frac` 0, `shot_no` 0x3F800000, `time_ms` = session ms, extra block (`extra_obj` = target object, `extra_kind` 24, `extra_a` = the hit point, `extra_b` (-1, 0, 0)), `flag_c` 0, `last` on the last pellet, `hit_dist` = where the ray enters the target's box, `hit_obj` = target object, `stat` = 2 x messages so far |
| the update after a shot | 0x64 | `pu_flags` \| 0x2000, control \| 0x400 |
| clip empty (`[Weapon<n>] ShotsPerClip`) | `UP_RELOAD` 0x83 `[ammo_id:u16]` | C2S §4.2 layout (not observed live: UNDECIDED) |

The dc-dserver reader (`dc-dserver/src/up.rs::parse_fire`, written independently) accepts the bot's 887-bit message
and applies damage from it (loopback test `tests/shooter_loopback.rs`: two kills); the reference stack accepted all 200
of them in attempts 3 and 4 (`bot-shooter_REFERENCE_FINDINGS.md`).

## 6. Cross references (added by spec-fold-5, 2026-10-25; the record above is unchanged)

Where the facts of this record are used by DeCombat design rows (D = a DC_DEFAULTS row, proposed; TESTED / RUN = crate test / loopback run of our own stack):

| fact here (PROVEN-CAPTURE unless marked) | used by | place in the base specs |
|---|---|---|
| section 2: equipment slot i = weapon slot i; item number = weapon number for weapons; `aux` = the ammunition id | DSV-38 (the server grants weapons by the slot and item number of the pipe's `loadout=` tokens) and LDO-1 (the lobby writes them) | `GAME_MESSAGES_2009_S2C.md` 4.4.1 (DeCombat paragraph), `DSERVER_PIPE.md` 11, 12.A |
| section 2: the client draws the lowest granted slot at the spawn | DSV-38 (0x7f automatic switch to the lowest slot, else 0xb3) | S2C 4.4.1 |
| section 2: the reference stack's 0xb2 form `[weapon][ammo_kind][ammo][class_pos 0][slot][-1 x 4]` (REFERENCE) | DSV-38 (`class_pos` 0 as the reference stack sent; one grant block per spawn instead of the reference stack's two) | S2C 4.4, 4.4.1 |
| section 3: the 887-bit `UP_FIRE` with the extra block (`extra_obj` = `hit_obj`, `extra_kind`, the hit point) | DSV-30 (the damage factor of the skeleton node named by `extra_kind`, PLAUSIBLE that it is a node index), DSV-34 (the shot trace from `origin` along `dir`) | `GAME_MESSAGES_2009_C2S.md` 4.1, 4.1.1, Appendix F.2 (K1-K3) |
| section 3: hit points 24-27 units above the victim's centre with a box half height of 50 | DSV-30 (the node decides the zone; the box band only without a named node) | C2S 4.1.1 |
| section 4: control bit 0x400 and `pu_flags` 0x2000 only around shots | C2S 2.1.1 (K4) | C2S 2.1.1 |
| section 5: the bot's forms | `dc-botclient --shooter`; RUN against `dc-dserver` slice 4: 54 hit reports, 7 kills, 0 trace refusals | `reports/dserver-slice4.md` section 3 |

Not yet captured (unchanged): a reload (0x83), multi-pellet weapons, melee, a head shot by the real client, the real client's own death.
