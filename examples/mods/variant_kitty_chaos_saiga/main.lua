-- Made with dc-gunkit: row 551 is row 128 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_kitty_chaos_saiga/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon551",
        variant_of = 128,
        variant_name = "Saiga 20K Kitty Chaos",
        variant_rarity = "rare", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:128:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:128:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
