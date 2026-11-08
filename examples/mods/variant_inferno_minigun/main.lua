-- Made with dc-gunkit: row 523 is row 209 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_inferno_minigun/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon523",
        variant_of = 209,
        variant_name = "M134 Minigun Inferno",
        variant_rarity = "legendary", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:209:PVSkin1:flow=0,-0.08,0.05,4:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:209:HHSkin0:flow=0,-0.08,0.05,4:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
