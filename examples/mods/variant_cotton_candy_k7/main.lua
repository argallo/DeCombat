-- Made with dc-gunkit: row 543 is row 45 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_cotton_candy_k7/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon543",
        variant_of = 45,
        variant_name = "K7 Cotton Candy",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:45:PVSkin1:flow=0,0.04,0.08,5:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:45:HHSkin0:flow=0,0.04,0.08,5:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
