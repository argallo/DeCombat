# RELEASES.md: one build for every machine

Three workflows, none of which has run on GitHub yet at the time of writing (they were validated with actionlint and the
scripts with shell tests; see "Not yet proven" at the end).

| workflow | when | what |
|---|---|---|
| `ci.yml` | pull requests, pushes to master, by hand | builds and tests on every platform, fmt, clippy, `tools/assetcheck.py`, the deploy script tests |
| `release.yml` | a tag `v*`; pushes to master that touch the code (artifacts only); by hand | every package; on a tag, one GitHub Release |
| `deploy-servers.yml` | by hand only | moves the VPS and the game hosts to a commit |

A release does not touch a server, and a deploy does not publish a release.

## Cutting a release

    git tag v0.2.0 && git push origin v0.2.0

The run publishes one Release with these assets (`<tag>` in each name), SHA256SUMS and notes generated from the merged
pull requests:

| asset | what |
|---|---|
| `DeCombat-win-<tag>.zip` | dc-client.exe (static C runtime, no build machine paths, DeCombat icon and file details), DeCombat.bat, README |
| `DeCombat-win-<tag>-bots.zip` | dc-botclient.exe and bots.bat, unzipped into the same folder; kept apart so antivirus heuristics about the bot tool never touch the game |
| `DeCombat-mac-<tag>.zip` | DeCombat.app, universal, README |
| `DeCombat-iOS-<tag>.ipa` or `DeCombat-iOS-<tag>-unsigned.ipa` | the iPad app, see "iOS"; optional: a failed iOS build leaves it out |
| `DeCombat-MapKit-win-<tag>.zip`, `DeCombat-MapKit-mac-<tag>.zip` | the community map builder |
| `decombat-servers-linux-x86_64-<tag>.tar.gz` | the servers, for people hosting their own |

**All or nothing.** The publishing job needs every build job. It refuses a tag that is not on master, checks each asset is
there exactly once, attaches them to a draft, compares the draft's list with the build's, and publishes last. Any failure
leaves no public release (at most a draft, replaced by the next run). A release that is already published is refused: delete
it first to replace it. A tag with a hyphen (`v0.2.0-rc1`) is marked a pre-release. No release build restores a cache.

