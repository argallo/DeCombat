-- Freight Hall: an indoor warehouse from the user's plan, sized for 3v3. Models and textures: NVIDIA SimReady.
local K = 60 -- map units per metre: the 100 unit tall player is a person
map.meta{
  description="A small freight warehouse for 3v3: red walls split the hall, a railed balcony overlooks them from the north and a low covered corridor flanks it on the south.",
  players={min=2,max=6},
}
-- each of these leaves a bullet hole that shows (`dc-mapkit rom surfaces` says which do): plastic as rubber, cardboard and wrap as drywall
local SURF = { metal=20, tin=21, wood=30, paper=31, plastic=91, ceramic=13, concrete=11, cloth=31 }

-- measurements, from the plan at 2.1 units per pixel: about 5 seconds from a spawn to the middle at run speed
local AX, AZ = 1370, 737       -- the hall's half sizes
local H = 480                  -- roof
local L2 = 200                 -- balcony floor; a standing eye up there is near 285
local BZ = AZ + 168            -- balcony back wall
local SX0, SX1, SZ = 1096, 695, AZ + 426 -- stairwell outer x, inner x, north wall
local IN, DIV = SX1 + 190, SX1 + 210 -- the inner flight ends, the divider ends; the outer flight runs to SX0
local LZ = SZ - 100            -- the landing starts here
local SY = 400                 -- stairwell ceiling
local HX, HZ, HY = 777, AZ + 256, 200 -- corridor half length, south wall, ceiling
local DX, DW, DH = 620, 150, 170 -- door centre, width, height
local RED, OR = 200, 150       -- wall heights: the balcony sees over both, the floor over neither
local RX, OX, OW = 113, 1010, 25 -- red half width, orange centre and half width

map.material.set{ id="ground", texture="assets/wh_34.png", tile=420, surface_id=11 }
map.material.set{ id="wall", texture="assets/sw_0.png", tile=640, surface_id=11 }
map.material.set{ id="block", texture="assets/wh_35.png", tile=512, surface_id=11 }
map.material.set{ id="sky", texture="assets/wh_8.png" }
map.material.set{ id="m_red", texture="assets/paint_red.png", tile=420, surface_id=11 }
map.material.set{ id="m_orange", texture="assets/paint_orange.png", tile=420, surface_id=11 }
map.material.set{ id="m_deck", texture="assets/wh_29.png", tile=300, surface_id=20 }
map.material.set{ id="m_step", texture="assets/wh_35.png", tile=300, surface_id=11 }
map.material.set{ id="m_steel", texture="assets/wh_5.png", tile=200, surface_id=20 }
map.material.set{ id="m_roof", texture="assets/wh_6.png", tile=512, surface_id=20 }
map.material.set{ id="m_upper", texture="assets/sw_3.png", tile=640, surface_id=11 }
map.material.set{ id="core_53", texture="assets/wh_5.png", tile=384, surface_id=53, flags={ solid=true, no_step=false, sky=false, hidden=true } }
map.material.set{ id="core_20", texture="assets/wh_5.png", tile=384, surface_id=20, flags={ solid=true, no_step=false, sky=false, hidden=true } }
local paint = { solid=false, no_step=false, sky=false }
-- thin collision over paint: bullet marks land on it, in front of the paint, and it casts no shadow
local skin = { solid=true, no_step=false, sky=false, hidden=true, skin=true }
map.material.set{ id="hit_11", texture="assets/sw_1.png", tile=512, surface_id=11, flags=skin }
map.material.set{ id="hit_20", texture="assets/wh_5.png", tile=200, surface_id=20, flags=skin }
map.material.set{ id="t_grime_up", texture="assets/clad.png", tile=420, surface_id=20, flags=paint }
map.material.set{ id="t_hazard", texture="assets/hazard.png", tile=72, surface_id=20, flags=paint }
map.material.set{ id="t_cap", texture="assets/en_8.png", tile=256, surface_id=20, flags=paint }
map.material.set{ id="t_top", texture="assets/sw_1.png", tile=420, surface_id=11, flags=paint }
map.material.set{ id="m_line", texture="assets/wh_12.png", tile=256, surface_id=11, flags=paint }
map.material.set{ id="t_dark", texture="assets/wh_5.png", tile=384, surface_id=11, flags=paint }
map.material.set{ id="t_grime", texture="assets/sw_1.png", tile=512, surface_id=11, flags=paint }
map.material.set{ id="t_yellow", texture="assets/wh_12.png", tile=256, surface_id=20, flags=paint }
map.material.set{ id="t_blue", texture="assets/wh_36.png", tile=256, surface_id=11, flags=paint }
map.material.set{ id="t_orange", texture="assets/paint_orange.png", tile=160, surface_id=20, flags=paint }
map.material.set{ id="t_steel", texture="assets/wh_5.png", tile=200, surface_id=20, flags=paint }
map.material.set{ id="t_duct", texture="assets/wh_29.png", tile=240, surface_id=20, flags=paint }
map.material.set{ id="t_light", texture="assets/wh_8.png", tile=256, surface_id=20, flags=paint }
map.material.set{ id="t_rust", texture="assets/sw_7.png", tile=128, surface_id=20, flags=paint }
map.material.set{ id="t_slab_a", texture="assets/wh_34.png", tile=420, surface_id=11, flags=paint }
map.material.set{ id="t_slab_b", texture="assets/sw_3.png", tile=512, surface_id=11, flags=paint }
map.material.set{ id="t_slab_c", texture="assets/wh_35.png", tile=460, surface_id=11, flags=paint }
map.material.set{ id="t_joint", texture="assets/wh_5.png", tile=128, surface_id=11, flags=paint }

