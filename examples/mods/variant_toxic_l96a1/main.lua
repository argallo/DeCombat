-- An animated weapon variant (docs/GUNKIT_PLAN.md): row 503 is row 27 with a new look and nothing else. The pack holds only
-- its own pattern: the client paints it over the player's own texture when the gun loads. `keep` leaves parts unpainted: x,y,radius
-- in texels of that texture. `fx` adds the pattern over the gun as light: drift u, drift v, lowest gain, highest gain, seconds per pulse (0: steady).
---@param mod ModCapability
return function(mod)
    -- the dark paint, then the veins the effect moves over it
    local art = mod.assets:path("assets/toxic_base.dtx") .. "|" .. mod.assets:path("assets/toxic.dtx")
    mod.content.weapons:register({
        id = "Weapon503",
        variant_of = 27,
        variant_name = "L96A1 Toxic",
        variant_rarity = "legendary", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:27:PVSkin1:keep=128,243,122/333,222,16/333,250,16/333,278,16/333,306,16/333,334,16/290,335,18:fx=0.035,0.02,0,1,0:" .. art,
        HHSkin0 = "@camo:paint:27:HHSkin0:keep=78,152,23:fx=0.035,0.02,0,1,0:" .. art,
    })
end
