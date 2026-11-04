# SERVER_ROM.md: running the servers without the whole client folder

Branch `server-rom` (2026-10-29). Rules: DSV-370 (what a server ROM holds), DSV-371 (the readers open it), DSV-372
(the client refuses it) in DC_DEFAULTS.md.

`dc-lobby-server` and `dc-dserver` read tables, world scripts, worlds, a few models and the archive key from the
2009 client folder (`CA_ROM`). A host that runs only the servers, for example on a Linux VPS, does not need the rest
of the folder (textures, interface pictures, sounds, fonts, executables). `dc-server-rom` copies the part the
servers read into a folder of plain files, the **server ROM**; point `CA_ROM` (or `paths.rom`) at it and the
servers run as on the full folder.

## Make one

On the machine that has the client folder (the EU-test client of 2009-12-21):

```
dc-server-rom "<client folder>" "<out folder>"
dc-server-rom --verify "<out folder>"
```

(from a checkout: `python cargo_env.py cargo build -p dc-rez --bins`, then `target/debug/dc-server-rom.exe`.)
The out folder must be new, empty, or an earlier server ROM (replaced). The output is a function of the client
folder only: the same client gives the same bytes (checked: two builds, `diff -r` identical). `--only-worlds
GRAYHAMMER,JUNK_FLEA` keeps only those worlds' files (the others stay listed; a room on a left-out world fails at
its start with "entry not in this server ROM").

## Sizes (EU-test client)

| | files | size |
|---|---|---|
| client folder | 198 | 3166 MB |
| server ROM | 2828 | 916.4 MB (874 MiB) |
| of which worlds (`WORLDS.rez`, 21 worlds, 7.7-54.3 MB each) | 21 | 485.5 MB |
| of which body models (`CHARS_M_BODY.rez`, 50 models; the shared child model alone 43.2 MB) | 50 | 149.2 MB |
| of which third-person weapon models (`GUNS_M_HH.rez`) | 229 | 4.5 MB |
| of which first-person weapon models (11 archives `GUNS_M_PV_AR.rez` .. `GUNS_M_PV_Throwing.rez`; for the bot, BOT-82) | 797 | 266.3 MB |
| of which effect list (`CLIENTFX.rez`) | 1 | 5.1 MB |
| of which text tables (`ATTRIBUTES.rez` 3.9 MB, `GMS.rez` 0.3 MB, others) | 199 | 4.2 MB |
| of which wave heads (about 49 bytes each) | 1529 | 0.07 MB |
| shell strings image (`Game/CShell.strings`) | 1 | 0.37 MB |
| manifest (`server-rom.json`) | 1 | 1.1 MB |
| measured minimum (the union of the reads below, without the two executables) | 45 | 195.4 MB |

The worlds are most of it: a host who offers only some maps can cut the server ROM with `--only-worlds`.

## What it contains

- `server-rom.json`: the source build (the SHA-256 of the five sentinel files of the client folder it was made
  from), the archive key (checked against the pinned digest when read), every archive's full entry list (name,
  size, archive order; the SHA-256 and stored length of each entry that is present) and the SHA-256 of the other
  files. Every read checks the file against it; `dc-server-rom --verify` and the lobby doctor (`identify_rom`)
  hash all files.
- `Game/<archive>.rez/<entry path>`: the entries the servers read, as plain files, text tables already decrypted
  (DSV-370): every text table (`.txt`, `.gms`, `.bms`) of every archive, every world (`WORLDS.rez`), every effect
  list (`.fxf`), every body model and third-person weapon model (`.ltb` of `CHARS_M_BODY.rez`, `GUNS_M_HH.rez`; the
  weapon models because a dropped weapon plays its model's animation, checked by a `dc-dserver` test) and every
  first-person weapon model (`.ltb` of `GUNS_M_PV_AR.rez` and the ten other first-person archives: the bot reads its
  weapons' fire cycle from their clips, BOT-82); every wave file only up to its `data` chunk
  header (the game server needs only the play time). Every archive has its folder, empty when nothing of it is
  kept (the game server lists the `Game` folder to search names).
- `Game/CShell.strings`: the strings image of the shell library: its DOS header, stub, signature and file header
  (the build time stamp the lobby reads), then the printable strings that end in a NUL (the game server looks client
  variable names up by hash). No section bytes, no code.

It is still the game's own data, taken from the host's own copy of the 2009 client: the host must own that
client, and the server ROM is not for redistribution (like the client folder itself). It holds no executable
(`Engine.exe`, `CShell.dll`, `CRes.dll`, `ClientFx.fxd` and the other `.dll` / `.exe` files are not copied).

The client (`dc-client`) refuses a server ROM at start-up with "... is a server ROM ...; the client needs the full
2009 client folder" (DSV-372): players keep the full folder. The bot (`dc-botclient`) runs on a server ROM (BOT-82),
so bots can run on the VPS next to the servers (docs/VPS.md "Bots").

## What the bot reads (BOT-82)

Measured with `DC_ROM_TRACE` in the seven bot matches above (`measure/*/rom_trace.tsv`, process `dc-botclient`):
the world files (`WORLDS.rez`: navigation, sight lines, ladders), the text tables of `ATTRIBUTES.rez` (weapons,
ammunition, rooms), the archive key, the first-person weapon models (`GUNS_M_PV_AR.rez` and ten more, about 300 models, 156 MB
per bot start: only the fire and postfire clip lengths, BOT-60) and one data word of `Game/CShell.dll` (the
integrity answer). Nothing visual is drawn, no texture, sound or interface file is opened. The first-person models
were the only reads the server ROM lacked: they are kept now (+266 MB). The library's data word is not (the strings
image keeps no section bytes): on a server ROM the bot answers the integrity ask with 0, which `dc-dserver` does
not check. Check: `dc-botclient` test `rom_a_server_rom_gives_the_same_weapon_rows` (`CA_ROM` = client folder,
`DC_SERVER_ROM` = server ROM: 297 weapon rows, 295 fire cycles, identical) and
the maintainers' bot match harness on a server ROM (TDM on Junk Flea PASS, every read of the lobby,
the game server and the four bots from the server ROM; remote-lobby/match_tdm/).

