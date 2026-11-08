-- Made with dc-gunkit: row 515 is row 80 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_extra_pepperoni_p90/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon515",
        variant_of = 80,
        variant_name = "P90 Extra Pepperoni",
        variant_rarity = "common", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:80:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:80:HHSkin0:keep=149.45407,113.95776,11.026787/10.328192,243.95686,3:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
