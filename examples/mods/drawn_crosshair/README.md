# Custom Crosshair

A cosmetic local mod (`client_visual`). Download it from the Get mods section of the Mods screen, or copy this folder
into `<profile>/mods/local`. It is off until you enable it, and it never loads on a vanilla server or on a server that
does not allow local mods.

Its Settings page has three groups and a live preview:

- **Shape**: style (cross, circle, dot, cross + dot, circle + dot), length, thickness, gap (negative lets the arms cross),
  centre dot, T shape, black outline and its thickness. A circle takes its radius from the gap and its line from the
  thickness.
- **Colour**: 16 presets, red, green, blue and opacity.
- **Recoil**: Dynamic grows with the spread and shrinks back in step with the stock aim mark. Static keeps the weapon's
  rest size. On fire only ignores the spread and kicks for a moment on each shot. Recoil scale multiplies the growth.

Copy code and Paste code share a whole setup as text (Ctrl+C and Ctrl+V work too); a bad code changes nothing. Reset to
defaults gives back the original look. Changes apply at the next match you load. Values saved by version 1.0.0 keep
working.

The shape is drawn by the client from HUD override keys (`crosshair_style`, `crosshair_length`, ...; see
`docs/MODDING_GUIDE.md`), so it needs no picture and stays sharp at every resolution. No Lua runs per frame.
