-- canyon_river.lua: rolling terrain with a river bed that winds from -z to +z, on the flat template
-- (docs/MAPKIT_AGENTS.md section 6).
--
--   dc-mapkit new my_canyon --template flat
--   dc-mapkit run my_canyon examples/mapkit/canyon_river.lua
--   dc-mapkit render my_canyon --view top -o build/top.png
--
-- Terrain work goes through the bulk ops (gen, flatten, smooth): a handful of calls reshape the whole 64 x 64
-- grid, so the script stays far below its instruction budget. Objects placed by the template sit at the old
-- floor height; the last loop grounds them again.

-- 1. Hills over the whole map (the template's terrain keeps its 64 x 64 cells of 64 units).
map.terrain.gen{ preset = "hills", amplitude = 80, scale = 900, octaves = 4, seed = 11 }

-- 2. The river bed: a chain of flatten brushes along a sine curve, then one smoothing pass over everything.
local bed = -48
for z = -2048, 2048, 128 do
  local x = 420 * math.sin(z / 760)
  map.terrain.flatten{ x = x, z = z, radius = 300, height = bed, falloff = "smooth" }
end
map.terrain.smooth{ passes = 1 }

-- 3. Banks: a gentle ridge on each side of the bed so the river reads as a canyon from the spawns.
for z = -1800, 1800, 300 do
  local x = 420 * math.sin(z / 760)
  map.terrain.raise{ x = x - 620, z = z, radius = 260, amount = 36 }
  map.terrain.raise{ x = x + 620, z = z, radius = 260, amount = 36 }
end

-- 4. Keep the kill slab under the lowest point: the template put it 96 units under the old minimum.
local min_y = map.info().terrain.min_y
for _, s in ipairs(map.solids{ brush = "kill_volume" }) do
  map.solid.set(s.id, { pos = { 0, min_y - 96, 0 } })
end

-- 5. Two bridges and a line of stepping stones across the bed.
for _, z in ipairs({ -900, 900 }) do
  local x = 420 * math.sin(z / 760)
  map.solid.box{ pos = { x, bed + 60, z }, size = { 760, 24, 160 }, material = "block", group = "bridge" }
  map.solid.box{ pos = { x - 380, 0, z }, size = { 96, 24, 160 }, material = "block", group = "bridge", ground = true }
  map.solid.box{ pos = { x + 380, 0, z }, size = { 96, 24, 160 }, material = "block", group = "bridge", ground = true }
end
for i = -3, 3 do
  local z = i * 90
  local x = 420 * math.sin(z / 760) + (i % 2 == 0 and -40 or 40)
  map.solid.cylinder{ sides = 8, pos = { x, 0, z }, size = { 72, 32, 72 }, material = "block", group = "stone", ground = true }
end

-- 6. Crates on the banks near each team's side, mirrored on x.
for _, sx in ipairs({ -1, 1 }) do
  for _, sz in ipairs({ -1, 1 }) do
    map.solid.box{ pos = { sx * 1000, 0, sz * 600 }, size = { 80, 80, 80 }, material = "block", group = "crate", ground = true }
    map.solid.box{ pos = { sx * 1100, 0, sz * 520 }, size = { 64, 64, 64 }, material = "block", group = "crate", ground = true }
  end
end

-- 7. Ground every object on the new terrain (spawns keep their standing offset, zones their half height).
for _, o in ipairs(map.query()) do
  map.move(o.id, { ground = true })
end

local t = map.info().terrain
print(string.format("canyon: %d ops, terrain %.0f .. %.0f", map.ops(), t.min_y, t.max_y))
