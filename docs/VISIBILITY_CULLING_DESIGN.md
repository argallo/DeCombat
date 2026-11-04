# VISIBILITY_CULLING_DESIGN: server-side visibility culling for vanilla DeCombat servers

Status: DESIGN DRAFT 1 (2026-10-25, worker `visibility-design`). The proposed rules are staged as VIS-1 .. VIS-14 in
`staging/visibility-design_DC_DEFAULTS.md` (PROPOSED, not approved). *Updated by spec-fold-7:* implemented in
`dc-dserver` by the worker `dserver-visibility` (branch head f61327d, merged into master 7fbad44; bot side by bot-vis13, master 765c756); unit-, session- and ROM-gated tested and
run headless with `dc-botclient`; **not tested against the real 2009 client** (V-VIS-1 .. 9 open). Section 10 is the implementation record;
where it differs from sections 3-8 the implementation and its staged row corrections
(`staging/dserver-visibility_DC_DEFAULTS.md`, PROPOSED) describe the code.

Scope: the vanilla-server anti-wallhack rule of `rust/HOSTING_AND_MODS.md` §3.1, last row ("server-side visibility
culling: positions of enemies a player could not plausibly see are not sent ... a wallhack server simply disables
the culling"). Covers the per-pair visibility test, its cost, what else leaks positions, how a culled enemy is shown
to the 2009 client, the interaction with mods, spectators and the Rust client, the configuration, and an
implementation plan for `dc-dserver` in waves.

Sources used (all allowed by `CLEANROOM.md` §2 and `CLEANROOM.md`): `specs/UDP_SESSION_2009_B.md`
(§3-§5, §8, §9, §12), `specs/CAPTURE_tdm2_TWOPLAYER_VALIDATION.md`, `specs/GAME_MESSAGES_2009_S2C.md`,
`specs/GAME_MESSAGES_2009_C2S.md` §7.3, `specs/FMT_DAT85.md` §5, §9, §10-§11, `specs/RENDER_2009.md`
§5.1, `specs/MOVEMENT_2009.md` §9, `DC_DEFAULTS.md`, `rust/HOSTING_AND_MODS.md`, and our crates
(`crates/dc-world`, `crates/dc-sim`, `crates/dc-dserver`, master 79587db). Server rule values in this document are
DeCombat choices; none is taken from the reference stack. Measurements in Appendix M were taken with a scratch program
outside the tree against the user's ROM (read-only); the program printed no ROM names, and none are quoted here.

---

## 1. Problem and what the 2009 server did

A wallhack is a modified client that draws objects it has data for, walls or not. The only defence that does not
depend on the client is to send no data for enemies the player could not see. The 2009 wire allows this: the object
stream (`S2C_OBJ_STREAM`, id 10) is unreliable, per object and per client, and the client ignores any object it does
not hear about (B §4.3, §12.2 item 2).

The 2009 server itself did not cull by occlusion. Its stream selection (B §4.3, engine+0x8f8b0) orders entries by
object size, speed, time since the last send and inverse distance from the client's view point, and cuts at a bit
budget. There is no line-of-sight term. Positional engine sounds are dropped beyond twice their outer radius (B §8.2,
engine+0x91dc0); that is the only distance-based withholding in the 2009 writer. Culling is therefore a DeCombat
addition. It has to stay inside what the 2009 client accepts on the wire, and it must not change what an honest
player sees or hears.

Two errors are possible, and they are not equally bad:

| error | effect | rank |
|---|---|---|
| false hidden (the enemy is visible but its data is missing or stale on the client) | pop-in: an enemy appears late or out of nowhere, a gameplay bug every honest player notices | must not happen; every margin in this design works against it |
| false visible (the enemy is hidden but streamed anyway) | the cheater learns a position it could not see | acceptable in small amounts; this is the cost of the margins |

Rule of thumb for every choice below: when the test is unsure, overloaded or not yet run, the pair counts as
**visible** (fail-open).

## 2. What the inputs offer

### 2.1 World data (dc-world, FMT_DAT85)

- Every BSP model of a 2009 world has `n_portals = 0` and `n_leaves = 0` (FMT_DAT85 §5.2, §11: all 21 ROM worlds; the
  loader even fails on a non-zero portal count). `dc_world::Leaf` exists but no ROM world fills it
  (`crates/dc-world/src/bsp.rs`). **There is no precomputed PVS and no portal graph in the format.** A leaf-to-leaf
  PVS can only come from our own preprocessing.
- The BSP nodes are polygon nodes (`BspNode { poly, front, back }`) whose leaves are two engine sentinels (-1 / -2,
  `NodeRef::TermA` / `TermB`). Whether those sentinels mean "solid" and "empty" is UNPROVEN (MOVEMENT_2009 §16.1
  item 5); `dc-sim` treats both as "no child" (`crates/dc-sim/src/from_world.rs`, `link`). So a solid-leaf
  line-of-sight walk (the classic exact BSP trace) is not available without first proving the sentinel meaning and
  that the level is sealed.
- The level geometry is the one model per world that no object links to; brush objects link the other models by name
  (FMT_DAT85 §6.4; `dc_sim::from_world::WorldFilter`). Surface flag bit 0 marks collision polygons (MOVEMENT §9.8);
  render-only polygons lack it (FMT_DAT85 §5.2, Appendix Z D6).
- World-model visibility is an object property: the invisible flag is set on all kill volumes, ladders and hazard
  volumes and on others; blend mode opaque / translucent / additive and an alpha property exist (RENDER_2009 §5.1
  step 6, R5). The renderer draws world sections back-face culled (RENDER_2009 §5.1, DeCombat note: `Face::Back`).
- The blocker soup (FMT_DAT85 §9.1) is invisible player clipping. It must not occlude sight.
- The render data has designer occluder polygons in only 5 of 21 worlds, and sky portals (FMT_DAT85 §10, §11.1); the
  2009 client's own render visibility is a frustum plus occluder walk over render blocks (RENDER_2009 §5.1 step 2).
  Nothing there is a PVS either.

### 2.2 Ray code (dc-sim)

`dc_sim::CollisionWorld::ray(from, to)` (`crates/dc-sim/src/collide.rs`) is a zero-size sweep through the
movement code. It returns the nearest hit with a 0.1 back-off, walks every model's polygon BSP with a
mid-point-sphere cull, and tests only polygons the segment enters from the front (MOVEMENT §9.8). It is exact enough,
but it was built for short moves. Measured on the 21 ROM worlds (Appendix M), long segments between spawn points
cost **52 µs on average and up to 293 µs on average per world** (x86_64 release). Blocked segments are the slow
ones: 25-510 µs, against 0.3-7.5 µs for clear segments. That is too slow for a per-tick test of 240 pairs.

A dedicated occluder structure fixes this. A triangle BVH over the same collision polygons, with an any-hit segment
query that accepts front faces only, cost **1.23 µs on average (x86_64) / 1.35 µs (default i686 toolchain), with a
worst single segment of 49 µs**. Build time was 3-29 ms per world. It agreed with `dc-sim`'s point sweep on 99.86-100 %
of 684,480 segments (Appendix M). A double-sided version occluded 0-1.6 % more segments than the sweep. That is
consistent with the sweep and the renderer both ignoring back faces, so the visibility test is one-sided too.

### 2.3 Object stream and tick (dc-dserver)

- `Game::stream_for(viewer, now)` (`crates/dc-dserver/src/game/relay.rs`) lists every other placed player's
  character, longest-unsent first (CD-65). `Session::tick` (`crates/dc-dserver/src/session.rs`) writes one id 10
  message per in-world client per tick within `GameTuning::stream_budget_bits` (DSV-17, 4000 bits), with the time
  entry last (B §9.1). `mark_streamed` records the last send time and rotation per viewer and object.
- Removals go out as a kind 3 record in id 8 (`stream::removal_group`; B §5.1). The character and its weapon child
  are removed together (CV2-8, tdm2 §4.2).
- The tick rate is `dserver.tick_hz` (CD-63: 10 by default, 30 in the measured runs). The measured cost was 0.45 % of a
  core and 7 MB working set for 2 players at 30 Hz. The stream part grows
  as n x (n - 1) entries; 16-player cost is not measured yet.
- Stale remote players are streamed with velocity 0 after 250 ms without an accepted update (DSV-21), so the client
  stops extrapolating them.

### 2.4 What the 2009 client does when an object stops being streamed

From the specs, not from a capture of this exact case:

- An id 10 entry for an unknown object is parsed and ignored. A known object moves only if the new sample differs
  from the client's prediction by more than the thresholds of B §4.3. With no entries, nothing on the wire moves the
  object. The prediction service at VA 0x6e1cec is PLAUSIBLY an extrapolation from the last velocity (B §4.3). So an
  unstreamed character probably **freezes, or keeps sliding with its last velocity**. Which of the two is UNPROVEN
  (capture item V-VIS-2). The 2009 writer's reliable follow-up "repeat the position with velocity 0 so the client
  stops extrapolating" (B §4.2 item 6) points to sliding.
- A removal record tears the object down; a later create with the same id replaces it (B §3.3, §5.1). The client
  shows the effects of removal only through the roster messages: 0x6c removes a player from the HUD lists (S2C §3.2)
  and 0x6b binds roster records to characters by object (tdm2 §2.3: the client asked with 0x8c when 0x6b named an
  object it did not have yet).
- tdm2 observed removal only for leavers (character and weapon child in one record, ids reused later; tdm2 §4.2,
  V-B5 SETTLED). **A removed-then-re-created live character was never observed.** Neither was a character that went
  unstreamed. Both are UNPROVEN for the real client.

## 3. The visibility test

### 3.1 Candidate algorithms

| id | algorithm | exact? | cost per pair-tick | availability | verdict |
|---|---|---|---|---|---|
| A | leaf-to-leaf PVS from the file | conservative | O(1) lookup | **not in the format** (`n_leaves = 0`, `n_portals = 0`) | impossible |
| B | own portal / PVS preprocessing over the BSP (public: portal generation plus flood) | conservative if the level is sealed | O(1) | needs a solid-leaf BSP and sealed geometry; the ROM BSP is a polygon BSP with unproven sentinels, detail polygons cross nodes (FMT_DAT85 §5.2: about 3 nodes per polygon) | research only (W5) |
| C | segment tests against an occluder BVH, sampled over the enemy's box and the viewer's possible eye positions, plus an exposure window and lookahead | not conservative by itself; the margins make it so in practice | 1-15 segments of about 1.3 µs | everything exists: dc-world polygons, our own BVH (public algorithm) | **chosen** |
| D | dc-sim `CollisionWorld::ray` as the segment test | as C | 40x slower than C on blocked segments | exists | rejected for the runtime, kept as the reference in tests |
| E | coarse cell-to-cell visibility (voxel cells, offline sampled with C, dilated by one cell) as an early reject in front of C | not conservative (a sampled gap can be missed) | O(1) reject, then C | ours, cache per world hash in the operator's data directory | optional accelerator, off until the census of W5 shows zero disagreement with C on every ROM world |
| F | solid-leaf BSP trace (exact, O(depth)) | exact if the sentinel meaning holds | about 0.2-1 µs | only if V-W1 (sentinel meaning) is proven | research only (W5) |

C is the design. E and F are only accelerators. Neither changes the policy; both are judged by C's results.

### 3.2 Occluder set

An occluder is a polygon an honest client draws as opaque, from the side it is seen from. The set (VIS-3):

1. the level geometry model, and world models whose object role is `PlainWorldModel`, with the invisible flag
   clear and blend mode opaque (RENDER_2009 §5.1 step 6);
2. of those models, only polygons with surface flag bit 0 (the collision polygons, MOVEMENT §9.8). Render-only
   polygons (foliage, decals, details) do not occlude;
3. tested **front face only** (corners counter-clockwise seen from the front, FMT_DAT85 §5.2 D2; the renderer culls
   back faces);
4. never: the blocker soup, kill / hazard / trigger volumes, ladders, translucent and additive models, `Breakable`
   models (they can stop being there), unlabelled models (doors and other movers whose state the server does not
   simulate yet), sky models.

Every exclusion moves the test towards "visible". A glass pane modelled as an opaque collision polygon is the one known
way to get a false hidden. Translucent models are excluded by their blend mode. Glass inside the level geometry
cannot be told apart by any flag we know (surface flag meanings beyond bit 0 are kept opaque, FMT_DAT85 §13 item 3).
Two mitigations: the census of W4 compares the occluder set with the render data's translucent sections, and the
operator config can list model indices to treat as transparent (`vis.transparent_models`, per world hash, kept in the
operator's config; no ROM names in the tree).

The BVH is built once per loaded world and shared read-only across rooms (`Arc`, HOSTING §2 "world data shared").
Measured: 16-97 thousand triangles, a few MB, 3-29 ms to build (Appendix M).

### 3.3 Per-pair test

For each viewer V and enemy E (section 6.1 says who is an enemy), each test round:

```
L  = lookahead (3.4)
e0 = V.pos + eye offset                          eye offset: box centre + 0.75 x half-height (VIS-4)
P  = E.pos ; P' = E.pos + E.vel * L              enemy now and ahead
s  = unit vector perpendicular to (P - e0) in the horizontal plane
targets T0 at P:  centre, head (centre + 0.9 h), low (centre - 0.6 h),
                  left / right edge (centre +- (half_width + m) s)            m = margin (VIS-6)
targets T1 at P': centre, head
eyes   E1:        e0 + V.vel * L,  e0 + k s,  e0 - k s                        k = peek offset (VIS-6)
segments:         e0 -> T0 (5), e0 -> T1 (2), each eye of E1 -> {centre, head} at P (6)   = 13 at most
visible(V, E) = any segment is clear in the occluder BVH
```

- The segments run in a fixed priority order. The order starts with the segment that was clear last time (cached
  per pair), so a pair that stays visible costs about 1 segment. A hidden pair costs all 13.
- The test is per ordered pair (V sees E). It is not assumed symmetric.
- The height values are DeCombat defaults (VIS-4). The box is the character's stand box (DSV-27). A crouched box is not
  used; the stand box is the larger one, which is the safe direction.

### 3.4 Lookahead, margin, exposure window: how pop-in is hidden

Pop-in happens when the honest client should already draw E but has no recent data for E. That client draws the
world from its own eye now, and draws remote players at a past position (2009: unknown smoothing; Rust: CLI-11
`now - delay`, 50-250 ms). The server decides from a viewer position that is half a round trip old. So the server
must start streaming E **before** the line of sight opens:

```
L = clamp(rtt(V) + client_delay(V) + 2 * tick + test_interval(pair), L_min, L_max)        (VIS-5)
client_delay = 150 ms for a 2009 client (unknown smoothing, assumed), 250 ms (CLI-11 maximum) for a DeCombat client
L_min = 100 ms, L_max = 500 ms
```

Then:

- **Lookahead targets and eyes** (3.3) cover straight-line motion of both sides over L.
- **Peek offset** `k = min(run_speed * L, 64 units)` covers a strafe out from behind a corner, the usual peek. The
  lateral direction `s` is perpendicular to the line of sight, which is the direction that opens a corner.
  `run_speed` comes from the ROM movement parameters through `dc-sim` when loaded, else 1500 units/s as the
  plausibility bound of DSV-14.
- **Margin** `m` (16 units) widens the box sideways. It hides box approximation errors and the sampling gaps between
  targets.
- **Exposure window** `W = 1000 ms` (VIS-7): a pair that tested visible stays visible for W after its last positive
  test. A pair turns hidden only after W of continuous negative tests. This is hysteresis: no flicker at corners, no
  park / show churn (section 5). It also keeps the client's interpolation buffer full during short breaks in the
  line of sight (a pillar between two players).
- **Fail-open** (VIS-8): a pair that was not tested for `max_test_age = 100 ms` (budget exhausted, a hitch) counts as
  visible.

Remaining false-hidden risks, and what covers each:

| risk | cover | residual |
|---|---|---|
| sudden acceleration (jump pad, fall off a ledge, teleport) | velocity lookahead plus peek offset; a server teleport (0xa0) of E forces a re-test in the same tick | short pop-in if the new position is visible and the viewer's interpolation is shorter than L; measured by the harness (8.4) |
| transparent geometry that is opaque in the occluder set | 3.2 exclusions, census, operator list | glass in the level geometry; the census decides |
| a small hole between targets (a narrow slit) | 5 targets plus margin plus 13 segments | a slit narrower than the sample spacing can hide a sliver of the enemy; accepted (a sliver behind a slit is also hard to see for a human) |
| viewer camera not at the eye (3rd-person death camera, killcam) | section 6.3 rules for dead viewers | depends on V-VIS-9 |
| high latency spikes | L is clamped to L_max; above it the window W and fail-open still apply | pop-in possible above about 500 ms one-way delay; at that latency the game is unplayable anyway |
| scope zoom | none needed: occlusion does not depend on field of view | none |

The design deliberately does **not** cull by field of view. Turning 180 degrees takes one mouse movement, and the
server sees the view direction late. A rear-cone rule would cause pop-in on every fast turn.

### 3.5 Scheduling and the CPU budget

Per tick, before `stream_for`:

1. Build the list of enemy pairs (6.1) and their deadlines. A visible pair is re-tested every tick, cheap thanks to the
   cached segment. A hidden pair is re-tested every second tick (15 Hz at 30 Hz, every tick at 10 Hz). A pair whose
   enemy got a server teleport, a spawn or an event (section 4) is re-tested now.
2. Run the tests in priority order (deadline first, then distance between eye and enemy, nearest first) until the
   per-tick segment budget `vis.max_segments_per_tick` is used up.
3. Pairs left over keep their state; one older than `max_test_age` turns visible (fail-open).

Cost model, at the measured 1.3 µs per segment (Appendix M; segment counts from 3.3):

| scenario | enemy pairs | hidden share h | segments per second | CPU at 1.3 µs |
|---|---|---|---|---|
| 8v8 TDM at 30 Hz | 128 | 0.8 (79 % of spawn-to-spawn segments are blocked, Appendix M; used as a proxy) | 128 x (0.8 x 13 x 15 + 0.2 x 1.5 x 30) = 21,100 | 27 ms/s = **2.7 % of a core** |
| 16-player FFA at 30 Hz | 240 | 0.8 | 39,600 | 51 ms/s = 5.1 % |
| 8v8 at 10 Hz (CD-63 default) | 128 | 0.8 | 7,000 | 0.9 % |
| worst: 16 FFA, all hidden, every tick at 30 Hz | 240 | 1.0 | 93,600 | 12 %, capped by the budget |

With the default budget of 400 segments per tick (12,000 per second at 30 Hz, 1.6 % of a core), the worst case is
capped. The pairs left out fail open: weaker protection, never pop-in. The 30 Hz FFA case exceeds the budget unless
the accelerator (E) or the cheaper hidden-pair schedule is in place. Targets for the implementation (to be measured,
8.4): **average at most 1.5 % of a core for a 16-player room at 30 Hz, worst tick at most 2 ms**, against the measured
0.45 % baseline of the whole server at 2 players.

Ways to get under target if the harness misses it, in order:

1. Fewer segments for hidden pairs: drop E1 eyes that do not move (a still viewer has no ahead-eye) and T1 targets for
   a still enemy. A still pair costs 5 segments, not 13.
2. The cell accelerator E. Most hidden pairs are separated by large structures; an O(1) reject removes them before
   any segment.
3. Lower test rate for distant hidden pairs (every third tick beyond 3000 units). The lookahead grows with the test
   interval, so the margin stays correct.

Bandwidth falls with culling. A character entry is about 240 bits (16 + 4 + 48 + 1 + 72 + 8 + an animation block of
three trackers, `stream::entry_bits`). Fifteen remote players are about 3.6 kbit per tick, close to DSV-17's
4000-bit budget; with 80 % hidden it is about 0.7 kbit.

## 4. Information that leaks besides the stream

Culling the stream is worthless if other messages carry the same positions. What the Rust server sends today
(`crates/dc-dserver/src/game.rs`, `gs.rs`) and what each item reveals:

| message | sent today to | position content | policy (VIS-10, VIS-12) |
|---|---|---|---|
| effect type 0x19 weapon sound (relay of 0x80) | every introduced player, sender included (CV2-3) | exact weapon position (vec96) and the player id | sent only to viewers within `vis.hear_radius_weapon` (3000 units), in line with C2S §4.3's "relay to the other clients near the player" and the 2009 engine's own distance cut for positional sounds (B §8.2). Each such viewer gets the shooter **event-exposed** (below) |
| effect type 0x19 `k` 0x0f landing | everyone | exact landing position | within `vis.hear_radius_move` (1000 units), event exposure |
| effect type 0x19 `k` 0x12 throw sound | everyone | thrower position | as weapon sound |
| effect type 0x06 shot effect (relay of 0x82) | everyone (CV2-3) | `from` = muzzle, `to`, shooter object | as weapon sound: hearing radius, event exposure |
| projectile object (kind 1, type 0x13 blob) | everyone (tdm2 §3.4) | its own flight, starting at the hand | not culled (a projectile is a world object); its start point reveals the thrower once, the same as the throw sound |
| `CF_FILES` weapon change on the weapon child (0x7f relay) | everyone | none (which weapon only) | unchanged in W1-W3; deferring it until exposure is a later option |
| `GS_KILL` 0x6d | everyone | none: pids, weapon, zone, flags (S2C §3.3.1) | unchanged: the kill feed is information every honest player has |
| `GS_DAMAGE_TAKEN` 0x74 to the victim | the victim | `from_dir` (direction of the attacker) | unchanged: the victim's own damage indicator. The attacker is event-exposed to the victim for W_event, because the victim may turn and see the attacker |
| 0x84 radar object on (scanner relay, C2S §7.3) | the scanning player's team | none itself, but the 2009 radar draws the object at its client-side position | while a radar mark is on, the object is exposed to that team (the radar needs the position) |
| 0xa2 ping table, 0x6b roster, 0x4a / 0xb5 scores | everyone | none | unchanged |
| 0xa6 notes to characters | (the server sends 0x11 / 0x1b) | box only | unchanged |
| corpse (character in states 2 / 3) | the stream | the death position | dead characters are not culled (static, VIS-2) |

**Event exposure** (VIS-10). When a relay that names or locates E goes to viewer V while E is hidden for V, the
server first shows E to V (the show record of 5.2, reliable, in the same tick's id 8, which leaves before the
unreliable block; B §12.2 items 3-4). Then it queues the relay, and keeps the pair visible for
`W_event = 500 ms`. The client therefore never plays a sound or effect for a character it has parked at the park
point. Two client behaviours are unknown here: whether the 2009 client places these effects at the wire position or
at the named character, and whether it draws the shooter's muzzle flash on the character (V-VIS-4). Event exposure
is right in both cases. A cheater learns positions only while the enemy makes noise within earshot. An honest player
gets roughly the same from the sound's direction.

**Footsteps** (VIS-12). tdm2 shows no server relay for steps: only landings were relayed (tdm2 §3.5, 13 relays for 16
landings, none for walking). Weapon sounds reach other players only because the shooter's client reports its own
weapon model's animation-key sounds (0x80, C2S §4.3). Sounds on the character model's own animation keys are PLAUSIBLY
played by each client from the remote character's streamed animation trackers (CV2-2); not traced. If the 2009 client plays remote footsteps itself from the streamed motion and
animation, a parked enemy goes silent. Honest players would lose footsteps through walls, a real gameplay change.
Until capture V-VIS-5 settles it, the default exposes a **moving** enemy (horizontal speed above
`vis.footstep_min_speed`, 0.6 x run speed) to viewers within `vis.footstep_radius` (800 units). It is switched off if
the capture shows that the 2009 client plays no remote footsteps.

**Hit claims** (VIS-14). Culling does not stop a modified client from claiming hits by object id (the 2009 client
reports `hit_obj`, C2S §4.1). A reported hit on an enemy that was parked for the shooter at the shot time (with a
200 ms grace) is rejected and logged. That is not a replacement for the server-side shot trace the slice-4 list
plans, but it closes the cheapest hole culling opens: "shoot the id you were never sent".

## 5. How an unstreamed enemy is represented to the 2009 client

### 5.1 Options

| option | wire | what the client does (from the specs) | artefacts | info to a cheater | cost |
|---|---|---|---|---|---|
| a. stop streaming only | no id 10 entries | object stays at the last sample, PLAUSIBLY extrapolating with its last velocity (2.4) | **a visible ghost**: a statue at the last-seen spot, or a body sliding through walls. Another viewer angle later shows it while the real enemy is elsewhere. Remote characters may also block the local mover (unknown) | last seen position only | 0 |
| b. freeze plus hide by flag | reliable change: `CF_FLAGS` clearing bit 0x0001 (PLAUSIBLY "visible", tdm2 §1.4) plus `CF_POS` with velocity 0 | unknown: the meaning of flag 0x0001 is not traced (tdm2 §9 item 4); whether the weapon child, shadow and name tag follow is unknown; whether the local mover still collides with it is unknown | none if the flag means "visible"; otherwise as (a) | last seen position | about 300 bits per hide / show |
| c. **park**: snap to a hidden point | reliable change: `CF_POS_SNAP` with the park point and velocity 0; id 10 entries stop | the object is placed at the park point. `CF_POS_SNAP` is the bit the reader sets itself on create, PLAUSIBLY "place without smoothing" (B §4.1, §13.2 item 4); the weapon child follows by attachment (B §4.2 item 8) | none if the snap does not smooth. If it smooths, the slide happens while the enemy is behind a wall (hide) or before the line of sight opens (show; the lookahead L covers a smoothing time shorter than L) | nothing ("hidden") | about 184 bits per hide / show (record head 32 + cf 16 + id 16 + pos48 + vec72) |
| d. removal plus re-create | kind 3 removal of character plus weapon child; later the full create (type 0x18 blob of 580 bits, model blocks, attachment), the join refresh (`CF_FILES`, flags, 0xa6 sub 0x11 / 0x1b) | the client tears the object down; re-create replaces. The roster binding by object (0x6b), the HUD lists, the kill feed's character lookups and spawn latches (0xa6 0x1f / 0x25) may reset or misfire; the client asked 0x8c when 0x6b named a missing object (tdm2 §2.3) | probable: spawn effects, roster churn, model load hitches (preload, B §8.7) | nothing | about 2-3 kbit per show, per viewer |

### 5.2 Proposal: park with snap (option c), flag hide (b) as a later refinement

Default `vis.hide_mode = "park"` (VIS-9):

- **Hide** (pair turns hidden, after the exposure window): in the tick's id 8 group for V, one change record of the
  character: `cf = CF_POS_SNAP` (0x0200, written with the high-byte marker; B §3.2), position = park point,
  velocity 0. (Implemented as `cf` 0x0202 = `CF_POS | CF_POS_SNAP`, the bits of the accepted spawn record; section 10.3.) Stream entries of E to V stop. The weapon child is attached and gets no record.
- **Park refresh**: once, `2 x max(L, 200) ms` after the hide, the same record again. Unreliable messages wait at most
  `2 x max(L, 200)` ms in the sender queue before they are dropped (B §12.2 item 5, transport 11.4). So after that
  time no stream entry with E's true position from before the hide can still leave the server. The refresh puts E
  back at the park point if a late entry overtook the hide record on the wire (B §12.2 item 3).
- **Show** (pair turns visible): in the same tick's id 8 group, `cf = CF_POS_SNAP` with E's true position and
  velocity. Then E's stream entry (with `yaw8` and the animation block, CV2-2) in the same tick's id 10, which leaves
  after the reliable block. The record must never use `CF_NEW` (that would be a create) and never touch `CF_FILES` or
  `CF_ATTACH`.
- **Park point** (VIS-9): a point on the bottom face of the world box (the `pos48` box, `crates/dc-dserver/src/objects.rs`
  `PosBox`) that the census (W4) finds blocked from every sampled standing position. Appendix M: the four bottom
  corners of the world box are blocked from every spawn eye in 20 of 21 worlds; in the remaining world 2 of 106 spawn
  eyes see the best corner, so the census must search the bottom face, not only the corners. The point is
  per world and computed at load. It is never written into the tree.
- Mode `"flag"` adds the `CF_FLAGS` clear / restore to the same records once V-VIS-3 proves the flag meaning. Mode
  `"remove"` exists only for the V-VIS-8 experiment.

Why this choice: parking needs only record forms the 2009 client already accepted (change records with `CF_POS` /
`CF_POS_SNAP` on characters; tdm2 §1.4 shows `CF_POS_SNAP` in a character's change record at seq 753). It reveals
nothing to a cheater beyond "hidden". It leaves no statue, and it keeps the object, the roster binding and every HUD
reference intact.

### 5.3 Capture items to verify (for the coordinator; need the user's GO)

| id | question | how (one `dc-dserver` test switch, to be added in W2: `-vis-test-park <secs>` toggles a bot's park / show while the bot stands in the open) | pass condition |
|---|---|---|---|
| V-VIS-1 | does `CF_POS_SNAP` place a remote character without a visible slide? | user watches the bot; screen recording; dc-botclient confirms the records on the wire | the bot vanishes and reappears in place, no motion trail |
| V-VIS-2 | without a park record, does an unstreamed character freeze or keep sliding? | `-vis-test-mute <secs>` stops the stream for a walking bot | answer recorded (decides how bad a lost hide record is) |
| V-VIS-3 | does clearing flag 0x0001 hide the character, its weapon child and its name tag, and stop local collision? | `-vis-test-flag` | all hidden, the user can walk through the spot |
| V-VIS-4 | are type 0x19 / 0x06 effects placed at the wire position or at the named character? Is a muzzle flash drawn on the character? | parked bot fires (event exposure off in this test switch) | answer recorded |
| V-VIS-5 | does the real client play footsteps of a walking remote character? | culling off, the bot walks behind a wall; user listens | answer decides VIS-12's default |
| V-VIS-6 | does an unsilenced shot put the enemy on the 2009 radar without a server 0x84? | bot fires out of sight | answer decides whether shots need radar-length exposure |
| V-VIS-7 | death camera of the 2009 client in states 2 / 3: own body, killer, or a teammate? | the user dies to a bot | answer decides VIS-11's killer exposure |
| V-VIS-8 | removal plus re-create of a live character: roster, scoreboard and kill feed intact? spawn blink? 0x8c queries? | `-vis-test-recreate` | answer recorded (mode `"remove"` stays off either way) |
| V-VIS-9 | does the 2009 client ever report a hit on a character behind a wall (penetration)? | user shoots a bot through a thin wall with culling off | answer decides whether thin occluders need an exposure rule |

## 6. Interactions

### 6.1 Who is an enemy, what is never culled (VIS-2)

- Culled: the characters (and, through attachment, the weapon child) of living players of another team. In FFA mode
  classes, every other player.
- Never culled: teammates (team radar and markers draw them), the viewer's own object (not streamed anyway, DSV-21 /
  DSV-23), dead characters (static corpses until respawn), pickups, projectiles, world objects. Bots are players and
  follow the same rules.
- At respawn, a character becomes alive again and is tested in the same tick. The respawn teleport makes the corpse
  vanish for every viewer anyway, so a park at that moment adds no artefact.

### 6.2 Modded servers

- The rule is a `dc-server-logic` default (HOSTING §3.1) with config key `vis.enabled` (default true). A wallhack or
  x-ray server sets it false in its server-rule mod or its `server.toml`. Everyone on that server gets everything,
  as the 2009 server sent.
- Hook `dserver.visibility_policy` (VIS-1) runs once at match start. It returns `off`, `on` or a table of the VIS
  parameters, so a mode mod can, for example, make one team see through walls in its own mode. A per-pair Lua hook is
  not offered: 240 pairs x 30 Hz would put Lua on the hot path.
- Server list: the server-list JSON entry (HOSTING §5) gains `"vis_culling": true|false`. A vanilla-policy server
  with culling off is listed with a note, so players can tell.

### 6.3 Spectators, observers, dead players (VIS-11)

- Player state 6 (spectator, S2C §4.1) and accounts in observer mode (CD-43, OPS-6): full stream (`vis.spectator =
  "full"`). They are not playing. A spectator who is also on a playing team gets the `"team"` view below, so
  spectating does not turn into a wallhack for that team.
- Dead or waiting players (states 2 / 3): the union of what the living teammates see (`"team"`), plus the killer for
  `dying_ms` if V-VIS-7 shows the death camera looks at the killer. The union is information the team could pass on
  by voice anyway. In FFA the dead player keeps its own last eye position plus the killer.
- Moderators with remote console rights (CD-64) get no special stream unless they are in observer mode.

### 6.4 The Rust client (CLI-11) and other clients

- The lookahead includes the client's interpolation delay (3.4). For a DeCombat client (identified by its mod-set
  hash login, HOSTING §4 item 4) the server assumes CLI-11's maximum of 250 ms. For a 2009 client it assumes 150 ms
  (its smoothing is not read).
- Client rule for `dc-client` (VIS-13): a `CF_POS_SNAP` change record is a teleport. The interpolation buffer of that
  object is reset, nothing is interpolated across the snap, and the object is drawn from the next sample. An object
  at the park point is skipped by the renderer (cheap; the park point is announced to nobody, but the client may
  treat "outside the playable volume" as hidden).
- `dc-botclient` and `synth_client` read the same records. Their seen-object tables must apply a snap as a jump. The
  bot's target choice must not keep a parked enemy as a target (test in W2).

### 6.5 Other dserver rules

- DSV-16 ("out of enemy sight" spawn choice by distance) can use the same visibility module later, as a separate
  proposal.
- DSV-17 / CD-65: culled entries simply do not enter `stream_for`'s list. The budget order is unchanged.
- DSV-21: the stale-velocity rule stays; a parked object gets velocity 0 by construction.
- The planned server-side shot trace (slice-4 list) needs a nearest-hit segment query against the world. It should
  use the same BVH crate (W1) with a nearest-hit query, rather than `CollisionWorld::ray`, for the cost reasons of 2.2.

## 7. Configuration and DeCombat rows

Config keys (`server.toml`, section `[vis]`; staged rows VIS-1 .. VIS-14 in
`staging/visibility-design_DC_DEFAULTS.md`):

| key | default | row |
|---|---|---|
| `vis.enabled` | true (vanilla) | VIS-1 |
| `vis.hide_mode` | `"park"` (`"flag"`, `"remove"` experimental) | VIS-9 |
| `vis.eye_height_frac`, `vis.head_frac`, `vis.low_frac` | 0.75, 0.9, -0.6 of the half-height above the box centre | VIS-4 |
| `vis.margin_units`, `vis.peek_max_units` | 16, 64 | VIS-6 |
| `vis.client_delay_ms_2009`, `vis.client_delay_ms_dc`, `vis.lookahead_min_ms`, `vis.lookahead_max_ms` | 150, 250, 100, 500 | VIS-5 |
| `vis.exposure_ms`, `vis.event_exposure_ms` | 1000, 500 | VIS-7, VIS-10 |
| `vis.max_segments_per_tick`, `vis.max_test_age_ms`, `vis.hidden_test_every_ticks` | 400, 100, 2 | VIS-8 |
| `vis.hear_radius_weapon`, `vis.hear_radius_move` | 3000, 1000 units | VIS-10 |
| `vis.footstep_radius`, `vis.footstep_min_speed_frac` | 800 units, 0.6 | VIS-12 |
| `vis.spectator` | `"full"` for state 6 / observers, `"team"` for dead players | VIS-11 |
| `vis.transparent_models` | empty (per world hash, operator file) | VIS-3 |
| `vis.reject_parked_hits_ms` | 200 | VIS-14 |
| `vis.cell_accel` | false | (W5; needs its own row once measured) |

All distances are world units; all times milliseconds. None of the values is ROM data or an reference stack value.

## 8. Implementation plan

### 8.1 Module boundaries

```
crates/dc-vis (new, pure, no I/O, no_std-friendly like dc-sim)
  occluders.rs   OccluderSet: triangles from dc-world (feature "world": VIS-3 selection) or from caller polygons
  bvh.rs         Bvh::build, occluded(p, e) any-hit front-face, nearest(p, e) for the shot trace
  pair.rs        PairState (visible_until, last_test, cached_segment, parked), PairTest (3.3 sample sets)
  sched.rs       per-tick scheduler with the segment budget and fail-open (3.5)
  census.rs      park point search, occluder statistics, optional cell accelerator (W5)
crates/dc-dserver
  game/vis.rs    policy glue: enemy pairs from teams and life states, event exposure, spectator / dead rules,
                 hit-claim check; calls dc-vis once per tick from Game::tick (before stream_for)
  game/relay.rs  stream_for skips pairs that are hidden for the viewer
  objects.rs / modelobj.rs   park / show change record writer (CF_POS_SNAP, pos48 + vec72)
  session.rs     queues the hide / show records in the tick's id 8 before the id 10 message
  cli.rs, config  [vis] keys; test switches -vis-off, -vis-test-park, -vis-test-mute, -vis-test-flag, -vis-test-recreate
crates/dc-xtask (or a dc-vis example)   `vis-bench --rom`: the Appendix M measurement as a tool (ROM-gated)
```

`dc-vis` does not depend on `dc-dserver` and can be shared by the shot trace and, later, by `dc-client` (for example
for a spectator camera). `dc-dserver` gains dependencies on `dc-vis` and, through its `world` feature, on dc-world
data it already parses.

### 8.2 Waves

| wave | content | tests | size |
|---|---|---|---|
| W1 | `dc-vis` crate: OccluderSet (VIS-3 selection), BVH, any-hit and nearest-hit, front-face rule; `vis-bench` over the ROM | unit tests on synthetic polygons (a wall, a window gap, a back-facing wall, a slit); property test against `dc_sim::CollisionWorld::ray` on random segments in synthetic worlds; ROM-gated agreement test (at least 99.8 % agreement, every disagreement in the "visible" direction) | 1 Opus worker, about 1 day; about 900 lines with tests |
| W2 | `game/vis.rs` pair test plus scheduler, park / show records, `stream_for` filter, config keys, test switches | synthetic world from `dc-world::synth` (two rooms joined by a corridor with a corner and a window); two or three `synth_client`s with scripted paths. Asserts: no stream entry while hidden, hide record after W, show record before the corridor corner opens (with RTT 0 / 100 / 200 ms injected), park refresh timing, fail-open when the budget is 0; dc-botclient reads the records without a record error | 1 Opus worker, about 1.5 days; after dserver-slice4 merges (same files) |
| W3 | event exposure (0x19, 0x06, throw, 0x74, 0x84), hearing radii, footstep exposure, dead / spectator rules, hit-claim rejection | per-relay tests: a hidden shooter fires, the viewer gets show then relay in that order; a viewer out of earshot gets nothing; a dead viewer gets the team union; a hit on a parked id is rejected | 1 Sonnet worker with this document as recipe, about 1 day |
| W4 | measurement harness: CPU per tick and per pair; segments per tick; bandwidth; **pop-in reference** (8.4); exposure ratio; ROM census of park points and occluder statistics | harness runs in CI without ROM on synthetic worlds; ROM-gated runs report numbers only (no names) | 1 Opus worker, about 1 day |
| W5 | live capture round V-VIS-1 .. V-VIS-9 (user GO), then tuning; optional accelerators E (cell reject) and F (solid-leaf trace, needs the sentinel proof) | capture report; accelerator agreement census with zero disagreement on every ROM world before `vis.cell_accel` may default on | coordinator plus 1 Opus worker |

Order: W1 can start at once (new crate, no conflicts). W2 waits for the dserver-slice4 merge. W3 needs W2. W4 can run
alongside W3. W5 needs the user.

### 8.3 Synthetic test worlds

`dc-world::synth::SynthWorld` writes version-85 files with BSP models from caller polygons
(`crates/dc-world/src/synth.rs`). `dc_sim::build_bsp` builds BSP nodes for convex polygons
(`crates/dc-sim/src/collide.rs`). The W2 world: a 2048 x 512 x 2048 box, an L-shaped corridor (corner peek), a
wall with a 64-unit window (sight through a hole), a pillar (short break of sight), a one-sided polygon seen from
behind (must not occlude), a translucent world model (must not occlude) and a blocker polygon across an open hall
(must not occlude). All of it is made up; nothing comes from the ROM.

### 8.4 Pop-in reference and acceptance

The harness keeps the true position history of every player at server tick resolution. It replays each synthetic
viewer's frames at 60 Hz, applying the injected one-way delays and the viewer's interpolation delay (CLI-11 for the
DeCombat client model, 150 ms for the 2009 model):

- `truth(f)`: some segment from the viewer's true eye at frame f to the enemy's true box at `f - delay` is clear in
  the occluder BVH (the same 5 targets, no margin, no lookahead).
- `shown(f)`: on that viewer's synthetic client, the enemy is not parked, and its position is within 32 units of the
  true position at `f - delay`.
- pop-in event: `truth(f) && !shown(f)`.

Acceptance: 0 pop-in events in the scripted scenarios (corner peek, window, pillar, jump over a crate, respawn in view)
at one-way delays 0, 50 and 100 ms. Below 1 pop-in frame per 10,000 visible frames in 10-minute random walks with 16
synthetic clients. CPU within 3.5's targets. The exposure ratio (time an enemy is streamed while `truth` is false,
divided by the time `truth` is false) is reported. It measures how much a cheater still gets. No target for it in W4.

## 9. Open questions

1. Footsteps (V-VIS-5) and radar on shots (V-VIS-6) decide whether VIS-12 and a radar exposure stay on. They are the
   two places where culling could change honest gameplay.
2. Glass inside the level geometry: no known flag tells it apart. The census (W4) compares the occluder set with the
   translucent render sections; whether that comparison is possible at polygon level is not checked.
3. Movers (doors) are not simulated by `dc-dserver`; they are excluded from the occluder set. Closed doors therefore
   never hide anyone. When movers are simulated, a dynamic occluder layer is needed.
4. The meaning of the BSP sentinels (MOVEMENT_2009 §16.1 item 5) gates accelerator F.
5. Whether the 2009 client's own smoothing of remote characters is longer than 150 ms (VIS-5's assumption) is not
   read. The pop-in reference uses the assumption; V-VIS-1's recording can measure it.
