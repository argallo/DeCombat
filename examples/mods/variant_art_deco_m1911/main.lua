-- Made with dc-gunkit: row 560 is row 1 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_art_deco_m1911/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon560",
        variant_of = 1,
        variant_name = "M1911 MEU Art Deco",
        variant_rarity = "rare", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:1:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:1:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
