# UDP_SESSION_2009_B addendum: character trackers (upper body, jump, hit reaction)
Scope: which clips a server streams on a player character's trackers 1 and 2; the correction of the "hit reaction" label. Not: the
client's own first-person animation, corpses (3.5.5), co-op AI bodies.
Evidence sources: cshell+0x1980, +0x15220, +0x15250, +0x16040, +0x17bd73 (clean decompiler / own disassembly); captures tdm1, tdm6 (REFERENCE);
DC_DEFAULTS DSV-125 .. DSV-127 (staged).
Status: DRAFT 1 (dserver-anims worker, 2026-10-25; the author also implemented it); second read by spec-fold-9 (2026-10-26) against `reports/dserver-anims.md`, see "Second read" at the end. Grades per item. Applies to `UDP_SESSION_2009_B.md` 3.5.5 and 4.3 (character
entries of the object stream) and to the animation block of 4.4. Clip numbers are combined clip indices of the EU-test default body
(the wire values the reference stack sent); names are never written here, a clip is named by its role.

## A1. Correction: clip 110 is the rifle fire one-shot, not a hit reaction

`UDP_SESSION_2009_B.md` 3.5.5 (row seq 10126) and 4.3 ("A dead character in the stream") call clip 110 on tracker 1 "the hit
reaction". By the ROM body's clip name (read at run time, compared by its key) clip 110 is the **rifle family's still fire one-shot**
(100 ms, `state1` 0): the bot fired its rifle 62 ms before the kill. The reference stack streamed **no** hit reaction clip in tdm1 or tdm6
(REFERENCE: none of the body's two short hit clips appears in any stream entry). Read both rows as "tracker 1 on the rifle fire
one-shot". The 2009 client picks no clip for another character: it plays the streamed trackers and blends on a clip change
(`reports/client-anims.md` 1, PROVEN).

## A2. The 2009 character animation table (DISASM)

The client carries a character animation table object (builder cshell+0x1980, a per-model cache cshell+0x15710; lookups
cshell+0x15220 / +0x15250; the player at cshell+0x16040, used by a preview / NPC animator, not by the in-world remote characters).
Its contents are the 2009 rule for which clip goes with which state:

- **Upper entries** (tracker 1): index `((dir + anitype * 12) * 17 + action)`. `anitype` = the weapon row's `AniType` (read by the
  weapon row reader at cshell+0x17bd73); 44 types, rows 30 / 31 / 32 / 35 hold no player family. `dir`: 0 still, 1 forward, 2 back,
  3 left, 4 right; 5 / 6 (forward diagonals) repeat 1, 7 / 8 (back diagonals) repeat 2, 9-11 repeat 1. `action`: 0 aim, 2 fire,
  1 / 3 the hold and release phases of a throwable (2 is its middle phase), 4 select, 5 reload, 6 / 7 the two hit reactions (the
  same two clips in every family), 9 an alternate (melee second attack; the still aim for guns).
- Each entry: `[clip run-stance, clip walk, clip crouch, clip sprint, -1, loop]`, clips found by name in the model (missing = -1).
  Column choice (cshell+0x16040): walk gait -> column 1, crouch -> 2, sprint -> 3, other gaits -> 0; a column without a clip keeps
  column 0. `loop` is 1 for aim, 0 for every action (play once).
- **Lower entries** (tracker 2): index `(gait * 3 + 0x8c4) * 4 + dir` (a second set for a variant flag): gait 0 walk, 1 run, 2
  crouch, 3 jump (play once), 4 / 5 turn in place (standing / crouched), 6 sprint, 7 sprint jump (play once); dir as above with the
  four diagonals separate (0 still, 1-4, 5-8).
- Every family's clip names follow one scheme: a family prefix and a slot suffix (stance, action, direction class). A server can
  therefore classify the body model's clips at run time (DeCombat: by the key of the lower-case prefix and suffix, `dc-dserver::anims`).
- Fire entries of the knife families use one clip for every direction; the rifle family names moving fire clips the default body
  does not have (falls back, A3).

## A3. What the reference stack streamed (REFERENCE; tdm1 object 408, tdm6 object 140, the real client's own inputs)

