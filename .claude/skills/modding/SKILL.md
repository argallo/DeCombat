---
name: modding
description: Write or change a DeCombat mod (Lua and data in a folder with manifest.json) with dc-modkit, including Vanilla++ packs and new guns. Use when the user wants a mod, a weapon patch, a new gun, a HUD or crosshair mod, a server rule, or asks why dc-modkit validate or lint fails. For maps use the mapkit skill.
---

# modding: write a DeCombat mod for the user

Read `docs/MODDING_GUIDE.md` once per session: section 1 (kinds), 4 (capabilities), 5 (sandbox), 6 (the `mod.*` API),
8 (assets), 9 (testing), 12 (Vanilla++).

## The loop

1. Ask where it will run, because that decides what is allowed: Vanilla++ (reviewed, shares progress with vanilla), the
   user's own modded server (no review), or only the user's client (`client_visual`).
2. `dc-modkit scaffold <id> --profile content|overhaul|total_conversion|client_visual|map` (`cargo run -q -p dc-modkit --
   ...` from a checkout). Don't write a manifest by hand when scaffold can start it.
3. Declare only the capabilities you use. `dc-modkit lint` reports undeclared use as an error.
4. `dc-modkit validate <dir>` (add `--rom <client folder>` to check against the user's real tables), then
   `dc-modkit lint <dir>`. Fix every error. A clean validate does not show that the mod plays well.
5. `dc-modkit pack <dir> -o <id>.zip` and give the user the printed mod-set entry.
6. Tell the user what you ran and what still needs a real match.

## Hard rules

- Never put a Nexon asset, string, table or key in a mod, and nothing extracted from the client. A mod may reference ROM
  paths and transform them at run time, and it may ship its own original assets (guide section 8).
- A mod is Lua text and data. No native code, no bytecode, no file or network access beyond the sandbox.
- Don't use a `world.full_visibility` or `combat.no_recoil` style capability quietly: a mod that declares one says so in
  its description (the Hacking Mod in `examples/mods/hacking_mod` is the model).
- Don't present an unverified mod as verified. The badge comes from `community/community.json`.

## Vanilla++ packs (guide section 12)

- Kind `shared`, declaring only `content.weapons`, `content.maps`, `content.text`, `content.hud`, `assets`. The lobby and
  the client refuse anything else. That rules out `hooks.lobby`, `content.shop`, and any change to rewards, EXP, GP, NX,
  shop prices or lobby rules.
- A new gun takes a row in the reserved numbers 900..999.
- **Balance against the original guns.** Look up the closest original gun in the user's ROM and start from its numbers:
  damage, fire rate, recoil, spread, magazine, reload, movement speed. Every gain over it is paid for somewhere else, so
  a player can reasonably prefer either gun. No new best in class.
- Write the reference gun and each difference into the description. Say plainly that you have not tested it in matches,
  and ask the user to test it against the reference gun at the ranges that class is used at.
- **NX is earned, not bought:** players get it from daily login bonuses, clan wars and special events. An NX price is a
  rarity choice in line with comparable guns. Never add a way to pay real money, and never hand out NX.
- Keep the 2009 feel: handling, sounds, look and names.
- Submission is one row in `community/vanilla_plus.json` with the source repository and the zip's `sha256`.
  `dc-config check-vanilla-plus community/vanilla_plus.json` must pass, and a person reviews every pack.

## Commands at a glance

```
dc-modkit scaffold <id> --profile P [--kind server_rules|shared|client_visual]
dc-modkit validate <dir> [--rom <client folder>] [--json]
dc-modkit lint <dir> [--json]
dc-modkit pack <dir> -o out.zip [--json]
dc-modkit modset <dir>
```
