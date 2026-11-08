-- A weapon variant (docs/GUNKIT_PLAN.md): row 500 is the G36E (row 25) with a new look and nothing else. A variant may set only
-- cosmetic keys, and the game refuses anything else. The pack holds only its own pattern: the client paints it over the
-- player's own texture when the gun loads. `keep` leaves the scope's lens unpainted: x,y,radius in texels of that texture.
---@param mod ModCapability
return function(mod)
    local candy = mod.assets:path("assets/candy.dtx")
    mod.content.weapons:register({
        id = "Weapon500",
        variant_of = 25,
        variant_name = "G36E Candy",
        variant_rarity = "epic", -- CRATE-6: how often a Camo Crate gives it and how its opening is shown
        PVSkin1 = "@camo:paint:25:PVSkin1:keep=928,928,92:" .. candy,
        HHSkin0 = "@camo:paint:25:HHSkin0:keep=37,30,25:" .. candy,
    })
end