| state | tracker 1 | tracker 2 |
|---|---|---|
| standing, any weapon | the family's still aim loop (run column): rifle 96, pistol 247, knife 291, throwable 339 | stand 6 |
| running forward / diagonal forward | run-aim forward: rifle 98, pistol 249, knife 293, throwable 341 | run set 37 / 41 / 40 |
| running back / left / right | run-aim back / left / right (rifle 101 / 107 / 104) | run set |
| walking (no `RUN`) | walk-aim (pistol 250) | walk set 47 .. |
| crouched | crouch aim (pistol 251 moving, 248 still) | crouch 18 / 19 |
| sprinting | the run clips above (the table's sprint column was never used) | run set |
| fire | play-once fire clip by direction where the family has one (pistol 265 moving, 261 still; shotgun moving / still), else the still clip (rifle 110 in every direction; the crouch fire clip 111 is the table's, never observed); one per shot | unchanged |
| reload | play-once: still variant 119, moving variant 115 (rifle), clip length 1866 ms, then back to aim | unchanged |
| weapon change | play-once select: still 120 / moving 117 (rifle), 283 (pistol), 335 (knife), 397 (throwable); the clip chosen at the start is kept while the direction changes | unchanged |
| throw | hold: the throwable's hold phase (still / moving), release: its release phase (355 still) | unchanged |
| jump | unchanged | jump clips, play once (forward 30, right 29); seen only for 2 / 4 of the real client's jumps (the report's wording; which capture holds which count is not stated) |
| dead | as before the death | as before; main tracker on the corpse's death clip (3.5.5) |
| hit | **nothing** | nothing |

A play-once clip is streamed with `state1` 0 and its time since the action's start; after its length the aim loop of the current
motion follows. Replaying the real client's messages of tdm1 / tdm6 through the DeCombat rule (A4) gives the reference stack's tracker-1 clip
in 89 % of the stream entries and the tracker-2 clip in 88-89 % (the rest: input timing, the reference stack's still clips while the control
word moves, ours jump clips, still / moving select variants; `dc-dserver/tests/anims_cap.rs`).

## A4. DeCombat rules (DC-DEFAULT, `DC_DEFAULTS.md` DSV-125 .. DSV-127, staged)

- DSV-125: tracker 1 by `AniType` of the drawn weapon (unknown type or no clips: the rifle family), stance column (run for standing
  and running, walk, crouch; no sprint column, as the reference stack), direction class; fallbacks: run column, then still class. Fire / select
  / reload / throw phases are play-once clips chosen at the action's start, a new action replaces the running one. `-directionalanims
  off` restores the plain walk pair.
- DSV-126: the jump clips on tracker 2 from the client's jump event (`UP_MOVE_EVENT` category 1 event 1) to the next ground report
  (at least 150 ms); the throw hold lasts while `UP_THROW_HOLD` repeats (1 s window).
- DSV-127: hit reaction off by default (the reference stack's behaviour); `-combat hit_clip=on` streams the table's hit reaction (action 6, or
  7 for a shot from behind) once on the victim's tracker 1 when a hit takes health and no other action than a fire one-shot plays.
  Whether the 2009 server streamed one is UNPROVEN (project decision).

## Second read (spec-fold-9, 2026-10-26)

Checked against the dserver-anims report (sections 1-3) and the clean decompiler tree (`CShell.dll/` 15220, 15250, 16040):
- A1: agrees with the report (clip 110 = the rifle still fire one-shot, 100 ms; no hit clip in any reference stack stream; hit actions 6 / 7 exist only in the table). `UDP_SESSION_2009_B.md` 3.5.5 and 4.3 and the rust3 capture record now carry the correction with a pointer here.
- A2: the upper index `((dir + anitype * 12) * 17 + action)` (cshell+0x15220) and the lower index `(dir + (gait * 3 + 0x8c4) * 4)` (cshell+0x15250; a second set at 0x8f4 for the variant flag) match the decompile; the column choice of cshell+0x16040 reads the gait value (gait 1, the run gait, uses column 0; 0 walk, 2 crouch and 6 sprint use columns 1, 2 and 3, as written). Not re-derived in this read: the 4 empty `anitype` rows (30 / 31 / 32 / 35), the direction repeats of the diagonals, the meaning of action 9, the lower gaits 3-5 and 7 (the report states only 44 rows, 17 actions, the column scheme and the loop flag); treat those as DISASM by the worker, one reader.
- A3 / A4: agree with report section 2 and the staged rows DSV-125 .. DSV-127. Two wordings were tightened: the crouch fire clip 111 is DISASM only, and the jump counts are quoted as the report gives them.

## Open

- V-ANIM-1: a capture of the real 2009 client watching a remote character run, fire, reload, get hit and die on the reference stack
  (frames for the comparison; settles DSV-127 only if the 2009 server is ever observed).
- The throwable's middle phase (a 1 ms hold pose) is not streamed by DeCombat (the hold phase is held at its end instead).
- The turn-in-place lower clips (gait 4 / 5) and the sprint column are not used (the reference stack used neither).
