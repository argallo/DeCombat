-- Made with dc-gunkit: row 525 is row 224 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_party_popper_rpg7/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon525",
        variant_of = 224,
        variant_name = "RPG-7 Party Popper",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:224:PVSkin1:fx=0,0.12,0.4,0.4,0:" .. mod.assets:path("assets/pattern.dtx") .. "|" .. mod.assets:path("assets/glow.dtx"),
        HHSkin0 = "@camo:paint:224:HHSkin0:fx=0,0.12,0.4,0.4,0:" .. mod.assets:path("assets/pattern.dtx") .. "|" .. mod.assets:path("assets/glow.dtx"),
    })
end