6. Weapon penetration (V-VIS-9): if the 2009 client reports hits through thin walls, thin occluders need a rule, or the
   hit-claim check (VIS-14) would reject honest wallbangs.
7. 16-player server cost without culling is not measured (only the 2-player 0.45 % figure); the W4 harness measures
   both.

## 10. Implementation record (dserver-visibility, merged at master 7fbad44; folded by spec-fold-7, 2026-10-25)

Source: `reports/dserver-visibility.md` (worker dserver-visibility, branch `dserver-visibility`, head f61327d) and its staged row corrections
`staging/dserver-visibility_DC_DEFAULTS.md` (PROPOSED). Statuses per CLEANROOM section 6: implemented, merged (master 7fbad44), unit-tested,
session-tested (deterministic), ROM-gated tested, run headless against `dc-botclient`; **not** tested against the real 2009 client. The numbers below are the worker's
measurements on this machine (rule values are DeCombat defaults, VIS rows proposed; nothing from the reference stack).

### 10.1 What was built

- `crates/dc-dserver/src/visibility.rs` (with hooks in `session.rs`, `world.rs`, `lib.rs`, `runner.rs`, `cli.rs`), not a separate crate (8.1 had proposed one); the occluder
  BVH is built once per world in `world_from_bytes` and shared.