## Copy it to a VPS

```
tar czf server-rom.tgz -C "<out folder>" .          # on the machine with the client
scp server-rom.tgz user@vps:                         # or any copy that keeps empty folders
ssh user@vps 'mkdir -p ~/decombat/rom && tar xzf server-rom.tgz -C ~/decombat/rom'
ssh user@vps 'CA_ROM=~/decombat/rom ./dc-server-rom --verify ~/decombat/rom'
```

Then start the servers with `CA_ROM=~/decombat/rom` (or `paths.rom` in the lobby config); the lobby passes it to
every `dc-dserver` it starts. The lobby and the game server open `Game/<archive>.rez` folders in place of the
archives. Folder and file names keep the archive's letter case; lookups ignore case, so a case-sensitive file
system is fine.

## Measurement (how the content was chosen)

`DC_ROM_TRACE=<file>` (`dc-rom` `trace.rs`) appends one line per archive entry, archive directory and loose file
the workspace readers open: `process  kind  file  entry  bytes`; several processes may share the file. Runs on the
full client folder, evidence under `server-rom/measure/<run>/` (`rom_trace.tsv`, `harness.txt`,
`bot_match/`):

| run | how (bot match harness, 4 bots) | result | entries read by the servers | bytes |
|---|---|---|---|---|
| tdm | `--bot-mode 0 -roundsecs 120` (Junk Flea) | PASS 144 s | 22 | 62.6 MB |
| snd | `--bot-mode 1 -roundsecs 60` (default map) | PASS 220 s | 21 | 75.2 MB |
| ctf | `--bot-mode 4 --map 16 -roundsecs 240` (Gravedigger) | PASS 128 s | 22 | 71.4 MB |
| ctf_default | `--bot-mode 4 -roundsecs 120` (Gray Hammer) | PASS 144 s | 19 | 70.0 MB |
| qrt | `--bot-mode 7 -roundsecs 50` (Overdose) | PASS 306 s | 24 | 71.1 MB |
| spy | `--bot-mode 5 -roundsecs 120` | PASS 144 s | 23 | 70.2 MB |
| fireteam | `--bot-mode 2 -roundsecs 120` (Sand Hog) | PASS 144 s | 28 | 102.5 MB |
| lobby | the lobby harness suites (the suites stopped early on a starter-count assertion in that one run, cause not confirmed; the lobby's reads are its start-up) | start-up | 6 | 1.0 MB |

The lobby reads its tables once at start-up (colour, room, HiSec, item, level and shop tables) and the room's
`.GMS` at a game start. The game server reads every archive's directory (it looks names up by hash across all
archives: the bomb look, the voices, resource sizes) and, per match, the tables, the world, the room's world
script, the body models of the players, a few wave files (play time) and the effect list.

Union of the reads (lobby and game server, seven matches):

| what | entries | bytes |
|---|---|---|
| `ATTRIBUTES.rez` text tables (attachments, colour, FX, room, HiSec, items, levels, marks, model attributes, preload, random table, shop, surface, teams, weapons) | 15 | 3.18 MB |
| `GMS.rez` world scripts | 9 | 0.03 MB |
| `WORLDS.rez` worlds (Gravedigger, Gray Hammer, Junk Flea, Overdose, Sand Hog CP1) | 5 | 141.6 MB |
| `CHARS_M_BODY.rez` body models (5 bodies, 1 vest) and the shared child model (43.2 MB) | 7 | 44.9 MB |
| `CHARS_SND.rez`, `SND.rez` wave files (pain, fall, infected / host voices, gate doors) | 8 | 0.76 MB |
| `CLIENTFX.rez` effect list | 1 | 5.14 MB |
| `Engine.exe` (archive key) | file | 3.05 MB |
| `Game/CShell.dll` (client variable names by hash, header time stamp) | file | 4.86 MB |
| **total** | | **203.3 MB** |

Every measured read falls under a DSV-370 rule; the rules take whole kinds (all worlds, all tables, all body
models, all wave heads) so a map or mode the measurement did not play still works.

## Check on the server ROM

Evidence under `server-rom/`:

- `dc-rez` test `server_rom`: every kept entry reads the same bytes as from the client folder (a wave head is a
  prefix), the entry lists match in order and size, a left-out entry does not read, key and build identity match,
  a changed file is refused.
- the `dc-lobby-server` (265 unit + 40 integration) and `dc-dserver` (232 unit + 179 integration) tests with
  `CA_ROM` = the server ROM: all pass (one test needed the third-person weapon models, now kept).
- `match_tdm/` (TDM, Junk Flea, with a headless viewer: result board in `bot_match/viewer/`) and `match_fireteam/`
  (Fireteam, Sand Hog): bot matches with the lobby and the game server on the server ROM and the bots and the viewer
  on the full client folder (the bot match harness), both PASS; their
  `rom_trace.tsv` shows no read of the client folder by either server.
- `client_refusal.txt`: `dc-client` and `dc-botclient` on the server ROM stop with the DSV-372 message, exit 2
  (the bot's refusal was lifted by BOT-82 on 2026-10-29).
