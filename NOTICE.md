# Notice

- DeCombat is an **unofficial**, independent, community project. It is **not affiliated with, endorsed by or
  sponsored by** Nexon, Valofe, or any other owner of the original game or its technology.
- "Combat Arms" and all related names, logos, game content and trademarks belong to their respective owners.
  They are used here only to describe what the emulator plays.
- DeCombat is an emulator: it plays the 2009 Combat Arms EU client folder (the "ROM") the way a console emulator
  plays a cartridge. This repository contains no game assets, data, binaries or keys. To use DeCombat you need your own
  copy of the original client, which you supply yourself; DeCombat reads it read-only at run time, never runs
  its executables and never redistributes it.
- The code is written from observed behaviour of the 2009 build, analysis of its binaries (disassembly),
  the game's own data files at run time, public algorithms and our own documented defaults; the source rules are in
  [CLEANROOM.md](CLEANROOM.md).
- Use DeCombat only with software and servers you are entitled to run. Do not use it to connect to or disrupt
  services you do not operate.

## Bundled front-end libraries

The lobby's operator dashboard (`crates/dc-lobby-server/src/admin_app/app.js` and `app.css`, built from `web/admin`)
contains code of the packages below. Their full licence texts are in
`crates/dc-lobby-server/src/admin_app/THIRD_PARTY_NOTICES.txt`, which the build writes and the dashboard serves at
`/ops/licenses.txt`. `tools/admin_web_check.py` fails when this list and the bundle disagree.

- `@astryxdesign/core` 0.7.7 and `@astryxdesign/theme-neutral` 0.7.7 (MIT), Copyright (c) 2026 Meta Platforms, Inc.
- `@stylexjs/stylex` 0.19.1 (MIT), Copyright (c) Meta Platforms, Inc. and affiliates.
- `react` 19.3.1, `react-dom` 19.3.1 and `scheduler` 0.28.0 (MIT), Copyright (c) Meta Platforms, Inc. and affiliates.
- `lucide-react` 1.67.0 (ISC), Copyright (c) 2026 Lucide Icons and Contributors; parts Copyright (c) 2013-present
  Cole Bemis (Feather, MIT).
- `intl-messageformat` 11.2.15 (BSD-3-Clause), Copyright (c) 2023, Oath Inc.
- `@formatjs/fast-memoize` 3.1.7, `@formatjs/icu-messageformat-parser` 3.5.18 and
  `@formatjs/icu-skeleton-parser` 2.1.11 (MIT), Copyright (c) 2023 FormatJS.