- Occluders (VIS-3): the level model and the visible, opaque `PlainWorldModel` brushes; collision polygons only (surface bit 0), triangulated fans, front faces only;
  median-split BVH, 4 triangles per leaf, Moller-Trumbore any-hit, hits within 0.05 units of either end ignored.
- Actors per tick (before the stream): enemies = the other team (class 6: everyone else). Culled: alive, joining and loading characters. Never culled: teammates, the
  own object, corpses (states 2 / 3), projectiles, world objects.
- Pair test (VIS-4 .. VIS-6): eye = centre + 0.75 h to centre / head (+0.9 h) / low (-0.6 h) / both side edges (+-(w + 16)); two lookahead targets along the enemy's
  velocity; the viewer's ahead-eye and two peek eyes (+-min(1500 x L, 64)) to centre and head; L = rtt + 150 + 2 x tick + re-test interval, clamped 100 .. 500 ms; the
  last clear segment first.
- Schedule (VIS-7 / VIS-8): visible if within the always-visible radius, or a sound of the enemy within 500 ms, or a clear segment within the 1000 ms window, or not
  tested for 100 ms (fail-open); visible pairs re-tested every tick, hidden pairs after 60 ms; most overdue first, then nearest; at most 1000 segments per tick.
- Dead viewers (VIS-11): a viewer that is not alive sees what its living teammates see plus its own last eye (FFA: its own last eye only).
- Records (VIS-9): one reliable `S2C_OBJ_GROUP` per viewer before the tick's stream. Hide = `cf` 0x0202 (`CF_POS | CF_POS_SNAP`, two-byte head) with the park point and
  velocity 0, the enemy's stream entries to that viewer stop, the same record once more after `2 x max(L, 200)` ms; show = `cf` 0x0202 with the true position and
  velocity, then the stream entry in the same tick. Form and evidence: `specs/UDP_SESSION_2009_B.md` 4.2 (DeCombat paragraph).
