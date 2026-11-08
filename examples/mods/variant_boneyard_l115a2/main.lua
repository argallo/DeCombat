-- Made with dc-gunkit: row 538 is row 275 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_boneyard_l115a2/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon538",
        variant_of = 275,
        variant_name = "L115A2 Boneyard",
        variant_rarity = "rare", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:275:PVSkin1:keep=130.0439,241.5954,126.25942:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:275:HHSkin0:keep=78.788994,154.98367,24.867626:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
