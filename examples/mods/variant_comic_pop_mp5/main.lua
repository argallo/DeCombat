-- Made with dc-gunkit: row 516 is row 3 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_comic_pop_mp5/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon516",
        variant_of = 3,
        variant_name = "MP5A4 Comic Pop",
        variant_rarity = "rare", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:3:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:3:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
