-- Made with dc-gunkit: row 552 is row 44 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_macaw_k3/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon552",
        variant_of = 44,
        variant_name = "K3 Macaw",
        variant_rarity = "rare", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:44:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:44:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
