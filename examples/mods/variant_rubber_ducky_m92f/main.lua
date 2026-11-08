-- Made with dc-gunkit: row 561 is row 47 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_rubber_ducky_m92f/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon561",
        variant_of = 47,
        variant_name = "M92F SE Rubber Ducky",
        variant_rarity = "common", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:47:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:47:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
