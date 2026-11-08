-- Made with dc-gunkit: row 558 is row 38 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_royal_marble_g23/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon558",
        variant_of = 38,
        variant_name = "G23 Royal Marble",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:38:PVSkin1:flow=0,0.015,0.09,10:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:38:HHSkin0:flow=0,0.015,0.09,10:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