-- a solid by its bounds
local function box(x0,x1,y0,y1,z0,z1,mat,group)
  return map.solid.box{ pos={(x0+x1)/2,(y0+y1)/2,(z0+z1)/2}, size={x1-x0,y1-y0,z1-z0}, material=mat, group=group or "shell" }
end
local function both(x0,x1,y0,y1,z0,z1,mat,group)
  box(x0,x1,y0,y1,z0,z1,mat,group)
  box(-x1,-x0,y0,y1,z0,z1,mat,group)
end

-- the shell
map.terrain.gen{ preset="flat", amplitude=0, base=0, cells_x=24, cells_z=20, cell_size=128 }
local X, Z = 1536, 1280
map.solid.set("solid_1", { pos={0,H/2,Z+32}, size={2*X+128,H+128,64} })
map.solid.set("solid_2", { pos={0,H/2,-Z-32}, size={2*X+128,H+128,64} })
map.solid.set("solid_3", { pos={X+32,H/2,0}, size={64,H+128,2*Z+128} })
map.solid.set("solid_4", { pos={-X-32,H/2,0}, size={64,H+128,2*Z+128} })
map.solid.set("solid_5", { pos={0,H+96,0}, size={2*X+128,64,2*Z+128} })
map.solid.set("solid_7", { pos={0,-96,0}, size={2*X+256,64,2*Z+256} })
map.settings{ bake_lightmap=10, fog_enable=false, far_z=8192, sun_dir={-0.42,-0.82,0.34}, sun_color={255,240,214}, sun_bright=1.5, ambient={150,158,172} }

-- hall walls, east and west
both(AX,AX+64,0,H,-AZ-64,AZ+64,"wall")
-- the south wall with its two doors
box(-AX,-DX-DW/2,0,H,-AZ-40,-AZ,"wall")
box(-DX+DW/2,DX-DW/2,0,H,-AZ-40,-AZ,"wall")
box(DX+DW/2,AX,0,H,-AZ-40,-AZ,"wall")
both(DX-DW/2,DX+DW/2,DH,H,-AZ-40,-AZ,"wall")
-- door frames: paint, nothing to catch on, standing clear of the wall's faces all round so nothing flickers
both(DX-DW/2-12,DX-DW/2+3,0,DH+12,-AZ-46,-AZ+4,"t_steel","door")
both(DX+DW/2-3,DX+DW/2+12,0,DH+12,-AZ-46,-AZ+4,"t_steel","door")
both(DX-DW/2+3,DX+DW/2-3,DH-3,DH+12,-AZ-46,-AZ+4,"t_steel","door")
-- the corridor
box(-HX,HX,0,1,-HZ,-AZ-40,"t_slab_c","corridor")
box(-HX,HX,0,HY+40,-HZ-40,-HZ,"wall","corridor")
box(-HX-40,-HX,0,HY+40,-HZ-40,-AZ-40,"wall","corridor")
box(HX,HX+40,0,HY+40,-HZ-40,-AZ-40,"wall","corridor")
box(-HX-40,HX+40,HY,HY+40,-HZ-40,-AZ-40,"m_roof","corridor")
box(-HX,HX,88,102,-HZ,-HZ+3,"t_blue","corridor")
box(-HX,HX,0,88,-HZ,-HZ+2,"t_grime","corridor")
box(-HX,-DX-DW/2-12,0,88,-AZ-42,-AZ-40,"t_grime","corridor")
box(-DX+DW/2+12,DX-DW/2-12,0,88,-AZ-42,-AZ-40,"t_grime","corridor")
box(DX+DW/2+12,HX,0,88,-AZ-42,-AZ-40,"t_grime","corridor")

