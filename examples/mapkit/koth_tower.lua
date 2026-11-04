-- koth_tower.lua: a king-of-the-hill tower in the middle of the flat template (docs/MAPKIT_AGENTS.md section 6).
--
--   dc-mapkit new my_tower --template flat
--   dc-mapkit run my_tower examples/mapkit/koth_tower.lua
--
-- A stepped tower of cylinders with a ramp spiralling up its tiers; bomb site A and the item spawns move to the
-- top platform, so BOMB and TDM fights happen on the hill. Shows `ground = true` stacking (each tier rests on
-- the one below), moving template objects, convex prisms and point lights.

local tiers = 4
local tier_h = 96
local base_d = 760       -- diameter of the lowest tier
local step = 150         -- each tier is this much narrower

-- 1. The tiers: every cylinder is grounded on the previous one (the solid top under its centre).
local top_y = 0
for i = 1, tiers do
  local d = base_d - (i - 1) * step
  map.solid.cylinder{ sides = 16, pos = { 0, 0, 0 }, size = { d, tier_h, d }, material = "block", group = "tower", ground = true }
  top_y = top_y + tier_h
end

-- 2. A ramp per tier, turning 90 degrees each time so the climb spirals. The wedge's slope rises towards its
--    local +z (yaw 0 = +z, 90 = +x), and its high edge meets the tier's rim.
local from = { { 0, -1 }, { -1, 0 }, { 0, 1 }, { 1, 0 } }   -- the side each ramp climbs from
for i = 1, tiers do
  local d = base_d - (i - 1) * step
  local reach = d / 2 + 96
  local x, z = from[i][1] * reach, from[i][2] * reach
  map.solid.wedge{ pos = { x, (i - 1) * tier_h + tier_h / 2, z }, rot = (i - 1) * 90, size = { 192, tier_h, 192 }, material = "block", group = "ramp" }
end

-- 3. Battlements around the top platform: eight low blocks on the rim.
local top_d = base_d - (tiers - 1) * step
for k = 0, 7 do
  local a = k * math.pi / 4
  local r = top_d / 2 - 40
  map.solid.box{ pos = { r * math.cos(a), 0, r * math.sin(a) }, rot = -k * 45, size = { 96, 48, 40 }, material = "block", group = "rim", ground = true }
end

-- 4. Bomb site A (number 0) and the item spawns move onto the platform; site B stays at its template spot.
for _, site in ipairs(map.query{ class = "dc.bomb_site" }) do
  if site.props.number == 0 then
    map.move(site.id, { pos = { 0, 0, 0 }, ground = true })
    map.rename(site.id, "hill")
  end
end
for k, item in ipairs(map.query{ class = "dc.item_spawn" }) do
  local a = (k - 1) * math.pi / 2 + math.pi / 4
  map.move(item.id, { pos = { 120 * math.cos(a), 0, 120 * math.sin(a) }, ground = true })
end

-- 5. Sandbag arcs at the foot of the tower (convex prisms, local x / z outline, extruded by size[2]) and four
--    lights on the tiers (a dc.point_light needs a radius > 0).
for k = 0, 3 do
  local a = k * math.pi / 2 + math.pi / 4
  local cx, cz = 620 * math.cos(a), 620 * math.sin(a)
  map.solid.prism{ pos = { cx, 0, cz }, rot = -math.deg(a), size = { 240, 48, 48 }, material = "block", group = "sandbag", ground = true,
    points = { { -120, -24 }, { 120, -24 }, { 90, 24 }, { -90, 24 } } }
  map.add{ class = "dc.point_light", pos = { 300 * math.cos(a), top_y - 48, 300 * math.sin(a) }, name = "tower_light_" .. k,
    props = { color = { rgb = { 255, 220, 180 } }, radius = 420, bright = 1.0, lights_objects = true } }
end

print(string.format("tower: %d ops, top at %d", map.ops(), top_y))
