-- Made with dc-gunkit: row 529 is row 21 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_tie_dye_k1a/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon529",
        variant_of = 21,
        variant_name = "K1A Tie-Dye",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:21:PVSkin1:flow=0,0,0.12,8:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:21:HHSkin0:flow=0,0,0.12,8:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
