-- A weapon inspect for two guns (docs/MODDING_GUIDE.md, "Weapon inspect"): poses and the guns' own sounds, nothing else.
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
end
