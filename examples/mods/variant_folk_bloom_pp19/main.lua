-- Made with dc-gunkit: row 546 is row 152 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_folk_bloom_pp19/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon546",
        variant_of = 152,
        variant_name = "PP-19 Folk Bloom",
        variant_rarity = "rare", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:152:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:152:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
