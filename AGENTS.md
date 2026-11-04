# AGENTS.md: rules for AI coding agents

## In every case

- Nothing of the game goes into a commit, a mod or a map: no Nexon assets, no strings, tables, keys or binaries, and nothing
  extracted from the client folder (the "ROM"). Read it at run time.
- Never use leaked or proprietary source code of the game or its engine, or SDK text, and don't "recall" it either.
  [CLEANROOM.md](CLEANROOM.md) is binding.
- Say what you actually ran. "It builds", "the tests pass" and "it played in a match" are different claims.

## Changing DeCombat itself

Follow [CONTRIBUTING.md](CONTRIBUTING.md). Keep the provenance tags you find (`EVIDENCE`, `DISASM`, `SPEC`,
`DC-DEFAULT`), and tag what you add. `python tools/assetcheck.py` must pass. Don't invent 2009 behaviour: without
evidence a rule is our own default and gets a row in [DC_DEFAULTS.md](DC_DEFAULTS.md).

## Writing a mod

Use `.claude/skills/modding/SKILL.md`. The reference is [docs/MODDING_GUIDE.md](docs/MODDING_GUIDE.md). Check each pack with
`dc-modkit validate` and `dc-modkit lint`. For a Vanilla++ pack, section 12 of the guide sets the limits:

- The pack is kind `shared` and declares only `content.weapons`, `content.maps`, `content.text`, `content.hud` and `assets`.
- A new gun starts from the numbers of the closest original gun, pays for every gain with a trade-off, and is never the new
  best in class. Name the reference gun and each difference in the pack's description.
- NX is earned in play here, never bought, so an NX price is a rarity choice. Don't add anything that sells NX or
  changes rewards, EXP, GP, NX, shop prices or lobby rules.

## Building a map

Use `.claude/skills/mapkit/SKILL.md`. The playbook is [docs/MAPKIT_AGENTS.md](docs/MAPKIT_AGENTS.md). Look at the rendered
picture before you say a map is done.
