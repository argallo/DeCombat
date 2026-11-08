-- Made with dc-gunkit: row 566 is row 188 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_marshmallow_roast_flamethrower/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon566",
        variant_of = 188,
        variant_name = "Flamethrower Marshmallow Roast",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:188:PVSkin1:flow=0,0.025,0.07,8:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:188:HHSkin0:flow=0,0.025,0.07,8:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
