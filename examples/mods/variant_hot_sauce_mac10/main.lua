-- Made with dc-gunkit: row 544 is row 73 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_hot_sauce_mac10/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon544",
        variant_of = 73,
        variant_name = "MAC-10 Hot Sauce",
        variant_rarity = "common", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:73:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:73:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
