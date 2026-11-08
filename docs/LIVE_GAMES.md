# Discord live games board

DeCombat's destination is [#live-games](https://discord.com/channels/1566571195888046182/1568389016183640074) (Discord server `1566571195888046182`, channel `1568389016183640074`). Members can read the channel. A dedicated incoming webhook owns one board message and edits it as matches change.

The game host and the Discord publisher are separate. Every 30 seconds an opted-in lobby writes a small JSON snapshot from its in-memory game state. The publisher needs read access to those files and a channel webhook. The admin login, the operator port and the player database are not involved, and the lobby holds no Discord credential.

A public row has eight fields: the server-list name, the channel, the room, the map, the mode, the humans, the bots and the capacity. Passworded and clan rooms, player names, room titles, addresses and ports are never written. A snapshot older than 90 seconds shows "Status unavailable". A fresh empty snapshot shows "No public matches running".

## What a row says

The labels are the ones the game client shows:

- **Server** is the server-list name of the entry (`[[lobby.servers]]`, the first entry is `server_name`).
- **Channel** is the number the channel list shows: "Channel 3" is the entry's third channel, whatever its internal id.
- **Room** is the room list's number: the room's number plus one, three digits (`001`).
- **Map** and **mode** are the room list's texts: the map's name and the mode's short name, "Gun Game" for a Gun Game room. Possession and Hardcore rooms add `+ Possession` or `(Hardcore)` to the mode, which the room list does not show but the waiting room does.
- **Seats** are the occupied slots over the room's capacity; bot seats are counted and named.

Only a room that is playing, unlocked, on a normal server-list entry, with a ready game server that has not ended, is listed.

## Enable the snapshot on the lobby host

Deploy a lobby binary with this change through the normal server release process, and restart in a quiet window. The setting is `monitor.public_games_file` (empty = off), so in the usual order it can come from `--config` / `server.toml`, from `lobby.json`, or from the environment as `DC_MONITOR__PUBLIC_GAMES_FILE`. The path must be absolute. On the VPS layout the environment file is the natural place.

Create the service-owned directory once:

```sh
sudo install -d -o decombat -g decombat -m 0755 /var/lib/decombat-public-games
```

Each lobby process gets its own file. A process with several server-list entries lists all of them in its one file.

| lobby | unit | environment file | line to add |
|---|---|---|---|
| default | `decombat-lobby` | `/opt/decombat/config/decombat.env` | `DC_MONITOR__PUBLIC_GAMES_FILE=/var/lib/decombat-public-games/default.json` |
| instance `<name>` | `decombat-lobby@<name>` | `/opt/decombat/instances/<name>/decombat.env` | `DC_MONITOR__PUBLIC_GAMES_FILE=/var/lib/decombat-public-games/<name>.json` |

Then restart the unit (`sudo systemctl restart decombat-lobby`, or `decombat-lobby@<name>`); the file appears within 30 seconds, written as a temporary file and renamed, mode 0644. The lobby logs one line if a write fails and one when writes resume.

The units run as `decombat` with `ProtectSystem=full`, where `/var/lib` is writable. If the hardening drop-in is installed (`ProtectSystem=strict`, docs/OPS.md section 9), add the directory to the unit as well:

```sh
sudo systemctl edit decombat-lobby      # or decombat-lobby@<name>
# [Service]
# ReadWritePaths=/var/lib/decombat-public-games
```

## Preview, then turn on the Discord publisher

1. The publisher is `tools/vps/live_games.py`; a deploy installs it as `/opt/decombat/tools/live_games.py` and puts the two systemd unit examples in `/opt/decombat/tools/examples/`. It uses only Python 3's standard library. Install the units (a deploy never does this or enables anything):

   ```sh
   sudo install -m 0644 /opt/decombat/tools/examples/decombat-live-games.service /opt/decombat/tools/examples/decombat-live-games.timer /etc/systemd/system/
   ```

   Edit the `--source` arguments of `decombat-live-games.service` (`sudo systemctl edit --full decombat-live-games.service`) to one `LABEL=/absolute/path` per lobby process, for example `--source Vanilla=/var/lib/decombat-public-games/default.json --source Modded=/var/lib/decombat-public-games/modded.json`.
2. Preview what the publisher would send, without any network access:

   ```sh
   python3 /opt/decombat/tools/live_games.py --print --source 'Lobby=/var/lib/decombat-public-games/default.json'
   ```

   Compare a known public game's server, channel and room with the game client.
3. Create a dedicated incoming webhook under **#live-games, Edit Channel, Integrations**. Put its URL on the host in a root-only file; create the file with mode 0600 before entering the URL:

   ```sh
   sudo install -d -m 0700 /etc/decombat
   sudo install -m 0600 /dev/null /etc/decombat/live-games.webhook
   sudo nano /etc/decombat/live-games.webhook
   ```

   Keep the URL out of chat, shell history, logs and git. systemd's `LoadCredential` hands the file to the publisher only; the URL grants publishing access to #live-games.
4. After approval to publish the board, run `sudo systemctl daemon-reload`, then `sudo systemctl start decombat-live-games.service`. Check the first message in Discord before running `sudo systemctl enable --now decombat-live-games.timer`. The timer runs every 30 seconds and re-stamps the "Last checked" clock on every run, so a clock older than about a minute means the publisher is stalled. To stop posting updates, stop and disable the timer.

## Recovery

The publisher saves the Discord webhook ID and message ID in `/var/lib/decombat-live-board/board.json`. The file holds no webhook token. A missing message is recreated only when Discord says the old message was deleted. Rate limits defer the next request. A missing, malformed or stale lobby snapshot shows as unavailable, separately from an empty lobby. If Discord itself is unreachable, the previous message keeps its last-checked time, which makes a stalled publisher visible.

If an initial POST has an uncertain result (a dropped connection), the state is marked `pending` so a second board cannot appear. Stop the timer and service, look at #live-games, and take the message ID from Discord's "Copy Message Link" if the board exists. Set the state file to `{"webhook":"WEBHOOK_ID","message":"MESSAGE_ID"}`, keeping its ownership and permissions. If the channel shows the post never appeared, set it to `{"webhook":"WEBHOOK_ID"}`. Start the service, check the board, and resume the timer. Rotating to a different webhook also means reconciling the old message and its state.

The publisher's unit tests run anywhere with Python 3: `python3 -m unittest discover -s tools/vps/tests -p test_live_games.py`.
