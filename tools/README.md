# tools/

## assetcheck.py

    python tools/assetcheck.py              # every tracked file
    python tools/assetcheck.py --root DIR   # another checkout or folder (without git it walks the folder)

Checks what a contributor must not commit: game files by extension (`.rez`, `.dtx`, `.ltb`, `.ltc`, `.spr`, `.fxd`, `.dll`,
`.exe` and the like; a `.dat` or `.dtx` is allowed only under `examples/mods/`, where maps made with our own tools live), files
over 10 MB, private key blocks and lines holding a private marker. Python 3, standard library only. Output lines are
`ASSET FAIL kind file[:line] what`; exit 1 on any failure. Passing says nothing about where code came from
([CLEANROOM.md](../CLEANROOM.md)).

The markers are `DC-PRIVATE` and `DO-NOT-COMMIT`: put one on a line you are keeping locally and the check stops it from being
pushed.

## modrepo.py

    python tools/modrepo.py init <dir> --github <owner/repo> --decombat-rev <DeCombat tag>   # a mod repository: mods/, generators/, CI, this tool
    python tools/modrepo.py add <repo> <mod folder>... [--generator <dir>]
    python tools/modrepo.py build <repo> [--modkit <dc-modkit>] [--rom <client folder>]   # validate, lint, pack -> dist/
    python tools/modrepo.py rows <repo> --format vanilla-plus|known-mods|server-toml|catalog-toml
    python tools/modrepo.py verify <repo> --against community/vanilla_plus.json community/community.json
    python tools/modrepo.py lock <repo>                            # packs.lock.json: CI fails when its build hashes differently
    python tools/modrepo.py selftest

A pack's home is a repository of its own (docs/MODDING_GUIDE.md section 12, "Your own mod repository"). `build` packs from a
clean `git archive` copy so line endings never change a hash, writes `dist/packs.json` (id, version, sha256, size, kind, tags,
source_repo), `SHA256SUMS` and release notes; `verify` exits 1 when a built pack no longer hashes to its directory row. The
generated workflow builds `dc-modkit` with `cargo install --git` at the DeCombat tag in `modrepo.json` (a branch name is refused:
`cargo install --rev` takes a commit, and a branch moves), checks its build against the committed `packs.lock.json`, and publishes
a release on a `v*` tag. Python 3, standard library only; `selftest` needs no dc-modkit (CI runs it).

## gunkit_examples.py

    python tools/gunkit_examples.py <out dir> --release <owner/repo> <tag>    # the five example camos from a mods-repository release
    python tools/gunkit_examples.py <out dir> --from examples/mods            # or copied from a checkout (no hash check)
    python tools/gunkit_examples.py selftest

Fills the `examples/` folder the GunKit package ships next to its exes (VAR-13; `gunkit_pack_win.py --examples DIR`,
`gunkit_pack_mac.sh --examples DIR`). The list is `tools/gunkit_examples.txt` (id, version, sha256 of the release zip); a zip
that hashes differently unpacks nothing. The studio reads `DC_GUNKIT_EXAMPLES`, else `examples/` next to the program, else
(under cargo) the checkout's `examples/mods`; it bakes nothing in any more.

## ship_servers.sh

Builds this checkout and deploys it to the VPS and the game host with one command; `--dry-run` shows the plan, `--help` lists
the settings file. docs/RELEASES.md "By hand, as before". Tests: `bash tools/ship_servers_test.sh`.

## Release and deploy automation

`package_servers.sh` (the server tarball), `ci_env.sh`, `ci_check_paths.sh`, `ci_vps_ssh.sh`, `ios_release.sh`, `mac_notarise.sh`:
the scripts behind `.github/workflows/release.yml` and `deploy-servers.yml`. `host/update.sh`: a game host pulls its build.
`vps/backup.sh`, `vps/restore.sh`, `vps/healthcheck.sh`, `vps/migrate_nonroot.sh`, `lobby_soak.py`: operations, docs/OPS.md. Tests: `bash tools/vps/tests/{backup,healthcheck,migrate}_test.sh`.

`vps/deploy_remote.sh`: the VPS side of a deploy. docs/RELEASES.md. Tests: `bash tools/host/update_test.sh`.

`vps/packs_sync.sh <owner/repo> <tag> [--only id,...] [--mods-dir DIR] [--format entry|set] [--dry-run]`: installs the packs of a
pack repository's tagged release into the lobby's mods folder as `<sha256>.zip`, every zip checked against the release's
`SHA256SUMS` and the hash its `packs.json` row declares; one failure installs nothing, a re-run changes nothing, a branch name
is refused, `server.toml` is never edited (the rows to paste are printed). docs/PACKS_FROM_REPOSITORIES.md section 2. Tests:
`bash tools/vps/tests/packs_sync_test.sh` (a fake release folder, no network).
