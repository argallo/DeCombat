-- Made with dc-gunkit: row 557 is row 91 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_candy_corn_g18/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon557",
        variant_of = 91,
        variant_name = "G18 Candy Corn",
        variant_rarity = "common", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:91:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:91:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
