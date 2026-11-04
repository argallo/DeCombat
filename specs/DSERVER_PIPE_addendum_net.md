# Addendum: the `@net` event (link windows to the lobby), net-tooling, 2026-10-29

Extends `DSERVER_PIPE.md` §4 (server -> lobby events). Tags as in §0 of the base spec: D = our design (DeCombat rule).
Rows DSV-411 (server), DSV-412 (the window's round trip) and LG-185 (lobby) in `DC_DEFAULTS.md`. Nothing here comes
from the 2009 client or server: the 2009 game server's channel to its lobby is not known, and the client has no part
in this line.

## 1. The line

```
@net client=<id> uid=<uid> ms=<window> rtt=<mean> rtt_min=<ms> rtt_max=<ms> rtt_n=<samples> jitter=<ms>
     input=<per s> gap=<ms> resends=<n> drops=<n> in=<n> out=<n>
```

One line (shown wrapped), every 10 s per linked client, in the same tick as the server log's `client N: link 10 s:`
line (CLI-1331), from the same window.

| key | type | meaning | tag |
|---|---|---|---|
| `client` | u16 | the client id of `@join` / `@leave` | D |
| `uid` | u32 | the roster uid (the `user` line's field 1) once the client was identified by its hello name, else 0 | D |
| `ms` | u32 | window length in ms (about 10 000; the first window starts when the link opened) | D |
| `rtt` / `rtt_min` / `rtt_max` / `rtt_n` | ms, ms, ms, count | mean, min and max of the transport's raw round-trip samples taken in the window, and their count (DSV-412); a window without a sample carries the link's 16-sample estimate in all three and `rtt_n=0` | D |
| `jitter` | ms, 1 decimal | interarrival jitter of the client's player updates against their client time (RFC 3550 §6.4.1 form, CLI-1331) | D |
| `input` | per s, 1 decimal | player updates received per second in the window | D |
| `gap` | ms | longest gap between two player updates in the window | D |
| `resends` / `drops` | count | reliable resends and flow-control drops of the server's sends to this client in the window | D |
| `in` / `out` | count | datagrams received from / sent to the client in the window | D |

Numbers are the window's, not totals. Unknown keys are ignored by the reader (base §4 parsing rule); a line without
`client=` is an ignored event.

## 2. Lobby use (LG-185)

The lobby keeps the newest `@net` per client id of each game (with its arrival time) and serves them on the admin
endpoint's `GET /net` together with the account (`@join` table), the nick and the age of the player's lobby TCP
connection (LG-186). The values are diagnostics: nothing in the lobby's match handling reads them.

## 3. Compatibility

An older server sends no `@net`: `GET /net` lists its players with `report_age_secs: null`. An older lobby logs the
line as an ignored event (only with `--dserver-verbose`).
