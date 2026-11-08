-- An animated weapon variant (docs/GUNKIT_PLAN.md): row 505 is row 33 (MK16 SCAR-L) with a new look and nothing else. The pack
-- holds only its own art: the client paints the rainbow smears and violet chrome of prism.dtx over the player's own texture when
-- the gun loads, and `flow` makes that paint roll over the gun like a lava lamp (VAR-12): drift u, drift v (repeats per second),
-- swirl (repeats), seconds per swirl. The picture was drawn by tools/camo_prism.py, with no game pixels in it.
---@param mod ModCapability
return function(mod)
    local art = mod.assets:path("assets/prism.dtx")
    mod.content.weapons:register({
        id = "Weapon505",
        variant_of = 33,
        variant_name = "SCAR-L Prism",
        variant_rarity = "legendary", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:33:PVSkin1:flow=0.03,0.01,0.1,8:" .. art,
        HHSkin0 = "@camo:paint:33:HHSkin0:flow=0.03,0.01,0.1,8:" .. art,
    })
end
