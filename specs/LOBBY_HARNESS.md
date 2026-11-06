# LOBBY_HARNESS.md: what the Rust lobby must accept to be a drop-in for `lobby_harness`

Status: DRAFT spec, 2026-10-24. Not yet implemented.
Header convention (no `specs/README.md` existed; proposed): Status / Scope / Evidence legend / numbered sections / Open
questions.
Scope: command line, start-up order, log formats, test-suite needs, DB schema the tests read, and the staged harness
change that lets the harness launch the Rust binary.

Evidence legend: **H** = fact from `lobby_harness` (`HARNESS: lobby_harness:<line>`),
**A** = from the command-line block of the reference lobby (original harness code, the only
part of that file read), **B** = stated in the task brief and not independently verified here, **D** = our design.
Nothing here is copied from forbidden sources. Wire constants that appear (message ids, offsets) are 2009 interop
facts already used by the harness.

## 1. Command line (A, lines 5178-5279)

The binary is `dc-lobby` (name D). It must accept every flag below with these defaults and meanings. Unknown flags are
a usage error (exit 2), like argparse. `--flag value` and `--flag=value` both work.

| flag | type | default | meaning |
|---|---|---|---|
| `--port` | u16 | 10000 | auth port (the client's `-authport`) |
| `--lobbyport` | u16 | 0 | lobby port; 0 => `port + 2` |
| `--clanport` | u16 | 0 | lobby port of the "Clan Server" entry; 0 => `lobbyport + 1` (resolved AFTER lobbyport) |
| `--lobbyip` | string | `127.0.0.1` | address advertised in the server list (offset 0x16+0x88 of the 0x2000 list carries the lobby port, H: lobby_harness:79-80) |
| `--bind` | string | `127.0.0.1` | listen address of auth, lobby, clan sockets and clan web; `0.0.0.0` = all interfaces |
| `--db` | path | `<exe dir>/data/ca.db` | SQLite file; created with its schema if absent |
| `--log` | path | empty = none | append-mode UTF-8 log (errors replaced, never fatal) |
| `--pktlog` | path | empty = none | append-mode hex packet log, format in section 4 |
| `--gameip` | string | `127.0.0.1` | game-server address put in the game-start packet |
| `--gameport` | u16 | 27888 | game-server port in the game-start packet (H: asserted, lines 541-545 and 603, main scenario start check) |
| `--port-per-room` | flag | off | game-start port = `gameport + channel*100 + room` instead of `gameport` (A help text; the arithmetic is the help text's, D to keep) |
| `--countdown` | f64 secs | 1.0 | seconds between the countdown ticks (message 0x3044); tests pass 0.05 |
| `--dserver` | flag | off | start a dedicated server per room on Start (see DSERVER_PIPE.md) |
| `--dserver-exe` | path | `<exe dir>/dc-dserver` | server executable; `.py` accepted (DSERVER_PIPE.md section 2) |
| `--dserver-world` | string | empty | world for every room; empty => derive from the room map |
| `--dserver-guid` | `ca` (legacy alternative accepted, unused) | `ca` | network identity |
| `--dserver-idle` | i64 secs | 60 | server stops after N s without clients |
| `--dserver-timeout` | f64 secs | 30 | wait for `@ready` |
| `--dserver-args` | string | empty | extra arguments, whitespace split |
| `--dserver-logdir` | path | empty | where per-room server logs and cwd go |
| `--dserver-verbose` | flag | off | copy server console lines into the lobby log |
| `--test-bot-account` | string, repeatable | `[]` | LOCAL TEST ONLY: register fresh test-bot accounts and adapt their kits (A help; the exact bot naming is in forbidden territory, D: accept and register the given names) |
| `--require-ready` | flag | off | refuse Start until every guest is ready |
| `--start-exp` | i64 | 0 | starting experience of new characters |
| `--start-gp` | i64 | 10000 | starting GP of new characters (H: suites assume 10000, e.g. line 589) |
| `--start-nx` | i64 | 0 | starting NX of new characters (tests pass 5000, then expect 5000 and 3900 after a purchase, lines 137-153) |
| `--server-max` | i64 | 1000 | capacity shown in the server list (client derives Low/Moderate/... load from players/capacity) |
| `--channel-max` | i64 | 0 | channel capacity; 0 => 200 (status and the "channel full" refusal) |
| `--clanweb` | i32 | -1 | clan web page port; -1 => `port + 8080`; 0 => off (H: `test_clan` relies on the `port+808080` default, line 736-760) |
| `--delay` | f64 secs | 0 | seconds before answering message 0x20a (for screenshots of the wait box) |
| `--lenient` | flag | off | accept message 0x300 with an unknown session key (old stub clients) |
| `--demo` | flag | off | stand-in rooms/users for single-client tests |
| `--check-password` | flag | off | first login sets the account password, later logins must match (refusal code 0x25 in the 0x201 reply, i.e. `0x00030025`; H: lines 611-640) |
| `--autochar` | flag | off | new accounts get a character named after the login (skips the nickname screen) |
| `--grant` | string | empty | login or nick of an account to raise (applied at start-up and after first-login character creation) |
| `--grant-rank` | string | empty | rank number 0..55 or a rank name; sets `--grant-exp` to that rank's exp (rank table from ROM Levels data at run time, CD-05) |
| `--grant-exp` | i64 | none | experience to grant |
| `--grant-gp` | i64 | none | GP to grant |
| `--grant-nx` | i64 | none | NX Prepaid AND NX Credit to grant |
| `--grant-force` | flag | off | set the values exactly; default only raises, never lowers |
| `--mods-dir` | path | none (new, D) | directory of server-hosted mod packages (docs/MODDING_GUIDE.md section 7); absent = no mods; the harness never passes it |

Resolution order (A, lines 5217-5244 as the code applies it): parse; if `--grant-rank` non-empty compute grant-exp from
it (a bad rank is a usage error); `lobbyport = port+2` if 0; `clanport = lobbyport+1` if 0; if `--channel-max` != 0
override every channel capacity; `clanweb = port+8080` if negative; open logs; open the DB.

Harness expectations on grants (H: `test_account_grant`, lines 657-705): `--grant gina --grant-rank "General Of The Army"
--grant-gp 999999 --grant-nx 999999` with `--autochar` creates the account at first login and yields exp 8000000,
GP 999999, NX 999999/999999 (8000000 = the harness's rank table; CD-05 says the DeCombat table is ours, so the Rust
implementation either keeps the harness numbers behind a config or the harness is updated: OQ-2). Rank name matching is
case-insensitive (`"major 3rd"` matches). Only-raise: GP below the starting 10000 stays 10000. `--grant-force` sets
exactly (rank 5 gives exp 8700, GP 50, NX 7/7).

Process-level behaviour (H): the harness sets `PYTHONIOENCODING=cp1252` for the server (line 808-810): the Rust binary
must not fail when its stdout is a cp1252 pipe or closed (`stdout=DEVNULL`); logging to `--log` is UTF-8. Stop: the
harness sends `terminate()` (SIGTERM / TerminateProcess) and in the persist suite `kill()`, a hard stop. The DB must be
consistent after a kill: commit every handler transaction before replying (H: `test_persist_restart`).

## 2. Start-up order (A, `main()` lines 5240-5270; D where it says so)

1. Parse flags, resolve defaults (section 1).
2. Open `--log` (append) and `--pktlog` (append).
3. Open the DB, creating the base schema.
4. Register `--test-bot-account` accounts.
5. Create on-demand tables (GM tables, coupon table, recommend table) before any handler transaction.
6. Apply `--grant` to an existing account.
7. Rank clans by exp (clan list order).
8. If `--demo`, build the demo rooms/users.
9. Bind three listeners on `--bind`: auth (`port`), lobby (`lobbyport`), clan lobby (`clanport`). All three speak the
   same session protocol.
10. If `--clanweb` != 0, start the clan web server on `clanweb`, bound to `--bind`.
11. Start the periodic item-expiry push thread (CD-06 cadence; push message 0x33f).
12. Log one line naming the three ports, the advertised lobby ip, the DB path and `gameip:gameport`, plus counts of
    maps, modes and clan modes loaded from the ROM.
13. Run until killed.

Harness timing (H): every `Popen` is followed by `time.sleep(1.0)` before the first connect (lines 546, 671, 758, 815).
All listeners must be accepting within 1 s of process start, with a cold DB file. The listeners in step 9 must be
bound before the log line of step 12 (D). If a bind fails, exit non-zero with a message (D).

## 3. Port assumptions in the harness (H)

`free_port()` binds `127.0.0.1:0`, reads the port, closes (lines 9-11). The suites then guarantee five distinct values
`{port, lport, webport, cport, lport+1}` because the clan default is `lport+1` and consecutive free ports collided
(lines 805-808). `test_clan` runs with NO `--clanweb`, so it also needs `port+808080` free and tries to bind it first (lines
742-751). The Rust binary must not open any other port, and must not bind `0.0.0.0` unless `--bind` says so.

## 4. Packet log (`--pktlog`)

Format as stated in the brief (B, the writer code was not read): one line per packet, in both directions,

```
<time> <kind>#<sid> <dir> <fid> <hex>
```

- `time`: wall-clock timestamp of the line (format not verified; use `YYYY-MM-DD HH:MM:SS.mmm` and treat as OQ-3).
- `kind`: which listener, e.g. `auth`, `lobby`, `clan` (names not verified: OQ-3).
- `sid`: numeric session id, unique per accepted TCP connection, increasing from 1.
- `dir`: `C->S` or `S->C`.
- `fid`: frame id, decimal or `0x` hex as the harness frame header carries it (frame header = two i32, id then length,
  H: `pkt()` lines 13-15; inner header u16 id + u32 length).
- `hex`: the frame payload as lowercase hex without separators.

To be diffable against the Python server's log, timestamps are expected to differ; the diff tool must strip column 1.
Verify the exact column layout against a real Python-produced log before freezing (OQ-3).

## 5. Suites in `main()` (H) and what each needs

`main()` (lines 802-916) runs, in order, with their requirements. The task brief says 16; I count 15 distinct test
functions plus one inline end-to-end scenario in `main` (login, lobby, chat incl. the Korean line, room, team, ready,
start). Counting the inline scenario as a suite and the Korean-chat line separately gives 16 (OQ-1).

| # | suite | needs |
|---|---|---|
| 1 | `test_stale_port` (line 445) | IN-PROCESS import of `reference lobby` (`udp_port_free`, `free_udp_port`, `GS_ALL`); NOT a black box. Rust needs a unit test with the same asserts; the Python harness cannot run this against the Rust binary (section 7) |
| 2 | inline scenario in `main` (lines 815-870) | 4 free ports, temp DB, `--countdown 0.05 --start-nx 5000 --clanweb --clanport --log`, cp1252 env, UTF-8 log containing the Korean chat line verbatim; two accounts; locked room; wrong then right room password (refusal code 0x2a); team/ready/settings; start => 0x3044 then 0x3043 with ip `127.0.0.1` and port 27888; host hand-over, kick (refusal 0x8a for the removed player) |
| 3 | `test_rejoin` (line 707) | running room, leave to room and re-ready without relogin; master Start refused while room plays |
| 4 | `lobby3` (line 118) | shop lists, GP buy/equip/release/discard, NX buy -> inbox -> activate, gift, clans through chat commands and the clan web pages (`/clan?url=guild/list...`, `/clan/join?c=..&p=..`) on `--clanweb`, nation, profile, room invite; DB reads in section 6 |
| 5 | `test_loadout` (196) | backpack slot rules; direct DB inserts with the equipment-slot column at -1; default items per new character |
| 6 | `test_lb5grid_shoplists` (243) | shop list packets 0x358/0x35a/0x35c incl. a second framing with outer id 0x356 |
| 7 | `test_lb5cash` (258) | megaphone, coupons (a seeded coupon code present in a fresh DB; the code string is server data, not specified here), HiSec chat command, tuning update |
| 8 | `test_lobby5` (290) | chat commands `/notice`, `/expire`, `/shopver`, `/grade`; packet 0x368 framed as 0x324 |
| 9 | `test_lb5gm` (313) | GM flag via `UPDATE accounts SET gm=2`; ban/punish/lookup replies; ban refusal code `0x00030089` with days at offset 0x2a; report row |
| 10 | `test_lb5use` (364) | cash-item use 0x330/0x331 for 8 item kinds, inbox supply case (0x32e) |
| 11 | `test_lb5nick` (401) | referral 0x20e/0x20f, `recommends` row, nickname screen flow (needs NO `--autochar`) |
| 12 | `test_persist_restart` (524) | FAKE_DSERVER python script as `--dserver-exe`, `--dserver --dserver-world --dserver-logdir --gameport`, `--start-nx 5000`, `--countdown 0.05`, hard kill and restart on the same DB, second start with `--check-password`, offline gift notice, expiry while down |
| 13 | `test_clan` (736) | default `--clanweb` (port+8080), clan create via HTTP GET, persistence over restart, HTML fragments (see below) |
| 14 | `test_account_grant` (657) | `--autochar`, `--grant*`, three restarts on the same DB |
| 15 | `test_world_for_room` (424) | IN-PROCESS import (`L.GameRoom.maps()`, `L.world_for_room`, `L.Room`); skipped when WORLDS.rez is unavailable; world rule: map `Showdown` + mode `QRT` => world path ending `warehouse_qrt`; map index 0 + mode `TDM` => `Worlds\Snow...`. Needs a Rust unit test over `CA_ROM` tables |
| 16 | (printed) `ALL OK` | exit code 0 |

Clan web fragments the harness matches verbatim (H, lines 767-796), so the Rust pages must emit them:
`color:#fc0">Clan <name> created.`, `Cannot` for a refused second create, `<td>Leader</td><td>Carol</td>`,
`<td>Wolves</td><td>1</td><td>Carol</td>` (clan name, member count, master nick), `[Join]` present only for non-members,
`clan <b>Wolves</b>` for a member with no session, `You joined Wolves.`. Since LG-420 (DC_DEFAULTS) a join is an
application the leader accepts (`Application sent to Wolves.`, link `[Apply]`); the harness's fragments hold with the
lobby override `clan.join_approval` = false. Query parameters the tests send: `p`
(passport/login), `name`, `emblem`, `bg`, `c`, and the legacy `strPassport` (lobby3 uses `strPassport=LOCAL:bob`). The
login used as passport may have the `LOCAL:` prefix or not.

Client login model (H, `login()` lines 63-88): connect to `--port`, send 0x206 (32 zero bytes) expect 0x207 success; send
0x200 with login `LOCAL:<user>`, password `pw`, a MAC string, i32 0; reply 0x201: code 0 with reason `0x0003001e` means
"no character yet"; then 0x204 nickname check, 0x202 create (face id 6002/6004), then the 0x2000 server list (size 0xf16,
count at 0x12 >= 1, lobby port at 0x16+0x88 must equal `--lobbyport`), 0x20a select => 0x20b with a 16-byte session key
at offset 0xe; then connect to the lobby port, 0x300 with key + user number.

## 6. Tables and columns the tests read or write directly (SQLite)

The Rust schema MUST keep these names (H; types by use). Tests open the DB file with their own connection while the
server runs, so the server must not hold an exclusive lock between handlers (journal mode WAL or short transactions;
both fine). 64-bit integers needed (a test stores `1<<60`).

| table | columns the tests touch | notes |
|---|---|---|
| `accounts` | `user_no` (PK), `login`, `gm` | `login` WHERE `login='dave'` (no prefix) is used in main, persist, clan; `account_grant` and `lb5nick` accept `LOCAL:name` OR `name`. => store the login WITHOUT the `LOCAL:` prefix. `gm` int, default 0, tests set 2 |
| `characters` | `user_no`, `nick`, `exp`, `gp`, `nx`, `nx_cr`, `kills`, `deaths`, `wins`, `losses`, `headshots`, `matches`, `nick_color`, `options`, `clan_uid`, the clan-name column, the clan-type column, `face`, `nation` | one row per account after character creation. Clan type 5 = clan master (H line 772). `nx` = NX Prepaid, `nx_cr` = NX Credit. `face` stores the face index derived from the creation id (6004 => 4, 6002 => 2) |
| `items` | `uid` (autoinc PK), `user_no`, `item_no`, `start_time`, `end_time`, the equipment-slot column, `default_type`, `parent_uid`, `use_type`, `remain`, `used` | inserted by tests with only some columns; DEFAULTS must be: equipment slot -1, `start_time 0`, `end_time 0` (0 = permanent), `default_type 0`, `parent_uid 0`, `use_type 0`, `remain 0`, `used 0`. `end_time` is a Windows FILETIME of the local wall clock (H `local_ft`, line 517). `uid` must be returned by `lastrowid`/`MAX(uid)`. Tests delete expectations: an expired equipped row is removed (`COUNT(*)==0`) and the default item returns |
| `inbox` | `user_no`, `product_id`, `created`, `uproduct` (autoinc PK) | one row per purchased-not-yet-activated product |
| `friends` | `user_no`, `friend_no` | row exists only after the target accepted |
| `clans` | `clan_uid`, `name`, `master`, `emblem`, `background` | `master` = creator's `user_no` |
| `match_log` | any; test uses `COUNT(*)` | exactly 1 row per finished match |
| `reports` | `kind`, `target_nick`, `text`, `extra`, `reporter` | filled from the 0x312 report, within 0.3 s |
| `recommends` | `user_no`, `recommender` | recommender stored as the canonical nick of the referred character (case-corrected) |

The schema is our own (D); additional tables and columns are free. Seed data the tests assume to exist after first
start: a coupon code usable once per account (second use refused with reason 0x72; a bad one 0x71) giving product 1748,
the ROM-derived product catalogue (product 9 at 3920 GP for 7 days, product 1475 as an NX gift product), default
items per new character (four default weapons plus a standard backpack permanent: item count 5, one default per
weapon class with `default_type` set), and three channels (0x376 list count 3). These are ROM or DC-DEFAULT data read at
run time (CLEANROOM.md section 4): the Rust lobby derives them from `CA_ROM`; the coupon is a lobby seed whose code
is a config value, not shipped in source (D).

## 7. Subprocess launch sites and the `CA_LOBBY_SERVER_CMD` change

Lobby launch sites in `lobby_harness` (each builds `[sys.executable, HERE/reference lobby, '--port', ...]`):

- line 541, `start()` inside `test_persist_restart`;
- line 668, `start()` inside `test_account_grant`;
- line 755, `start()` inside `test_clan`;
- line 812, the `srv = subprocess.Popen(...)` in `main()` (also passes `env=`).

Line 465 is NOT a lobby launch (a socket-holder child inside `test_stale_port`) and stays unchanged.

Staged change (D; the harness is NOT edited by this task; apply it in a harness-owning worktree):

```python
def lobby_cmd():
    """argv prefix that starts the lobby under test. CA_LOBBY_SERVER_CMD (a shell-style string, e.g.
    'target/i686-pc-windows-msvc/debug/dc-lobby.exe') replaces 'python reference lobby'."""
    c = os.environ.get('CA_LOBBY_SERVER_CMD')
    return shlex.split(c, posix=(os.name != 'nt')) if c else [sys.executable, os.path.join(HERE, 'reference lobby')]
```

and at each of the four sites replace `[sys.executable, os.path.join(HERE, 'reference lobby'), '--port', ...]` by
`lobby_cmd() + ['--port', ...]`. Add `import shlex`. Notes:

- `test_stale_port` and `test_world_for_room` import `reference lobby` in-process and cannot honour the variable; when
  `CA_LOBBY_SERVER_CMD` is set, `main()` should skip them with a printed note and the Rust crate carries equivalent unit
  tests (D).
- The persist suite passes a `.py` file as `--dserver-exe`; the Rust lobby launches `.py` through `CA_PYTHON` or
  `python` (DSERVER_PIPE.md section 2).
- Environment inherited from the harness: `main()` sets `PYTHONIOENCODING` and `PYTHONUTF8`; they are harmless to Rust.
  Also set `CA_ROM` for runs that need ROM tables (shop catalogue, worlds); tests that need it are skipped by the Rust
  side when it is unset (CLEANROOM.md section 4).
- Exit status: the harness never checks the lobby's exit code after `terminate()`/`kill()`; Rust may exit with any code
  on those signals.

## 8. Open questions

- OQ-1: suite count 16 (brief) vs 15 functions + inline scenario counted here; confirm the intended list.
- OQ-2: the grant test hard-codes the Python rank table (8000000 for the top rank, 1679000 for "Major 3rd", 8700 for
  rank 5), which CD-05 declares ours. Rust either reads the same numbers from a shipped `ranks.toml` default (a
  DC-DEFAULT, allowed) or the harness numbers change.
- OQ-3: the `--pktlog` column layout was taken from the brief and not from the writer; verify against a real log.
- OQ-4: `--test-bot-account` bot naming/kit adaptation is described only by the help text; the Rust behaviour is
  design unless a later task authorizes reading that code.
- OQ-5: tests rely on message handlers I could only see from the client side (ids and offsets above). The PROVEN rows
  of PROTOCOL.md are the authority for each layout; this file lists none of them, only the harness's use.
