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
  only; answers are `no-store`, `X-Frame-Options: DENY`, a Content-Security-Policy with `frame-ancestors 'none'` and no
  inline script except the dashboard's and the beta page's own, allowed by the hashes of the static templates (computed once, never from a served page).

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
  token, which it reads from the instance's `/ops` page just before. The pages the browser gets carry the session's
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
