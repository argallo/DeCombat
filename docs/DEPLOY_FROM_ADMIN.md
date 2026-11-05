# DEPLOY_FROM_ADMIN.md: deploying from the dashboard's Updates tab

The dashboard (`https://admin.<your domain>` behind the security key, docs/ADMIN_WEB.md, or the loopback admin port
through the SSH tunnel) has an **Updates** tab: what the VPS lobby and each game host run, the commits on master since
then, a **Deploy** button per commit, the live progress of a deploy and the last outcomes. Rules: DC_DEFAULTS.md LG-392 ..
LG-394 and HA-2. Status: implemented and tested on Windows (crate tests, shell tests with stand-ins for git, cargo,
systemctl and deploy_remote.sh); **not yet run on Linux, under systemd or against GitHub**.

```
dashboard --POST /ops/updates/deploy--> lobby (user decombat) --writes--> /opt/decombat/deploy/requests/<id>.json
                                                                                     |
          decombat-deploy.path (root) --starts--> deploy_from_git.sh run: validate, git archive, cargo build,
          package, install.sh + restart every enabled lobby (vps), host/desired.txt + tarball (hosts)
                                                                                     |
lobby --update / updpart / updend over the host connection--> dc-host --> /opt/decombat/incoming
                                                     tools/host/update.sh (root timer on the home host) installs it
decombat-src-fetch.timer (root, every 5 min) --git fetch with a read-only deploy key--> /opt/decombat/src.git
                                             --> /opt/decombat/deploy/available.json (what is new)
```

The lobby never holds the GitHub key, never runs git or cargo, and only drops a small request file; root's units check
it and do the rest. The lobby restarts during a deploy, so it must not be the process running it.

## One-time setup

1. **Ship this version once the old way**, between matches: on the Windows PC
   the operator's own manual deploy (`deploy_all.sh deploy <commit>`, kept outside this repository) (the VPS gets the new lobby and `tools/vps/deploy_setup.sh` /
   `deploy_from_git.sh` in `/opt/decombat/tools`; the home host gets a `dc-host` that understands build offers).
