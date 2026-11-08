-- Made with dc-gunkit: row 537 is row 196 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_mirage_tar21/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon537",
        variant_of = 196,
        variant_name = "TAR-21 Mirage",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:196:PVSkin1:keep=191.7266,764.2122,40.386032:flow=0,0.02,0.035,5:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:196:HHSkin0:keep=6.9104643,209.0592,9.34832/60.708096,225.08942,14.933857:flow=0,0.02,0.035,5:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
