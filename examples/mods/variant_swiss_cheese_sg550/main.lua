-- Made with dc-gunkit: row 535 is row 104 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_swiss_cheese_sg550/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon535",
        variant_of = 104,
        variant_name = "SG550 Swiss Cheese",
        variant_rarity = "common", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:104:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:104:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