-- the corridor's floor: slabs and joints like the hall's, and a worn line down the middle
do
  local kinds = { "t_slab_b", "t_slab_a", "t_slab_c", "t_slab_a" }
  local k = 0
  for x0 = -HX, HX-1, 300 do
    k = k + 1
    box(x0,math.min(x0+300,HX),0.5,1.5,-HZ,-AZ-40,kinds[k % #kinds + 1],"floor")
    box(x0-3,x0+3,1.5,2.5,-HZ,-AZ-40,"t_joint","floor")
  end
  box(-HX,HX,1.5,2.5,-AZ-112,-AZ-104,"m_line","floor")
end

-- the north side: a solid front, the deck on top, the back wall above it
box(-SX1,SX1,0,L2-2,AZ,BZ,"block","balcony")
box(-SX1,SX1,L2-2,L2,AZ,BZ,"m_deck","balcony")
box(-SX1+40,SX1-40,L2,H,BZ,BZ+40,"wall","balcony")
-- stairwells: entrance at ground on the outer half, up, across the landing, back up to the inner half
for _, s in ipairs({-1,1}) do
  local function b(x0,x1,y0,y1,z0,z1,mat,g)
    if s < 0 then box(-x1,-x0,y0,y1,z0,z1,mat,g) else box(x0,x1,y0,y1,z0,z1,mat,g) end
  end
  b(SX0,AX,0,H,AZ,AZ+40,"wall","north")                     -- hall wall outside the stairwell
  b(DIV,SX0,DH+20,H,AZ,AZ+40,"wall","north")                -- over the entrance
  b(IN,DIV,0,SY,AZ+40,LZ,"wall","stairs")                   -- divider, behind the wall above it
  b(IN,DIV,0,H,AZ,AZ+40,"wall","north")                     -- the end of the divider and the wall above it
  b(SX0,SX0+40,0,SY+40,AZ+40,SZ+40,"wall","stairs")         -- outer wall, behind the hall wall
  b(SX1-40,SX1,0,SY+40,BZ,SZ+40,"wall","stairs")            -- inner wall behind the balcony end
  b(SX1-40,SX0+40,0,SY+40,SZ,SZ+40,"wall","stairs")         -- north wall
  b(DIV,SX0,SY,SY+40,AZ,SZ,"m_steel","stairs")               -- ceiling over the outer flight
  b(SX1,DIV,SY,SY+40,BZ,SZ,"m_steel","stairs")               -- ceiling over the inner flight
  b(SX1,IN,0,L2-2,AZ,BZ,"block","stairs")                     -- the inner platform, level with the deck
  b(SX1,IN,L2-2,L2,AZ,BZ,"m_deck","stairs")
  b(SX1,SX0,0,L2/2,LZ,SZ,"m_step","stairs")                 -- the landing
  b(SX0-2,SX0,88,102,AZ+40,SZ,"t_blue","trim")
  b(SX1-40,SX0,88,102,SZ-2,SZ,"t_blue","trim")
  -- a steel frame round the entrance, standing clear of the wall's faces
  b(DIV-4,DIV+12,0,DH+24,AZ-5,AZ+44,"t_steel","door")
  b(SX0-12,SX0+4,0,DH+24,AZ-5,AZ+44,"t_steel","door")
  b(DIV+12,SX0-12,DH+8,DH+24,AZ-5,AZ+44,"t_steel","door")
  -- a lamp in a cage over the entrance and one on the landing
  b((DIV+SX0)/2-28,(DIV+SX0)/2+28,DH+30,DH+42,AZ-6,AZ-1,"t_light","light")
  b(SX0-4,SX0-1,L2/2+90,L2/2+130,LZ+20,LZ+60,"t_light","light")
  local N = 6
  local run = LZ - BZ
  for i = 1, N do
    local top = (L2/2) * i / N
    b(DIV,SX0,0,top,BZ + run*(i-1)/N,LZ,"m_step","steps")   -- outer flight, rising north
    b(SX1,IN,0,L2/2+top,BZ,LZ - run*(i-1)/N,"m_step","steps") -- inner flight, rising south
    -- a worn yellow edge on every step
    b(DIV,SX0,top,top+1.5,BZ + run*(i-1)/N,BZ + run*(i-1)/N + 9,"m_line","trim")
    b(SX1,IN,L2/2+top,L2/2+top+1.5,LZ - run*(i-1)/N - 9,LZ - run*(i-1)/N,"m_line","trim")
  end
end

-- the plan's walls: three red, four shorter orange
for _, r in ipairs({ {349,AZ}, {-193,195}, {-AZ,-346} }) do
  box(-RX,RX,0,RED,r[1],r[2],"m_red","red")
  box(-RX-2,RX+2,0,20,r[1]-2,r[2]+2,"t_dark","trim")
  box(-RX-2,RX+2,RED-8,RED+2,r[1]-2,r[2]+2,"t_cap","trim")
  box(-RX+8,RX-8,RED+2,RED+4,r[1]+8,r[2]-8,"t_top","trim")
  -- corner guards where a forklift would hit
  for _, sx in ipairs({-1,1}) do
    local x0, x1 = math.min(sx*(RX-14),sx*(RX+3)), math.max(sx*(RX-14),sx*(RX+3))
    if r[1] > -AZ then box(x0,x1,20,132,r[1]-3,r[1]+14,"t_hazard","trim") end
    if r[2] < AZ then box(x0,x1,20,132,r[2]-14,r[2]+3,"t_hazard","trim") end
  end
end
for _, r in ipairs({ {130,388}, {-454,-195} }) do
  for _, s in ipairs({-1,1}) do
    box(s*OX-OW,s*OX+OW,0,OR,r[1],r[2],"m_orange","orange")
    box(s*OX-OW-2,s*OX+OW+2,0,18,r[1]-2,r[2]+2,"t_dark","trim")
    box(s*OX-OW-2,s*OX+OW+2,OR-5,OR+2,r[1]-2,r[2]+2,"t_cap","trim")
    box(s*OX-OW-3,s*OX+OW+3,18,104,r[1]-3,r[1]+10,"t_hazard","trim")
    box(s*OX-OW-3,s*OX+OW+3,18,104,r[2]-10,r[2]+3,"t_hazard","trim")
  end
end

-- the floor: poured slabs of three concretes with dark joints, so it never reads as one repeating picture
local kinds = { "t_slab_a", "t_slab_b", "t_slab_a", "t_slab_c", "t_slab_a" }
local SLAB = 460
local row = 0
for z0 = -AZ, AZ-1, SLAB do
  row = row + 1
  local col = 0
  for x0 = -AX, AX-1, SLAB do
    col = col + 1
    local kind = kinds[(col * 3 + row * 2 + (col * row) % 3) % #kinds + 1]
    box(x0, math.min(x0+SLAB,AX), 0, 1, z0, math.min(z0+SLAB,AZ), kind, "floor")
  end
  box(-AX, AX, 1, 2, z0-3, z0+3, "t_joint", "floor")
end
for x0 = -AX, AX, SLAB do
  box(x0-3, x0+3, 1, 2, -AZ, AZ, "t_joint", "floor")
end
-- safety lines
both(880,894,1,3,-AZ,AZ,"m_line","paint")
both(RX+70,RX+84,1,3,-AZ,AZ,"m_line","paint")

-- walls: grime at the foot, a blue stripe, bare concrete above the stripe's band, steel columns
both(AX-2,AX,0,92,-AZ,AZ,"t_grime","trim")
both(SX0,AX,0,92,AZ-2,AZ,"t_grime","trim")
both(DX+DW/2+12,AX,0,92,-AZ,-AZ+2,"t_grime","trim")
box(-DX+DW/2+12,-RX,0,92,-AZ,-AZ+2,"t_grime","trim")
box(RX,DX-DW/2-12,0,92,-AZ,-AZ+2,"t_grime","trim")
both(AX-3,AX,92,104,-AZ,AZ,"t_blue","trim")
both(SX0,AX,92,104,AZ-3,AZ,"t_blue","trim")
both(DX+DW/2+12,AX,92,104,-AZ,-AZ+3,"t_blue","trim")
box(-DX+DW/2+12,-RX,92,104,-AZ,-AZ+3,"t_blue","trim")
box(RX,DX-DW/2-12,92,104,-AZ,-AZ+3,"t_blue","trim")
both(AX-2,AX,104,H,-AZ,AZ,"t_grime_up","trim")
both(SX0,AX,104,H,AZ-2,AZ,"t_grime_up","trim")
both(DIV,SX0,DH+20,H,AZ-2,AZ,"t_grime_up","trim")
box(-SX1+40,SX1-40,L2+58,H,BZ-2,BZ,"t_grime_up","trim")
box(-AX,AX,DH+12,H,-AZ,-AZ+2,"t_grime_up","trim")
both(DX+DW/2+12,AX,104,DH+12,-AZ,-AZ+2,"t_grime_up","trim")
box(-DX+DW/2+12,DX-DW/2-12,104,DH+12,-AZ,-AZ+2,"t_grime_up","trim")
-- steel columns along the long walls and the north wall, things you can hug
for _, z in ipairs({-600,-200,200,600}) do
  both(AX-26,AX,0,H,z-20,z+20,"m_steel","column")
end
for _, x in ipairs({-1250,-360,360,1250}) do
  box(x-20,x+20,0,H,-AZ,-AZ+26,"m_steel","column")
end
both(1180,1220,0,H,AZ-26,AZ,"m_steel","column")
-- a run of pipes along the south wall and the two ends of the north wall, with brackets, out of reach
for _, y in ipairs({300,322}) do
  box(-AX,AX,y,y+10,-AZ+30,-AZ+40,"t_rust","pipe")
  both(SX0,AX,y,y+10,AZ-40,AZ-30,"t_rust","pipe")
end
for x = -1300, 1300, 260 do
  box(x-3,x+3,292,338,-AZ+24,-AZ+42,"t_dark","pipe")
end
-- frosted windows high on the end walls: the daylight's other way in
for _, z in ipairs({-430,0,430}) do
  both(AX-4,AX,350,450,z-120,z+120,"t_light","window")
  both(AX-6,AX,346,350,z-126,z+126,"t_dark","window")
  both(AX-6,AX,450,454,z-126,z+126,"t_dark","window")
end

-- the roof, with two skylights the sun comes through
local SKY = { x0=480, x1=860, z0=-190, z1=190 }
local function roof(x0,x1,z0,z1) box(x0,x1,H,H+18,z0,z1,"m_roof","roof") end
roof(-AX-64,AX+64,SKY.z1,BZ+64)
roof(-AX-64,AX+64,-AZ-64,SKY.z0)
roof(-AX-64,-SKY.x1,SKY.z0,SKY.z1)
roof(-SKY.x0,SKY.x0,SKY.z0,SKY.z1)
roof(SKY.x1,AX+64,SKY.z0,SKY.z1)
for _, s in ipairs({-1,1}) do
  local a, b = math.min(s*SKY.x0,s*SKY.x1), math.max(s*SKY.x0,s*SKY.x1)
  box(a-8,b+8,H-10,H,SKY.z0-8,SKY.z0,"t_dark","roof")
  box(a-8,b+8,H-10,H,SKY.z1,SKY.z1+8,"t_dark","roof")
  box(a-8,a,H-10,H,SKY.z0,SKY.z1,"t_dark","roof")
  box(b,b+8,H-10,H,SKY.z0,SKY.z1,"t_dark","roof")
end

-- lamps
local function lamp(x, y, z, rgb, radius, bright)
  map.add{ class="dc.point_light", pos={x,y,z}, props={ color={rgb=rgb}, radius=radius, bright=bright, lights_objects=true } }
end
local COOL, WARM, SODIUM = {226,236,255}, {255,226,186}, {255,190,120}

-- a model; it is solid unless told otherwise
local function put(name, x, z, rot, o)
  o = o or {}
  map.asset{ asset=name, pos={x,o.y or 0,z}, rot=rot or 0, scale=K*(o.s or 1), solid=o.solid ~= false, surface=SURF[o.surf or "wood"] }
end
-- on both sides: the same size class, two looks
local function pair(a, b, x, z, rot, o)
  put(a, -x, z, rot, o)
  put(b, x, z, -(rot or 0), o)
end
-- a pallet with a load of boxes: seen as parts, one simple block to fight around
local function turned(x, z, rot, dx, dz)
  local a = math.rad(rot)
  return x + dx*math.cos(a) + dz*math.sin(a), z - dx*math.sin(a) + dz*math.cos(a)
end
local BOX = { "card_a", "card_b", "card_c" }
local function load(x, z, rot, layers, y0, seed)
  y0 = y0 or 0
  rot = rot or 0
  put("pallet", x, z, rot, {y=y0, solid=false})
  local n = seed or 1
  for l = 0, layers-1 do
    for _, o in ipairs({ {-17,-12},{17,-12},{-17,12},{17,12} }) do
      if not (l == layers-1 and o[1] > 0 and o[2] > 0) then
        local bx, bz = turned(x, z, rot, o[1], o[2])
        n = n % #BOX + 1
        put(BOX[n], bx, bz, rot + (n % 2) * 3, {y=y0+11+l*25, solid=false, s=0.8})
      end
    end
  end
  map.solid.box{ pos={x,y0+(11+layers*25)/2,z}, rot=rot, size={70,11+layers*25,48}, material="core_53", group="core" }
end
local function loads(x, z, rot, layers, y0)
  load(x, z, rot, layers, y0, 1)
  load(-x, z, -rot, layers, y0, 2)
end
-- a rack with boxes on its shelves; the rack is one solid block
local SHELF = { rack_c={27, 140} }
local function stocked(name, x, z, rot)
  put(name, x, z, rot, {surf="metal"})
  local span = 76
  local n = 1
  for _, h in ipairs(SHELF[name]) do
    for _, d in ipairs({-span, 0, span}) do
      n = n + 1
      if (n + math.floor(x)) % 4 ~= 0 then
        local bx, bz = turned(x, z, rot, d, 0)
        put(BOX[n % #BOX + 1], bx, bz, rot + n * 7, {y=h, solid=false, s=0.9})
      end
    end
  end
end
local function stocks(name, x, z, rot)
  stocked(name, x, z, rot)
  stocked(name, -x, z, -rot)
end

-- north of each half: a forklift by a few pallets
pair("fork_a","fork_b", 610, 520, 25, {surf="metal"})
loads(770, 560, 10, 2)
loads(800, 480, 35, 3)
-- middle of each half: a crate fort
pair("plyw_big","plyw_big", 610, -30, 90)
pair("plyw_long","plyw_long", 610, -50, 85, {y=61})
pair("plyw_tall","plyw_tall", 680, 100, 10)
pair("plyw_cube","plyw_cube", 540, 110, 25)
-- beside the red walls
pair("drum_s","drum_b", 300, 60, 0, {surf="metal"})
pair("drum_w","drum_s", 345, 95, 0, {surf="plastic"})
pair("stacker","tilt", 350, -210, 200, {surf="metal"})
-- south of each half: crates and wrapped loads, clear of the doors
pair("plyw_big","plyw_big", 340, -600, 0)
pair("plyw_tall","plyw_tall", 300, -500, 12)
pair("ply_flat","ply_flat", 440, -500, 25)
pair("wrap_blue","wrap_yellow", 800, -350, 10, {surf="cloth"})
pair("box_big","box_big", 790, -440, 30, {surf="paper"})
-- the gaps in the red walls: cover on the way through
pair("bar_orange","bar_orange", 240, 272, 90, {surf="plastic"})
pair("plyw_long","plyw_long", 240, -270, 80)
-- racks on the red walls and the walls of the bases
stocks("rack_c", RX+65, 560, 90)
stocks("rack_c", AX-70, 600, 90)
stocks("rack_c", AX-70, -630, 90)
pair("bin_blue","bin_green", AX-50, -360, 90, {surf="plastic"})
pair("plyw_tall","plyw_tall", 1240, 690, 15)
pair("drum_b","drum_w", 960, -640, 0, {surf="plastic"})
pair("drum_s","drum_b", 1005, -680, 0, {surf="metal"})
pair("ladder","ladder", 790, 680, 70, {surf="metal"})
pair("wetsign","warnsign", 1140, 640, 30, {surf="plastic"})
pair("ibc","ibc", 1230, 300, 0, {surf="metal"})
pair("extinguisher","extinguisher", 1320, -180, 90, {surf="metal"})
put("extinguisher", -RX-12, 330, 90, {surf="metal"})
put("extinguisher", RX+12, -330, -90, {surf="metal"})
-- a long duct under the roof, on hangers
box(-AX+10,AX-10,H-74,H-26,-440,-380,"t_duct","duct")
for x = -1300, 1300, 200 do
  box(x-2,x+2,H-26,H,-444,-440,"t_dark","duct")
  box(x-2,x+2,H-26,H,-380,-376,"t_dark","duct")
  box(x-4,x+4,H-78,H-74,-446,-374,"t_dark","duct")
end
pair("fence","fence", OX, 412, 90, {surf="metal"})
pair("fence","fence", OX, -478, 90, {surf="metal"})
put("board", -AX+5, 0, 0, {y=52, solid=false})
put("board", AX-5, 0, 180, {y=52, solid=false})
put("board", -270, -HZ+6, -90, {y=48, solid=false})
put("board", 270, -HZ+6, -90, {y=48, solid=false})

-- paint: faces +z at rot 0, so rot says which way a wall looks; 1.5 off the wall
local SHEET = { gf_rush={264,132}, gf_three={264,132}, gf_crew={228,114}, gf_cover={216,108}, gf_skull={144,144},
  gf_tag={360,90}, gf_camp={360,90}, gf_up={132,66}, gf_high={132,66}, gf_marks={162,162}, gf_flank={264,132},
  gf_wrong={192,96}, gf_dead={192,96} }
local function paint(name, x, y, z, rot, mat)
  map.asset{ asset=name, pos={x,y,z}, rot=rot, scale=K, solid=false }
  -- marks land just in front of the paint
  local w, h = SHEET[name][1]/2, SHEET[name][2]
  mat = mat or "hit_11"
  if rot == 90 then box(x-6,x+1,y,y+h,z-w,z+w,mat,"hit")
  elseif rot == -90 then box(x-1,x+6,y,y+h,z-w,z+w,mat,"hit")
  elseif rot == 0 then box(x-w,x+w,y,y+h,z-6,z+1,mat,"hit")
  else box(x-w,x+w,y,y+h,z-1,z+6,mat,"hit") end
end
-- bullet marks: thin collision in front of everything painted, as a mark lands on what the shot hit
do
  local W, FLOOR = 3.2, 3.2
  box(-AX,AX,0,FLOOR,-AZ,AZ,"hit_11","hit")
  box(-HX,HX,0,FLOOR,-HZ,-AZ-40,"hit_11","hit")
  both(DX-DW/2,DX+DW/2,0,FLOOR,-AZ-40,-AZ,"hit_11","hit")
  both(DIV,SX0,0,FLOOR,AZ,BZ,"hit_11","hit")
  -- hall walls: concrete below the stripe, steel above
  both(AX-W,AX,0,104,-AZ,AZ,"hit_11","hit")
  both(AX-W,AX,104,H,-AZ,AZ,"hit_20","hit")
  both(SX0,AX,0,104,AZ-W,AZ,"hit_11","hit")
  both(SX0,AX,104,H,AZ-W,AZ,"hit_20","hit")
  both(DIV,SX0,DH+20,H,AZ-W,AZ,"hit_20","hit")
  box(-SX1+40,SX1-40,L2+58,H,BZ-W,BZ,"hit_20","hit")
  both(DX+DW/2+12,AX,0,104,-AZ,-AZ+W,"hit_11","hit")
  box(-DX+DW/2+12,DX-DW/2-12,0,104,-AZ,-AZ+W,"hit_11","hit")
  box(-AX,AX,DH+12,H,-AZ,-AZ+W,"hit_20","hit")
  both(DX+DW/2+12,AX,104,DH+12,-AZ,-AZ+W,"hit_20","hit")
  box(-DX+DW/2+12,DX-DW/2-12,104,DH+12,-AZ,-AZ+W,"hit_20","hit")
  -- door frames, as drawn
  both(DX-DW/2-12.2,DX-DW/2+3.2,0,DH+12.2,-AZ-46.2,-AZ+4.2,"hit_20","hit")
  both(DX+DW/2-3.2,DX+DW/2+12.2,0,DH+12.2,-AZ-46.2,-AZ+4.2,"hit_20","hit")
  both(DX-DW/2+3.2,DX+DW/2-3.2,DH-3.2,DH+12.2,-AZ-46.2,-AZ+4.2,"hit_20","hit")
  both(DIV-4.2,DIV+12.2,0,DH+24.2,AZ-5.2,AZ+44.2,"hit_20","hit")
  both(SX0-12.2,SX0+4.2,0,DH+24.2,AZ-5.2,AZ+44.2,"hit_20","hit")
  both(DIV+12.2,SX0-12.2,DH+8,DH+24.2,AZ-5.2,AZ+44.2,"hit_20","hit")
  -- corridor walls
  box(-HX,HX,0,102,-HZ,-HZ+W,"hit_11","hit")
  box(-HX,-DX-DW/2-12,0,88,-AZ-43.2,-AZ-40,"hit_11","hit")
  box(-DX+DW/2+12,DX-DW/2-12,0,88,-AZ-43.2,-AZ-40,"hit_11","hit")
  box(DX+DW/2+12,HX,0,88,-AZ-43.2,-AZ-40,"hit_11","hit")
  -- the walls of the plan: foot, cap and corner guards
  for _, r in ipairs({ {349,AZ}, {-193,195}, {-AZ,-346} }) do
    box(-RX-2.2,RX+2.2,0,20,r[1]-2.2,r[2]+2.2,"hit_11","hit")
    box(-RX-2.2,RX+2.2,RED-8,RED+4.2,r[1]-2.2,r[2]+2.2,"hit_20","hit")
    for _, sx in ipairs({-1,1}) do
      local x0, x1 = math.min(sx*(RX-14.2),sx*(RX+3.2)), math.max(sx*(RX-14.2),sx*(RX+3.2))
      if r[1] > -AZ then box(x0,x1,20,132,r[1]-3.2,r[1]+14.2,"hit_20","hit") end
      if r[2] < AZ then box(x0,x1,20,132,r[2]-14.2,r[2]+3.2,"hit_20","hit") end
    end
  end
  for _, r in ipairs({ {130,388}, {-454,-195} }) do
    for _, s in ipairs({-1,1}) do
      box(s*OX-OW-2.2,s*OX+OW+2.2,0,18,r[1]-2.2,r[2]+2.2,"hit_11","hit")
      box(s*OX-OW-2.2,s*OX+OW+2.2,OR-5,OR+2.2,r[1]-2.2,r[2]+2.2,"hit_20","hit")
      box(s*OX-OW-3.2,s*OX+OW+3.2,18,104,r[1]-3.2,r[1]+10.2,"hit_20","hit")
      box(s*OX-OW-3.2,s*OX+OW+3.2,18,104,r[2]-10.2,r[2]+3.2,"hit_20","hit")
    end
  end
  -- the posters, windows and hanging panels, which shots otherwise pass
  box(-AX+5-0.2,-AX+6.4,48,142,-65,65,"hit_11","hit")
  box(AX-6.4,AX-5+0.2,48,142,-65,65,"hit_11","hit")
  for _, x in ipairs({-270,270}) do box(x-65,x+65,44,138,-HZ,-HZ+7.4,"hit_11","hit") end
  for _, z in ipairs({-210,210}) do
    for _, x in ipairs({-820,-280,280,820}) do
      box(x-110.2,x+110.2,H-96.2,H-89.8,z-60.2,z+60.2,"hit_20","hit")
    end
  end
  for _, x in ipairs({-560,-190,190,560}) do
    box(x-70.2,x+70.2,HY-6.2,HY,-AZ-190.2,-AZ-109.8,"hit_20","hit")
  end
  -- overhead: the duct and the pipes, which shots otherwise pass
  box(-AX+10,AX-10,H-74.2,H-25.8,-440.2,-379.8,"hit_20","hit")
  for _, y in ipairs({300,322}) do
    box(-AX,AX,y-0.2,y+10.2,-AZ+29.8,-AZ+40.2,"hit_20","hit")
  end
  for _, y in ipairs({160,174}) do
    box(-HX,HX,y-0.2,y+7.2,-HZ+4.8,-HZ+13.2,"hit_20","hit")
  end
end

-- every piece once, each wholly on its wall (the build warns of `paint.off_wall` otherwise)
paint("gf_rush", -RX-1.5, 24, -40, -90)
paint("gf_three", RX+1.5, 30, 35, 90)
paint("gf_marks", -400, 30, AZ-1.5, 180)
paint("gf_skull", RX+1.5, 36, -440, 90)
paint("gf_crew", -OX+OW+1.5, 24, 259, 90)
paint("gf_cover", OX-OW-1.5, 24, -325, -90)
paint("gf_camp", 0, L2+70, BZ-3.5, 180, "hit_20")
paint("gf_tag", 320, 34, AZ-1.5, 180)
paint("gf_up", DIV+1.5, 34, AZ+110, 90)
paint("gf_high", -DIV-1.5, 34, AZ+110, -90)
paint("gf_flank", 300, 22, -AZ-43.5, 180)
paint("gf_wrong", HX-1.5, 40, -885, -90)
paint("gf_dead", -HX+1.5, 40, -885, 90)

-- the balcony: rails along its front, cover on the deck
-- steel rails: two bars and posts, with a block behind them that stops you and covers a crouched player
local rails = 0
local RH = 58
box(-IN,IN,L2+RH-8,L2+RH,AZ+6,AZ+14,"t_orange","rail")
box(-IN,IN,L2+RH/2-4,L2+RH/2+4,AZ+6,AZ+14,"t_orange","rail")
for x = -IN, IN, 150 do
  box(x-6,x+6,L2,L2+RH+3,AZ+3,AZ+17,"t_orange","rail")
  rails = rails + 1
end
map.solid.box{ pos={0,L2+RH/2,AZ+10}, size={2*IN,RH,10}, material="core_20", group="core" }
loads(470, BZ-40, 8, 2, L2)
pair("drum_s","drum_s", 280, BZ-30, 0, {y=L2, surf="metal"})
pair("plyw_long","plyw_long", 120, BZ-34, 0, {y=L2})

-- the corridor: lockers along the wall, things to get round
-- doors and bench to the corridor, backs to the wall
pair("locker_b","locker_b", 470, -HZ+26.5, 180, {surf="metal"})
pair("locker_b","locker_b", 390, -HZ+26.5, 180, {surf="metal"})
pair("plyw_cube","plyw_cube", 190, -AZ-140, 15)
pair("box_big","box_big", 680, -HZ+36, 10, {surf="paper"})
put("bin_blue", 0, -HZ+40, 0, {surf="plastic"})
put("extinguisher", -130, -HZ+20, 0, {surf="metal"})
put("extinguisher", 610, -HZ+20, 0, {surf="metal"})
put("drum_s", -610, -HZ+36, 0, {surf="metal"})
put("drum_b", -566, -HZ+32, 0, {surf="plastic"})
-- services along the corridor, out of reach
for _, y in ipairs({160,174}) do
  box(-HX,HX,y,y+7,-HZ+5,-HZ+13,"t_rust","pipe")
end
for x = -720, 720, 180 do
  box(x-3,x+3,156,184,-HZ+2,-HZ+15,"t_steel","pipe")
end
for _, x in ipairs({-560,-190,190,560}) do
  box(x-70,x+70,HY-6,HY,-AZ-190,-AZ-110,"t_light","light")
end

-- overhead: trusses, a duct, the panels; two panels are dead, which is what real warehouses look like
for _, z in ipairs({-440,440}) do
  for _, x in ipairs({-1100,-550,0,550,1100}) do
    put("beam", x, z, 0, {y=H-88, solid=false, surf="metal", s=0.7, solid=false})
  end
end
local dead = { ["-820,-210"]=true, ["280,210"]=true }
for _, z in ipairs({-210,210}) do
  for _, x in ipairs({-820,-280,280,820}) do
    box(x-110,x+110,H-96,H-90,z-60,z+60, dead[x..","..z] and "t_cap" or "t_light", "light")
    if not dead[x..","..z] then lamp(x, H-110, z, COOL, 900, 0.85) end
  end
end
-- warm work lights over the forklift bays and the crate forts, sodium at the base walls: every area its own colour
for _, s in ipairs({-1,1}) do
  lamp(s*700, 300, 620, SODIUM, 480, 0.9)
  lamp(s*620, 300, -560, WARM, 460, 0.7)
  lamp(s*1220, 310, 600, COOL, 520, 0.6)
  lamp(s*1220, 310, -600, SODIUM, 520, 0.75)
  -- what the frosted windows let in
  lamp(s*(AX-60), 400, -220, {214,226,255}, 620, 0.55)
  lamp(s*(AX-60), 400, 220, {214,226,255}, 620, 0.55)
  -- stairwells: one lamp a flight, dim and warm
  lamp(s*(SX0-90), 320, BZ+40, WARM, 460, 0.8)
  lamp(s*(SX1+100), 340, LZ, WARM, 420, 0.7)
  -- the balcony
  lamp(s*480, L2+220, AZ+110, WARM, 520, 0.7)
  -- a green exit light over each door, a red one by the stairs
  lamp(s*DX, DH+30, -AZ+26, {90,255,140}, 200, 0.3)
  lamp(s*1000, 210, AZ-20, {255,70,50}, 200, 0.35)
end
-- the corridor: dim pools with gaps between them
for _, x in ipairs({-560,560}) do
  lamp(x, HY-14, -AZ-150, WARM, 400, 0.75)
end
for _, x in ipairs({-190,190}) do
  lamp(x, HY-14, -AZ-150, COOL, 280, 0.28)
end

-- the objects
local t1, t2, tn = {}, {}, {}
for _, o in ipairs(map.query{ class="dc.player_spawn" }) do
  if o.props.team == "1" then t1[#t1+1] = o elseif o.props.team == "2" then t2[#t2+1] = o else tn[#tn+1] = o end
end
local STAND = 54
local grid = { {-1270,-440},{-1270,-170},{-1270,170},{-1270,430},{-1110,-470},{-1110,-170},{-1110,170},{-1110,470} }
for i, o in ipairs(t1) do map.move(o.id, { grid[i][1], STAND, grid[i][2] }) end
for i, o in ipairs(t2) do map.move(o.id, { -grid[i][1], STAND, grid[i][2] }) end
local free = {
  {-1200,0},{1200,0},{-480,380},{480,380},{-560,-270},{560,-270},{-880,-620},{880,-620},
  {-300,-870},{300,-870},{-560,830},{560,830},{-220,0},{220,0},{-1170,-680},{1170,-680},
}
for i, o in ipairs(tn) do
  local x, z = free[i][1], free[i][2]
  local y = STAND
  if z > AZ then y = L2 + STAND end
  map.move(o.id, { x, y, z })
end
for _, o in ipairs(map.query{}) do
  local c = o.class
  if c == "dc.bomb_site" then
    local z = (o.props.number == 0) and 300 or -330
    map.move(o.id, { 620, 48, z })
  elseif c == "dc.flag_zone" or c == "dc.capture_flag" then
    local sx = (o.props.team == "1") and -1 or 1
    map.move(o.id, { sx*1210, o.pos[2], 0 })
  end
end
local items = { {-480,660},{480,660},{-90,-870},{90,-870} }
local k = 0
for _, o in ipairs(map.query{ class="dc.item_spawn" }) do
  k = k + 1
  map.move(o.id, { items[k][1], 0, items[k][2] })
end
local info = map.info()
print(string.format("freight hall: %d ops, %d rails, spawns %d / %d", map.ops(), rails, info.spawns.team1, info.spawns.team2))
