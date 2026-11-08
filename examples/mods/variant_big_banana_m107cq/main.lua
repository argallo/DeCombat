-- Made with dc-gunkit: row 519 is row 257 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_big_banana_m107cq/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon519",
        variant_of = 257,
        variant_name = "M107CQ Big Banana",
        variant_rarity = "common", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:257:PVSkin1:keep=865.71826,681.09973,64.8887/881.6507,427.3833,50.738785/849.3972,518.90485,44.656742:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:257:HHSkin0:keep=187.7248,76.10304,19.416885:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
