-- Made with dc-gunkit: row 514 is row 108 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_overclocked_m416/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon514",
        variant_of = 108,
        variant_name = "M416 Overclocked",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:108:PVSkin1:fx=0.06,0,0.25,0.5,2.5:" .. mod.assets:path("assets/pattern.dtx") .. "|" .. mod.assets:path("assets/glow.dtx"),
        HHSkin0 = "@camo:paint:108:HHSkin0:fx=0.06,0,0.25,0.5,2.5:" .. mod.assets:path("assets/pattern.dtx") .. "|" .. mod.assets:path("assets/glow.dtx"),
    })
end
