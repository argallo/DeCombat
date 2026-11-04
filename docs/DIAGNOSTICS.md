# Diagnostic reports (beta)

Our own design, no 2009 counterpart. When something goes wrong for a player (the match link drops, the server goes
away, the game stalls or crashes, the GPU resets, a join fails), the client can send the owner of the lobby a
**diagnostic report**: the facts needed to find the cause, without the player mailing log files by hand. Players
choose whether this happens automatically, after a question, or never. A **connection test** tells a player in plain
words whether the network between them and a server is good enough to play.

Status: implemented and unit-tested (client bundle, redaction, consent, queue, upload, probe; lobby endpoint, UDP
echo, storage, mail, CLI). Not yet tested against a real player's failure.

- DC-DEFAULT rows: CLI-1840 (bundle and redaction), CLI-1841 (consent), CLI-1842 (queue and upload), CLI-1843
  (connection test), LG-360 (upload endpoint, limits, storage, mail), LG-361 (UDP echo), LG-362 (`reports` CLI,
  retention). See `DC_DEFAULTS.md`.
- Privacy: `docs/legal/PRIVACY.md` (section "Diagnostic reports").

## 1. What the player sees

- **The first problem** (or the first start after a crash) asks once: *"Send diagnostic reports automatically when
  the game has a problem? ..."* Yes = **Always**. No asks a second question: *"Ask you each time instead?"* Yes =
  **Ask each time**, No = **Never**. The answer is kept in the profile (`[DeCombat] diag.consent`).
- **Ask each time**: after the 2009 box of the failure (for example "the connection was lost") a box asks
  *"Send a diagnostic report to the server's owner?"* with **Send report** / **Test connection** / **No** (the test
  button only after a lost connection, a server that went away or a failed join). Test connection runs the test of
  section 4 against that server, shows the result in plain words and asks again whether to send the report, now with
  the result attached.
- **Always**: the report goes in the background; the lobby chat says when it went (or that it waits). After a
  network failure a box still offers **Test connection** / **OK**; the test's result then goes as a report of its own.
- **Never**: nothing is gathered, nothing is sent, nothing is queued.
- **By hand** (any consent, also Never): `/report <what happened>` in the lobby chat or in a match's chat sends one
  now with the text as the player's note. After the in-game report form is sent with the **Bug** category, a box
  asks *"Attach diagnostics to your bug report?"*.
