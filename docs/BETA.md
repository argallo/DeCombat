# Beta signup by e-mail

The beta is open (since 2026-11-08, LG-433): anyone registers on the game's Sign up screen or at
`https://beta.decombat.com/register` without a code. The e-mail list stays as an updates list: people type their address
on the website and confirm it with a mailed link; you may still mail an invite code (it works, it is just not needed).
Rules and numbers: DC_DEFAULTS.md LG-370 .. LG-379, LG-381, LG-433.

```
decombat.com (Firebase) --fetch--> https://beta.decombat.com (Cloudflare, proxied) --TLS, port 18080--> lobby web port
                                                                                                          |
                           you: operator page (ssh tunnel) or `dc-lobby-server beta ...` ----------------+--> invite / feedback mail (Resend)
```

## 0. What a tester sees

Players register on the game's Sign up screen or on `https://beta.decombat.com/register`; neither asks for an invite
code since the open beta (a mailed code still works, and is required on a lobby run with `--register`). Notes for new
players:

- The registration makes the **login**. The **in-game name** is chosen at the first sign-in, on the game's nickname screen.
  It must differ from the login (ignoring case) and be 4 to 12 characters long; the lobby refuses a name that is taken.
- Names beginning with "GM" are reserved: only an account the operator has flagged as a game master (`dc-lobby-server
  account ... gm <NAME> --on`, docs/VPS.md section 7) can use one. A tester who types one is refused the name.
- The Hacking Mod lobby (the modded instance) keeps its own progress. The login works there too (one identity), but the
  character, rank, items and NX on that lobby are new and separate from the vanilla lobby's; its name is taken from the
  in-game name, with a number added if that name is already used there.

## 1. The lobby: `[beta]` and `[mail]`

`[mail]` must work first (docs/SERVER.md section 4b; the same Resend account that sends the recovery mails). Then in the
default lobby's `/opt/decombat/config/server.toml`:

```toml
[beta]
enabled = true                              # off by default: every /beta/* path is a 404 until then
site_origin = "https://decombat.com"        # the one site whose page may call the signup (CORS)
public_url = "https://beta.decombat.com"    # where the mail links point (the lobby's web port, as people reach it)
download_url = "https://decombat.com"       # shown in the invite mail
reply_to = "contact@decombat.com"           # Reply-To of every beta mail; empty = none
max_mail_per_day = 90                       # all beta mail in any 24 h (Resend free tier: 100 a day, shared with recovery mail)
confirm_reserve = 10                        # of those, kept for confirmation mails (an operator batch cannot use them)
invite_days = 14                            # how long a mailed invite code works
trust_cf_header = true                      # take the client address from CF-Connecting-IP (only behind Cloudflare, see 2)
ip_per_hour = 5                             # signup requests per client address per hour
global_per_hour = 60                        # signup requests per hour in all
```

`DC_BETA__ENABLED=true` etc. work as environment overrides. `dc-config check /opt/decombat/config/server.toml` validates
the origins and the reply address. Then `sudo systemctl restart decombat-lobby`. The live unit runs the open beta
(`--register-open`, LG-433). To go back to invite-only, run the lobby with `--register` instead: the invite codes are
then what the beta list hands out.

## 2. Cloudflare

The web port (live: 18080) speaks **TLS 1.3 with the lobby's own certificate** (self-signed unless you put a CA pair in the
data folder, docs/SERVER.md). On a public lobby a plain-HTTP request from any non-loopback address is refused (a GET is
redirected to https, a POST gets 403), and Cloudflare is a non-loopback address.

1. **DNS:** an `A` record `beta` -> the VPS address, **Proxied** (orange cloud).
2. **SSL/TLS -> Overview:** encryption mode **Full** (not Flexible: Flexible talks plain HTTP to the origin, which the lobby
   refuses, and the redirect loops; not Full (strict) unless the lobby has a CA-issued certificate for `beta.decombat.com`).
   Full accepts the self-signed certificate.
3. **Rules -> Origin Rules -> Create rule:** *When* hostname equals `beta.decombat.com`; *Then* "Destination Port" ->
   Rewrite to `18080`. (Without it Cloudflare connects to 443.)
4. **Firewall:** nothing to change; TCP 18080 is already open, and it must stay open to everyone because the game client
   uses it directly (registration, password recovery, the diagnostic upload). With `trust_cf_header = true` the lobby takes
   `CF-Connecting-IP` only from a connection out of Cloudflare's published ranges (built in, `betaweb.rs`); a direct
   connection that writes the header is counted by its own address.
5. Test: `curl -i -X OPTIONS https://beta.decombat.com/beta/signup -H "Origin: https://decombat.com"` answers `204` with
   `Access-Control-Allow-Origin: https://decombat.com`. A `525` means Cloudflare could not do the TLS handshake (rule or
   firewall); a `404` means `[beta] enabled` is off or not loaded.

