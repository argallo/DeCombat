# CLEANROOM.md: where DeCombat's code may come from

DeCombat is published under `MIT OR Apache-2.0`, so everything in this repository must be original work. These rules
apply to all code, tests, Lua, specs, docs and tools here. [CONTRIBUTING.md](CONTRIBUTING.md) has the short version.

## 1. The model

DeCombat works like a console emulator. The player supplies the 2009 Combat Arms EU client folder (the "ROM"), which
DeCombat opens read-only and checks by hash. Every asset, string table, game table, key and binary stays in that folder
and is read at run time. Nothing from it is committed.

## 2. Allowed sources

| source | what you may take from it | tag in the code |
|---|---|---|
| Observed behaviour of the real 2009 client against our servers: packet captures, screenshots, logs | anything the client sends, accepts or displays | `// OBSERVED: <capture or date>` |
| Analysis of the 2009 binaries you obtained yourself (Engine.exe, CShell.dll, ClientFx.fxd, CRes.dll): disassembly and decompiler output | algorithms, constants, formats, state machines, re-expressed in your own structure and names | `// EVIDENCE: cshell+0x1eca0d` (module + rva), `// DISASM: engine+0x216a0` |
| The specs in [specs/](specs/README.md) | wire layouts, formats, harness contract | `// SPEC: PROTOCOL_2009.md §3.2`, `// PROTOCOL: <section> (PROVEN)` |
| The game's own data files, at run time | everything; read it, never copy it | none needed |
| Our own test harnesses | fixtures and expected behaviour | `// HARNESS: <file>::<suite>` |
| Public format and algorithm knowledge (Blowfish, zlib, PE, SHA-256, independent format descriptions) | algorithms | name or link the reference |
| Our own decisions where the 2009 client gives no evidence | documented defaults, overridable by config or mod | `// DC-DEFAULT: <id>`, a row in [DC_DEFAULTS.md](DC_DEFAULTS.md) |

Decompiler output can be wrong: confirm it against observed behaviour. Never paste decompiled text into the tree.

## 3. Forbidden sources

Never open or use these for anything you contribute, and never contribute anything derived from them, including ports,
translations and "inspired by" rewrites:

- leaked or otherwise proprietary source code of the game, its servers or its engine, from any year;
- engine or platform SDK text (headers, samples, interface shapes), including Microsoft SDK headers. Constants come from
  public documentation.

Names follow the same rule: invent your own. Wire message ids and their 2009 names are interop facts and are allowed.

If a task seems to need a forbidden source, stop and open an issue.

## 4. What must never be in the tree

- ROM data of any kind: byte tables, string tables, default configs, word filters, shop or weapon rows, model node tables,
  texture names copied from the client. Read them from the ROM at run time (`dc-rom`, `dc-rez`, `dc-bute`). Tests use
  synthetic fixtures, or run against the ROM when `CA_ROM` is set and are skipped otherwise.
- Nexon binaries or assets, including in test fixtures.
- Decrypt keys. `dc-rom` reads the key from the user's own client by address and accepts it only if its SHA-256 equals
  the pinned digest. Only the digest is committed.

### 4a. Dependencies

Third-party crates are allowed when they are vetted and needed. Keep the list small, justify each in `Cargo.toml`, keep
`Cargo.lock` committed. Licences must be compatible with MIT, Apache-2.0, BSD or zlib.

## 5. The gate

`python tools/assetcheck.py` must pass on every change. It refuses game files by extension, oversized files, private
keys and strings marked private. See [tools/README.md](tools/README.md). It cannot tell where a piece of code came from:
that part of these rules rests on each contributor and on review.

## 6. Status words

"Implemented", "merged", "unit-tested" and "tested against the real 2009 client" mean different things. Docs and pull
requests name the one that is true.

## ROM fingerprint

These five files identify the ROM. Files you added to the folder are ignored.

| file (relative to the client folder) | bytes | sha256 |
|---|---|---|
| `Engine.exe` | 3046144 | bb9a1e41b430a09ab8722e1de31f89dc53a00e5a96abbe236f65c4ceddcf574f |
| `Game/CShell.dll` | 4855296 | 2de3e2f4e1a762f0a484f427560acac62742341ff7ccf8d6b1e95cae73cb3b50 |
| `Game/ClientFx.fxd` | 208896 | bd10e4afda2f01870640f97cee4376750b8e3dee032117db525c128d7a1667b6 |
| `Game/CRes.dll` | 163840 | 62362a78fe5266ccd3d3056f8525fc3591fd550ad10f048cd9ef995626ddc539 |
| `ltmsg.dll` | 7168 | 884c44e82cc56f2325be6b45de1a778afd38130b472e0471e534cc00672fbdf4 |
