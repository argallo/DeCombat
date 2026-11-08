# Contributing

Every line of DeCombat has to be original work. [CLEANROOM.md](CLEANROOM.md) is the policy; this is the summary.

## What you may use

- Observed behaviour of the real 2009 client: captures, screenshots, logs.
- Analysis (disassembly, decompilation) of the 2009 EU client binaries you obtained yourself.
- The game's data files, read at run time. Never copied.
- The [specs](specs/README.md), our own harnesses and our own rules ([DC_DEFAULTS.md](DC_DEFAULTS.md)).
- Public algorithm and format knowledge.

## What you must not use or contribute

- Anything derived from leaked or proprietary source code of the game, its servers or its engine, or from SDK text. That
  includes ported, translated or "inspired by" code and its identifiers. Invent our own names.
- ROM data of any kind: tables, strings, shop or weapon rows, textures, model node tables, keys. Hashes only.
- Nexon binaries or assets, including in test fixtures.

If a task seems to need a forbidden source, stop and open an issue.

## Evidence tags

Every non-obvious behaviour carries a comment that names its source: `// EVIDENCE: engine+0xRVA` (or `cshell+0x...`),
`// DISASM: ...`, `// SPEC: <file> <section>`, `// PROTOCOL: <section> (PROVEN)`, `// OBSERVED: <capture or date>`,
`// HARNESS: <file>::<suite>`, `// DC-DEFAULT: <id>`.

## Build and test

You need a stable Rust toolchain (`rust-toolchain.toml`), plus the MSVC build tools on Windows or the Xcode command line
tools on macOS. The crate map is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

```sh
cargo build --release -p dc-client -p dc-lobby-server -p dc-dserver -p dc-botclient
cargo test --workspace
CA_ROM=/path/to/client/folder cargo test --workspace   # also runs the ROM-backed tests
```

`cargo_env.py` is an optional shim for a portable Windows toolchain layout (`DC_TARGET=x86_64 python cargo_env.py cargo
test --workspace`); you don't need it elsewhere.

Debug and test builds keep function names and line numbers, so panics, backtraces and hang reports point at the right
place, but they carry no variable information: a debugger shows no locals. When you need them, build with
`CARGO_PROFILE_DEV_DEBUG=true`. This keeps the test build about 40% faster and its target folder about 60% smaller.

## Before you open a pull request

- Add tests with every change. Use synthetic fixtures; ROM-backed tests must skip when `CA_ROM` is unset.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` and
  `python tools/assetcheck.py` must pass (no game files, nothing oversized, nothing marked private).
- Keep dependencies minimal and justify each in `Cargo.toml`.

## Mod API stability

The Lua mod API (`api` field in `manifest.json`, hook names, `mod.content.*` domains) is versioned. Do not change the
behaviour of an existing hook or registry verb within an API version: add new names, or bump the version and document the
migration.

## AI tools

[AGENTS.md](AGENTS.md) holds the rules for AI coding agents. The skills in `.claude/skills/` cover map and mod work. The
person who opens the pull request answers for what it contains.

## Licensing

Contributions are licensed `MIT OR Apache-2.0`, as the repository is. By submitting you state that the work is your own
and follows the rules above.
