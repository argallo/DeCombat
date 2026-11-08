-- Made with dc-gunkit: row 527 is row 41 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_gummy_bears_ak74u/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon527",
        variant_of = 41,
        variant_name = "AK-74U Gummy Bears",
        variant_rarity = "common", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:41:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:41:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
