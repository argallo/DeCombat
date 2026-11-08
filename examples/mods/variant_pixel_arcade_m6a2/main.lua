-- Made with dc-gunkit: row 534 is row 271 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_pixel_arcade_m6a2/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon534",
        variant_of = 271,
        variant_name = "M6A2 Pixel Arcade",
        variant_rarity = "rare", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:271:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:271:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
