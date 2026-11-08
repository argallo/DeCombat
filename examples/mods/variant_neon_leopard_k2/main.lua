-- Made with dc-gunkit: row 530 is row 43 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_neon_leopard_k2/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon530",
        variant_of = 43,
        variant_name = "K2 Neon Leopard",
        variant_rarity = "rare", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:43:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:43:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
