# MOVEMENT_2009 addendum: the edge / vertex sweep and the stair / probe corrections (sim-edges, 2026-10-26)

Staged by the sim-edges worker as corrections to §9.9 / §9.9a / §9.7 / §10.1 / §16.1 of MOVEMENT_2009.md; folded as an addendum file (the main text is not rewritten). Evidence: engine+0x76120 / 0x77530 / 0x75f40 / 0x75520 (sweep), engine+0x78c50 (stair move), engine+0x79210 (ground probe).

# MOVEMENT_2009 addendum (sim-edges): the edge / vertex sweep, the stair landing, the ground probe

Status: staged by the sim-edges worker (2026-10-25) for the next spec-fold. Our words; evidence by RVA in the clean
decompiler tree (`Engine.exe/<rva>.c`) and capstone disassembly of the user's Engine.exe
(DISASM). Implemented in dc-sim on branch `sim-edges`. Grades: PROVEN = visible in the C or the disassembly.

## E1. §9.9 step 4: the edge / vertex sweep is read (engine+0x76120, PROVEN; replaces "not read")

Called by the polygon test (engine+0x77530) for each edge `A -> A + e` whose edge plane the contact point `P`
(the centre at the slab entry) lies outside of by more than 0 and at most that plane's support. Its arguments: the
face plane `(n, dist)`, `A`, `e`, the move `S -> S + d`, the best time so far (initially the face's slab EXIT time)
and the output plane. It lowers the time and replaces the normal when it finds an earlier contact; it returns
false only when the edge cannot be touched at all. The polygon test then: when no edge lowered the exit time, the
face is missed if any edge returned false, else hit at the slab entry with the face normal (unchanged reading).

Steps (r = radius, h = half height of the mover context +0x24 / +0x28; "never" / "always" = -/+ FLT_MAX,
constants at VA 0x675b68 / 0x675b6c):

1. Vertical edge (`|e.x| < 0.001` and `|e.z| < 0.001`): a finite vertical cylinder of radius r around the edge,
   from `min(A.y, A.y + e.y) - h` up by `2h + |e.y|` (point sweep of engine+0x75f40: the side when the hit lies
   strictly inside the height band, else the crossed cap with normal `(0, -1, 0)` from below / `(0, 1, 0)` from
   above; the infinite-cylinder part engine+0x75520 gives the radial normal and t = 0 for a start inside).
2. Otherwise three frames: `w` horizontal and perpendicular to the edge, pointing out of the polygon (`n.xz` for a
   wall, `|n.y| <= 0.001`; else `(-s e.z, 0, s e.x)` with `s = sign(n.y)`, flipped unless
   `dot(cross(e, n), w) > 0`); `u = normalize(e.x, 0, e.z)` with the span `L = dot(e, u)`; `m = normalize(cross(e,
   w))` turned to `m.y >= 0`, the band `h m.y` and its support `h m.y + r |dot(m, u)|`.
3. One interval of times from the constraints: `dot(c, w) - dot(A, w) <= r` (one-sided), `-r <= along u <= L + r`,
   one side of the m-band chosen by `e.z n.x - e.x n.z <= -0.001` (then `offset >= -support`, else `offset <=
   support`; DISASM engine+0x769e3..0x769f9, EDX = the face plane), and the face slab `|dot(c, n) - dist| <=
   support(n)`. For a move parallel to the band plane the lower band bound compares the ALONG-EDGE offset with
   `-support` (as written in the binary, DISASM engine+0x76871..0x76878). Empty, or starting at t >= 1: false.
4. Side contact: when the centre reaches distance r from the vertical plane through the edge (moving toward it),
   after it entered the span `[0, L]` and the band, and the contact point there lies inside the span and inside
   the band (`|c.y - edge height there| <= h`): time of that plane, normal `w`, true.
5. Else the vertex at the end the move ENTERS the span from (A when the move runs along +u, A + e otherwise;
   for a move across the edge, A + e when the along offset is >= 0; DISASM engine+0x76d03..0x76d25): a vertical
   cylinder of radius r around it from `v.y - h - 0.001` up by `2h + 0.002` (engine+0x75f40).
6. A cap against the edge, the top cap when the centre starts above the band plane, else the bottom one: for a
   horizontal edge (`e.y == 0`) the time the cap reaches the edge's height, accepted when the centre is within r of
   the edge's vertical plane and strictly inside the span: normal `(0, +-1, 0)`; for a sloped edge the problem is
   sheared so that the edge becomes vertical and solved with the vertical-cylinder sweep; the normal is the part of
   `c - (closest edge point)` perpendicular to the edge. A cap contact at or after the best time returns true
   without a change.
7. Else, when step 5 found a vertex: true. Else the edge segment against the cylinder AT THE START (a static
   overlap): no overlap -> false; else the edge parameter where it enters becomes the time (as written) and the
   normal is the part of `S - (closest edge point)` perpendicular to the edge.

Consequences (OBSERVED in dc-sim with this rule, synthetic triangles and frame-sized moves of up to 5 units):
99.5 % of random encounters agree with a sampled first-overlap time; the rule has no phantom contacts; it misses a
contact reached past the near vertex while leaving the edge's span (step 5 tests only the entering end), and the
edge planes are judged at the slab entry only (a long move that starts inside the slab and reaches an edge far
from `P` is missed). Moves of 5 units per frame make both rare.

Also in engine+0x77530 (PROVEN): a move parallel to the face (`dot(n, d) == 0`) never hits it; the sloped-face edge
planes are `normalize(cross(edge, k))` with no orientation flip, edges walked from the last corner.

## E2. §9.9a: dc-sim deviations (replaces the bullets)

- Step 1 pre-filter: exact closest point (unchanged, `todo::SPHERE_PREFILTER_EXACT`).
- Step 4: no deviation any more (the conservative "polytope clip" stand-in is gone). The stand-in pushed every
  edge plane out by its full support (up to h = 50 units for the steep in-plane edge planes of a sloped face), so
  bodies stopped next to slanted panels, beams and terrain seams they did not touch: the dc-client edge catches.
- Zero-size sweeps (rays) use the in-plane edge planes on sloped faces (SIM-8 proposed).
- Blocker polygons use the polygon rule, not the capsule solver (SIM-7 proposed).

## E3. §9.7 stair move (engine+0x78c50, PROVEN; corrections)

- Step 1 condition: the probe below hits, the move does not leave the surface (`dot(n, d) <= 0`), `0 < n.y <
  0.7071` (hit normal), and the move projected onto the plane still climbs (`d.y - dot(n, d) n.y > 0`).
- The landing test uses the hit POLYGON's plane normal (`poly->plane` normal y; the hit normal only when no polygon,
  i.e. a blocker hit).