- Spawn hook: the spawn change record of an enemy (and of its weapon child) carries the park point for each viewer that cannot see the spawn (full test at that moment).
- Event hook (VIS-10, exposure only): `UP_FIRE`, the weapon-sound report and the `UP_*_EVENT`s first show their sender to every viewer that has it parked and keep it
  visible 500 ms; then the relays go to everyone, unculled.
- Park point: a 5 x 5 grid on the bottom face of the `pos48` box, corners first, looked at from every spawn point (+30 / +90 units); the point seen least wins.
- Configuration: `-visibility on|off`, `-visibility-window <ms>`, `-visibility-radius <units>`; dc-config `[vis]` (`vis.enabled`, `vis.exposure_ms`, `vis.max_segments_per_tick`,
  `vis.always_radius`); a start-up log line.

### 10.2 Measured numbers

| measurement | result |
|---|---|
| unit tests (`cargo test -p dc-dserver --lib visibility`) | 9 passed (BVH faces, window, 3000 random segments over 3200 triangles agree with brute force, park / refresh timing, teammates / FFA / corpse / radius / event, dead-viewer union, budget 0 fails open, culling off, spawn decision, lookahead, record layouts, park census) |
| session tests (`--test vis_sim`) | 4 passed: an enemy behind the wall is parked and never streamed (culling off: > 30 entries each way); stepping into view snaps back and then streams (shown at x = 30 with the wall face at x = -8, before the line of sight opened); an enemy walking into cover is parked one window after the last sight (last clear 2935 ms, park 3950 ms: +1015 ms); teammates are streamed through the wall |
| occluder census vs `dc_sim::CollisionWorld::ray` (21 ROM worlds, 97,720 segments) | 99.990 % agreement; 0 "BVH blocked, ray clear", 10 "ray blocked, BVH clear" (one world; the visible direction); mean BVH 1.25 us, ray 55.2 us |
| 8v8 at 30 Hz, 20 s, the ROM world with the most occluder triangles (86,291), release, i686 | mean 869.4 us/tick, max 1319.1 us, 827.2 segments/tick (cap 1000), 1.05 us/segment, 2.61 % of a core; 0 fail-open verdicts; 122 of 128 enemy pairs parked at the end |
| same, x86_64 | mean 838.7 us/tick, max 1434.0 us, 1.01 us/segment, 2.52 % of a core |
| design estimate (3.5) | about 1.3 us/segment and 2.7 % of a core: measured 1.0-1.4 us and 2.5-2.6 % |
| workspace tests at f61327d | 941 passed, 0 failed (i686 and x86_64); scratch merge with master 58cfa72: 971 passed |

