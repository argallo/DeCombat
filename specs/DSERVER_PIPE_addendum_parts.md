# Addendum: weapon attachments on the `user` line (`parts=`), af-attachments, 2026-10-25

Extends `DSERVER_PIPE.md` §11 / §12.A (the `user` line's trailing tokens). Tags as in §0 of the base spec: D = our
design (DeCombat rule), O = observed. Rows LDO-2 (lobby) and DSV-99 / DSV-100 (server) are staged in
`staging/af-attachments_DC_DEFAULTS.md`.

## 1. The token

```
user <uid> <sid> <key> <team> <exp> <nick> [loadout=...] [parts=<slot>:<muzzle>/<sight>/<magazine>,...] [face=<n>] [mod=1]
```

| item | rule | tag |
|---|---|---|
| `slot` | the equipment slot of the weapon, the same number as in its `loadout=` entry (0..3 weapon slots, 10..13 backpack slots) | D |
| parts | exactly three, in the option-kind order 0x11 / 0x12 / 0x13 (muzzle, sight, magazine) of `PROTOCOL_2009.md` §4.2 0x33a; each the ROM item number of the attached option (`1000 + ItemID`), `-1` = nothing of that kind | D |
| sent when | at least one equipped weapon carries an option of a known kind; one entry per such weapon, ascending slot; the token is absent otherwise | D |
| source | the lobby's item rows: an option row whose parent unique id (`PROTOCOL_2009.md` table 3.3b `+8`) is the weapon's unique id; kind from the ROM item table (`ItemType` of the option row). An option of an unknown kind is left out; two options of one kind on one weapon: the lowest unique id | D |
| switch | travels with the other loadout tokens (`dserver.loadout_tokens`) | D |
| order on the line | `loadout`, `parts`, `face`, `mod` | D |

Backward compatibility: receivers ignore unknown keys (§11), so an older server grants as before (DSV-80 defaults);
a newer server without the token grants the row defaults. A bad entry (no slot, not three parts, a part that is not a
number) is skipped; a negative part is "none".

## 2. Server use (DSV-99, DSV-100)

- The weapon granted in a slot takes the `parts=` entry of the equipment slot whose `loadout=` item is that weapon.
- An option number `1000 + n` names the client's mod `n` (`[Mod<n>]` of the weapon table). Evidence: the 2009 client
  keeps an option's number modulo 1000 (cs+0x25c010) and files it under the parent weapon by kind into three option
  fields (cs+0x287930 -> cs+0x226700: 0x11 / 0x12 / 0x13 -> weapon record +0x30 / +0x34 / +0x38); those fields are the
  `e1..e3` bytes of its `UP_PLAYER_INFO` entries (`GAME_MESSAGES_2009_C2S.md` §5.1). ROM check at run time (test
  `rom_option_items_name_the_mod_of_their_socket`): every option row's `[Mod<ItemID>]` exists and its `Kinds` is the
  socket of the option's kind (0x11 -> 0, 0x12 -> 1, 0x13 -> 2).
- Grant parts (`GS_WEAPON_GIVE` 0xb2 / `GS_WEAPON_EQUIP` 0xb3): start from the row defaults (DSV-80); a chosen mod
  replaces the default of its socket when the mod row exists, its `Kinds` is that socket, the weapon row lists it
  (`ModName<k>`) and the index fits a part byte (DSV-100, D). Part positions: part 1 = sight socket, part 2 = muzzle,
  part 3 = magazine, finish -1 (DSV-80's order; the client files every part by the mod row's `Kinds`, cs+0x60d90 ->
  cs+0x235ec0, so the position carries no meaning for the client).
- Zoom: the 2009 client zooms when the sight socket holds a mod with `ZoomLevel` > 0 (cs+0x176f10 reads `Kinds` and
  `ZoomLevel`; cs+0x230a60 / cs+0x1f4180). The server logs `client <pid>: weapon <w> attachments [...] -> parts [...]
  (zoom <bool>)` at the first spawn of a player whose lobby options reached a grant.
- Switch: `-combat player_parts=on|off` (default on); `-combat weapon_parts=none` still sends -1 x 4 everywhere.
- The client's own `UP_PLAYER_INFO` option bytes are logged (`options <item>:<e1>/<e2>/<e3>`) but not used: the lobby's
  record is authoritative (DC-C2S-5).

## 3. Observations

| capture | what | tag |
|---|---|---|
| `20261025_153705_rust7` conn 5 (the user, our stack) | lobby: option item `1000 + 35` bought, 0x33a with the option in the sight field, DB row parent = the M4A1's unique id; the client's 0x72 entry `(145, ff, 35, ff)`; our 14 grants of weapon 145 carried -1 x 4 (the reported defect); rebuilt with this rule they differ in part 1 only (35) | O |
| `20261025_065016_tdm6`, `20261025_002236_tdm4` conn 6 (the real client against the reference stack) | no attachment: every 0x72 option byte 0xff, every grant -1 x 4 | O (REFERENCE) |
| same captures, conn 5 (the reference stack's bot) | the bot's 0x72 entries carry 0 / 0 / 0 and the reference stack's grants of those weapons carry 0 / 0 / 0 / -1: the reference stack copied the client's option bytes in order into parts 1..3; no sample with distinct values, so the order there tells nothing | O (REFERENCE, server-side choice, not followed) |

## 4. Open

- Real-client check (V-PARTS-1): attach a sight in the lobby, draw the weapon, zoom (scope view and zoom sound).
- Other players' view of the attachments (third-person model) is not part of 0xb2 (sent to the owner only); not traced.
- `dc-config` `[combat]` has no `weapon_parts` / `player_parts` keys yet (command line only).
- dc-client's own zoom does not read the sight socket yet.
