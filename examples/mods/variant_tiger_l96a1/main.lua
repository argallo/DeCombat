-- A weapon variant (docs/GUNKIT_PLAN.md): row 501 is the L96A1 (row 27) with a new look and nothing else. A variant may set only
-- cosmetic keys, and the game refuses anything else. The pack holds only its own pattern: the client paints it over the
-- player's own texture when the gun loads. `keep` leaves the scope's lens and the bullet unpainted: x,y,radius in texels of that
-- texture.
---@param mod ModCapability
return function(mod)
    local tiger = mod.assets:path("assets/tiger.dtx")
    mod.content.weapons:register({
        id = "Weapon501",
        variant_of = 27,
        variant_name = "L96A1 Purple Tiger",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:27:PVSkin1:keep=128,243,122/333,222,16/333,250,16/333,278,16/333,306,16/333,334,16/290,335,18:" .. tiger,
        HHSkin0 = "@camo:paint:27:HHSkin0:keep=78,152,23:" .. tiger,
    })
end
