---
name: gunkit
description: Design a weapon camo with the dc-gunkit studio over MCP or its CLI: pick a gun, paint it live, animate it, keep a lens clean, and save a Vanilla++ pack. Use when the user wants a new camo, gun skin or animated skin, wants to see a pattern on a gun, or asks about the GunKit window. For other kinds of mod use the modding skill.
---

# gunkit: design a camo on a real gun

Read `docs/GUNKIT_AGENTS.md` once per session. It has the loop, how a pattern is painted, and what may ship.

## The tools

With the MCP server connected (`claude mcp add dc-gunkit -- <path>/dc-gunkit mcp`), the tools change the gun on the user's screen as
you call them. Without it, `dc-gunkit render` gives one picture. From a checkout: `cargo build -p dc-gunkit --bins`.

## The loop

1. `select_gun`, then write a PNG and `set_pattern` with its `path`. After that, saving over the file updates the gun by itself.
2. `screenshot` after each change and look at it. Turn the gun with `set_camera`.
3. Check both `first_person` and `third_person` with `set_view`.
4. `describe_texture` and look at its picture: a scope's lens is kept clean by itself, and `set_keep` adds or changes circles. Each gun and view has its own.
5. `save_pack` when the user is happy.

## Rules

- Patterns are your own art. Nothing of the game goes in a pack or a commit, and nothing made from it: `get_texture base` is for looking at only, and a recolour of the game's texture is still the game's.
- A pattern must tile. Use mid colours, because the gun's shading multiplies them.
- An effect must stay subtle: a camo may never hide a player or give one away.
- A variant never changes how a gun plays.
- Say what you actually looked at. A `screenshot` of an animated camo is one moment.