- **Changing the choice**: `/diag always`, `/diag ask`, `/diag never` in the lobby chat; `/diag` alone shows it.
- **Connection test** (placement: project decision 2026-11-02): the **Connection Test** panel of the server select
  screen (after the login; the lower right panel of the 2009 page, which holds no 2009 control) with its **Test**
  button tests every entry of the list: one row per entry (*"Good: 38 ms, 0% loss"*, *"UDP blocked"*, *"Packet loss:
  12%, 80 ms"*) and the plain words of the highlighted entry under the rows, for example *"No answer: UDP is blocked
  on this network (school, office, some public Wi-Fi) or the server is down. The game cannot run like this."* Also
  the after-failure box (above) and `/nettest` in the lobby chat (`/nettest send` also sends the result as a report).
  An entry on another host than the lobby may have no echo; it shows *"No answer"* instead of *"UDP blocked"*.

A report that cannot be sent (no network, lobby down, limit reached) stays in `<profile>/diag_queue/` and goes at the
next start. At most 10 reports / 5 MB wait there; the oldest goes first when full; a report older than 14 days is
dropped.

## 2. What a report holds (CLI-1840)

One JSON document, gzip-compressed, at most **1 MiB compressed** (the client cuts the largest file tails until it
fits):

| key | content |
|---|---|
| `format` | `"dcdiag1"` |
| `id` | 16 hex digits made by the client; an upload repeated after a lost answer is stored once |
| `created` | unix seconds |
| `kind` | `link_lost`, `server_gone`, `stall`, `crash`, `panic`, `gpu_lost`, `join_failed`, `manual`, `bug_report`, `nettest` |
| `summary` | one line, at most 200 characters |
| `client` | `version` (release tag or `dev`), `commit`, `target`, `profile` |
| `system` | `os`, `arch`, `cpu`, `cores`, `ram_mb`, `gpu`, `driver`, `backend` (what the window's adapter reported) |
| `settings` | display mode and size, frame-rate cap, anti-aliasing, show-FPS, network overlay, consent |
| `match` | lobby address, game server address, map, mode, how the session ended (when there was a match) |
| `link` | the match link's last numbers: RTT mean / min / max, jitter, loss in / out, resends, largest datagram |
| `nettest` | the last connection test's result (when one ran) |
| `note` | the player's text (`/report`, the bug form), at most 1000 characters |
| `login` | the login name the player typed |
| `files` | `[{"name", "text"}]`: the tails of `dc-client.out` (256 KiB), the previous run's `dc-client.prev.out` (256 KiB), `dc-client.net.jsonl` (512 KiB, the last match), `dc-client.watchdog.log` (64 KiB), `dc-client.hang.txt`; the text files of `stall_*` and `crash_*` folders and `panic_*.txt` not yet reported (128 KiB each) |

**Never in a report** (redaction, applied to every text before it is packed): passwords (`password` / `--password`
values, JSON `"password"` fields), identity tokens (`tok:...`, `Bearer ...`, JSON `"token"` fields, any
`<base64url>.<base64url>` of token length), mail addresses, the Windows user name and the computer name (replaced
by `%USER%` / `%HOST%`, also inside paths: `C:\Users\%USER%\AppData\...`), and every IPv4 / IPv6 address other than
the servers' own and loopback (replaced by `[ip]`). Minidumps (`*.dmp`) are never sent: they hold memory, which can
hold a typed password. The owner who needs one asks the player for it.

## 3. Upload (CLI-1842, LG-360)

`POST /diag/report` on the lobby's web port (the auth port + 8080, TLS with the pinned certificate, the same port
as `/identity/token` and `/register`).

- Headers: `Content-Type: application/x-dcdiag`, `Content-Length` (at most 1 048 576), and
  `Authorization: Bearer <identity token>` when the player has one (CLI-1610). Without a token, or with one this
  lobby does not accept, the report is anonymous (it is still taken, under the anonymous limits).
- Body: the gzip document of section 2.
- The lobby inflates at most 16 MiB (more = 413), checks `format`, and answers JSON:
  - `200 {"id":N}`: stored (also for a repeated client `id`: the first row's number);
  - `400 {"error":...}`: not a report (the client drops it);
  - `413`: too large (the client drops it);
  - `429 {"error":...,"retry_after":S}`: a limit (the client keeps it queued);
  - `503`: the store is full or the endpoint is off (`DC_DIAG=off`, answers 404 then) (kept queued).
- Limits (counted from the stored rows, so a restart keeps them): per identity account 6 per hour and 20 per day;
  anonymous per IP 3 per hour and 10 per day; any per IP 30 per day; the whole lobby 300 per day; all stored
  reports together at most 2 GiB (`DC_DIAG_MAX_TOTAL_MB`).
- Storage: a row in the `diag_reports` table of the lobby DB and the uploaded bytes unchanged in
  `<DB folder>/diag_reports/<YYYY-MM>/<N>.dcdiag.gz`. Rows and files older than `DC_DIAG_RETENTION_DAYS` (30) are
  deleted at the start and every hour.
- Mail: one mail per report to `mail.reports_to` (the in-game reports' address, LG-316), through the same mail
  worker, at most `mail.reports_per_hour` diagnostic mails per hour (the rest wait). Subject
  `Diagnostic report #N: <kind>`; the body is a short summary (time, kind, summary line, account / login, client
  version, OS, GPU, the link line, the file list with sizes) and the command that shows the rest. The files stay on
  the server. With the mail off the rows are kept and never mailed.

## 4. Connection test (CLI-1843, LG-361)

The lobby answers a small UDP echo on the **web port number over UDP** (18080/udp for a lobby whose auth port is 10000).

- Request: 16 to 8192 bytes; bytes 0..4 `DCPR`, byte 4 version `1`, bytes 5..8 zero, bytes 8..16 the client's
  sequence (opaque to the lobby), the rest any filler.
- Reply: **exactly as long as the request, never longer**: `DCPA`, bytes 4..16 copied, the rest zero.
- The lobby drops (no reply): shorter / longer / wrong-magic datagrams, sources with a port below 1024, and anything
  over the rate limits: per source IP 40 datagrams and 160 KB per second (burst of 2 s), all sources 4000
  datagrams per second. `DC_DIAG=off` closes it.

The client sends 20 small probes 50 ms apart (RTT, jitter, loss), then sizes 512, 1024, 1200, 1300, 1400, 1472,
2000, 4000 and 8000 bytes (3 tries each, the larger ones are IP-fragmented on the way) and reports the largest size
that came back. Verdicts, in this order: no reply at all = *UDP blocked*; loss over 5 % = *packet loss*; RTT over
150 ms = *far / slow*; jitter over 30 ms = *unstable*; largest size below 1400 = *large packets dropped*; else
*good*. The open firewall rule for a VPS lobby: the web port over UDP (see `docs/VPS.md`).

## 5. Reading reports (LG-362)

On the server (works while the lobby runs; the DB is in WAL mode):

```text
dc-lobby-server reports [--db FILE] list [--limit N] [--kind KIND]
dc-lobby-server reports [--db FILE] show ID [--files]
dc-lobby-server reports [--db FILE] fetch ID [--out DIR]
```

- `list`: newest first: id, time (UTC), kind, account / login, version, size, the summary line.
- `show`: the row and the document's meta keys (client, system, settings, match, link, nettest, note) and the file
  list; `--files` prints the files' text too.
- `fetch`: writes `DIR/report_<ID>/` (default the working folder): `report.json` (the meta keys), one file per
  entry of `files`, and the original `<ID>.dcdiag.gz`.

On the VPS: `ssh -i ~/.ssh/decombat_vps root@<vps> 'dc-lobby-server reports --db /srv/decombat/lobby.db list'`, then
`fetch` and `scp` the folder.

## 6. Turning it off

- Lobby: `DC_DIAG=off` in the lobby's environment closes the upload endpoint (404) and the UDP echo.
- Client: `/diag never`; or `DC_DIAG=off` in the client's environment (nothing is gathered or sent, the boxes never
  open).
