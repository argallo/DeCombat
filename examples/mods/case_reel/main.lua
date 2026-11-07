-- Case Reel (CRATE-5 in DC_DEFAULTS.md): opening a supply case, a HiSec case or a Camo Crate first spins a strip of what the
-- case can hold past a centre mark, slowing down until it stops on what you got; then the award window opens as before.
-- A click, Enter or Escape stops the strip at once. The client draws the reel; this pack only switches it on and sets
-- how long a spin takes (1500 to 12000 ms). It is drawn on your own screen and changes nothing about what a case gives.
---@param mod ModCapability
return function(mod)
    mod.content.hud:override("case_reel", true)
    mod.content.hud:override("case_reel_ms", 7000)
end
