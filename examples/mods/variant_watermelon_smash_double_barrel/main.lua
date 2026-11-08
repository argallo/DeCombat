-- Made with dc-gunkit: row 521 is row 129 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_watermelon_smash_double_barrel/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon521",
        variant_of = 129,
        variant_name = "Double Barrel Watermelon Smash",
        variant_rarity = "common", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:129:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:129:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
