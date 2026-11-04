-- arena_symmetric.lua: a mirrored arena on the flat template (docs/MAPKIT_AGENTS.md section 6).
--
--   dc-mapkit new my_arena --template flat
--   dc-mapkit run my_arena examples/mapkit/arena_symmetric.lua
--   dc-mapkit validate my_arena
--
-- The flat template already carries the contract (walls, sky, kill slab, 8 + 8 + 16 spawns, 2 bomb sites,
-- 2 flag zones + flags, 4 item spawns); this script adds the play space: two raised bases, a central plateau
-- with four ramps, and cover that is mirrored on x so both teams get the same sight lines. Every call queues
-- one op; the batch is applied when the script ends (all or nothing).

-- Place a solid on both sides of x = 0. `s` is the table a map.solid.<kind>{} call takes; the yaw flips with x.
local function mirror(kind, s)
  for _, sx in ipairs({ -1, 1 }) do
    local t = {}
    for k, val in pairs(s) do t[k] = val end
    t.pos = { s.pos[1] * sx, s.pos[2], s.pos[3] }
    if s.rot then t.rot = s.rot * sx end
    map.solid[kind](t)
  end
end

-- 1. Team bases: a 640 x 64 x 1664 slab under each team's spawn line (x = +-1536).
mirror("box", { pos = { 1536, 0, 0 }, size = { 640, 64, 1664 }, material = "block", group = "base", ground = true })

-- 2. Centre plateau with a ramp on each side. A wedge's slope rises towards its local +z; yaw 0 = +z,
--    90 = +x, so the ramp on the -z side keeps yaw 0, the one on +z turns 180, and so on.
map.solid.box{ pos = { 0, 0, 0 }, size = { 512, 96, 512 }, material = "block", group = "plateau", ground = true }
local ramps = {
  { pos = { 0, 0, -352 }, rot = 0 },
  { pos = { 0, 0, 352 }, rot = 180 },
  { pos = { -352, 0, 0 }, rot = 90 },
  { pos = { 352, 0, 0 }, rot = -90 },
}
for _, r in ipairs(ramps) do
  map.solid.wedge{ pos = r.pos, rot = r.rot, size = { 192, 96, 192 }, material = "block", group = "ramp", ground = true }
end

-- 3. Mirrored cover between the bases and the plateau: low walls, crates and two round pillars per side.
for _, sz in ipairs({ -1, 1 }) do
  mirror("box", { pos = { 640, 0, sz * 320 }, size = { 128, 96, 320 }, material = "block", group = "cover", ground = true })
  mirror("box", { pos = { 1024, 0, sz * 700 }, size = { 96, 64, 96 }, material = "block", group = "crate", ground = true })
  mirror("box", { pos = { 1024, 0, sz * 700 }, size = { 64, 64, 64 }, material = "block", group = "crate", ground = true })
  mirror("wedge", { pos = { 900, 0, sz * 1100 }, rot = 90, size = { 192, 80, 192 }, material = "block", group = "cover", ground = true })
end
mirror("cylinder", { sides = 12, pos = { 360, 0, 0 }, size = { 160, 160, 160 }, material = "block", group = "pillar", ground = true })

-- 4. Side lanes along z = +-1500: a long low wall with gaps, the same on both x sides.
for _, sz in ipairs({ -1, 1 }) do
  for i = 0, 2 do
    mirror("box", { pos = { 300 + i * 400, 0, sz * 1500 }, size = { 256, 80, 64 }, material = "block", group = "lane", ground = true })
  end
end

-- 5. Re-ground every object: the bases lifted the floor under the spawns, zones and flags (`ground = true`
--    snaps to the highest solid top or the terrain, plus the standing offset).
for _, o in ipairs(map.query()) do
  map.move(o.id, { ground = true })
end

local info = map.info()
print(string.format("arena: %d ops queued, spawns %d / %d / %d", map.ops(), info.spawns.team1, info.spawns.team2, info.spawns.any))
