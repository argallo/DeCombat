# RADAR_2009: the in-match radar of the 2009 client (clean-room)

- **Title:** the HUD radar element (top right of the in-match HUD): its layout keys, the map layer and its world-to-picture
  mapping, the turning view disc, which objects it draws and how, and what dc-client implements.
- **Scope:** the radar element `hud_radar` (FMT_CRES_UI §8.1 row 4: ctor cshell+0xb9140, method table cshell+0x3d9010,
  update slot 35): init cshell+0xb3e30, per-frame update cshell+0xb9410 (method slot 4; decompiler's function starts inside it at
  cshell+0xb9423), entries update cshell+0xb60f0, draw cshell+0xb5f10 (method slot 3, no decompiler function), the map-quad setup
  cshell+0xb4bc0, the 0x45 setters cshell+0x47e30 / 0x47e60 / 0xb4ee0, the entry add cshell+0xb8870. Not covered in depth: the radar
  accessory (battery item, pings 0xcd / 0x84, scanner state, the shout text cshell+0xb4ca0), the mission icons of the objective modes
  (entry kinds 1, 3, 4, 5, 11 with their edge arrows), the on/off key (cshell+0xb4b20), the zoom-step keys.
- **Evidence sources:** the clean decompiler output (`CShell.dll`, image base 0x37100000, cited `cshell+0xRVA`),
  our own read-only capstone disassembly of the ROM CShell.dll and Engine.exe (tag DISASM), the engine draw-primitive interface
  (RENDER_2009 §12.3; Engine.exe image base 0x460000, cited `engine+0xRVA`), the 2009 client's screenshot
  `reports/sidebyside4/pairs/rest_1.png` and its capture `captures/20261025_202429_sbs4-real` (OBSERVED), the
  intro-block spec GAME_MESSAGES_2009_S2C.md §2.1.1. No ROM value appears here: keys are named by our names and pinned by hash in
  the code, values are described by role only.
- **Status:** DRAFT 1 (client-radar worker, 2026-10-25). Implemented in `crates/dc-client/src/hud/radar.rs`; compared with the 2009
  screenshot at the same camera point (section R-7). Not seen in a window, not user-confirmed.

## R-1. Layout keys

All keys live in the numbered HUD layout set with the set-0 fallback (one accessor each, cshell+0x53cd0 .. cshell+0x548c0 and
cshell+0x54b40, the `HUD set prefix + n` probe of cshell+0x4bce0). Our names; the literal rva and SHA-256 prefix are pinned in
`radar.rs` (`RadarKey`).

| our name | form | accessor | use |
|---|---|---|---|
| place | pair | cshell+0x540c0 | top-left of the map layer (virtual pixels) |
| disc used | pair | cshell+0x54050 | the drawn size of the map layer and the used texel region of both disc textures |
| disc texels | pair | cshell+0x53fe0 | full texel size of the disc textures (uv = used / full) |
| disc | text | cshell+0x54500 | stage-0 texture of the map layer: only its alpha is used (R-3) |
| view disc | text | cshell+0x545c0 | the turning overlay (R-4) |
| default map | text | cshell+0x54440 | the map picture until 0x45 names one |
| mate icon | text | cshell+0x54680 | team-mate entry icon |
| enemy icon | text | cshell+0x54740 | enemy / NPC entry icon (not used by dc-client, R-5) |
| objective icon | text | cshell+0x54800 | kind-2 entry icon (0x56) |
| blip origin x / y | real | cshell+0x53cd0 / cshell+0x53d40 | an entry icon's top-left at zero distance, relative to place |
| icon size | pair | cshell+0x541a0 | entry icon size |
| radius | int | cshell+0x543d0 | largest radar distance of a default-kind entry |
| blip scale | real | cshell+0x53e20 | radar distance to virtual pixels (copied to the current scale by method slot 5, cshell+0xb49c0) |
| char scale | real | cshell+0x54b40 | world distance to radar distance |
| rim form | text | cshell+0x548c0 | rim image path, a printf form with one `%d` |
| rim size | pair | cshell+0x54130 | rim image texel size, drawn 1:1 at place |

The display-scale keys (base / min / max / step, cshell+0x53db0 .. 0x53f70) and the co-op and gold icon keys of named sections are read
too; their use (zoom steps, co-op items) is not traced here.

## R-2. Draw order (cshell+0xb5f10)

The radar is a **layer-3** element (its constructor stores 3 at +0x08, DISASM cshell+0xb9164): the HUD manager's draw loop
(cshell+0x26b0d0..0x26b108) gates it by the render level of the life state, so it is drawn while alive and hidden while dead
(states 2 / 3 / 6) and in state 5 (client-deathhud's `hud::death::hud_level`, CLI-125).

Skipped while the observer flag of the player object (+0x304, byte +0x5bc) is set, while the element's shown flag +0x1c is clear (1 from
the ctor, toggled by the on/off key cshell+0xb4b20 with a sound) or while the enable byte +0x264 is clear (1 from the ctor). Then:

