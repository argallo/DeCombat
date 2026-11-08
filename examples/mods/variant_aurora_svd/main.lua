-- Made with dc-gunkit: row 518 is row 95 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_aurora_svd/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon518",
        variant_of = 95,
        variant_name = "Dragunov SVD Aurora",
        variant_rarity = "legendary", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:95:PVSkin1:keep=873.61945,890.3158,131.49736/67.9511,793.2544,66.68381:flow=0.02,0,0.07,6:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:95:HHSkin0:keep=120.9056,235.02823,20.51696:flow=0.02,0,0.07,6:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
