-- Made with dc-gunkit: row 549 is row 119 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_shark_week_m590/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon549",
        variant_of = 119,
        variant_name = "M590 Mariners Shark Week",
        variant_rarity = "rare", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:119:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:119:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
