-- Made with dc-gunkit: row 550 is row 125 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_clockwork_r870/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon550",
        variant_of = 125,
        variant_name = "R870 Clockwork",
        variant_rarity = "rare", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:125:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:125:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
