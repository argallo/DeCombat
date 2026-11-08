-- Made with dc-gunkit: row 542 is row 22 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_python_sr25/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon542",
        variant_of = 22,
        variant_name = "SR25 Python",
        variant_rarity = "rare", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:22:PVSkin1:keep=875.3357,81.94253,49.073994/114.88819,100.06732,42.116066:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:22:HHSkin0:keep=218.60442,20.224895,15.958496:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
