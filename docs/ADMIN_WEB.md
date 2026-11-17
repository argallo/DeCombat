# The admin web: the dashboard at `https://admin.<your domain>`, hardware keys only

The operator dashboard and the beta list (docs/OPS.md section 15, docs/BETA.md) on a public name, with one way in: a
hardware security key (a YubiKey) with its PIN. There is no password, no SMS, no phone or computer passkey. Rules and
numbers: DC_DEFAULTS.md LG-382 .. LG-387. Status: implemented and unit-tested with a software key; not yet tried with a
real YubiKey, a real browser or a real tunnel.

```
browser --https--> Cloudflare --Tunnel (outbound from the VPS)--> cloudflared on the VPS --http--> lobby 127.0.0.1:10096
                                                                                                    (sessions, WebAuthn)
```

No new port is opened on the VPS: `cloudflared` connects out to Cloudflare, and the lobby's new listener takes loopback
connections only. The old way in, the SSH tunnel to the admin port (`tools/vps/ops_tunnel.ps1`), stays exactly as it was:
it is the break-glass path.

## 1. The lobby: `[admin_web]`

In the default lobby's `/opt/decombat/config/server.toml`:

```toml
[admin_web]
enabled = true                              # off by default: nothing listens until then
bind = "127.0.0.1:10096"                    # loopback only (the check refuses anything else)
public_origin = "https://admin.example.com" # what the browser shows: https:// and the host, no trailing slash
rp_id = "admin.example.com"                 # the WebAuthn relying-party id: the host of public_origin
idle_minutes = 30                           # a session with no request for this long ends
session_hours = 12                          # and every session ends this long after the sign-in
max_keys = 4                                # most keys enrolled at once
attempts_per_ip = 10                        # sign-in / enrolment attempts per client address in 10 minutes
attempts_global = 60                        # and in all
```

Use your own name in place of `admin.example.com` (for DeCombat: `admin.decombat.com`). `rp_id` is bound into every key
at enrolment: if you change it later, every key must be enrolled again. `DC_ADMIN_WEB__ENABLED=true` and the other
`DC_ADMIN_WEB__*` variables work as environment overrides. `dc-config check /opt/decombat/config/server.toml` validates the
section. Then `sudo systemctl restart decombat-lobby`; the log says `admin web on http://127.0.0.1:10096 for https://...`.

The modded lobby needs no admin web of its own (leave its section off): the default lobby's serves its dashboard too
(section 7).

## 2. Cloudflare Tunnel

1. Cloudflare dashboard, **Zero Trust** (one.dash.cloudflare.com) → **Networks** → **Tunnels** → **Create a tunnel** →
   type **Cloudflared** → name it (for example `decombat-admin`) → Save.
2. "Choose your environment": **Debian**, 64-bit. The page shows the install commands; run them on the VPS. They are
   Cloudflare's apt repository, as of this writing:

   ```
   sudo mkdir -p --mode=0755 /usr/share/keyrings
   curl -fsSL https://pkg.cloudflare.com/cloudflare-main.gpg | sudo tee /usr/share/keyrings/cloudflare-main.gpg >/dev/null
   echo 'deb [signed-by=/usr/share/keyrings/cloudflare-main.gpg] https://pkg.cloudflare.com/cloudflared any main' \
     | sudo tee /etc/apt/sources.list.d/cloudflared.list
   sudo apt-get update && sudo apt-get install cloudflared
   sudo cloudflared service install <TOKEN>     # the token the dashboard shows; it makes the cloudflared service
   ```

   The dashboard lists the connector as **Connected** a few seconds later. Treat the token like a password (whoever has
   it can run your tunnel); `cloudflared service install` stores it in the service, nowhere in this repository.
3. **Public hostname** (newer dashboards: **Published application routes**) → Add: subdomain `admin`, domain
   `decombat.com`, path empty, service type **HTTP**, URL `127.0.0.1:10096` → Save.

The public hostname **creates the DNS record itself** (a proxied CNAME `admin` → `<tunnel id>.cfargotunnel.com`). If an
`admin` record already exists, delete it first or the save fails. The browser side is Cloudflare's certificate
(Universal SSL covers `admin.decombat.com`); between `cloudflared` and the lobby it is plain HTTP on loopback.

