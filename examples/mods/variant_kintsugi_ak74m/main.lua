-- Made with dc-gunkit: row 526 is row 216 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_kintsugi_ak74m/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon526",
        variant_of = 216,
        variant_name = "AK-74M Kintsugi",
        variant_rarity = "rare", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:216:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:216:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
