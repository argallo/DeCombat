-- Made with dc-gunkit: row 565 is row 171 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_egg_hunt_m32/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon565",
        variant_of = 171,
        variant_name = "M32 Egg Hunt",
        variant_rarity = "common", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:171:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:171:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