The lobby answers only requests whose `Host` is the configured name. `cloudflared` passes the browser's `Host` on; if the
log shows every request refused with 421, set the public hostname's *Additional application settings → HTTP Settings →
HTTP Host Header* to `admin.decombat.com`.

## 3. Enrol your keys

Each key: run on the VPS, as the service user, with the service's config:

```
sudo -u decombat /opt/decombat/bin/dc-lobby-server admin-web enroll --label "YubiKey 1" \
  --config /opt/decombat/config/server.toml --db /opt/decombat/data/ca.db
```

It prints a one-time link, `https://admin.decombat.com/enroll?t=...`, valid 15 minutes for one key. Open it on your PC,
plug the YubiKey in, press **Enrol security key**, and:

- when the browser asks whether the site may **see your security key's make and model**, allow it: that answer (the
  attestation) is how the server knows it is a YubiKey. Refused, the enrolment fails with "the browser sent no
  attestation";
- when Windows or the browser offers places to save a passkey, pick **Security key** (not this device, not a phone);
- enter the key's FIDO2 PIN (the browser asks you to set one if the key has none) and touch the key.

Then run the command again for the second key (or sign in and use **Add another key** on the Security keys page: it asks
for a touch of a key you already have first). Without `--label` the page's name field, or "Security key N", is used.

