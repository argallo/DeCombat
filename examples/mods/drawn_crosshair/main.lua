-- Custom Crosshair: the player's settings become HUD override keys; the client draws the shape itself.
-- The mod sees only its own settings: no players, no positions, no world.
-- `recoil` keeps the values of version 1.0.0 ("true", "false"), so a saved choice still means the same.
local RECOIL = { ["true"] = "dynamic", ["false"] = "static", fire = "fire" }

---@param mod ModCapability
return function(mod)
    local s = mod.settings
    mod.content.hud:override("crosshair_colour", s.colour)
    mod.content.hud:override("crosshair_style", s.style)
    mod.content.hud:override("crosshair_length", s.length)
    mod.content.hud:override("crosshair_thickness", s.thickness)
    mod.content.hud:override("crosshair_gap", s.gap)
    mod.content.hud:override("crosshair_dot", s.dot)
    mod.content.hud:override("crosshair_t_style", s.t_style)
    mod.content.hud:override("crosshair_outline", s.outline and s.outline_thickness or 0)
    mod.content.hud:override("crosshair_recoil", RECOIL[s.recoil])
    mod.content.hud:override("crosshair_recoil_scale", s.recoil_scale)
end
