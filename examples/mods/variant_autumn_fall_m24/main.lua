-- Made with dc-gunkit: row 539 is row 6 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_autumn_fall_m24/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon539",
        variant_of = 6,
        variant_name = "M24 Autumn Fall",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:6:PVSkin1:keep=760.5852,244.33664,43.532288/372.1042,312.99048,39.66538/376.18073,231.30649,31.692854:flow=0,0.03,0.05,7:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:6:HHSkin0:keep=163.16902,20.921082,26.636383/163.16902,276.92108,26.636383/25.911552,28.33459,29.408607/132.6926,231.05933,19.986908:flow=0,0.03,0.05,7:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
