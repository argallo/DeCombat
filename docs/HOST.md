# HOST.md: a second region on your own machine (remote game host)

Design: specs/HOST_AGENT.md. Rules: CD-21 (amended), CD-88, LG-300, LG-301, HA-1 in DC_DEFAULTS.md. The example here: a lobby on a VPS ("US Central") and a game host on a home Linux machine ("US West").

## How it fits together

- The lobby (dc-lobby-server, one process, one database) stays where it is and keeps every account, character,
  inventory and reward. Players log in there whichever region they pick.
- `dc-host` runs on the remote machine. It dials the lobby's host port (TLS, the lobby's certificate pinned, a
  shared token) and starts one `dc-dserver` per match on request; the match's pipe lines travel over that
  connection, so results reach the database exactly as from a local game server.
- Only UDP game traffic goes to the remote machine. The router forwards a UDP port range to it; nothing inbound on
  TCP is needed at the house.

## The machine

Ubuntu 22.04 / 24.04, a user you can ssh into with a key, and a sudoers drop-in that lets that user run exactly what
the deploy script needs without a password (`sudo visudo -c` must say "parsed OK" afterwards):

    <user> ALL=(root) NOPASSWD: /usr/bin/apt-get, /usr/bin/apt, /usr/bin/systemctl, /usr/sbin/ufw, /usr/bin/install, /usr/bin/tee /etc/systemd/system/*, /usr/bin/mkdir -p /opt/decombat*, /usr/bin/chown -R * /opt/decombat*

Give the machine a fixed address (a DHCP reservation in the router) and forward the UDP range (default
27888-27899) to it. The agent runs as that user under systemd (`decombat-host.service`), files under /opt/decombat.

## The lobby's side (server.toml on the VPS)

```toml
[[lobby.servers]]
name = "US Central"
capacity = 1000
host = ""                 # local game servers

[[lobby.servers]]
name = "US West"
capacity = 1000
host = "us-west"

[hosts]
port = 0                  # 0 = auth_port + 5 = 10005 (TCP, TLS only)

[[hosts.agents]]
name = "us-west"
token = "<at least 24 random characters>"
public_address = ""       # "" = the address the agent connects from (the house's public address)
```

Open TCP 10005 on the VPS (ufw and the provider's firewall) for the agent to reach. The token: make one with
`openssl rand -hex 24` and keep it in server.toml on the VPS and host.toml on the machine only.

## Deploy the host

From a checkout on the Mac (the machine builds the binaries itself; the first build takes minutes):

    tools/host/push.sh --host <machine address> --user <user> --name us-west \
      --lobby <lobby address>:10005 --fingerprint <lobby fingerprint> --token <token> --rom <server ROM dir or .tgz>

Later updates: `tools/ship_servers.sh --host-only` reads the saved settings from the machine's `host.toml`, or `tools/ship_servers.sh` updates the VPS as well (docs/RELEASES.md "By hand, as before"). By hand: the same line without `--rom` (the ROM stays). The lobby keeps running; the agent restart ends the
matches running on that host (a dropped network connection does not), so update between matches.

## Updates

The machine updates itself: the first install stays `tools/host/push.sh` (it brings the ROM and `host.toml`), and from then
on a timer pulls what the workflow `deploy-servers.yml` publishes (docs/RELEASES.md "Deploying the servers"). A home host
takes no inbound port and no CI runner for this: GitHub never connects to it. Needs the repository to be public, since the
download is anonymous.

With a private repository the build comes from the lobby instead (`SOURCE=lobby`, docs/DEPLOY_FROM_ADMIN.md): a deploy
from the dashboard's Updates tab offers the VPS build to the agent over its control connection, the agent leaves it in
`/opt/decombat/incoming`, and the same timer installs it with every check below once root's signature from the VPS
verifies and its sequence number is new. Switch with `sudo bash update.sh --setup --from-lobby --signer "<line>"` (the
line tools/vps/deploy_setup.sh prints; an existing `update.conf` is kept as `update.conf.url`); the setup also makes
the paths root runs from root's alone (docs/DEPLOY_FROM_ADMIN.md step 6, with the change deploy_all.sh then needs). An
offer older than the installed build (a by-hand install since) is dropped; an installed one is used up.

One-time setup, on the machine, after the first install:

    curl -fsSLO https://raw.githubusercontent.com/argallo/DeCombat/master/tools/host/update.sh
    sudo bash update.sh --setup --pointer-url https://github.com/argallo/DeCombat/releases/download/server-deploy/desired.txt

That installs `/opt/decombat/tools/host/update.sh`, writes `/opt/decombat/config/update.conf` (the address only) and
enables `decombat-host-update.timer`, which checks every 5 minutes (plus up to a minute at random). Each check:

1. Reads the pointer. Same commit as `/opt/decombat/bin/COMMIT.txt`: nothing to do.
2. A `dc-dserver` is running: waits for the next check. A host with a match that never ends stays on the old build (see
   `--force` below); nothing is downloaded meanwhile.
3. Downloads the tarball next to the pointer, checks its SHA-256 against the pointer and the binaries against the
   tarball's own list, checks the tarball is the build of that commit, runs the new `dc-host --version` (this machine's
   libraries) and `dc-server-rom --verify` on the ROM here. Any failure: nothing was changed.
4. Stops the agent, swaps `dc-host`, `dc-dserver`, `dc-server-rom`, starts it. If it does not stay up the old build comes
   back from `/opt/decombat/update/prev`, and that commit is not tried again until the pointer moves.

`config/host.toml` (the token), the ROM, `packs/` and the logs are never written; a tarball carries no config at all. A match
that starts in the moment between step 2's last look and the stop is ended by the restart, like any agent restart.

On the machine: `/opt/decombat/tools/host/update.sh --status` shows the last outcome, `journalctl -u decombat-host-update`
each check, and `sudo /opt/decombat/tools/host/update.sh --force` updates now though matches run or the commit failed before.
The lobby's `/status` lists the commit each agent reports, and the deploy workflow prints it.

A by-hand update (`tools/host/push.sh`, `tools/ship_servers.sh`) would be put back by the next check, so it sets a hold
(`/opt/decombat/update/hold`); the machine stays on that build until `sudo /opt/decombat/tools/host/update.sh --release-hold`.

## Check

- On the machine: `journalctl -u decombat-host -f` shows the connection ("ok" from the lobby) and each start.
- On the VPS: the lobby log notes the agent's hello and every remote game.
- In the client: the server list shows both entries; a room in US West starts its match on the house's address.

## Open

- Pipe lines sent while the control connection is down are lost (a match ending in that window stores nothing).
- No per-host player count on the server list yet (both entries count the one lobby's sessions).
