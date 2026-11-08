-- Made with dc-gunkit: row 510 is row 12 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_molten_core_ak47/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon510",
        variant_of = 12,
        variant_name = "AK-47 Molten Core",
        variant_rarity = "legendary", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:12:PVSkin1:flow=0.01,0.015,0.1,8:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:12:HHSkin0:flow=0.01,0.015,0.1,8:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
