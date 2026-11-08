-- Made with dc-gunkit: row 528 is row 39 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_abyss_aug_a1/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon528",
        variant_of = 39,
        variant_name = "AUG A1 Abyss",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:39:PVSkin1:keep=600.8392,584.236,67.76403/289.85498,59.994625,48.7391:fx=0,0.04,0.15,0.38,5:" .. mod.assets:path("assets/pattern.dtx") .. "|" .. mod.assets:path("assets/glow.dtx"),
        HHSkin0 = "@camo:paint:39:HHSkin0:keep=154.96332,68.81331,19.034208:fx=0,0.04,0.15,0.38,5:" .. mod.assets:path("assets/pattern.dtx") .. "|" .. mod.assets:path("assets/glow.dtx"),
    })
end
