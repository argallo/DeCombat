-- Made with dc-gunkit: row 541 is row 135 with a new look and nothing else. The pack holds only its own art, which the client
-- paints over the player's own texture when the gun loads. Drawn by tools/camos/variant_liquid_mercury_psg1/gen.py.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:register({
        id = "Weapon541",
        variant_of = 135,
        variant_name = "PSG-1 Liquid Mercury",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:135:PVSkin1:keep=504.23907,343.30933,67.70297:flow=0,0.04,0.1,5:" .. mod.assets:path("assets/pattern.dtx"),
        HHSkin0 = "@camo:paint:135:HHSkin0:keep=201.27655,25.134592,20.050652/158.5202,24.574976,19.777763:flow=0,0.04,0.1,5:" .. mod.assets:path("assets/pattern.dtx"),
    })
end
