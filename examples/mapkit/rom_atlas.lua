-- Every world of the user's ROM in one map, as ROM pieces (docs/MAPKIT_AGENTS.md sections 19 and 20, DC_DEFAULTS.md
-- MAP-116 .. MAP-125): the worlds are packed close together on one level ground; each stays closed (its own invisible
-- walls and kill volumes keep its players as safe as in the original); walled corridors link every two neighbouring
-- worlds, playable floor to playable floor; every region is sealed with invisible walls but where a corridor crosses;
-- rails close what a player could never leave or would die walking into; both faces of the worlds are drawn.
-- The script names no world: it takes the ROM's own list (`map.rom_worlds()`) at run time, so the project it makes
-- names ROM paths and stays the user's own (never commit it). Needs a ROM (--rom, CA_ROM, the profile).
--
-- The corridors, rails and left-out spawns are planned from the walk check (examples/mapkit/plan_links.py,
-- check_round.py and rails_from.py, beside this script):
--   1. dc-mapkit new atlas --template flat && dc-mapkit run atlas rom_atlas.lua --budget 2000000000 --json > plan.json
--      (LINKS, RAILS and DROPS empty: the closed worlds alone), then dc-mapkit build atlas --json
--   2. for each piece id: dc-mapkit walk atlas --reach --box <id> --seed-piece <id> --step 24 --nodes nodes/<id>.json
--      --json (the floor that world's players reach and leave under its own rules)
--   3. python3 plan_links.py plan.json nodes --gen rom_atlas.lua --write   (writes LINKS into this script)
--   4. run and build again (> run.json); python3 check_round.py dc-mapkit atlas run.json rounds/1: every region at
--      three grids (16, 24, and 48 shifted half a cell), seeded at its own spawns
--   5. python3 rails_from.py rounds/1 --gen rom_atlas.lua (writes RAILS and DROPS); repeat 4 and 5 (rounds/2, ...)
--      until a round finds no trap, no walk into a kill volume and no spawn to leave out. Then walk every corridor
--      both ways (`walk --routes` from each link's two points, which the run prints).
map.clear()

local GAP = 240           -- ground between two pieces' regions, crossed by the corridors (a player runs ~215 a second)
local PAD = 160           -- each region: the world's play area grown by this much across (the world's own
                          -- invisible walls stand at the play area's edge, the seals at the region's)
-- the arrangement of the maps as a first packing laid them out (region boxes in map coordinates: x0 z0 x1 z1), by
-- ROM path. Empty, the worlds are packed afresh. Filled in (from the `<id> <path> at x .. z ..` lines of a run),
-- every map keeps the neighbours it has here and slides up against them (GAP apart) when regions get smaller: the
-- corridors between them get shorter, and the piece ids stay the same (they are numbered in LAYOUT's order).
local LAYOUT = {
}
-- where a world's play area (the box round its invisible walls and spawns) reaches far past the floor its players
-- can walk (hills a player never reaches), the region is drawn in to that floor on that side, { west, east, south,
-- north } in units from the walk check (the outermost walkable node per side, less 40); never negative: the floor a
-- world's play area leaves out is out of bounds, and growing a region over it only adds floor to fence. Sides over
-- water keep their play area: a bridge crosses them. By ROM path.
local SHRINK = {
}
local MARGIN = 1600       -- ground around the whole layout
local CELL = 250          -- terrain cell
local LID = 7600          -- the sky lid over everything
local KILL_Y = -2700      -- the kill slab under everything (pieces reach 1,500 under their floor)
-- a world named like another one plus "_<suffix>" is a variant of it (the same ground for another mode): left out
local function stem(path) return (path:match("([^\\/]+)%.[^.]*$") or path):upper() end
local stems = {}
for _, r in ipairs(map.rom_worlds()) do stems[stem(r.path)] = true end
local function variant(path)
  local s = stem(path)
  for other in pairs(stems) do
    if other ~= s and s:sub(1, #other + 1) == other .. "_" then return true end
  end
  return false
end

-- the worlds and the region each keeps (`rom worlds`: play box grown by 1,200, floor - 1,500 .. top + 2,500)
local items = {}
for _, r in ipairs(map.rom_worlds()) do
  local w = r.world
  if not variant(r.path) and w and w.play and w.floor then
    -- the region: the play area grown by PAD across, from 1,500 under the floor to 2,500 over the play area
    local pl = w.play
    local sh = SHRINK[r.path] or { 0, 0, 0, 0 }
    local g = {
      min = { pl.min[1] - PAD + sh[1], w.floor - 1500, pl.min[3] - PAD + sh[3] },
      max = { pl.max[1] + PAD - sh[2], math.max(pl.max[2], w.floor + 500) + 2500, pl.max[3] + PAD - sh[4] },
    }
    items[#items + 1] = {
      path = r.path,
      region = g,
      play = w.play,
      floor = w.floor or g.min[2],
      w = g.max[1] - g.min[1] + GAP,
      d = g.max[3] - g.min[3] + GAP,
    }
  end
end
-- the pieces are numbered in this order: by the depth of the region as LAYOUT recorded it (so the ids stay the same
-- when a region is drawn in), else by the region's depth now
local function order_depth(it)
  local L = LAYOUT[it.path]
  return L and (L[4] - L[2]) or it.d
end
table.sort(items, function(a, b)
  local da, db = order_depth(a), order_depth(b)
  if da ~= db then return da > db end
  return a.path < b.path
end)

-- skyline packing (bottom-left) into a strip of the given width; returns placements and the depth used
local function pack(width)
  local sky = { { x = 0, w = width, y = 0 } }
  local placed, depth, right_used = {}, 0, 0
  for _, it in ipairs(items) do
    local best
    for i = 1, #sky do
      local x = sky[i].x
      if x + it.w <= width + 0.01 then
        local y, j, right = 0, i, x + it.w
        while j <= #sky and sky[j].x < right - 0.01 do
          y = math.max(y, sky[j].y)
          j = j + 1
        end
        if not best or y < best.y - 0.01 or (math.abs(y - best.y) <= 0.01 and x < best.x) then
          best = { x = x, y = y }
        end
      end
    end
    if not best then return nil end
    placed[it.path] = best
    local nx, right, ny = best.x, best.x + it.w, best.y + it.d
    local new = {}
    for _, s in ipairs(sky) do
      local s0, s1 = s.x, s.x + s.w
      if s1 <= nx + 0.01 or s0 >= right - 0.01 then
        new[#new + 1] = s
      else
        if s0 < nx then new[#new + 1] = { x = s0, w = nx - s0, y = s.y } end
        if s1 > right then new[#new + 1] = { x = right, w = s1 - right, y = s.y } end
      end
    end
    new[#new + 1] = { x = nx, w = it.w, y = ny }
    table.sort(new, function(a, b) return a.x < b.x end)
    sky = new
    depth = math.max(depth, ny)
    right_used = math.max(right_used, right)
  end
  return placed, right_used, depth
end

-- compaction: every map slides west and south against the maps it stood next to in LAYOUT (its neighbours there),
-- GAP apart; which maps count as neighbours comes from LAYOUT, how far they overlap from the new boxes
local function compact()
  local list = {}
  for _, it in ipairs(items) do
    local L = LAYOUT[it.path]
    if not L then return nil end
    list[#list + 1] = { it = it, x0 = L[1], z0 = L[2], x1 = L[3], z1 = L[4],
      w = it.region.max[1] - it.region.min[1], d = it.region.max[3] - it.region.min[3] }
  end
  local function overlap(a0, a1, b0, b1) return a0 < b1 - 1 and b0 < a1 - 1 end
  local minx, minz = math.huge, math.huge
  for _, p in ipairs(list) do
    minx, minz = math.min(minx, p.x0), math.min(minz, p.z0)
    p.nx0, p.nx1, p.nz0, p.nz1 = p.x0, p.x1, p.z0, p.z1
  end
  for _ = 1, 4 do
    table.sort(list, function(a, b) return a.x0 < b.x0 end)
    for i, p in ipairs(list) do
      local x = minx
      for j = 1, i - 1 do
        local q = list[j]
        if q.x1 <= p.x0 + 1 and overlap(q.nz0, q.nz1, p.nz0, p.nz1) then x = math.max(x, q.nx1 + GAP) end
      end
      p.nx0, p.nx1 = x, x + p.w
    end
    table.sort(list, function(a, b) return a.z0 < b.z0 end)
    for i, p in ipairs(list) do
      local z = minz
      for j = 1, i - 1 do
        local q = list[j]
        if q.z1 <= p.z0 + 1 and overlap(q.nx0, q.nx1, p.nx0, p.nx1) then z = math.max(z, q.nz1 + GAP) end
      end
      p.nz0, p.nz1 = z, z + p.d
    end
  end
  for i = 1, #list do
    for j = i + 1, #list do
      local p, q = list[i], list[j]
      if overlap(p.nx0, p.nx1, q.nx0, q.nx1) and overlap(p.nz0, p.nz1, q.nz0, q.nz1) then
        error(string.format("compaction: %s and %s overlap", p.it.path, q.it.path))
      end
    end
  end
  local placed, maxx, maxz = {}, -math.huge, -math.huge
  for _, p in ipairs(list) do
    placed[p.it.path] = { x = p.nx0, z = p.nz0 }
    maxx, maxz = math.max(maxx, p.nx1), math.max(maxz, p.nz1)
  end
  return { placed = placed, x0 = minx, z0 = minz, w = maxx - minx, d = maxz - minz }
end

local best = compact()
if best then
  print(string.format("layout %.0f x %.0f with %d maps (compacted)", best.w, best.d, #items))
else
  for width = 28000, 48000, 500 do
    local placed, used, depth = pack(width)
    if placed then
      local side = math.max(used, depth)
      if not best or side < best.side then
        best = { placed = placed, w = used, d = depth, side = side, packed = true }
      end
    end
  end
  print(string.format("layout %.0f x %.0f with %d maps", best.w, best.d, #items))
end

-- the pieces, the layout centred on the origin, every floor at y 0
local regions = {}
for _, it in ipairs(items) do
  local at = best.placed[it.path]
  local g = it.region
  local pos
  if best.packed then
    pos = { at.x + GAP / 2 - g.min[1] - best.w / 2, -it.floor, at.y + GAP / 2 - g.min[3] - best.d / 2 }
  else
    pos = { at.x - best.x0 - g.min[1] - best.w / 2, -it.floor, at.z - best.z0 - g.min[3] - best.d / 2 }
  end
  local id = map.piece{
    world = it.path,
    pos = pos,
    region = { min = g.min, max = g.max },
    spawns = "ffa",
    items = true,
    sky = #regions == 0,
    -- closed: the world's own invisible walls and kill volumes stay (the corridors cut through them); the bare ground
    -- round its play area is filled so it stands on something
    blockers = true,
    fill = true,
    fill_y = 0,
    -- the outer faces of a world got only its ambient light (it never showed them): lift its darkest light
    min_light = 72,
  }
  regions[#regions + 1] = {
    id = id,
    pos = pos,
    path = it.path,
    play = {
      min = { it.play.min[1] + pos[1], it.play.min[2] + pos[2], it.play.min[3] + pos[3] },
      max = { it.play.max[1] + pos[1], it.play.max[2] + pos[2], it.play.max[3] + pos[3] },
    },
    min = { g.min[1] + pos[1], g.min[2] + pos[2], g.min[3] + pos[3] },
    max = { g.max[1] + pos[1], g.max[2] + pos[2], g.max[3] + pos[3] },
  }
  print(string.format("%s %s at x %.0f..%.0f z %.0f..%.0f pos %.1f %.1f %.1f", id, it.path,
    regions[#regions].min[1], regions[#regions].max[1], regions[#regions].min[3], regions[#regions].max[3],
    pos[1], pos[2], pos[3]))
end

-- spawns of the original maps left out (MAP-125): where `dc-mapkit walk --reach` finds one inside a kill volume,
-- on no floor, in a trap or on an island with no way out (examples/mapkit/plan_links.py --drop-spawns)
-- DROPS BEGIN
local DROPS = {}
-- DROPS END
for _, r in ipairs(regions) do
  if DROPS[r.id] then map.piece_set(r.id, { drop_spawns = DROPS[r.id] }) end
end

-- the bridges' deck and parapets: the template's materials (a ROM texture is the user's run-time choice, set in a project)
map.material.set{ id = "bridge_deck", texture = "assets/block.png", tile = 256 }
map.material.set{ id = "bridge_wall", texture = "assets/wall.png", tile = 160 }
-- the ground: a level terrain under and around everything (the pieces clear their own footprint)
local W, D = best.w + 2 * MARGIN, best.d + 2 * MARGIN
local cx, cz = math.ceil(W / CELL), math.ceil(D / CELL)
map.terrain.gen{ preset = "flat", cells_x = cx, cells_z = cz, cell_size = CELL, amplitude = 0, base = 0 }
local hx, hz = cx * CELL / 2, cz * CELL / 2

-- links: a walled corridor between two neighbouring maps, straight across the ground between them from the edge of
-- one map's playable floor to the edge of the other's; a ramp between the two play areas takes up the difference in
-- height. A corridor is open only at its two ends, so the maps stay closed (their invisible walls and kill volumes)
-- and the corridors, which cut through them, are the ways between maps. Where the corridors go is planned outside
-- this script (examples/mapkit/plan_links.py) from each map's playable floor as `dc-mapkit walk --reach` finds it under the
-- map's own rules, and written into LINKS below; with LINKS empty the script builds the closed maps alone, which is
-- what the planner measures.
-- LINKS BEGIN
local LINKS = {}
-- LINKS END
local LINK_W = 240          -- the corridor's width (the cut); its walls stand just outside (the planner checks the
                            -- playable landing this wide at each end)
local WALL_T, WALL_H = 32, 320
local CUT_H = 440           -- headroom cut out over the corridor's floor

local function solid_box(along_x, a0, a1, y0, y1, c0, c1, material)
  if a1 - a0 < 1 or y1 - y0 < 1 or c1 - c0 < 1 then return end
  local pos, size
  if along_x then
    pos, size = { (a0 + a1) / 2, (y0 + y1) / 2, (c0 + c1) / 2 }, { a1 - a0, y1 - y0, c1 - c0 }
  else
    pos, size = { (c0 + c1) / 2, (y0 + y1) / 2, (a0 + a1) / 2 }, { c1 - c0, y1 - y0, a1 - a0 }
  end
  map.solid.box{ pos = pos, size = size, material = material, group = "links" }
end
-- an invisible wall for players (a blocker: shots pass) in the corridor's own frame
local function blocker_box(along_x, a0, a1, y0, y1, c0, c1)
  if a1 - a0 < 1 or y1 - y0 < 1 or c1 - c0 < 1 then return end
  local pos, size
  if along_x then
    pos, size = { (a0 + a1) / 2, (y0 + y1) / 2, (c0 + c1) / 2 }, { a1 - a0, y1 - y0, c1 - c0 }
  else
    pos, size = { (c0 + c1) / 2, (y0 + y1) / 2, (a0 + a1) / 2 }, { c1 - c0, y1 - y0, a1 - a0 }
  end
  map.solid.box{ pos = pos, size = size, material = "wall", brush = "blocker", group = "links" }
end
local function cut(id, along_x, a0, a1, y0, y1, c0, c1)
  local mn, mx
  if along_x then mn, mx = { a0, y0, c0 }, { a1, y1, c1 } else mn, mx = { c0, y0, a0 }, { c1, y1, a1 } end
  map.piece_cut(id, { min = mn, max = mx, world = true })
end

local links, spots = {}, {}
local SEG = 480             -- walls and cuts follow the ramp in pieces this long
-- bridges: a piece of corridor whose deck lies more than BRIDGE_DROP over what the map has under it (or over nothing:
-- a sea, a canyon, a river) is a bridge: a parapet a player cannot jump (a jump rises about 50) with an invisible wall
-- over it instead of the tunnel's walls, and piers down to the ground under it
local BRIDGE_DROP = 150
local PARAPET_H = 90
local PIER_EVERY = 640
local PIER_T = 80
local bridge_segs, piers = 0, 0
local MOUTH = 128           -- each mouth: headroom cut and a floor this far into the map's playable floor (a wall
                            -- the cut takes often stood on no floor, and a kill volume's top may reach the mouth)
-- where corridors cross a region's sides: openings[id][side] = { {c0, c1, y0, y1}, ... } (side 1 west .. 4 north)
local openings = {}
local function opening(id, side, c0, c1, y0, y1)
  openings[id] = openings[id] or {}
  openings[id][side] = openings[id][side] or {}
  local t = openings[id][side]
  t[#t + 1] = { c0, c1, y0, y1 }
end
local MOUTH_DROP = 3         -- the mouth's slab: this far under the map's floor at the corridor's end
local CUT_LAP = 8            -- the cut pieces overlap this much: a wall lying exactly on the plane where two cut
                             -- boxes meet stayed (a world's wall across a mouth, found by the corridor walk)
local CUT_UNDER = 2          -- the cuts start this far under the corridor's floor: whatever of the map's ground
                             -- stood over it (a ridge the cut left 6 over the floor stood as a fin a walk caught
                             -- on) goes, and the mouth's slab and the corridor's floor are the floor
local RAMP_MIN = 22          -- a mouth whose slab stands more than this over the floor in front gets a ramp (a
                             -- player steps up 30)
local RAMP_SLOPE = 0.8       -- rise per unit (about 39 degrees; a player walks up 45)
local RAMP_MAX = 240         -- and never longer than this
local RAMP_MAX_RISE = 180    -- nor higher: a taller one leads down a slope that keeps falling away (its foot hangs
                             -- over the ground still)
-- a ramp narrower than the mouth, where something of the map that the ramp may not cut stands in front of part of it
-- (key "<a>|<b>|<end>": the offsets across from the corridor's line it spans)
local RAMP_SPAN = {
  -- ["piece_2|piece_5|a"] = { 35, 120 },   -- for example: a world model stands in front of the mouth's west part
}
local function floor_ahead(along_x, c, t, from_y, offs)
  local lo, any_nil = nil, false
  for _, off in ipairs(offs) do
    local x, z = t, c + off
    if not along_x then x, z = c + off, t end
    local y = map.probe(x, z, from_y)
    if y == nil then any_nil = true elseif lo == nil or y < lo then lo = y end
  end
  return lo, any_nil
end
local ramps = 0
local function ramp(ida, idb, which, along_x, c, front, dir, top, outer)
  local span = RAMP_SPAN[string.format("%s|%s|%s", ida, idb, which)] or { -outer, outer }
  local s0, s1 = span[1], span[2]
  local sm = (s0 + s1) / 2
  local offs = { s0 + 6, (s0 + sm) / 2, sm, (sm + s1) / 2, s1 - 6 }
  local f, gap = floor_ahead(along_x, c, front + dir * 16, top + 40, { (s0 + sm) / 2, sm, (sm + s1) / 2 })
  if gap or f == nil or top - f <= RAMP_MIN then return end
  -- the run follows the floor out: on ground that slopes away, the foot is probed again where the ramp ends
  local run = 0
  for _ = 1, 4 do
    run = math.min(RAMP_MAX, math.max(32, (top - f) / RAMP_SLOPE))
    local f2 = floor_ahead(along_x, c, front + dir * run, top + 40, offs)
    if f2 == nil or f2 >= f - 4 then break end
    f = f2
  end
  local h = top - f
  if h <= RAMP_MIN then return end
  if h > RAMP_MAX_RISE then
    print(string.format("no ramp %s %s c %.0f end %s: rise %.0f", ida, idb, c, which, h))
    return
  end
  local t0, t1 = front, front + dir * run
  local mid = (t0 + t1) / 2
  local rises
  if along_x then rises = dir < 0 and "east" or "west" else rises = dir < 0 and "north" or "south" end
  local pos = along_x and { mid, f + h / 2, c + sm } or { c + sm, f + h / 2, mid }
  map.solid.wedge{ pos = pos, size = { s1 - s0, h, run }, rises = rises, material = "block", group = "links" }
  -- the ramp's own footprint is cut out of the map (what stood on the slope under it, and a kill volume lying
  -- against that slope, which the ramp's surface ran through); the cut keeps to the inside of the parapets, so the
  -- map's invisible walls beside the ramp stay
  local lo_t, hi_t = math.min(t0, t1), math.max(t0, t1)
  cut(which == "a" and ida or idb, along_x, lo_t, hi_t, f - 60, top + CUT_H, c + s0, c + s1)
  -- a parapet along each side of the ramp: a ramp over a drop is a bridge's end, and an invisible wall over the
  -- parapet keeps a jump in
  for _, side in ipairs({ { c + s0 - WALL_T, c + s0 }, { c + s1, c + s1 + WALL_T } }) do
    solid_box(along_x, lo_t, hi_t, f - 10, top + PARAPET_H, side[1], side[2], "block")
    blocker_box(along_x, lo_t, hi_t, top + PARAPET_H - 4, top + WALL_H, side[1], side[2])
  end
  ramps = ramps + 1
  print(string.format("ramp %s %s c %.0f end %s rise %.0f run %.0f", ida, idb, c, which, h, run))
end

local function build_link(a, b, along_x, c, a_in, ha, b_in, hb)
  local ax = along_x and 1 or 3
  local a_edge, b_edge = a.max[ax], b.min[ax]
  -- the floor is one ramp from end to end
  local function h_at(t)
    local f = (t - a_in) / (b_in - a_in)
    if f < 0 then f = 0 elseif f > 1 then f = 1 end
    return ha + (hb - ha) * f
  end
  local half, outer = LINK_W / 2, LINK_W / 2 + WALL_T
  -- the cuts: through each map from a little inside its playable floor out past its region's edge, in pieces that
  -- follow the ramp, from just under its floor (the corridor's own floor takes the place of the map's)
  local function cuts(id, t0, t1)
    local t = t0
    while t < t1 - 1 do
      local u = math.min(t1, t + SEG)
      local lo = math.min(h_at(t), h_at(u))
      local hi = math.max(h_at(t), h_at(u))
      cut(id, along_x, t - CUT_LAP, u + CUT_LAP, lo - CUT_UNDER, hi + CUT_H, c - half, c + half)
      t = u
    end
  end
  -- each mouth, from just under the map's floor at the corridor's end (the mouth's slab is the floor there; a cut
  -- down to the ramp's lower end would take the floor away in front of it)
  cut(a.id, along_x, a_in - MOUTH, a_in + CUT_LAP, ha - CUT_UNDER, ha + CUT_H, c - half, c + half)
  cut(b.id, along_x, b_in - CUT_LAP, b_in + MOUTH, hb - CUT_UNDER, hb + CUT_H, c - half, c + half)
  cuts(a.id, a_in, a_edge + 4)
  cuts(b.id, b_edge - 4, b_in)
  -- the openings this corridor makes in the two regions' sealed sides: its inside, its floor to its walls' top
  local ya, yb = h_at(a_edge), h_at(b_edge)
  opening(a.id, along_x and 2 or 4, c - half, c + half, ya - 60, ya + WALL_H)
  opening(b.id, along_x and 1 or 3, c - half, c + half, yb - 60, yb + WALL_H)
  -- each mouth floored just under the map's own floor: a wall the cut took away often stood on no floor of its own
  -- (MOUTH_DROP under it: deeper, a ridge of the map's ground the cut leaves 6 over the floor becomes a step up
  -- that a player cannot take coming in)
  solid_box(along_x, a_in - MOUTH, a_in, ha - 56, ha - MOUTH_DROP, c - outer, c + outer, "block")
  solid_box(along_x, b_in, b_in + MOUTH, hb - 56, hb - MOUTH_DROP, c - outer, c + outer, "block")
  -- where the map's floor in front of a mouth lies lower than a player steps up (the corridor's end stood on a
  -- platform, or the ground slopes away), a ramp from the slab's front edge down to it
  ramp(a.id, b.id, "a", along_x, c, a_in - MOUTH, -1, ha - MOUTH_DROP, outer)
  ramp(a.id, b.id, "b", along_x, c, b_in + MOUTH, 1, hb - MOUTH_DROP, outer)
  -- what lies under the corridor at t: the lowest of three probes across (the map's own floor; nil when nothing)
  local function under(t)
    local h = h_at(t)
    local lowest, any = nil, false
    for _, off in ipairs({ -80, 0, 80 }) do
      local x, z = t, c + off
      if not along_x then x, z = c + off, t end
      local y = map.probe(x, z, h + 30)
      if y then
        any = true
        if lowest == nil or y < lowest then lowest = y end
      end
    end
    return lowest, any
  end
  local function is_bridge(t, u)
    local deep = 0
    for _, f in ipairs({ 0.1, 0.5, 0.9 }) do
      local tt = t + (u - t) * f
      local y, any = under(tt)
      if not any or h_at(tt) - y > BRIDGE_DROP then deep = deep + 1 end
    end
    return deep >= 2
  end
  -- the walls' pieces, from where the cuts begin inside each map's playable floor (so no gap is left beside the
  -- mouth where a world's own invisible wall was cut away), and which of them are bridges
  local segs, has_bridge = {}, false
  do
    local t = a_in - 64
    while t < b_in + 64 - 1 do
      local u = math.min(b_in + 64, t + SEG)
      local br = is_bridge(t, u)
      has_bridge = has_bridge or br
      segs[#segs + 1] = { t, u, br }
      t = u
    end
  end
  local deck_mat = has_bridge and "bridge_deck" or "block"
  local under_deck = math.abs(hb - ha) < 4 and ha - 48 or math.min(ha, hb)
  if math.abs(hb - ha) < 4 then
    solid_box(along_x, a_in, b_in, ha - 48, ha, c - outer, c + outer, deck_mat)
  else
    local rises = hb > ha and (along_x and "east" or "north") or (along_x and "west" or "south")
    local mid = (a_in + b_in) / 2
    local pos = along_x and { mid, (ha + hb) / 2, c } or { c, (ha + hb) / 2, mid }
    map.solid.wedge{ pos = pos, size = { 2 * outer, math.abs(hb - ha), b_in - a_in }, rises = rises,
      material = deck_mat, group = "links" }
    -- the slope's thin end gets a slab under it so its edge meets the floor it starts from
    local low_end = hb > ha and a_in or b_in
    solid_box(along_x, low_end - 24, low_end + 24, math.min(ha, hb) - 48, math.min(ha, hb), c - outer, c + outer, "block")
  end
  -- the walls, in pieces that follow the ramp: a tunnel's walls, or a bridge's parapets with an invisible wall over
  -- them and piers under the deck
  for _, sg in ipairs(segs) do
    local t, u, br = sg[1], sg[2], sg[3]
    local lo = math.min(h_at(t), h_at(u)) - 60
    local hi = math.max(h_at(t), h_at(u)) + WALL_H
    if br then
      local top = math.max(h_at(t), h_at(u)) + PARAPET_H
      solid_box(along_x, t, u, lo, top, c - outer, c - half, "bridge_wall")
      solid_box(along_x, t, u, lo, top, c + half, c + outer, "bridge_wall")
      blocker_box(along_x, t, u, top - 4, hi, c - outer, c - half)
      blocker_box(along_x, t, u, top - 4, hi, c + half, c + outer)
      bridge_segs = bridge_segs + 1
      local p = math.ceil(t / PIER_EVERY) * PIER_EVERY
      while p < u do
        if p > a_in + 100 and p < b_in - 100 then
          local y = under(p)
          local drop = y and (h_at(p) - y) or nil
          if drop and drop > 60 and drop < 1500 then
            solid_box(along_x, p - PIER_T / 2, p + PIER_T / 2, y + 8, under_deck + 12, c - outer, c + outer, "bridge_wall")
            piers = piers + 1
          end
        end
        p = p + PIER_EVERY
      end
    else
      solid_box(along_x, t, u, lo, hi, c - outer, c - half, "block")
      solid_box(along_x, t, u, lo, hi, c + half, c + outer, "block")
    end
  end
  -- the ground between the regions comes down under a corridor that runs low across it
  t = a_edge - 160
  while t <= b_edge + 160 do
    local h = h_at(t)
    if h < 40 then
      local x, z = t, c
      if not along_x then x, z = c, t end
      map.terrain.flatten{ x = x, z = z, radius = 300, height = h - 100, falloff = "smooth" }
    end
    t = t + 120
  end
  -- where the corridor meets each map's playable floor: a place to start a walk, or to spawn
  local function point(along, h)
    if along_x then return { along, h, c } else return { c, h, along } end
  end
  local pa, pb = point(a_in - 40, ha), point(b_in + 40, hb)
  links[#links + 1] = { a = a.id, b = b.id, pa = pa, pb = pb, ha = ha, hb = hb, len = b_in - a_in }
  spots[#spots + 1] = pa
  spots[#spots + 1] = pb
  print(string.format("link %s %s %s c %.0f floor %.0f/%.0f length %.0f from (%.0f,%.0f,%.0f) to (%.0f,%.0f,%.0f)",
    a.id, b.id, along_x and "x" or "z", c, ha, hb, b_in - a_in, pa[1], pa[2], pa[3], pb[1], pb[2], pb[3]))
end

local by_id, degree = {}, {}
for _, r in ipairs(regions) do
  by_id[r.id] = r
  degree[r.id] = 0
  print(string.format("region %s play %.0f %.0f %.0f %.0f edge %.0f %.0f %.0f %.0f", r.id,
    r.play.min[1], r.play.min[3], r.play.max[1], r.play.max[3], r.min[1], r.min[3], r.max[1], r.max[3]))
end
for _, l in ipairs(LINKS) do
  local a, b = by_id[l.a], by_id[l.b]
  if a and b then
    build_link(a, b, l.along_x, l.c, l.a_in, l.ha, l.b_in, l.hb)
    degree[a.id] = degree[a.id] + 1
    degree[b.id] = degree[b.id] + 1
  else
    print("planned link names a missing piece: " .. tostring(l.a) .. " " .. tostring(l.b))
  end
end
-- the seals: an invisible wall for players (a blocker: shots pass) round every region, just outside its edges, from
-- under its lowest floor to over its top, open only where a corridor crosses; so nobody leaves a map but by a
-- corridor, and nobody walks in from the ground between the maps (a world's own invisible walls stop players
-- only from inside, and its play area does not always reach them)
local SEAL_T = 32
local function seal_box(along_x, a0, a1, y0, y1, c0, c1)
  if a1 - a0 < 1 or y1 - y0 < 1 or c1 - c0 < 1 then return end
  local pos, size
  if along_x then
    pos, size = { (a0 + a1) / 2, (y0 + y1) / 2, (c0 + c1) / 2 }, { a1 - a0, y1 - y0, c1 - c0 }
  else
    pos, size = { (c0 + c1) / 2, (y0 + y1) / 2, (a0 + a1) / 2 }, { c1 - c0, y1 - y0, a1 - a0 }
  end
  map.solid.box{ pos = pos, size = size, material = "wall", brush = "blocker", group = "seals" }
end
local seals = 0
for _, r in ipairs(regions) do
  local y0, y1 = r.min[2] - 50, r.max[2] + 200
  for side = 1, 4 do
    -- the wall's thickness runs across the side (x for west/east, z for south/north), its length along it
    local across_x = side <= 2
    local t0, t1
    if side == 1 then t0, t1 = r.min[1] - SEAL_T, r.min[1]
    elseif side == 2 then t0, t1 = r.max[1], r.max[1] + SEAL_T
    elseif side == 3 then t0, t1 = r.min[3] - SEAL_T, r.min[3]
    else t0, t1 = r.max[3], r.max[3] + SEAL_T end
    local lo = across_x and r.min[3] - SEAL_T or r.min[1] - SEAL_T
    local hi = across_x and r.max[3] + SEAL_T or r.max[1] + SEAL_T
    local list = (openings[r.id] and openings[r.id][side]) or {}
    table.sort(list, function(p, q) return p[1] < q[1] end)
    local at = lo
    for _, o in ipairs(list) do
      -- a seal_box takes the thickness as its `along` range and the side's length as its `c` range
      seal_box(across_x, t0, t1, y0, y1, at, o[1])
      seal_box(across_x, t0, t1, y0, o[3], o[1], o[2])
      seal_box(across_x, t0, t1, o[4], y1, o[1], o[2])
      at = o[2]
      seals = seals + 3
    end
    seal_box(across_x, t0, t1, y0, y1, at, hi)
    seals = seals + 1
  end
end
print(string.format("%d seal walls", seals))

-- rails: an invisible wall for players across every move into a place a player could never leave (a pocket the
-- original map kept behind its invisible walls, or under a drop with no way back), planned from `dc-mapkit walk
-- --reach` by source/plan_links.py --rails; square to the move, midway between its two floors
-- RAILS BEGIN
local RAILS = {}
-- RAILS END
for _, r in ipairs(RAILS) do
  map.solid.box{ pos = { r.x, (r.y0 + r.y1) / 2, r.z }, size = { r.t or 12, r.y1 - r.y0, r.len }, rot_y_deg = r.yaw,
    material = "wall", brush = "blocker", group = "rails" }
end
print(string.format("%d rails", #RAILS))

-- one map from all: the maps the links join, from the first
local seen, queue = { [regions[1].id] = true }, { regions[1].id }
while #queue > 0 do
  local id = table.remove(queue, 1)
  for _, l in ipairs(links) do
    local o = (l.a == id and l.b) or (l.b == id and l.a) or nil
    if o and not seen[o] then seen[o] = true; queue[#queue + 1] = o end
  end
end
local joined = 0
for _, r in ipairs(regions) do
  if seen[r.id] then joined = joined + 1 else print("not joined: " .. r.id .. " " .. r.path) end
  print(string.format("%s %s: %d links", r.id, r.path, degree[r.id]))
end
print(string.format("%d links, %d of %d maps joined", #links, joined, #regions))
print(string.format("%d ramps", ramps))
print(string.format("%d bridge pieces, %d piers", bridge_segs, piers))

-- walls round the ground, the sky lid over it, the sky box, the kill slab under it
local wall_h = LID - KILL_Y
local wall_y = (LID + KILL_Y) / 2
-- the walls are sky faces like the lid: the sky pass draws through all of them, so the horizon is sky
map.solid.box{ pos = { 0, wall_y, hz + 32 }, size = { 2 * hx + 128, wall_h, 64 }, material = "sky", group = "bounds" }
map.solid.box{ pos = { 0, wall_y, -hz - 32 }, size = { 2 * hx + 128, wall_h, 64 }, material = "sky", group = "bounds" }
map.solid.box{ pos = { hx + 32, wall_y, 0 }, size = { 64, wall_h, 2 * hz + 128 }, material = "sky", group = "bounds" }
map.solid.box{ pos = { -hx - 32, wall_y, 0 }, size = { 64, wall_h, 2 * hz + 128 }, material = "sky", group = "bounds" }
map.solid.box{ pos = { 0, LID + 32, 0 }, size = { 2 * hx + 128, 64, 2 * hz + 128 }, material = "sky", group = "sky_lid" }
map.solid.box{ pos = { 0, LID - 900, 0 }, size = { 1024, 1024, 1024 }, material = "sky", brush = "sky", group = "sky" }
map.solid.box{ pos = { 0, KILL_Y, 0 }, size = { 2 * hx, 64, 2 * hz }, material = "wall", brush = "kill_volume", group = "kill" }

-- the map's own spawns stand on the corridors' ends inside the maps (level main floor, never the ground between the
-- maps, which no one can leave): team 1 at the westmost, team 2 at the eastmost, the rest team-less
if #spots < 16 then
  -- no corridors yet (the planning build): the team spawns go on the maps' own spawns
  for _, r in ipairs(regions) do
    for _, sp in ipairs(map.piece_spawns(r.id)) do
      if sp.kind == "player" then spots[#spots + 1] = { sp.x, sp.y, sp.z } break end
    end
  end
end
table.sort(spots, function(p, q) return p[1] < q[1] end)
local function spawn(p, team, yaw)
  map.add{ class = "dc.player_spawn", pos = { p[1], p[2] + 1, p[3] }, rot = { 0, yaw, 0 }, props = { team = team, modes = "" } }
end
local n = #spots
for k = 1, math.min(8, n) do spawn(spots[k], "1", 90) end
for k = 1, math.min(8, n) do spawn(spots[n + 1 - k], "2", 270) end
local free = 0
for k = 9, n - 8 do
  if free < 16 then spawn(spots[k], "", 0); free = free + 1 end
end
print(string.format("%d spawn spots, %d team-less spawns", n, free))

-- both faces: a world shows its surfaces only towards its own play area (MAP-118)
-- a brighter ambient for the worlds' own geometry: the corridors' walls away from the sun read as black blocks otherwise
map.settings{ fog_enable = true, fog_near = 3000, fog_far = 11500, fog_color = { 196, 182, 152 }, far_z = 14000, two_sided = true,
  ambient = { 120, 116, 108 } }
map.meta{
  name = "ROM Atlas",
  description = "Every world of your ROM in one map, joined by corridors, composed from your own ROM when the map loads.",
  modes = { "TDM", "FFA" },
  players = { min = 2, max = 16 },
}
print(string.format("ground %.0f x %.0f, %d pieces", 2 * hx, 2 * hz, #regions))
