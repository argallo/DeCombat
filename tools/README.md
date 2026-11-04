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

## ship_servers.sh

Builds this checkout and deploys it to the VPS and the game host with one command; `--dry-run` shows the plan, `--help` lists
the settings file. docs/RELEASES.md "By hand, as before". Tests: `bash tools/ship_servers_test.sh`.

## Release and deploy automation

`package_servers.sh` (the server tarball), `ci_env.sh`, `ci_check_paths.sh`, `ci_vps_ssh.sh`, `ios_release.sh`, `mac_notarise.sh`:
the scripts behind `.github/workflows/release.yml` and `deploy-servers.yml`. `host/update.sh`: a game host pulls its build.
`vps/backup.sh`, `vps/restore.sh`, `vps/healthcheck.sh`, `vps/migrate_nonroot.sh`, `lobby_soak.py`: operations, docs/OPS.md. Tests: `bash tools/vps/tests/{backup,healthcheck,migrate}_test.sh`.

`vps/deploy_remote.sh`: the VPS side of a deploy. docs/RELEASES.md. Tests: `bash tools/host/update_test.sh`.