The synthetic clients of the cost test walk through walls (the server does not collide them), so the 95 % hidden share is a property of that motion, not of play.

Live headless rounds (dc-lobby-server + release dc-dserver + two `dc-botclient --shooter --hunt`, goal 3, 300 s; no window, no real client):

| round | culling | kills | round end | culling statistics (last dserver log line) |
|---|---|---|---|---|
| off | off | 5 | goal 3:2 at 105.8 s | culling off |
| on | radius 800 | **0** | none (300 s) | 9006 ticks, mean 34.9 us, max 229.9 us, 12.7 segments/tick, 6 parks, 2 refreshes, 2 shows |
| on_r1200 | radius 1200 | 0 | none | mean 36.9 us, 6 parks, 2 shows |
| on_r2000 (exploratory) | radius 2000 | 5 | goal 3:2 at 88.6 s | mean 8.1 us, 1.0 segments/tick, 6 parks, 4 shows |

With culling on both bots took the parked enemy at the park point as their target and hunted it for the round (156 `hunt_stuck` events each; off: 7 and 3), so they
never met and never fired: VIS-13 was missing on the bot side (6.4). In every round the bots' `fatal`, `integrity` and `parse_notes` were empty: the `cf` 0x0202 records
parse in `dc-botclient`. "Bots still fight with culling on" was therefore **not** shown for the default radius by this worker.