## 3. The site

The Firebase homepage source is `web/landing/`. Its beta form uses `web/beta/beta.js` and `web/beta/beta.css`, which the landing build copies into the generated `web/landing/dist/` folder. The endpoint is the `BETA_ENDPOINT` constant in `web/beta/beta.js` (default `https://beta.decombat.com/beta/signup`). Firebase's Content Security Policy permits that origin for both JavaScript requests and plain form posts.

```sh
cd web/landing
npm test
npm run deploy
```

`npm run deploy` builds the static site and deploys only Firebase Hosting project `decombat`. Server configuration and mail credentials stay on the lobby host; none belong in website source.

## 4. The signup flow

1. The person submits the form: address, optional platform, the consent box.
2. The lobby stores a `pending` row and mails ONE confirmation link (a repeat signup is mailed again after 24 h at the
   earliest). The answer is the same for every address, new or not.
3. The click makes the row `confirmed`. Pending rows are deleted after 7 days.
4. You invite from the operator page or the command line: the row becomes `invited` and the invite code is mailed.
5. When the code is used (the registration page or the game's Sign up screen) the row becomes `registered` with the account.
6. Every mail has an unsubscribe link (one click); an unsubscribed row is never mailed again.

## 5. The operator page

Open the dashboard as in docs/OPS.md section 15 (`tools\vps\ops_tunnel.ps1 -Host <VPS_IP>`), then the **Beta list** link in
its header (or `http://127.0.0.1:10090/ops/beta?token=<DC_ADMIN_TOKEN>`).

- The banner shows the mail budget: used, the limit, and how many mails one batch may still send.
- Filters: status, platform, signed up after / before, part of an address. Registered rows show the account, its last login
  and the matches played; every row shows its newest mail (queued / sent / FAILED, it refreshes a few seconds after a send).
- **Select oldest confirmed** takes the first N confirmed rows (longest waiting first); boxes pick single rows.
- **Preview invites** shows what would go out and changes nothing. **Send invites to selected** makes one code per row and
  mails it. A row whose invite mail failed can be invited again (it keeps its code).
- **Feedback request...** opens an editable subject and text (`{name}` = the account's login or the part of the address before
  the @, `{email}`); it goes to the selected invited or registered testers. The default text asks for feedback and points to
  the in-game Report a Bug form and the `/report` chat command; replies come to `reply_to`.
- **Delete selected** (after a confirm prompt) removes the selected signups for good, see section 8; **Preview delete** shows
  what it would do. Each row's result is listed.
- A batch over the remaining budget is refused whole, with the numbers.
- Writes only work from this page (they need the page's own token and an `Origin` of the tunnel address), not from any other
  site open in your browser.

## 6. The command line

On the VPS, as the service user, with the same config and environment as the service (the SMTP password is in
`decombat.env`):

```
sudo -u decombat bash -c 'set -a; . /opt/decombat/config/decombat.env; set +a; \
  exec /opt/decombat/bin/dc-lobby-server beta list --config /opt/decombat/config/server.toml --status confirmed'
```

```
dc-lobby-server beta list [--status S] [--platform P] [--before YYYY-MM-DD] [--after YYYY-MM-DD] [--q TEXT] [--limit N]
dc-lobby-server beta invite (--emails a@x,b@y | --next N) [--dry-run]
dc-lobby-server beta delete (--emails a@x,b@y | --ids 1,2) [--dry-run]
dc-lobby-server beta template > feedback.txt        # edit it: "Subject: ...", a blank line, the text
dc-lobby-server beta mail --template feedback.txt (--emails a@x,b@y | --status invited) [--dry-run]
```

It runs the same code as the page, sends through its own mail connection and prints one line per row with the mail
server's answer. `--dry-run` sends nothing and changes nothing. The limit is shared with the running lobby (it is counted
from the database).

## 7. The mail budget

`max_mail_per_day` counts every beta mail queued in the last 24 hours, confirmations included, sent or failed. The
recovery and report mails use the same Resend quota but are not counted here, which is why the default is 90 of 100.
Confirmation mails may use the whole limit; operator batches must leave `confirm_reserve` free. When the budget is used up a
new signup is still stored, and its confirmation is mailed on its next signup attempt.

## 8. Data

Addresses are in the lobby database table `beta_signups`; the lobby log names rows by id only. Pending rows go after 7 days,
the mail log after 30; unsubscribed rows stay (they are the "never mail again" list). The client address of a signup is kept
for tracing abuse and cleared on unsubscribe. To delete a person on request (LG-381) use **Delete selected** on the operator page or `beta delete --emails x@example.org`
(`--dry-run` first shows what would go). It removes the row and the person's invite code if it is still unused; a used code
and the account made with it are left alone (delete the account separately if asked), and the mail-log entries lose their
link to the person but stay, so the mail budget count is unchanged. A deleted address that signs up again starts from pending.
