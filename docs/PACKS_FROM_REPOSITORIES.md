# PACKS_FROM_REPOSITORIES.md: packs come from pack repositories, pinned by release and hash

Status: design, 2026-11-09. Decided: our own packs (the weapon camos, the Case Reel, the inspect pack, the community
maps) leave this repository for repositories of their own, and the game sources them from there. This note says how,
what each part trusts, and in which order the pieces land. Nothing here changes the client.

## 1. The shape

| where | what | how it reaches the game |
|---|---|---|
| a **pack repository** (ours: `DeCombat-mods`, `DeCombat-maps`; anyone's: made with `tools/modrepo.py init`) | the sources: `mods/<id>/` folders, `generators/`, the map projects | its workflow validates, lints and packs every mod on each push; a `v*` tag publishes a **release** with `<id>-<version>.zip` per pack, `packs.json` (id, version, sha256, size, kind, tags, source_repo), `SHA256SUMS`, and the rows ready to paste |
| this repository | the game, `dc-modkit`, `dc-mapkit`, `dc-gunkit`, the directory files, three example packs the tests read (`hacking_mod`, `drawn_crosshair`, `starter_map`) | the directory rows name a pack by `(id, version, sha256)` and its source repository; a row may also name the release the zip came from |
| a **lobby** | `[[mods.set]]`, `[[lobby.servers]] mods = [...]`, `[[mods.catalog]]`: explicit `(id, version, sha256)` rows, the zip at `<mods dir>/<sha256>.zip` | a sync step downloads a **tagged** release's zips, checks every one against `SHA256SUMS` and the declared hash, and installs it under its hash; the lobby itself never fetches anything |
| the **client** | unchanged | downloads from the lobby by hash, as today (MODDING_GUIDE section 7) |

The invariant that holds the design together: **a pack is its hash.** A release asset is immutable once published, a
row names a hash, the lobby serves a zip whose name is its hash, and the client refuses anything else. Where the bytes
came from is provenance, never trust: a GitHub tag can be moved, so no part of the game ever follows a branch, a
"latest" alias or a tag without the hash beside it.

## 2. The server: a sync step, not a live fetch

`tools/vps/packs_sync.sh` (proposed) runs on the host, by the operator or from the dashboard:

```
packs_sync.sh <owner/repo> <tag> [--only id,id] [--mods-dir DIR] [--dry-run]
```

1. Downloads `SHA256SUMS` and `packs.json` from `https://github.com/<owner>/<repo>/releases/download/<tag>/`.
2. For each pack (or the `--only` ones): skips it when `<mods dir>/<sha256>.zip` already exists with that hash
   (the cache is the mods dir itself); otherwise downloads `<id>-<version>.zip`, checks its SHA-256 against
   `SHA256SUMS` **and** against the `sha256` of its `packs.json` row, and installs it as `<sha256>.zip`
   (owner `decombat`, mode 0644). A mismatch leaves nothing behind and exits 1.
3. Prints the `[[mods.set]]` block or the entry's `mods = [...]` line for what it installed, so the operator pastes
   rows; it never edits `server.toml`.

`server.toml` gains an optional record of where packs come from, for the dashboard and for `dc-config check --fs`:

```toml
[[packs.sources]]
repo = "argallo/DeCombat-mods"
tag = "v1"            # a tag, never a branch; dc-config refuses "main", "master", "latest"
```

`dc-config check` accepts `owner/repo` and a tag that is not a branch name; `--fs` warns when a declared row's zip is
missing and names the source that would provide it. The dashboard's Updates tab gets a **Sync packs** action that runs
the script for every source (the same path the deploy uses: the lobby signals, a host-side script does the work) and
shows its output. Declaring stays explicit: a new release of a pack repository changes nothing on a server until an
operator pastes the new row, so a running set never moves under the players.

## 3. The review list: rows name their release

A `community/vanilla_plus.json` row keeps `id`, `version`, `sha256`, `source_repo`, `reviewer_note` and gains an
optional `release`: the release page the zip was published on
(`https://github.com/<owner>/<repo>/releases/tag/<tag>`; the asset is `<id>-<version>.zip`). `dc-config
check-vanilla-plus` checks the URL shape; `tools/modrepo.py rows --format vanilla-plus` prints it when the repository's
`modrepo.json` names the tag. Review stays what it is: a person reads the pack, the merged row is the acceptance, and
the operator installs that exact hash (section 2). `community.json` `known_mods` rows gain the same optional field.

A map pack is a `dc-mapkit export`, which is not byte-identical across machines, so for maps the release asset is the
single source of the hash: the maps repository's workflow exports and publishes the zips, and the row names that
release.

## 4. dc-gunkit: examples at run time

Today the studio bakes five example packs into its binary with `include_bytes!` (about 11 MB of DTX in every GunKit
download, and five folders this repository cannot drop). Done (VAR-13): the GunKit package ships an `examples/` folder
next to the executable, filled at package time by `tools/gunkit_examples.py` from the pack repository's release
(each zip checked against `tools/gunkit_examples.txt`); the studio reads `DC_GUNKIT_EXAMPLES`, else `examples/` next
to the program, else (under cargo) the checkout's `examples/mods`, and shows what it finds in the same menu; without a
folder the menu says so. The five folders can now leave `examples/mods` with the others once the release exists;
`assetcheck` and the export allow-lists then lose their `.dtx` exception for them.

## 5. The website: the workshop lists release packs

`web/landing/scripts/sync-packs.mjs` (like `sync-releases.mjs`) reads `packs.json` of the latest release of each
configured pack repository at build time and writes `data/packs.json`; the workshop lists those packs as "Built pack"
with the release link and the source folder. The **Verified** badge still comes only from `community.json`, so a pack
appearing on the site says nothing about trust; the text says so.

## 6. What stays as it is

- The client: it downloads from the lobby by hash and shows the Mods window from the set, the cache and the catalogue.
- The lobby's load rules (MODDING_GUIDE section 7): a declared zip missing or hashing differently stops the start.
- The three example packs the tests read, and `dc-modkit scaffold` for a new mod.

## 7. The pieces, in order (one branch each, an hour each)

1. **mods-out**: the folders leave this repository (except the three fixtures and, until step 4, the five gunkit
   examples); the review-list rows and the website links point at the pack repositories.
2. **packs-sync**: `tools/vps/packs_sync.sh` with its test (`tools/vps/tests/packs_sync_test.sh`, a local fake
   release folder), `docs/VPS.md` and `docs/OPS.md` paragraphs. No lobby change yet.
3. **packs-sources** (done, LG-434): `[[packs.sources]]` in `dc-config` (check + `--fs` hint) and the dashboard's Sync packs action
   calling the script; a correctness review at merge (lobby code).
4. **gunkit-examples**: run-time examples, the packaging scripts fill `examples/` from the release; the five folders
   leave `examples/mods`.
5. **vanilla-plus-release** (done, LG-315 amended): the optional `release` field on review-list and known-mods rows,
   the check, the 64 rows of DeCombat-mods v1 filled in (the two hand-exported maps name none), `modrepo.py rows
   --release-tag` printing it. The shipped `community.json` keeps its known-mods rows without the field: clients
   before 2026-11-09 refuse a directory row with an unknown field, so it goes in once they are gone.
6. **website-packs**: `sync-packs.mjs` and the workshop listing.

Steps 2 and 3 need a release to exist (the first tag of `DeCombat-mods`); step 1 needs the repositories pushed.
