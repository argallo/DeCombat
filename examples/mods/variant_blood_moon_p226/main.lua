-- Made with dc-gunkit: row 562 is row 61 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_blood_moon_p226/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon562",
        variant_of = 61,
        variant_name = "P226 Blood Moon",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:61:PVSkin1:fx=0.04,0.08,0.3,0.5,0:" .. mod.assets:path("assets/pattern.dtx") .. "|" .. mod.assets:path("assets/glow.dtx"),
        HHSkin0 = "@camo:paint:61:HHSkin0:fx=0.04,0.08,0.3,0.5,0:" .. mod.assets:path("assets/pattern.dtx") .. "|" .. mod.assets:path("assets/glow.dtx"),
    })
end
