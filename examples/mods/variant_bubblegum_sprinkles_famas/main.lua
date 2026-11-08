-- Made with dc-gunkit: row 512 is row 79 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_bubblegum_sprinkles_famas/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon512",
        variant_of = 79,
        variant_name = "FAMAS Bubblegum Sprinkles",
        variant_rarity = "common", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:79:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:79:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
