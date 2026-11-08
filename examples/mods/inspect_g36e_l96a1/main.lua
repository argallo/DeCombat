-- A weapon inspect for the G36E, the L96A1 and the SCAR family (docs/MODDING_GUIDE.md, "Weapon inspect"): poses and the guns' own sounds, nothing else.
-- A variant takes its base gun's inspect, so these poses are also every camo's of these guns.
-- A pose is "ms pitch yaw roll x y z": degrees (nose up, nose right, top to the right) and a move in the view. The motion
-- runs through the poses without stopping. Keep the nose up and to the left, and the gun low: the first-person model ends
-- at the forearms, and a nose-down pose or a higher gun shows where its sleeves end.
---@param mod ModCapability
return function(mod)
    mod.content.weapons:patch(25, {
        InspectLen = 3900,
        InspectPivot = "3 -4 3",
        InspectPose1 = "480 16 -50 -3 -2.6 0.4 2.9",    -- lifted, a little too far
        InspectPose2 = "800 14 -46 -2 -2.5 0.35 2.9",       -- settles: the side of the gun
        InspectPose3 = "1500 16 -52 4 -2.4 0.2 2.9",   -- turns slowly further round
        InspectPose4 = "2300 18 -46 6 -2.5 0.2 2.8",       -- and tips towards you
        InspectPose5 = "2950 12 -40 -1 -2.3 0.25 2.4", -- on the way back
        InspectSound1 = "90 4",
        InspectSound2 = "3250 5",
    })
    mod.content.weapons:patch(27, {
        InspectLen = 4100,
        InspectPivot = "2 -2.6 2",
        InspectPose1 = "520 16 -50 -3 -1.98 0.32 2",
        InspectPose2 = "850 14 -46 -2 -1.93 0.3 2",
        InspectPose3 = "1600 12 -52 -2 -1.93 0.3 2.2",
        InspectPose4 = "2400 16 -50 4 -1.83 0.28 2.1",
        InspectPose5 = "3100 11 -38 -1 -1.73 0.25 1.7",
        InspectSound1 = "90 5",
        InspectSound2 = "3450 5",
    })
    -- the SCAR-L: the MK16 (33), the Black (49), the Trispear (99) and the Trispear LE (251) share one first-person model
    local scar_l = {
        InspectLen = 3900,
        InspectPivot = "3 -4 3",
        InspectPose1 = "480 16 -50 -3 -3.7 0.2 3.7", -- lifted, a little too far
        InspectPose2 = "800 14 -46 -2 -3.6 0.15 3.7", -- settles: the side of the gun
        InspectPose3 = "1500 16 -52 4 -3.5 0.05 3.7", -- turns slowly further round
        InspectPose4 = "2300 18 -46 6 -3.6 0.05 3.6", -- and tips towards you
        InspectPose5 = "2950 12 -40 -1 -3.3 0.05 3.2", -- on the way back
        InspectSound1 = "90 4",
        InspectSound2 = "3250 5",
    }
    for _, row in ipairs({ 33, 49, 99, 251 }) do
        mod.content.weapons:patch(row, scar_l)
    end
    -- the SCAR-H (116): a model of its own, with the left hand on the magazine well and that arm running more sideways, so
    -- it sits closer than the SCAR-L would: its sleeve's end then stays below the screen, in 4:3 and 16:9 windows alike
    mod.content.weapons:patch(116, {
        InspectLen = 3900,
        InspectPivot = "3 -4 3",
        InspectPose1 = "480 16 -50 -3 -3.5 -0.15 4.4",
        InspectPose2 = "800 14 -46 -2 -3.4 -0.2 4.4",
        InspectPose3 = "1500 16 -52 4 -3.3 -0.3 4.4",
        InspectPose4 = "2300 18 -46 6 -3.4 -0.3 4.3",
        InspectPose5 = "2950 12 -40 -1 -3.1 -0.3 3.8",
        InspectSound1 = "90 4",
        InspectSound2 = "3250 5",
    })
end
