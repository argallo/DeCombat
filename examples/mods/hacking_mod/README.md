# Hacking Mod

A demo server mod where everyone hacks in the open: see players through walls (box, chams or skeleton in your own
colours), enemy nametags that stay on, health bars, separate team mate colours, and rooms with no recoil and no spread.
The chat commands are `/wallhack`, `/nametags`, `/health` and `/teammates`; `/help` in a match lists them.

## Why a declared server mod

Vanilla servers cull enemy positions a player could not plausibly see. This mod declares the capability
`world.full_visibility` (MOD-22), which turns that culling off, and `hooks.render`, which lets the client draw the
outlines. It also declares `combat.no_recoil` (MOD-23). With both, the waiting room's game settings get a Hacks
setting (LG-274): Wallhacks (default), Wallhacks + no-recoil, or No-recoil only (culling stays on in that room).
A local client mod can never do this: local mods may not use render hooks and have no world state to draw.
The server advertises the mod in its mod set, the client downloads and hash-checks it, and everyone gets the same view.

## `render.entity_visibility` context (CLI-1620)

The Lua nil-checks every field:

- `ctx.entities`: array of `{ id, kind ("player"), team, is_enemy, distance, is_local, is_visible_by_line_of_sight }`.
  The client asks about 30 times a second, not every frame (CLI-1635).
- Result: `{ entities = { { id, visible, outline, mode }, ... } }`; entries override the default visibility.
- The colours are the player's, not the mod's: enemies take the player's two picks, team mates other palette colours
  (or nothing with the player's Teammates switch off). There is no colour field in the answer.

The lobby banner key `IDS_XRAY_SERVER_BANNER` is a placeholder, not a ROM string id.

## Run it

```sh
dc-modkit pack examples/mods/hacking_mod -o hacking_mod-1.1.1.zip
sha256sum hacking_mod-1.1.1.zip     # copy the zip to the modded instance's mods dir as <sha256>.zip
```

Declare it in the instance's `server.toml` under `[[mods.set]]` (id, version, sha256), then start the instance with
`tools/vps/instance.sh modded ...`. Add a `github` field to the manifest once the mod has a repository.

## The picture in the Mods window

The package holds only its own original-pixels screenshot: a mod never contains game art. An operator can show a real
in-game frame from their own server: put `hacking_mod.preview.png` (PNG, at most 1 MiB) in the instance's mods dir, next
to the zip, and restart the lobby. It stays outside this repository and the package, and changes no hash (MOD-27).