Refused at enrolment: phone and computer passkeys, password-manager passkeys, keys of other makes, U2F-only keys without a
PIN, and any key whose attestation certificate does not chain to Yubico's published FIDO roots
(<https://developers.yubico.com/PKI/>, embedded in the lobby).

## 4. Daily use

The dashboard's **Updates** tab deploys a commit of master to the VPS and the game hosts: docs/DEPLOY_FROM_ADMIN.md
(set up once as root; a deploy to the VPS restarts the lobby, which ends this session).

`https://admin.decombat.com` → **Sign in with security key** → PIN → touch. The header of the dashboard has a **Security
keys** link: the list, rename, add, remove (adding and removing ask for a fresh touch; the last key cannot be removed
there). A session ends after 30 minutes without a request or 12 hours after the sign-in, and when the lobby restarts.
Sign out on the Security keys page.

On the command line:

```
dc-lobby-server admin-web keys list   --config ... --db ...
dc-lobby-server admin-web keys remove 2 --config ... --db ...     # the last key only with --force
```

Sign-ins, enrolments, key changes and refusals are events of kind `admin_web` in the dashboard's Problems / events list
(refusals count as problems) and lines `admin-web: ...` in the lobby log, by key number, name and a short credential id.

## 5. A lost or stolen key, or both lost

- **One key lost or stolen:** sign in with the other key, Security keys → **Remove** the lost one (or
  `admin-web keys remove <id>` over SSH). Its sessions end with their next request. Enrol a replacement.
- **Both keys lost:** nobody can sign in at the web page any more; that is the point. SSH to the VPS (the key in
  `~/.ssh/decombat_vps`) and run `admin-web keys list`, then `admin-web keys remove <id> --force` for each lost key, and
  `admin-web enroll` for each new one. Until then the dashboard is still reachable through the SSH tunnel
  (`tools/vps/ops_tunnel.ps1`, docs/OPS.md section 15).
- **Turning it off:** `enabled = false` (or stop the `cloudflared` service: `sudo systemctl stop cloudflared`) and restart
  the lobby.

## 6. What is checked (summary)

- Only the configured `Host`; the TCP peer (always `cloudflared`) is never trusted; the client address for the limits is
  `CF-Connecting-IP`.
- Without a session: the sign-in and enrolment pages, their script, and the four ceremony requests; nothing else.
  Everything of the dashboard and the beta list needs a session; the admin port's `/status`, `/stop` and room commands
  are not served at all.
- Every POST: `Content-Type: application/json` and `Origin` equal to `public_origin`; with a session also the session's
  CSRF token (`X-Admin-CSRF`, the beta page sends it as `X-Beta-CSRF`).
- WebAuthn: one-time challenges of 3 minutes; the client data's type, challenge and origin; the authenticator data's
  rpId hash, user presence, user verification (the PIN), not a synced passkey (backup-eligible flag); the signature with
  the stored key (ES256 or EdDSA); a signature counter that went back is refused (a key that keeps 0 stays at 0).
- Enrolment: `packed` attestation with an x5c chain whose every link's signature and validity is checked up to an
  embedded Yubico root; the leaf is a version-3 "Authenticator Attestation" certificate, not a CA, of a P-256 key, whose
  AAGUID extension (when present) names the model in the authenticator data.
- Session: 32 random bytes in `__Host-dcadmin` (Secure, HttpOnly, SameSite=Strict, Path=/), held in the lobby's memory
  only; answers are `no-store` (but for the new dashboard's two files, section 8), `X-Frame-Options: DENY`, a Content-Security-Policy with `frame-ancestors 'none'` and no
  inline script except the dashboard's and the beta page's own, allowed by the hashes of the static templates (computed once, never from a served page). Styles may come
  from the listener's own stylesheet and inline (`style-src 'self' 'unsafe-inline'`, LG-442).

## 7. Further lobby instances (the Hacking Mod lobby)

A box that runs several lobby instances (docs/VPS.md "Several instances on one box") needs only one admin web: the
default lobby's. It serves every other instance's dashboard (Live, Problems, Matches, Players and the account writes,
Alerts, Bots) and beta list under `/i/<name>/ops`, behind the same sign-in, keys and session. Rules: DC_DEFAULTS.md
LG-399, LG-400. Status: implemented and unit-tested (a second lobby's admin port in the tests, the pages rendered in
headless Edge with stub data); not yet run on the VPS.

In the default lobby's `/opt/decombat/config/server.toml`, inside `[admin_web]` (before any other `[section]`):

```toml
label = "Vanilla lobby"                     # this lobby's name in the switcher
instances = [
  { name = "modded", label = "Hacking Mod lobby", admin = "127.0.0.1:10091",
    env_file = "/opt/decombat/instances/modded/decombat.env" },
]
```

- `name`: the path part (`https://admin.decombat.com/i/modded/ops`), 1..32 of `a-z 0-9 -`.
- `admin`: that instance's loopback admin port, `DC_ADMIN` in its `decombat.env` (`tools/vps/instance.sh list` shows it).
- `env_file` (optional): when that file sets `DC_ADMIN_TOKEN`, the admin web sends it as the instance's admin token.
  It is read at each request (the file is `root:decombat 0640`, so the lobby's user can read it); the browser never
  sees it. Leave it out when the instance's admin port has no token.

`dc-config check /opt/decombat/config/server.toml` validates the lines; then `sudo systemctl restart decombat-lobby`.
Nothing changes on the modded instance: it keeps its loopback admin port (and the SSH tunnel to it) as before.

What you see: the top bar carries a switcher, **Vanilla lobby | Hacking Mod lobby**, with the open one highlighted; on
the other instance's pages the top bar has an amber rule, its name stands next to "DeCombat" and in the tab title, so a
ban or a bot change is never made on the wrong lobby by mistake. The switcher keeps the open tab (Live, Players, ...).

How it works and what is checked:

- The browser talks only to the admin web. A request under `/i/<name>/` passes the admin web's own checks first (session,
  `Origin`, the session's CSRF token on every POST); then the admin web sends it to `admin` itself, as the SSH tunnel
  would: `Host` and `Origin` the instance's loopback address, its admin token, and on a write the instance's own page
  token, which it reads from the instance's `/ops` page just before (either dashboard page carries it). The pages the browser gets carry the session's
  token, never the instance's.
- Only configured names; only `/ops` and `/ops/*`; plain paths (no `..`, no percent escapes in the path). The deploys
  (`/ops/updates*`) are not forwarded and the Updates tab is gone on the other instance's pages: a deploy is for the whole
  box and is started from the default lobby's Updates tab.
- Bodies up to 64 KiB, 3 s to connect, 20 s for the answer, answers up to 8 MiB, only JSON or the two pages; a redirect is
  never followed (502). An instance that is down answers 502 "does not answer" in the page.
- Account writes on the instance record `the admin web, key #<n> from <address>` in its Players tab, as on the default
  lobby (the header `X-Admin-Via`, which the instance takes only behind its loopback checks). The default lobby also logs
  each forwarded write as an `admin_web` event (`key #<n> from <address>: POST /ops/player/ban on instance modded (answer
  200)`).
- Both instances must run the same build (they share `/opt/decombat/bin`): the admin web's Content-Security-Policy allows
  the pages' inline scripts by the hashes of its own templates.

## 8. The new dashboard (`/ops`) and the classic one (`/ops/classic`)

The dashboard is being rebuilt as an app with a side navigation, screens that work on a phone and clearer confirmations
(rules: DC_DEFAULTS.md LG-441 to LG-451). The app is what `/ops` opens: `https://admin.<your domain>/ops` after the
sign-in (a further instance: `/i/<name>/ops`), or `http://127.0.0.1:<admin port>/ops` through the SSH tunnel (with
`?token=` when the port has a token). Its screens arrive one by one.

The dashboard the sections above describe (the tabs Live, Problems, Matches, Players, Alerts, Bots, Updates) is the
**classic dashboard** and stays whole at `/ops/classic` (`/i/<name>/ops/classic`, and the same path on the admin port)
until every screen has moved. Nothing of it is gone:

- the app's navigation ends with **Classic dashboard**, and the classic page's bar has **New dashboard**;
- a screen that has not moved yet says so and opens the matching classic tab with one button (Bots opens
  `/ops/classic#bots`, Deploy `#updates`);
- an address saved before the change (`/ops#bots`, `/ops#match=12`) opens the app on the same thing;
- the beta list (`/ops/beta`) and the key page (`/keys`) are where they were;
- every JSON address and every write under `/ops` is unchanged, so scripts that read them need no change.

`/ops/next`, the app's address while it was a preview, redirects to `/ops` on the admin web.

The first screen, **Overview**, is in the app (LG-445). Top to bottom:

- the **status headline**: the worst thing that is wrong right now in one sentence, green "All clear." when nothing is;
- **Needs attention**: one row per thing that is wrong or waiting, the serious ones first, each a link to the screen
  where it is looked at: an offline game server or one on another build, people with a bad or shaky connection (the
  link opens their match), failing alert checks, problems logged in the last hour, fewer bots running than wanted, a
  deploy under way or failed in the last hour, new commits on master, a release ready to tag or failed, open bug
  reports. What only waits (new commits, a release to tag, open bug reports) is listed but leaves the headline green;
- four numbers: players online, matches running, rooms open, game servers online;
- a card per **running match** with each person's round trip and connection (bots are counted, not listed);
- the **game servers** and the open **rooms**.

It asks the lobby again every 30 seconds while the tab is visible and has a Refresh button; when the lobby does not
answer it says so and keeps the last answer on the page. It only reads: the same JSON the classic Live tab reads
(`/ops/live`, `/ops/alerts`, `/ops/events`), and for the list `/ops/bots`, `/ops/updates`, `/ops/updates/release` and
`/ops/triage` (the last three not on a further instance, where they are for the whole box).

**Health** (LG-446) is the classic tabs Alerts and Problems on one screen: a line that says whether a check fails
or something went wrong in the last hour, the alert checks with the failing ones first, and the problem log, newest
first (the newest 50, with a button for all 300 the lobby keeps in the answer). A log row opens the person it is
about, or its match when it names no person. The classic addresses `/ops#alerts` and `/ops#problems` open it.

**Matches** (LG-447) is the classic tab Matches and its match page. The list shows the newest 100 recorded matches
(when, how long, room, mode and map, server and build, people and bots), marks one that is running, one that ended
any other way than with a result, and silent drops. A match opens its page: the facts of the match, a card per person
with the connection over the whole match (round trip mean, p95 and worst, jitter, input rate, longest gap, resends,
drops, packets, a small chart of the round trip per 10 s, and how the person left when it was not the match's end),
the match's events, and behind a button the reports the game server sent every 10 s as a table that scrolls sideways
on a phone. The classic addresses `/ops#matches` and `/ops#match=<id>` open it.

**Bug reports** (LG-448) is the card of that name from the classic Updates tab as a screen of its own (not on a
further instance): the poller's state, the open reports first and the done ones below, each with its **Done** tick.
A tick is saved at once and can be taken back; when the lobby refuses it the screen says why. A long report shows its
first 600 characters with a button for the rest. A report's text and its author are shown as text; only an attachment
the lobby accepted as a link is a link, and it opens in a new tab.

**Players** (LG-450) is the classic tab Players, read only. The list shows every account, 50 at a time with a
button for the next 50: found by a part of the character name or the login as you type, ordered by last seen (the
people online first), newest account or most matches, without the bots unless **Include bots** is ticked, only the
banned ones with **Only banned**. A row says who is online, when the others were last seen, when they joined, their
kills and deaths, how much they played (a bar against the most of the rows shown) and whether the account is banned,
disabled, muted, a GM or a bot. A player opens their page: where they are now, a red banner when they are banned (until
when, by whom, why) and a yellow one when the login is disabled, their rank, EXP, GP and NX, kills for every death,
the matches of the last 14 days and how many of them they left another way than by quitting or at the end, their
typical round trip with the mix of good, shaky and bad connections, a bar per match of the mean round trip in the
colour of the connection, their matches per day, the account's facts, the matches as a list (each opens the match,
each with the fixed-scale round trip bar) and their events. The classic address `/ops#timeline=<name>` opens it.

**Changes to an account** (LG-451) are a row of buttons on the player's page: Give or take, Ban (Change the ban,
Lift the ban), Kick, Mute (Change the mute, Unmute), Disable login (Enable login), Send a reset mail, Delete account.
A button that cannot be used right now is grey and a line under the row says why (not online, no character yet, no
recovery address, mail not set up, a GM account, online). Every change that somebody notices asks first, in one
dialog: the form when the change has one (amount, length, minutes, reason), then the question: the change as a
sentence, what will happen with the numbers of this moment (the balance before and after, until when the ban
runs, that they are online and will be dropped), the reason again, and a button that names the change (`Ban for 3
days`, `Give 500 NX`), red when it drops or locks somebody out. **Back** returns to the form with everything typed.
Deleting an account also needs its login typed. On a further lobby instance the question starts with `On: <lobby>`.
When the lobby refuses a change the dialog stays open and says why; nothing typed is lost. When it worked the dialog
closes, a note at the edge of the window shows the lobby's own sentence, and the page reads the account again.
Unmute and Enable login are undone with one press, so they do not ask. The writes are the same as the classic
page's (`POST /ops/player/<action>`), recorded the same way. The loadout is still given on the classic page.

**Charts** (LG-449). The screens draw what they read instead of only listing it. The Overview opens with the
status sentence large on a coloured card, and its numbers each carry a small picture: where the players online are,
the connections of the people playing, the seats taken, a dot per game server, the bots running. Every person in a
running match has the classic page's bar again: the round trip on a fixed scale of 0 to 300 ms in the colour of the
connection, with a tick at the worst of the window, so two people compare by eye. A room shows its seats as a bar.
Two charts say what happened lately: matches per day over 14 days (the ones that ended another way than with a
result in red) and problems per hour over 24 hours; Matches and Health show the same two, Health also the problems by
kind. A match's page draws everybody's round trip over the whole match on one axis (the four highest in a colour of
their own, the others grey; point at a time to read every value), the score of the two sides as one bar, and the
fixed-scale bar on each person's card. The navigation has a picture per screen. A chart is never the only place a
number is: it sits beside the list or the figures it is drawn from. A list the lobby cut at its limit does not reach
all the way back: the days or hours before its oldest row are hatched as not known, not drawn as nought. The charts
are drawn by the app itself with plain elements; no chart library is bundled.

Status: the frame, the Overview, Health, Matches, Bug reports, Players and the charts are implemented, unit-tested (the lists' rules, the charts' arithmetic) and looked at in a headless browser
on the admin port at desktop and phone width, with a quiet lobby and with made-up busy answers; not yet tried in a real
browser behind the tunnel or with real matches.

It is the same lobby, the same JSON and the same guards; only the page is new. The page loads two files of the lobby
itself, `/ops/app.js` and `/ops/app.css`, embedded in the binary: nothing comes from another site, there is no inline
script, and the libraries' licences are at `/ops/licenses.txt`. Their sources are in `web/admin` (web/admin/README.md
says how to change and rebuild them); the servers never need Node.

On the admin web the browser may keep those two files (LG-443): the page names them by the build
(`/ops/app.js?v=<16 hex digits>`), and asked by that address they answer `Cache-Control: private, max-age=31536000,
immutable`; everything else, the page included, stays `no-store`. So a phone downloads the app once per server build
and a new build is picked up on the next visit. `private` means a proxy in front of the lobby must not keep them. The
loopback admin port does not do this. Status: implemented and unit-tested; not yet watched in a real browser behind the
tunnel (a second visit should show the two files coming from the browser's cache).
