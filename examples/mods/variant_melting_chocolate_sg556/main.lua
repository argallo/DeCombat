-- Made with dc-gunkit: row 536 is row 166 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_melting_chocolate_sg556/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon536",
        variant_of = 166,
        variant_name = "SG556 Melting Chocolate",
        variant_rarity = "common", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:166:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:166:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