2. **On the VPS, as root**: `sudo bash /opt/decombat/tools/deploy_setup.sh`. It needs git, python3, `ssh-keygen` and
   root's rust toolchain (the one deploy_all.sh already builds with, `~/.cargo`). It makes the deploy key, pins
   GitHub's host key (checked against GitHub's published SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU), the mirror
   `/opt/decombat/src.git`, the folder `/opt/decombat/deploy` with `deploy.conf`, the key that signs the hosts' offers
   (`deploy/sign/host_update_ed25519`, root only), the lobby drop-in and the four units, enables
   `decombat-src-fetch.timer` and `decombat-deploy.path`, and prints two lines: the deploy key's public half (for GitHub)
   and the **signer line** `decombat-deploy ssh-ed25519 AAAA...` (for the home host, step 6). Options for a first run:
   `--repo git@github.com:OWNER/REPO.git` (default the DeCombat repository), `--branch master`,
   `--required-instances "modded"` (a deploy refuses to start while that instance's unit is not enabled).
3. **On GitHub**: the repository > Settings > Deploy keys > Add deploy key. Title "VPS deploy", paste the printed line,
   leave **Allow write access** unticked, Add key.
4. **Check the key** on the VPS: `sudo bash /opt/decombat/tools/deploy_setup.sh --check` (one fetch, then the units'
   state). "fetched master: <sha>" means the key works.
5. **Tell the lobby** in `/opt/decombat/config/server.toml` (the default instance only):

   ```toml
   [deploy]
   dir = "/opt/decombat/deploy"
   ```

   then `sudo -u decombat /opt/decombat/bin/dc-config check /opt/decombat/config/server.toml` and
   `sudo systemctl restart decombat-lobby` (this restart also gives the hardened lobby its write access to
   `deploy/requests`). The Updates tab now lists the commits.
6. **Home game host** (optional; without it the VPS still deploys and the host stays with deploy_all.sh): switch its
   updater to the lobby's build. From the Windows PC copy the new updater over
   (`scp -i ~/.ssh/decombat_vps tools/host/update.sh <user>@<box>:`), then on the box
   `sudo bash update.sh --setup --from-lobby --signer "decombat-deploy ssh-ed25519 AAAA..."` (the line of step 2;
   `--agent-user NAME` when the agent does not run as the owner of `/opt/decombat/logs`). It:
   - makes the paths root runs from or writes to **root's alone** (the one-time migration of a box `tools/host/push.sh`
     set up, which gave the agent's user all of `/opt/decombat`): `/opt/decombat`, `bin/`, `config/`, `tools/`,
     `update/` and `rom/` go to root:root without group or other write; `config/host.toml` to root:<agent's group> 0640
     (the agent reads its token); only `logs/`, `packs/` and `incoming/` stay the agent's;
   - pins the signer line in `/opt/decombat/config/update_signers`, installs `/opt/decombat/tools/host/update.sh`, writes
     `update.conf` as `SOURCE=lobby` with `AGENT_USER` (an existing URL config is kept beside as `update.conf.url`),
     and enables `decombat-host-update.timer` (every 5 minutes).

   **deploy_all.sh after this migration**: its home install (`install_home`) writes into `/opt/decombat/bin` as the ssh
   user without sudo, which a root-owned `bin/` refuses. Change its three writes to go through the sudoers drop-in of
   docs/HOST.md, which already allows `/usr/bin/install`:

   ```
   for b in dc-dserver dc-host dc-server-rom; do sudo -n /usr/bin/install -m 0755 ~/decombat-staged/$b /opt/decombat/bin/$b; done
   sudo -n /usr/bin/install -m 0644 ~/decombat-staged/COMMIT.txt /opt/decombat/bin/COMMIT.txt
   ```

   (`install` replaces the file, the running agent keeps the old one until its restart). Until the migration,
   deploy_all.sh works unchanged; `tools/host/push.sh` restores the root-owned layout itself at its end.

`deploy.conf` (`/opt/decombat/deploy/deploy.conf`, read as `KEY=value`, never run): `REPO`, `BRANCH`,
`REQUIRED_INSTANCES`, `MATCH_WAIT_MIN` (15), `BUILD_JOBS` (cargo `-j`; empty = all cores; the build runs at nice 10),
`CARGO` (empty = root's PATH, else `/root/.cargo/bin/cargo`).

## Check now

The fetch timer looks for new commits every 5 minutes. **Check now** (next to the "checked ... ago" line) asks for it
at once: `POST /ops/updates/fetch` writes `requests/fetch-<unix>.json`; root's `decombat-deploy.path` starts
`deploy_from_git.sh run`, which removes the file unread and runs one fetch (never a deploy). The button waits until the
"checked" time changes. Limits: none within 30 s of the last check (429 from the lobby; root ignores it too), 409 while
a deploy runs or waits. This needs the `deploy_from_git.sh` with the fetch handling in `/opt/decombat/tools`. There is no
one-time step: any dashboard deploy of a commit that has it installs it (install.sh copies `tools/vps/*.sh` into
`/opt/decombat/tools`), and no unit changes. Until that deploy the old script deletes the request file unread and the
button just times out after 90 s.

## Checking what is running

On the VPS, without the dashboard:

- `/opt/decombat/deploy/available.json`: written by every fetch. `running` is the commit the lobby was installed from,
  `master` is the newest commit of master the mirror has; the commits between are listed beside them.
- `/opt/decombat/bin/COMMIT.txt`: the commit, install time and tarball checksum of the installed binaries.
- `systemctl is-active decombat-lobby decombat-lobby@modded`: `active` for each lobby (the default one and the Hacking Mod
  instance). A deploy checks the same after its restart.
- `sudo bash /opt/decombat/tools/deploy_from_git.sh status`: the last deploy's status.

## Deploying

Updates tab: tick **VPS** (the lobbies restart: everyone is disconnected for seconds) and / or **Game hosts**, then
**Deploy <sha>** on a commit and confirm. Older commits (rolling back) are under "The running build and the ones before
it". While matches run, the request is refused unless **End the running matches** is ticked (the box only shows while
matches run); without it a deploy that finds matches at install time waits up to `MATCH_WAIT_MIN` minutes for them to
end, then fails and installs nothing. The status block shows the phase (fetching, building, packing, waiting, installing,
offering to the game hosts) and the log tail, every 3 seconds.

On the admin web a VPS deploy ends your session (the lobby restart signs everyone out): the page says "The lobby
restarted with the new build. Sign in again." with a Sign in button; the tab shows the outcome afterwards. On the
loopback port the page keeps polling; reload it before the next deploy (its page token changed with the restart).

What a deploy does on the VPS (`deploy_from_git.sh run`, as root, one at a time): checks the request (the commit must be
on the mirror's master), `git archive` of that commit into `/opt/decombat/deploy/work/src` (the cargo target stays warm
there), the same two builds as deploy_all.sh with `DC_GIT_COMMIT=<short sha>`, the tarball of
`tools/package_servers.sh`, then for VPS that tarball's own `deploy_remote.sh install` (install.sh: binaries, scripts,
units; config, DB and ROM untouched; every enabled lobby instance restarted, each checked active on the new commit; a
firewall change fails the job), for Game hosts the tarball and `desired.txt` into `/opt/decombat/deploy/host`. Logs:
`/opt/decombat/deploy/logs/<id>.log`, `journalctl -u decombat-deploy`; `sudo bash /opt/decombat/tools/deploy_from_git.sh
status` prints the last status.

## The home game host

