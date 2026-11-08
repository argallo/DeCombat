-- Made with dc-gunkit: row 540 is row 42 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_stained_glass_msg90/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon540",
        variant_of = 42,
        variant_name = "MSG-90 Stained Glass",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:42:PVSkin1:keep=97.243645,99.30701,91.33383:fx=0,0.12,0.2,0.45,0:" .. mod.assets:path("assets/pattern.dtx") .. "|" .. mod.assets:path("assets/glow.dtx"),
        HHSkin0 = "@camo:paint:42:HHSkin0:keep=123.525894,133.10068,20.964317:fx=0,0.12,0.2,0.45,0:" .. mod.assets:path("assets/pattern.dtx") .. "|" .. mod.assets:path("assets/glow.dtx"),
    })
end