- The steep-landing retry runs from the RAISED START `P0 + (0, step, 0)` to `r2 + dir * dist` with `dir` the
  horizontal UPHILL direction `normalize(-(n.y n.x), 0, -(n.z n.y))` of the landing polygon's plane (the spec said
  downhill), `dist = max(0.5 r, 2 (1 - n_hit.y^2) step + 0.1)`; when that probe is free or hits a polygon with plane
  normal y >= 0.7071 the landing `D` goes on to the progress choice, else `r1`. Implemented (the PLAUSIBLE item of
  9.7 and `todo::STAIR_STEEP_RETRY` are settled).

## E4. §10.1 ground probe (engine+0x79210, PROVEN; corrections)

- Depth factor bits 0x3ca3d70b (one unit in the last place above 0.02; DISASM engine+0x79225), rim share 0.11 bits
  0x3de147af.
- No hit, or a hit without an object (a blocker hit): clear the standing contact (when move flag bit 0).
- The steepness test comes first and uses the hit polygon's plane normal: below 0.7071 under a body whose previous
  standing polygon was standable -> second probe from `pos + (0, step, 0)` along the horizontal UPHILL direction
  (as in E3) by `max(0.5 r, 2 (1 - n_hit.y^2)(0.02 h t + step) + 0.11 r)`; free -> keep the old contact; a hit
  BECOMES the standing contact. Only then: a hit normal y below 0.001 clears; anything else becomes the contact.
- The standing contact is the polygon; the integrator (8.3) and the mover's ground state (10.2) read its plane
  normal, not the edge / cap normal of the contact.

## E5. §16.1 item 4

engine+0x76120 (edge / vertex), engine+0x75f40 / engine+0x75520 (vertical cylinder) are read (E1). Still not read:
engine+0x75860 (box vs box), engine+0x79450 (touch pass), engine+0x71020 (riders); engine+0xc4bf0 is read in shape
(capsule solver, SIM-7) but not ported.

## E6. Capture notes (V-M10, partial)

The 2009 client on JUNK_FLEA (tdm6, reference stack; ae-scope and aa-sounds, our stack): replayed interval by interval
in dc-sim, the intervals where the client moved more than 5 units and dc-sim stayed under 30 % of that fell from
13 / 23 / 14 to 0 / 16 / 12 with the rule of E1-E4. Two places where the 2009 client was blocked have no geometry
in our collision set within reach (ae-scope near (-382..-450, -968..-898), aa-sounds at (-510, 43, -328) beside the
EndlessFall brush `Box_Kill01`, which the body's box touches: PLAUSIBLY that volume's container effect); both stay
open.
