# Privacy

**Draft, not legal advice, needs review.**

This page says what the servers the DeCombat project runs store about you, based on the server's database and logs. Other
operators run the same software but make their own choices: ask them. How long things are kept is not fixed by the software.
Before this page is published, the operator fills in the marked periods.

## What the server stores

**Account** (database):

- login name, and a password hash (Argon2id; the password itself is never stored or logged);
- e-mail address, only if you gave one (used for password recovery and login-name reminders);
- created and last-login times, and flags such as disabled, GM or bot;
- for a login through an identity server: that server's id and your account number there, instead of a password;
- invite code used, and hashed recovery codes with their expiry.

**Character and play** (database): nickname, EXP, GP and NX, kills, deaths, wins, losses, clan membership and records, items and
loadout, shop purchases and coupon use, friends, friend requests, inbox messages and match results (map, mode, score and
per-player detail).

**Reports** (database): when you send a report, its text and category, the name it's about, your account number, login and
nickname, where you were on the server, and the time. If the operator has turned on report mail, the same goes by e-mail to the
operator's address.

**Diagnostic reports** (database row and a file on the server), only when you allow them (the client asks the
first time: always, ask each time, or never; `/diag` changes it) or send one yourself (`/report`, the bug form's
"attach diagnostics"). A report holds: the client version, your operating system, processor, memory and graphics card
with its driver, the display settings, the tails of the game's own log files on your machine, the network figures of
the match that failed (round-trip time, loss, packet sizes), the result of a connection test if you ran one, the
game's stall and crash notes, your login name, the time, and the IP address it came from. Before it leaves your
machine the client removes passwords, login tokens, e-mail addresses, your Windows user and computer names (also in
file paths) and every IP address other than the servers'; it never sends memory dumps. If report mail is on, the
operator gets a short summary by e-mail; the report itself stays on the server. Reports are deleted after 30 days by
default (`DC_DIAG_RETENTION_DAYS`). **[operator: confirm the period]** Details: `docs/DIAGNOSTICS.md`.

**Connection test**: the client sends small UDP packets to the server you test; the server answers each with a packet of
the same size and keeps nothing (it counts packets per address in memory to limit them).

**Moderation** (database): bans and mutes with reason and time, and a record of operator actions.

**Server logs** (text files on the server):

- logins and refused logins with the login name;
- IP addresses, in lines about refused logins, recovery requests and connection errors;
- lobby chat and clan chat: **every line, with the nickname**;
- rooms, matches and errors. Game server logs also hold per-connection network figures such as round-trip time.

The servers also see your IP address while you're connected, as any internet service does. They use it to connect you and to
limit login and registration attempts; the attempt counters are held in memory.

**Identity tokens.** If you use an identity login, the identity server gives your client a signed token holding its id, your
account number, login name, nickname, and issue and expiry times (24 hours by default). Each lobby you log in to with it reads
these, and your client caches the token in a file on your machine. Your password doesn't reach those lobbies.

## What your client keeps

Your client writes logs and settings on your own machine (`%LOCALAPPDATA%\DeCombat` on Windows, `~/Library/Logs/DeCombat` on macOS).
It sends parts of them only as a diagnostic report you allowed (see above). A report that could not be sent waits in
your profile folder (`diag_queue`, at most 10 reports, 14 days) and goes at the next start. The client fetches a public directory file for
the list of community servers, which is an ordinary web request.

## What it's used for

Running the game, keeping your progress, recovering your account, stopping abuse and fixing bugs. No advertising and no sale of
data. The software contains no analytics or tracking.

## Who sees it

The server operator and the people they make GMs. Your nickname, stats, clan and chat are visible to other players, as in any
online game. Hosting providers have the technical access their role implies, and e-mail goes through the operator's mail provider.
Data may be disclosed where the law requires it.

## How long

- Database records: until the account is deleted or the server is reset. **[operator: confirm]**
- Logs: **[operator: state the period, and rotate logs to match]**

The software doesn't delete logs or old rows by itself, except diagnostic reports (30 days by default).

## Your choices

Write to contact@decombat.com to ask for a copy of your data, a correction or deletion. Say which server and account, and write
from the e-mail address on the account if there is one. Depending on where you live, you may have further rights under laws such
as the GDPR. The service isn't meant for children under 13.

Terms: [TERMS.md](TERMS.md).