It takes no inbound connection, so the build comes over the connection the agent already has: the lobby offers the
build of `deploy/host/desired.txt` to every agent on another build (at its hello and at every ping), an agent with no
match running asks for it and gets it in pieces over the pinned TLS stream (specs/HOST_AGENT.md "Updates"), checks size
and SHA-256 and leaves it in `/opt/decombat/incoming`. The offer carries root's signature from the VPS: a deploy signs
the commit, tarball name, SHA-256, size and a sequence number one above the last (`ssh-keygen -Y sign`, namespace
`decombat-host-update`, the key in `deploy/sign/`, which neither the lobby nor the agent can read); the lobby and the
agent pass it on unchanged. Its updater (root, every 5 minutes) trusts nothing of the offer until the signature verifies
against the pinned `config/update_signers` and the sequence number is above the last one it took
(`update/lobby_seq`), so an old build cannot be replayed. Then it does the rest as with the GitHub pointer (docs/HOST.md
"Updates"): waits for matches to end, checks the tarball's SHA-256 against the signed one, runs the new `dc-host
--version` on this machine as the agent's user, swaps into its root-owned `bin/`, restarts the agent, rolls back a
build that does not stay up. In this mode it never installs the tarball's own `update.sh`: a new updater comes only with
the operator's `--setup`. The Running builds table shows "Offered <sha>" until the host reports the new build.

A new signing key (deploy_setup.sh run on a fresh VPS) means a new signer line on every host:
`sudo /opt/decombat/tools/host/update.sh --setup --from-lobby --signer "<new line>"`, and
`sudo rm /opt/decombat/update/lobby_seq` there (a new key starts its sequence at 1).

**glibc**: the VPS builds on its own Ubuntu (24.04); a host on an older release may not run those binaries. Then the
updater refuses the build ("the new dc-host does not run here", `update.sh --status`) and the host stays on its build:
use `deploy_all.sh deploy <commit> --home-only` for it, or upgrade the box.

## How it relates to deploy_all.sh

The manual `deploy_all.sh` keeps working unchanged and stays the break-glass way (and the way for a host that cannot run
the VPS build). It builds in `~/decombat-src` on each machine; the dashboard builds in `/opt/decombat/deploy/work`, so
the two never share a checkout. Do not run both at the same moment. A by-hand install never gets undone by an old offer:
the lobby offers its hosts only its own build or one published after it started, and the home updater drops an offer that
is older than the installed build.

## Security

- The lobby (user `decombat`) can write only `deploy/requests/`; everything there is untrusted. Root lists the folder
  with `find` (no glob misses a `..x`), deletes every entry that is not a regular file with a request name (links,
  FIFOs, dot files, names with a newline) unread, opens a request with `O_NOFOLLOW | O_NONBLOCK` and refuses anything
  but a regular file of at most 4 KiB, and keeps only its shape: `sha` (hex, must be a commit on master), `targets` (vps /
  hosts), `end_matches` (boolean), `by` (text, written back only). No field reaches a shell.
- Root's own rate limit, since a lobby can drop request files at will: no deploy within 60 s of the last finished one,
  and none of the same commit within 5 minutes (refusals by the limit stay out of the history). Logs keep the newest 50.
- The deploy key is read-only and lives in `/opt/decombat/deploy/key` (root, 0700); GitHub's host key is pinned. A
  fetch without the key refuses (it never falls back to root's own ssh identity).
- On the loopback admin port a deploy needs the port's `--admin-token` (`DC_ADMIN_TOKEN`); without one the tab is
  read-only there and deploys go through the admin web.
- Host offers are signed by root on the VPS and checked by root on the host (above); the home updater runs the new
  build's binaries for its checks as the agent's user (setpriv), refuses tarball entries that are not files or folders,
  copies the agent's files without following links, and refuses to run as root while any path it reads code or config
  from or writes to is writable by someone else.
- **Threat model**: a stolen admin session, the loopback port with its token, or a compromised lobby process can deploy
  **commits of master and nothing else**, on the VPS and on the game hosts (including an older commit as a new deploy,
  and ending running matches). It cannot deploy code that is not on master, change config, the DB or the firewall, reach
  the GitHub key or the signing key, or make a host take an offer root did not sign or one older than it has. A commit on
  master is built as root on the VPS (its build scripts run as root), as with deploy_all.sh today: master is the trust
  boundary, so protect who can push to it.
- The home box today runs the agent as the operator's ssh user, which the sudoers drop-in of docs/HOST.md lets run
  `install`, `chown -R` and `systemctl` as root: on that box a compromised agent is root whatever the layout. Run
  dc-host as a user of its own (no sudo; `User=` of `decombat-host.service`, `--agent-user` at the setup) to make the
  root-owned layout count.

## Not yet proven

Nothing here has run on Linux: the units, root's flock / setpriv / chgrp paths, `ssh` to GitHub with the deploy key, the
build on the VPS through the unit (HOME, PATH to cargo, memory next to a running lobby), the agent's transfer of a real
tarball over the pinned stream, and the home updater in lobby mode. Tested: `bash tools/vps/tests/deploy_test.sh`,
`bash tools/host/update_test.sh`, `cargo test -p dc-lobby-server --bins deploy`, `cargo test -p dc-host`.
