-- Made with dc-gunkit: row 563 is row 10 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_black_opal_usp/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon563",
        variant_of = 10,
        variant_name = "USP Black Opal",
        variant_rarity = "legendary", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:10:PVSkin1:flow=0,0.03,0.1,6:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:10:HHSkin0:flow=0,0.03,0.1,6:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
