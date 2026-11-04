-- starter_map 0.1.0: a DeCombat community map exported by dc-mapkit 0.1.0 from the project "Starter Map".
-- Generated file: edit the project (map.json) and export again rather than this file.
---@param mod ModCapability
return function(mod)
    mod.content.maps:register({
        id = "starter_map",
        name = "Starter Map",
        description = "Starter Map: a DeCombat community map made with dc-mapkit from the starter template.",
        world = mod.assets:path("assets/maps/starter_map.dat"),
        modes = { "TDM", "FFA", "ELIM", "BOMB", "FLAG" },
        players = { min = 2, max = 16 },
        radar = { picture = mod.assets:path("assets/pictures/starter_map_radar.dtx"),
                  min = { -2434.7825, -2434.7825 }, max = { 2434.7825, 2434.7825 } },
    })
end
