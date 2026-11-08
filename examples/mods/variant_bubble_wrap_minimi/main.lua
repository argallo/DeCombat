-- Made with dc-gunkit: row 553 is row 26 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_bubble_wrap_minimi/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon553",
        variant_of = 26,
        variant_name = "MINIMI Bubble Wrap",
        variant_rarity = "common", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:26:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:26:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