1. the two accessory text items +0x1c0 / +0x1c4 while their timers run (not covered);
2. the rim image: the form printed with 100 when the accessory flag +0x3a0 is set, else printed with 0 (image items +0x2a4 / +0x2a8, both
   made at init with the rim size at place, cshell+0xb40b4..0xb4149, cshell+0xb4375..0xb441f; DISASM cshell+0xb5fa4..0xb5fc3);
3. the map layer, quad +0x7c (R-3);
4. the view disc, quad +0x2d0 (R-4), after the state reset cshell+0xb4e70;
5. the entries in object-list order: icon quad entry+8 with texture entry+0x88; the second quad entry+0x90 / texture +0x8c only for the
   mission kinds that have an edge arrow (R-5).

DeCombat draws items 2..5 first in the display list (under every other HUD element; no 2009 element overlaps the radar).

## R-3. Map layer

Quad +0x7c: screen rectangle = place x screen factor (truncated) with size = disc used x screen factor (cshell+0xb4bc0, engine slot
+0xdc); uv set 0 = (0, 0, used / full) (init, cshell+0xb42ad..0xb42e2); colour white.

Draw state (DISASM cshell+0xb5fc6..0xb6021): stage-1 texture = element +0x17c (the default map from init; the 0x45 picture replaces it,
cshell+0xb4ee0), stage-0 texture = element +0x180 (the disc), colour op 4. Colour op 4 (engine+0x26630 case 4): stage 0 colour =
diffuse, alpha = texture; stage 1 colour = texture x current, alpha = current; stage 0 uses uv set 0, stage 1 uv set 1. So the layer's
colour is the **picture's** (times white) and its alpha the **disc's**: the picture shows through the disc's round translucent shape. The
disc texture's own colour is not used.

uv set 1 (the picture window), every frame (DISASM cshell+0xb956b..0xb95cb, engine slot +0xf4 writes vertex +0x18 / +0x1c):

    u0 = P.x * A.x / W + B.x - r        v0 = B.z - P.z * A.z / H - r        width = height = 2 r

