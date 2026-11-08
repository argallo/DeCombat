-- Made with dc-gunkit: row 513 is row 149 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_koi_pond_aug_a3/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon513",
        variant_of = 149,
        variant_name = "AUG A3 Koi Pond",
        variant_rarity = "legendary", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:149:PVSkin1:flow=0.02,0.01,0.04,6:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:149:HHSkin0:flow=0.02,0.01,0.04,6:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
