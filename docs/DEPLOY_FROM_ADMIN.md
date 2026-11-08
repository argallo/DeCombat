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

## Release card (dashboard)

The Updates tab also has a **Public release** card: the state of the release job on the VPS (the public export, its
push, the public CI, the tag and the GitHub release) and three buttons. Rule: DC_DEFAULTS.md LG-430. The lobby works as
for deploys: it reads `<[deploy] dir>/release/status.json` (written by the release job; missing means "no release state
yet") and writes one request file, `release/requests/<unix>-<action>.json` = `{"action", "by", "at"}`, where `by` is the
label of the signed-in security key (on the loopback port, "the admin port"). The card shows a headline from the job's
phase and message ("3 commits since v0.5.0-beta6 — ready to prepare", "Pushed 9e10335a, CI running", "Ready to tag
v0.6.0-beta7 — CI green", "Failed: ..." with the log tail open), when it last changed, the tags, both commits and CI.

- **Check now** (always): the job looks again (new commits since the last tag, the CI run).
- **Prepare release**: only when the job is idle, published or failed and there are commits since the last tag.
- **Tag & publish v...**: only when the job says ready (CI green); it asks once more on the card itself before it sends.

The lobby refuses with 409 a second request while one waits (a second Check now is answered without a new file), a
prepare or a tag the state does not allow, and with 503 when the release folder is missing. Every request is an ops
event of kind `release` ("release prepare requested by <name>"), and on the admin web also an `admin_web` event with the
key and the address. The writes have the deploy guard (JSON, the page's `Origin`, the page token; on the loopback port
also its `--admin-token`), and a further lobby instance's prefix takes no release request. Everything in `status.json`
is shown as text; only links to `https://github.com/argallo/DeCombat/...` become links. While a request waits or the
job is at work the card refreshes every 3 seconds, like a deploy.

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

## Releases from the dashboard

The same machinery publishes the beta releases: root on the VPS exports master of the working repository (the one the
deploy mirror `/opt/decombat/src.git` fetches), pushes the export to the public repository's master, waits for its `ci`
run, and on the owner's go tags it, waits for `release.yml` and writes the release notes. Rule: DC_DEFAULTS.md OPS-25;
the dashboard side is the Release card above (LG-430).
Status: implemented and tested on Windows (`bash tools/vps/tests/release_test.sh`, stand-ins for curl and the export
tool); **not yet run on Linux, under systemd or against GitHub**.

```
dashboard --> lobby (user decombat) --writes--> /opt/decombat/deploy/release/requests/<unix>-<check|prepare|tag>.json
decombat-release.path (root) --> release_from_git.sh run --> check / prepare / tag
decombat-release-check.timer (root, daily 06:00 UTC) --> release_from_git.sh daily: check, then prepare
                                                          (only with something new and "auto_prepare": 1)
progress: /opt/decombat/deploy/release/status.json, history.jsonl, logs/<id>.log
```

### Setup (once, after the deploy setup above)

1. **A token on GitHub**: Settings > Developer settings > Personal access tokens > Fine-grained tokens > Generate new
   token. Resource owner: the owner of the public repository; Repository access: **Only select repositories**, the
   public repository alone; Repository permissions: **Contents: Read and write**, **Actions: Read-only** (Metadata:
   Read-only comes with them); an expiry you will notice (90 days, say). Nothing else.
2. **The export tool and its word list** are not in this repository. Copy both from the machine you export on to the
   VPS (`scp` into root's home), together with a current checkout's `tools/vps` scripts (a deploy installs them).
3. **On the VPS, as root**:

   ```
   sudo GITHUB_TOKEN=<the token> bash /opt/decombat/tools/release_setup.sh \
       --export-tool /root/<the export tool> --private-strings /root/<its word list> \
       --last-tag v0.5.0-beta6 --private-sha <working commit of that beta> --public-sha <public commit it tags>
   ```

   Without `GITHUB_TOKEN` it asks for the token from the terminal (not shown). It writes
   `/opt/decombat/deploy/release.conf` (root 0600: `GITHUB_TOKEN=`, `PUBLIC_REPO=` default the DeCombat repository),
   installs the tool and the word list in `deploy/release/tool/` (root only), clones the public repository into
   `deploy/public`, seeds `deploy/release/state.json` (`last_tag`, `last_released_private_sha`, `last_public_sha`,
   `auto_prepare` 1), and installs and enables `decombat-release-check.timer` and `decombat-release.path`. A second run
   keeps the token (unless a new `GITHUB_TOKEN` is given) and `state.json` (unless `--reseed`); give `--export-tool` /
   `--private-strings` again to replace those files. Then delete the copies in root's home and restart the lobby once
   (`sudo systemctl restart decombat-lobby`) so it may write the request folder. `release_setup.sh --check` shows the
   units and the status.

### The daily check, prepare, ready, tag

- **Check** (the timer at 06:00 UTC, or the dashboard): fetch master; the status says `N commits since <last tag>` or
  `nothing new since <last tag>` (N counts first-parent commits). A check never changes a prepared or failed release.
- **Prepare** (the timer when something is new and `auto_prepare` is 1, or the dashboard): the export of master with the
  public clone as its parent; the scan must report `refusals=0`, else the phase is `failed` with the scan summary and
  nothing is pushed. The commit message is `Beta <N+1>: <the first-parent subjects, Merge prefixes off, cut to 200
  characters>`; a subject the export scan would refuse is left out of it. Then the push to the public master (phase
  `pushed`) and the wait for that commit's `ci` run, every 60 s for at most 60 min (phase `ci_running`): `ready` when
  every job is green, `failed` with the names and links of the jobs that are not. One prepare at a time and none within
  10 minutes of the last; a prepare of the commit that is already `ready` is refused (the phase stays `ready`).
- **Tag** (the dashboard, from `ready` only, and only while the public master is still the prepared commit): the
  annotated tag `v<MAJOR>.<MINOR+1>.0-beta<N+1>` (the client compares MAJOR.MINOR.PATCH only, so every beta moves MINOR:
  after v0.5.0-beta6 comes v0.6.0-beta7), pushed (phase `tagging`), then the wait for its `release` run (phase
  `publishing`, at most 60 min). On success it writes the release body (the notes below, marked pre-release), records the
  release as the last one in `state.json`, and the phase is `published` with the release page's link. `release.yml`
  publishes the assets and SHA256SUMS and asks for the website run itself.
- **Notes**: `sudo bash /opt/decombat/tools/release_from_git.sh notes <from> <to> [tag]` prints them: a header `DeCombat
  <tag>`, which clients update themselves, the first-parent subjects as bullets grouped by their rule ids (CLI-, VAR-,
  REN-: Client; DSV-, GMD-: Match servers; LG-, OPS-, CRATE-: Lobby and servers; the rest: Other), a count of the
  subjects left out because the export scan would refuse them, and the SmartScreen and SHA256SUMS note.
- By hand on the VPS: `release_from_git.sh check | prepare | tag | status`.

The request (`<unix>-check.json`, `<unix>-prepare.json`, `<unix>-tag.json`) is JSON `{"action": "<the name's action>",
"by": "<text>"}`; the newest wins, every other entry of the folder is removed unread, and the request file is removed as
soon as the job picks it up. A request waits while another job runs (a prepare can wait up to an hour for CI).

### When something fails

- **The export scan refused**: the log (`deploy/release/logs/<id>.log`) lists the refusals by file and line. Fix them on
  master; the next prepare exports the new master.
- **CI failed**: the status names the failed jobs and links them. A flaky job: re-run it on GitHub, then **Prepare**
  again: with master unchanged and the public master still the prepared commit, it only waits for CI again (no new
  export). A real failure: fix it on master and prepare again (a new export on top).
- **The release run failed**: on GitHub delete the tag (`git push --delete origin <tag>` from a clone, or the tag's page)
  and any draft release, then **Prepare** (it finds the commit unchanged and green: `ready`) and **Tag** again. Without
  deleting the tag, Tag again (or `release_from_git.sh tag` by hand) only waits for the release run of the tag that is
  there, which is what you want after re-running its failed jobs on GitHub.
- **The public master moved** (someone pushed to it by hand): the tag is refused; prepare again.
- **A job was cut off** (the machine restarted): the next job marks it `failed` ("interrupted"); prepare or tag again.

### Security

- The lobby can only drop a request; root checks its name and shape as for a deploy (no field reaches a shell). Rate
  limits on root's side: a check 30 s, a prepare 10 min, a tag 60 s after the last.
- The token lives only in `deploy/release.conf` (root 0600). It reaches curl in a config on standard input and git
  through an askpass helper and the environment: never on a command line, in a log, in `status.json` or in the public
  clone's config. Revoke it on GitHub to stop the job at once.
- **Threat model**: a stolen admin session or a compromised lobby can make the VPS publish **an export of master**, scanned,
  with green CI, and tag the prepared commit; nothing else. Master is the trust boundary here too: the export tool scans
  the tree, and the prepare runs as root with the tool and word list installed at setup.

## Bug reports from Discord

Players post bug reports in a Discord channel. A small job on the VPS reads that channel through Discord's REST API and
keeps the messages in a file the dashboard's Updates tab lists ("Bug reports", with a Done checkbox), so they can be read
there or with one `cat` on the VPS instead of being copied by hand. Rule: DC_DEFAULTS.md OPS-26.
Status: implemented and tested on Windows (`bash tools/vps/tests/triage_test.sh`, a stand-in for curl;
`cargo test -p dc-lobby-server --bins bug_reports`); **not yet run on Linux, under systemd or against Discord**.

```
Discord channel <--- decombat-triage.timer (root, every 5 min) --> discord_triage.sh poll
  /opt/decombat/deploy/triage/reports.jsonl   one JSON object per line, oldest first (id, at, author, content, attachments)
  /opt/decombat/deploy/triage/state.json      last_id, updated, fetched, ok, message
  /opt/decombat/deploy/triage/done.json       the reports marked Done on the dashboard (the lobby writes only this file)
dashboard --> GET /ops/triage (newest 50), POST /ops/triage/done {id, done}
```

### Setup (once, after the deploy setup above)

1. **A Discord bot** (Discord account that owns or administers the server): open the Developer Portal
   (discord.com/developers/applications) > **New Application** (any name) > **Bot** in the side menu. Under **Privileged
   Gateway Intents** switch on **Message Content Intent** (without it the bot gets empty message texts) and save. Under
   **Token** press **Reset Token** and copy it; it is shown once.
2. **Invite it to the server**: **OAuth2** > **URL Generator**; scope **bot** only; bot permissions **View Channel** and
   **Read Message History** only (nothing that sends, manages or reads voice). Open the generated URL, pick the server,
   authorise. In the server, make sure the bot's role can see the bug-report channel (a private channel needs the bot or its
   role added to it).
3. **The channel's number**: Discord Settings > Advanced > **Developer Mode** on; right-click the channel > **Copy
   Channel ID**.
4. **Write the conf on the VPS, by hand, over an interactive ssh session** (a terminal you type in; not a shell escape of a chat tool, not a script,
   not `echo token=... >` on a command line that stays in the shell history):

   ```
   sudo -e /opt/decombat/deploy/triage.conf     # two lines: token=<the bot token>  channel_id=<the channel number>
   sudo chown root:root /opt/decombat/deploy/triage.conf && sudo chmod 600 /opt/decombat/deploy/triage.conf
   ```

5. **On the VPS, as root, from a checkout of the repository** (a deploy installs the same scripts under
   `/opt/decombat/tools/`):

   ```
   sudo bash tools/vps/triage_setup.sh
   ```

   It checks the conf (shape, then one request to Discord: "the bot sees channel ... (#name)"), installs
   `discord_triage.sh` in `/opt/decombat/tools/`, makes `deploy/triage/` (root:decombat 0750) and an empty `done.json`
   (decombat 0600), installs and enables `decombat-triage.timer` (every 5 minutes) and the lobby drop-in
   `22-triage.conf` (`ReadWritePaths=-.../triage/done.json`), and runs one poll. Nothing is enabled when the conf is missing or
   Discord refuses it. Restart the lobby once so it may write `done.json` (`sudo systemctl restart decombat-lobby`; the next
   deploy does it too). `triage_setup.sh --check` shows the timer, asks Discord about the channel and prints the state.

### Reading the reports

- The dashboard, Updates tab, card **Bug reports**: the state line (last poll, new messages, or why it failed), the newest 50
  reports with author, time, text (the first 600 characters, "more" for the rest), attachment links and **Done**.
- On the VPS: `sudo bash /opt/decombat/tools/discord_triage.sh status`, and `sudo tail -n 20 /opt/decombat/deploy/triage/reports.jsonl`.
  By hand: `sudo bash /opt/decombat/tools/discord_triage.sh poll` (the timer does the same every 5 minutes).
- The first poll takes the newest 100 messages of the channel; later polls take what came after the last one seen. A message
  edited later is not read again. Messages with neither text nor an attachment (joins, pins) are skipped.

### When something fails

- The card's state line says why the last poll failed (and `state.json` keeps the message; reports and `last_id` stay):
  **HTTP 401**: the token is wrong or was reset: make a new one and rewrite `triage.conf`. **HTTP 403**: the bot lacks View
  Channel or Read Message History in that channel. **HTTP 404**: wrong `channel_id`, or the bot is not in that server.
  **Rate limited**: Discord asked for a wait longer than the job waits; the next run tries again.
- Empty message texts mean the Message Content Intent is off (step 1).

### Security

- The token lives only in `deploy/triage.conf` (root 0600). It reaches curl in a config on standard input: never on a command
  line, in a log, in `state.json` or in a report. Reset it in the Developer Portal to stop the job at once. The bot can read
  one server's channels it is allowed into and send nothing.
- The lobby never sees the token and never calls Discord: it reads `reports.jsonl` and `state.json` and writes `done.json`
  only. What strangers typed into the channel is untrusted text: control characters become spaces, the dashboard puts it in as
  text (no HTML), and only links to Discord's file servers are clickable.

## Not yet proven

Nothing here has run on Linux: the units, root's flock / setpriv / chgrp paths, `ssh` to GitHub with the deploy key, the
build on the VPS through the unit (HOME, PATH to cargo, memory next to a running lobby), the agent's transfer of a real
tarball over the pinned stream, and the home updater in lobby mode. Tested: `bash tools/vps/tests/deploy_test.sh`,
`bash tools/host/update_test.sh`, `cargo test -p dc-lobby-server --bins deploy`, `cargo test -p dc-host`.