with `P` the position of the camera object (`camera_mgr`+0x12c, read through the engine's object-position call; an offset from an
engine call is added when a float of another object truncates to non-zero, not traced), and the intro block of 0x45 (S2C §2.1.1): `A` =
the first vector (`radar_map_scale` in our reader, formerly `radar_min`; element +0x370..+0x378, cshell+0x47e30), `B` = the second vector (`radar_map_offset`, formerly `radar_max`; +0x37c..+0x384,
cshell+0x47e60), `r` = the first real (`radar_half`, formerly `radar_rot`; +0x394), `W` / `H` = the two sizes (`radar_w` / `radar_h`, formerly `radar_p3` / `radar_p4`; +0x38c / +0x390). The second real
(`radar_zoom`, +0x388) has no reader among the element's methods cshell+0xb3e30..0xb99c0 (our operand grep; PLAUSIBLE unused). So the map is **north-up**:
the window is axis aligned, world +X to the right, world +Z up, centred on the own point. The reader's field names were renamed to the window roles by spec-fold-9; the
mapping is the rule above. Room-line source of the fields: DSV-50 (the ROM mode file's radar keys).

Addressing outside 0..1: not read for stage 1; the device default is WRAP (RENDER_2009 §2.4). DeCombat repeats the picture (CLI-143).

## R-4. View disc

Quad +0x2d0, texture element +0x350 (the view disc), uv (0, 0, used / full) like the map layer. Corners every frame (DISASM
cshell+0xb95f7..0xb970f): angle `a = WorldNorth + pi/4 - yaw`, where `WorldNorth` is the console value of that name (cshell+0x19fec0; the
client copies the server variable into it at world entry, MOVEMENT_2009_addendum_servervars; DeCombat sends 0, DSV-60) and `yaw` the
camera manager's yaw (+0x4c); while the camera manager's flag +0x134 is set the angle of the previous frame is kept (+0x2cc). Centre
`c` = place + used / 2 (integer halves), radius `R` = used width / (2 sin(pi/4)); corner k (k = 0..3) at screen angle `t = -(a + k pi/2)`:
`(c.x + R cos t) x sx, (c.y + R sin t) x sy`, given to the quad in order (vertex 0 = texture top-left).

Read on a y-down screen: corner 0 is the texture's top-left at the upper right for `yaw = WorldNorth`, so the texture is mirrored left to
right; it turns clockwise as the yaw grows; its top points along the view. On the north-up map the disc's wedge and arrow therefore show
the view direction. The square is rotated in virtual units and then scaled per axis (on a non-4:3 screen a turned square is a sheared
shape; DeCombat's compositor scales first and turns in target pixels, an open difference, CLI-145).

## R-5. Entries (cshell+0xb60f0, cshell+0xb8870)

The element keeps a map of entries keyed by engine object: kind (+4), dead byte (+1), reveal byte (+0x118), team (+0x114), second object
(+0x128). Adds: the roster for every player with kind 0 (cshell+0x2ce0e, and the character spawns cshell+0x2a4d0 / cshell+0x2c0cd),
objective items kind 2 (0x56), NPCs kind 6 / 8 (FIRETEAM_2009 1.3), mode zones (GAME_MODES), mission objects. The add of the own client
object stores the own team at +0x1a0. 0x6d sets the dead byte (cshell+0x2db01 with 1), the roster add sets it again right after the add (cshell+0x2ce1f, the value's source not
traced); 0x6c removes the entry;
0x84 sets the reveal byte.

Per frame and entry (DISASM cshell+0xb6224..0xb6407): skipped (not drawn) when the dead byte is set or the object is the own client
object. Otherwise both positions get the same engine offset, `d` = horizontal distance x char scale, bearing `t` = atan2(dx, dz), and the
icon's top-left = (place + blip origin) x screen factor + (sin t, -cos t) x d x current scale x screen factor, truncated: world +X right,
+Z up, the same north-up frame as the map. For the default kinds (0, 2, 6..10) the icon is drawn while `d` x current scale <= radius, as a
quad at that point - 0.5 with size icon + 1 pixels and uv (0, 0, 1, 1):

| kind / condition | icon | drawn |
|---|---|---|
| 2 | objective icon | yes |
| 6, 7, 8 | enemy icon | yes |
| 9, 10 | co-op life / ammo icons | yes |
| other kinds (players: 0), team = own team | mate icon | while element +0x1a4 is set (1 from the ctor) |
| other kinds, team 0xff | none | no |
| other kinds, other team | enemy icon | only while `d` <= the accessory range +0x1b0 **and** (the reveal byte is set with +0x1b4 >= 0, or the scan flag +0x1b8 is set); or always when a mode flag (`player_mgr`+0x47a) and the second object's test cshell+0x15ba60 say so |

The accessory range +0x1b0 is written only by the radar pings (0xcd, cshell+0xb8650 / cshell+0xb8e70 from the item's charge) and the
scanner state (cshell+0xb77e0). Without the accessory the range is 0, so **enemies are not drawn in a plain match**; there is no rule that
shows an enemy while he fires. Mission kinds 1, 3, 4, 5, 11 compare the screen distance with a fixed 60 pixels and draw an edge arrow
(the second quad) when beyond it; not covered.

## R-6. dc-client (crates/dc-client/src/hud/radar.rs)

- Layout from the ROM (`RadarKeys` pinned, `RadarLayout::from_rom`), or our own generated radar without a ROM (CLI-140).
- Map layer composed on the CPU at target resolution: picture colour, disc alpha, picture window of R-3 with WRAP; before the match names
  a picture, the default picture is shown whole (CLI-143).
- View disc: the used region mirrored, drawn as a square of the used width about the centre and turned by `yaw - WorldNorth` (CLI-141:
  WorldNorth 0).
- Entries: players only, the rule of R-5: living team mates within the radius with the mate icon; dead players never (CLI-142); enemies only
  while the own heartbeat sensor scans, inside the camera's view frustum and within the row's scan distance (CLI-1914, `hud/sensor.rs`; the
  rim then shows the battery forms). The own point is the frame camera's position, the yaw the camera's.
- The radar follows the HUD's regular recompose (CLI-144), not every frame. Not implemented: the on/off key, the other scanner rows
  (sensor options 2 / 3) and the team share of option 1, mission icons, the observer gate (the life-state layer gate of R-2 IS applied), the frozen angle while the camera manager's flag +0x134 is set, the NPC / objective kinds.

## R-7. Comparison with the 2009 client

`sidebyside4/pairs/rest_1.png` (2009 client, 1920 x 1080, JUNK_FLEA, standing at the position and yaw of the capture's first 0x64 of that
moment: OBSERVED `captures/20261025_202429_sbs4-real/analysis/updates.csv`). dc-client's HUD composed at the same point and yaw with the
room line's radar field (test `crates/dc-client/tests/radar.rs`, `DC_RADAR_OWN`): the rim, disc, map lines, the two cross lines and the arrow
direction fall on the same pixels; the remaining difference is the scene behind the translucent disc (mean absolute difference about 10
per channel inside the disc, the 2009 frame has a lit wall behind it). PNGs (ROM-derived, not committed):
`reports/client-radar/compare_rest1_radar_samepos.png`.

## R-8. Open items

- O-R1: the camera-object offset term of R-3 / R-5 (engine call through the client interface +0x74) and the gate float of the other
  object: not traced; DeCombat uses the camera position.
- O-R2: the display-scale keys and the zoom steps (keys, element +0x58 / +0x60): not traced; DeCombat uses the base scale.
- O-R3: the view disc on non-4:3 targets (R-4 last paragraph).
- O-R4: the life-state gate is applied (R-2); the result screen and spectator cases are not traced.
