-- Made with dc-gunkit: row 548 is row 40 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_honey_heist_ump/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon548",
        variant_of = 40,
        variant_name = "UMP Honey Heist",
        variant_rarity = "rare", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:40:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:40:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