The Windows and macOS zips are preset to the public lobby from the variables below, so they go on the release; on a tag a
missing `DC_FRIEND_SERVER` fails the run rather than leaving them out. The preset is in the launcher only, so someone else
hosting a lobby edits `DeCombat.bat` (or the app's launcher script) or builds their own with the packers.

## Signing: what players will see

**Windows.** The exe is not code signed, so SmartScreen says "Windows protected your PC" on a fresh download until the file
has a reputation: "More info" > "Run anyway". The README in the zip says so. Avoiding it means buying a code signing
certificate; nothing here does that.

**macOS.** Without the `DC_MAC_*` secrets the zips are ad hoc signed and not notarised (the run warns and goes on), and
Gatekeeper makes the player allow the app in System Settings > Privacy & Security; the README in the zip walks through it.
With all of them, `tools/mac_notarise.sh` signs with the Developer ID, submits to Apple's notary and staples the ticket. A
Developer ID needs a paid Apple Developer account. Setting only some of them fails the run.

## iOS

`tools/ios_release.sh` builds with xcodegen and cargo (`aarch64-apple-ios`) on the Mac runner's default Xcode: `ios/project.yml`
no longer names an Xcode version or a team.

- **Without secrets**: `DeCombat-iOS-<tag>-unsigned.ipa`. It installs on nothing as it is; a sideloading tool (AltStore,
  Sideloadly and the like) signs it with the player's own Apple ID, and with a free Apple ID that expires after 7 days.
- **With all of `DC_IOS_CERT_P12`, `DC_IOS_CERT_PASSWORD`, `DC_IOS_PROFILE`, `DC_IOS_TEAM`**: `DeCombat-iOS-<tag>.ipa`, signed
  and exported according to the provisioning profile's kind. Some but not all of them fails the run.

**A signed ipa for the public requires a paid Apple Developer account.** Even then, the file alone does not reach the public:
an ad hoc profile installs only on the devices listed in it, and a wider audience means TestFlight or the App Store. Those are
a separate step this workflow does not do (uploading to App Store Connect) with a separate review by Apple, whose outcome
nothing here can promise.

## Setup: Settings > Secrets and variables > Actions

Variables (plain, shown in logs):
- `DC_FRIEND_SERVER`: the public lobby's address (`host` or `host:port`). Required on a tag.
- `DC_FRIEND_FINGERPRINT`: its certificate fingerprint (`dc-lobby-server tls --db ...` on the VPS). Without it the clients
  trust the server at the first login. Set it: the launcher then pins it, and it changes only if the lobby's certificate does.
- `DC_VPS_REQUIRED_INSTANCES` (optional): the lobby instances a deploy insists on, default `modded`; space separated.

Optional secrets for `release.yml`, as repository secrets:
- `DC_IOS_CERT_P12` (`base64 -i cert.p12`), `DC_IOS_CERT_PASSWORD`, `DC_IOS_PROFILE` (`base64 -i x.mobileprovision`), `DC_IOS_TEAM`.
- `DC_MAC_CERT_P12` (a Developer ID Application certificate, base64), `DC_MAC_CERT_PASSWORD`, `DC_MAC_SIGN_IDENTITY` (`Developer
  ID Application: ...`), `DC_MAC_NOTARY_APPLE_ID`, `DC_MAC_NOTARY_PASSWORD` (an app-specific password), `DC_MAC_NOTARY_TEAM`.

Secrets for `deploy-servers.yml`, in Settings > Environments > `production` (create it; add yourself under required
reviewers, and under deployment branches allow only `master`):
- `DC_VPS_HOST`: the VPS address. `DC_VPS_USER` (optional, default `root`; another user needs passwordless sudo).
- `DC_VPS_SSH_KEY`: a key made for the workflow alone: `ssh-keygen -t ed25519 -f deploy_key -N ""`, append `deploy_key.pub`
  to that user's `authorized_keys` on the VPS, paste `deploy_key` here, delete the file.
- `DC_VPS_KNOWN_HOSTS`: the VPS's host key, pinned; a changed key fails the deploy. Check it against the VPS's own console
  rather than trusting the network: on the VPS `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` gives the fingerprint to
  compare with `ssh-keyscan -t ed25519 <address> | ssh-keygen -lf -`, and that `ssh-keyscan` line is the value.

The repository must be public for the hosts' anonymous download. No secret is available to a fork's pull request.

## Deploying the servers

Actions > deploy-servers > Run workflow, from master. Inputs: `ref` (a tag or commit) and `targets` (`all`, `vps`, `hosts`).

1. **Gate.** A ref that is not on master is refused, and so is a run started from another branch.
2. **Build.** That commit, clean, into the tarball. The build happens before anyone approves anything.
3. **Approval.** The one job holding the secrets waits for the `production` environment.
4. **VPS** (`all`, `vps`). The tarball's own `install.sh` installs the binaries and restarts the default lobby and every enabled
   instance, `decombat-lobby@modded` included, with `tools/vps/deploy_remote.sh` around it: a required instance that is not
   enabled stops the deploy before any change; afterwards each unit must be active with a new start time. It runs no
   `bootstrap.sh`, so the firewall and the web ports stay as they are, and a change to the `ufw` rules during the deploy fails
   the job. The config (`server.toml`, `decombat.env`) and the database are not written. Restarting a lobby ends the matches on
   it, and on the game hosts (below): run it between matches.
5. **Pointer** (`all`, `hosts`). Only after the VPS succeeded: the tarball, then `desired.txt`, go on the rolling pre-release
   `server-deploy` (its page keeps this build and the one before it). Not a `v*` tag, so it starts nothing.
6. **Report.** What commit the VPS runs and, for up to 7 minutes, what commit each host reports to the lobby. It is in the run's
   summary. A host that is still busy is reported, not failed: the run's success means the VPS and the pointer are done.

A `DC_VPS_USER` other than root runs the root-owned scripts in `/opt/decombat/tools` under the limited sudoers of
`tools/vps/migrate_nonroot.sh` (docs/OPS.md section 9); that copy is the one the previous deploy installed.

After cutting a release, set `[client] latest` on the lobby so older clients hear of it (docs/OPS.md section 6). A `v*` tag build knows its version from
`DC_RELEASE_VERSION` (`tools/ci_env.sh`).

Rolling back is the same run with an older `ref`. Game hosts are set up once on the machine: docs/HOST.md "Updates".

### Mid-rollout: a lobby and a host on different builds

What the lobby does today (`crates/dc-lobby-server/src/hosts.rs`): it records the version in an agent's hello and logs it, and
never compares it. There is no version refusal. An agent is refused only for a wrong name or token or a second connection, and a
match only if it needs what the agent's hello does not offer (`content=1`, `packs=1`), with a log line.

So in a rollout with `targets: all`, the order is lobby first and host second, as `tools/ship_servers.sh` has it, and:

- **Lobby restart.** The VPS lobbies are down for seconds. The graceful stop asks every game server, remote ones too, to end its
  match and waits a grace period (`lifecycle.rs`), so matches end and are stored as at any restart. A remote server that outlives
  that is quit when its agent reconnects (within 30 s), because the new lobby does not track it. Either way the hosts are left
  idle, so they normally update at their next check.
- **New lobby, old host, up to about 6 minutes.** Not an outage: US Central is unaffected, and rooms on the host keep starting on
  the old agent and old `dc-dserver`. The risk is a release that changes the pipe lines or the game protocol between lobby,
  `dc-dserver` and client: nothing detects that, and matches on that host may misbehave until it updates. Such a release should
  be deployed when nobody is playing.
- **Host restart, a few seconds.** The agent is offline: a room start on that region is refused as a failed start and the room goes
  back to waiting (LG-301). Other regions are not touched.
- **Failed VPS step.** The pointer does not move, so hosts stay matched to whatever the lobby still has; a unit may be left on the
  old build, which the job's output names.
- **`targets: hosts` alone.** Old lobby, new agent. An agent line the lobby does not know is logged and ignored. Fine for a host-only
  fix, and the reverse order from the usual, so not for a change the lobby has to know about first.

## By hand, as before

`tools/ship_servers.sh` builds this checkout and deploys it to both servers: the VPS, then the game host. Its settings file
(`~/.config/decombat/ship_servers.conf`, `--help` lists the lines) holds only the addresses and ssh users; the host's
fingerprint, token, ports and server arguments are read from its own `host.toml` and passed back unchanged. It stops before
changing anything if a check fails, deploys only committed server code unless told otherwise, and finishes by asking both
machines which commit they run. Add `--dry-run` to see the plan first. Test it with `bash tools/ship_servers_test.sh`. A host with
the updater goes on hold after this (docs/HOST.md "Updates").

`tools/vps/push.sh --host <VPS> --build` (Mac) and `tools/vps/push.ps1` (Windows) still deploy from a working tree,
`tools/friend_pack_mac.sh` and `tools/friend_pack_win.py` still make a friend package locally, and
`tools/mapkit_pack_mac.sh [--bin-dir DIR] [--x86-dir DIR]` / `python tools/mapkit_pack_win.py --bin-dir DIR` make
a MapKit package from a local build (`cargo build --release -p dc-mapkit --bins` plus the three game binaries;
`--bin-dir target/debug` packs a debug build for a test; the zips land in `out_mapkit/`, ignored by git).

## Not yet proven

Tested here: `bash tools/host/update_test.sh` (the updater and the VPS deploy helper on fake tarballs made with the real packer, with
stand-ins for curl, systemctl and pgrep), actionlint on the workflows, `bash -n` on the scripts, and `DeCombat.bat` run under cmd on
Windows. Unproven until they run for real: every workflow on GitHub, the iOS build and export, notarisation, and the systemd units and
root behaviour of `update.sh` on a real host. The first tag should be a pre-release (`v0.0.1-rc1`) and the first deploy `targets: vps`.
