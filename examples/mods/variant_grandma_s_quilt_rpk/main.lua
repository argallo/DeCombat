-- Made with dc-gunkit: row 555 is row 220 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_grandma_s_quilt_rpk/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon555",
        variant_of = 220,
        variant_name = "RPK Grandma's Quilt",
        variant_rarity = "common", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:220:PVSkin1:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:220:HHSkin0:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
