-- Made with dc-gunkit: row 533 is row 255 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_carbon_venom_m417/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon533",
        variant_of = 255,
        variant_name = "M417 Carbon Venom",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:255:PVSkin1:keep=429.85626,128.42957,22.940062:fx=0,0,0.03,0.45,3:" .. mod.assets:path("assets/pattern.dtx") .. "|" .. mod.assets:path("assets/glow.dtx"),
        HHSkin0 = "@camo:paint:255:HHSkin0:keep=137.95328,170.3328,12.422241/193.18924,211.12717,7.880928:fx=0,0,0.03,0.45,3:" .. mod.assets:path("assets/pattern.dtx") .. "|" .. mod.assets:path("assets/glow.dtx"),
    })
end
