-- An animated weapon variant (docs/GUNKIT_PLAN.md): row 502 is row 25 with a new look and nothing else. The pack holds only
-- its own pattern: the client paints it over the player's own texture when the gun loads. `keep` leaves parts unpainted: x,y,radius
-- in texels of that texture. `fx` adds the pattern over the gun as light: drift u, drift v, lowest gain, highest gain, seconds per pulse (0: steady).
---@param mod ModCapability
return function(mod)
    local art = mod.assets:path("assets/galaxy.dtx")
    mod.content.weapons:register({
        id = "Weapon502",
        variant_of = 25,
        variant_name = "G36E Galaxy",
        variant_rarity = "legendary", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:25:PVSkin1:keep=928,928,92:fx=0.02,0.012,0,0.65,0:" .. art,
        HHSkin0 = "@camo:paint:25:HHSkin0:keep=37,30,25:fx=0.02,0.012,0,0.65,0:" .. art,
    })
end