**Follow-up merged after the fold base (bot-vis13, master 765c756; report `reports/bot-vis13.md`; BOT-19 (proposed, PENDING in DC_DEFAULTS)):**
`dc-botclient` treats an enemy as unseen when its decoded position sits on or below the world box's bottom face (+2 units: the park point) or it has had no stream
entry for 1500 ms; unseen enemies are never targeted, aimed at or walked toward, and with no visible target the bot patrols to the last-seen or first-seen position.
Live (same stack, goal 5, 300 s): culling on **9 kills** (4 + 5), 33 `target_unseen` (all "parked"), 70 `hunt_stuck`; culling off 8 kills, 22 `hunt_stuck`. So with
VIS-13 on the bot side the bots do fight with culling on at the default radius (RUN, our stack only). Open there: patrol gets stuck on geometry (61 `hunt_stuck` for one
bot), the silent-stream path never fired live, the bot does not know the spawn points. `dc-client` has no VIS-13 rule yet (it would draw a parked enemy at the park point
outside the playable volume, and its interpolation runs across a snap: CLI-11).

### 10.3 Where the implementation differs from this design

| place | design (sections 3-7, VIS rows) | implementation | why |
|---|---|---|---|
| budget (VIS-8, 3.5) | 400 segments per tick | **1000** (`vis.max_segments_per_tick`) | the design's own 8v8 estimate is 704 per tick at 30 Hz (128 pairs x (0.8 x 13 x 15 + 0.2 x 1.5 x 30) = 21,100 per second); measured 827 per tick with 95 % hidden; at 400 hidden pairs would miss the 100 ms fail-open age and flicker open; worst tick 1.3-1.4 ms (target 2 ms) |
| record (VIS-9, 5.2) | `cf` = `CF_POS_SNAP` (0x0200) | **`cf` 0x0202** (`CF_POS` + `CF_POS_SNAP`) | the spawn change record 0x468a of the bot-only reference run carries both bits and the reader tests 0x0202 (B 4.2 item 6). Evidence note (spec-fold-7): the real client accepted `CF_POS_SNAP` change records with the position block only for its **own** character (B 4.2, DeCombat paragraph); 5.2's "tdm2 §1.4 seq 753" was a record to Bot1's link, not to the real client |
| first park (VIS-7) | hidden after the exposure window | a never-seen pair (new pair, respawned enemy) parks at its **first negative test, without a window**; the window counts from the last clear test | otherwise every respawn would stream the spawn position for a full window |
| spawn record (not in the design) | the spawn record goes to everyone with the true position | the spawn change record (and the weapon child's) carries the park point for viewers that cannot see the spawn | a respawn does not reveal the spawn point |
| radius (VIS-12) | footstep exposure: enemies faster than 0.6 x run speed within 800 units | an **unconditional always-visible radius** of 800 units between the bodies, **no speed condition** (`vis.always_radius`, `-visibility-radius`) | footsteps are not culled; errs towards "visible" |
| hearing (VIS-10) | relays only to enemies within 3000 / 1000 units | **no hearing radii**: every relay still goes to everyone; event exposure (show + 500 ms) only | task rule: relays go to everyone |
| eye height (VIS-4) | 0.75 x half height | kept 0.75, with the observed 2009 eye 0.66 (CLI-3') as a 4.5-unit margin towards "visible" | re-derived |
| hidden re-test (VIS-8) | every second tick | after 60 ms (time based: every second tick at 30 Hz, every tick at 10 Hz) | the design's 15 Hz intent |
| client delay (VIS-5) | 150 ms (2009) / 250 ms (DeCombat client) | 150 ms for every client | the server does not identify the client kind yet |
| config | `[visibility]` in places | `[vis]`, keys `vis.*`; command line `-visibility*` | binds VIS-1 / VIS-7 in the dc-config coverage test |
| module (8.1) | a new crate | `dc-dserver/src/visibility.rs` | no dc-sim / dc-world addition was needed |

### 10.4 Not implemented

VIS-14 hit-claim rejection; VIS-10 hearing radii and the 0x74 victim / 0x84 radar exposure; VIS-11 killer exposure during `dying_ms` and the full stream for spectators /
observer accounts (dc-dserver has no spectator state); VIS-3 `vis.transparent_models`; VIS-1 server-list field `vis_culling` and the `dserver.visibility_policy` hook; a re-test
forced by a server teleport (corrections wait for the normal schedule); the 250 ms DeCombat client delay; the `-vis-test-*` capture switches of 5.3; hide / show records for the
weapon child (it follows by attachment, B 4.2 item 8, PLAUSIBLE; only the spawn record moves it); VIS-13 in `dc-client` (done for `dc-botclient` by bot-vis13, 10.2). Known
edges: a joiner's create records place the other players at the joiner's own spawn until the first tick parks them (at most one tick, during the loading screen); on a world
whose `pos48` box is very large the park point quantises to 1 unit.

### 10.5 What the real 2009 client must still show (needs the user's GO; 5.3)

| item | open question for this implementation |
|---|---|
| V-VIS-1 | does `cf` 0x0202 on a live character place it without a slide, and does the attached weapon child follow (10.4)? |
| V-VIS-2 | an unstreamed character freezes or slides (how bad a lost hide record is) |
| V-VIS-3 / V-VIS-8 | flag hide and removal plus re-create (modes not implemented; park only) |
| V-VIS-4 / V-VIS-6 | effects and radar of a parked shooter: event exposure shows him first, so these decide whether that is enough |
| V-VIS-5 | remote footsteps (decides whether the always-visible radius can become the speed rule of VIS-12) |
| V-VIS-7 | death camera (decides the killer exposure of VIS-11) |
| V-VIS-9 | hits through thin walls (decides VIS-14 before it is built) |

None of these has been run; the only reader of the records so far is `dc-botclient`. The row corrections of 10.3 are folded into `DC_DEFAULTS.md` (section
"visibility-design corrections from the implementation", PENDING) on master 7a19faf, which this record was rebased onto.

---

## Appendix M. Measurement (scratch, outside the tree)

What: a scratch Rust program (scratchpad `visbench`, path dependencies on master's `dc-world`, `dc-sim`, `dc-rez`;
nothing committed, nothing built inside `rust/`) loaded all 21 ROM worlds read-only from the user's ROM archive. It
built the `dc-sim` collision world with `WorldFilter { blockers: false, ..default }` and a triangle BVH (median-split,
slab test, Moller-Trumbore any-hit; public algorithms) over the collision polygons (surface bit 0) of the same
selected models. It then tested segments from every player spawn eye (spawn + 70 units up) to five points of every
other spawn (centre + 30, + 75, - 20, +- 20 sideways). It printed counts and times only, world index instead of name.

Machine: the user's Windows 11 machine, single thread, release build. x86_64 via `DC_TARGET=x86_64`; i686 via the
default toolchain.

Results (684,480 segments over 21 worlds):

| measure | value |
|---|---|
| segments blocked | 79.4 % overall; per world 7.9 % .. 91.0 % |
| `dc-sim` `CollisionWorld::ray`, mean per segment (x86_64) | 52.1 µs overall; per-world means 0.8 .. 296 µs; blocked segments 0.9 .. 510 µs mean per world, clear segments 0.3 .. 7.5 µs |
| BVH any-hit, one-sided, mean per segment | 1.23 µs (x86_64, mean of the per-world means), 1.35 µs (i686) |
| BVH worst single segment | 49 µs (x86_64), 48 µs (i686) |
| BVH triangles per world | 16.3 .. 97.0 thousand |
| BVH build time per world | 3 .. 29 ms |
| agreement BVH one-sided vs `dc-sim` ray (blocked or clear) | 99.86 .. 100 % per world |
| agreement BVH double-sided vs `dc-sim` ray | 98.37 .. 100 %; every difference was "BVH blocked, sim clear" (back faces) |
| park candidates: bottom corners of the world box seen from spawn eyes | 0 spawn eyes in 20 of 21 worlds; in one world 2 of 106 spawn eyes see the best corner |

Limits: spawn-to-spawn segments are a proxy for real player positions (spawns cluster at the two team bases, so the
blocked share is an estimate of h in 3.5, not a measurement of play). The scratch BVH is a first cut (median split,
4 triangles per leaf); a SAH build would be faster. Timing includes `Instant::now()` overhead (about 20-30 ns). The
`dc-sim` and BVH sets differ in one respect: the sweep backs off 0.1 units, the BVH ignores hits within 1e-4 of the
segment ends.
