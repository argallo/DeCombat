# Modern look: ideas on the shelf

Options that would make DeCombat look more modern without touching the 2009 game files or play. Noted
2026-11-04 from a conversation with the owner; nothing here is scheduled. The rules that bound every item:

- No shipped Nexon pixels or models (CLEANROOM.md, MODDING_GUIDE section 8). An effect may transform the player's
  own game files in memory at load, as the camo paint does (DC_DEFAULTS VAR-3), but never write them out.
- Visual-only and the same for everyone on an entry. Anything that changes how visible a player is (shadows,
  lighting, bloom on bodies) is reviewed by the owner first, like a camo pack.
- Off by default on the vanilla entry: vanilla looks like 2009. A "Modern" preset in Options turns the set on.

## In rough order of visible payoff for effort

1. **Texture enhance at load.** Upscale the ROM's textures 2-4x in memory when they load (pixel-art-aware or a
   small neural upscaler), with full mipmaps and 16x anisotropic filtering. Nothing shipped. Costs load time and
   VRAM. The biggest single change: blurry 256x256 walls are what date the game most.
2. **"Modern" post-processing preset.** Screen-space ambient occlusion, mild sharpen, light bloom on bright surfaces
   and the sky, a slight contrast / colour grade, SMAA over the existing MSAA. A few passes over the finished frame;
   cheap; backend-neutral.
3. **Dynamic character shadows.** A shadow map from each map's sun direction (we already read the global light) in
   place of blob shadows. Gameplay-neutral only if everyone has it: review first.
4. **Per-pixel lighting on models.** Normal maps generated from the textures at load (dc-mapkit already bakes normal
   maps for its own models), so guns and walls catch light instead of flat shading.
5. **Effects.** Soft particles for smoke (no hard cuts into walls), longer-lived impact decals, shell ejection, muzzle
   flashes that light the surroundings for a frame.
6. **Water and sky.** Animated sky; reflection / refraction on water maps (Waverider).
7. **Render scale, HUD scale, FOV.** Render the 3D view below native and the HUD at native (the fps gain of a lower
   display mode without a mode switch, and it works on Mac); a FOV slider. Not "modern looks" but expected.

## Related notes from the same conversation

- **Exclusive fullscreen:** not built, on purpose. A borderless window covering the screen gets Windows' independent
  flip, the same direct path exclusive mode used to be the only way to get; Vulkan on Windows has no exclusive mode
  at all. Exclusive still matters only for changing the display resolution (item 7 covers that need) or when
  something stops independent flip from engaging (an overlay, a window on top). Worth checking once with PresentMon
  that our window does get independent flip.
- **DLSS / frame generation:** not wanted. Its fixed per-frame cost (~0.5-1 ms) is as much as our whole frame; the
  game is not GPU-bound; the temporal smoothing blurs exactly the sharp low-res look; NVIDIA-only with a binary SDK
  that does not fit an open clean-room project. Render scale (item 7) gives the low-end gain instead.
- **Renderer backend:** DirectX 12 is the Windows default (user decision 2026-11-06, after a Vulkan driver crash on a GTX 960); Vulkan is the opt-in (it was
  the default for its lower cost on low-end PCs);
  macOS is Metal only. The Graphic tab's Renderer row picks between them (next start), and `--adapter dx12|vulkan` does for
  one run, also on the friend pack's `DeCombat.exe` (a shortcut with it appended to the target).
