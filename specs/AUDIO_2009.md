# AUDIO_2009: audible sound behaviour of the 2009 client, as a reimplementation target
Scope: what the 2009 EU-test client does with sound, so our own mixer reproduces it: output device setup, the
sound-instance model (local / ambient / positional), volume pipeline, 3D distance law, listener, reverb rooms,
background music (BGM) streaming, the sound attribute table, the WAV/MP3 formats present in the ROM, and the user
settings the client exposes. Out of scope: the internals of the third-party mixer DLL the client links (we do not
reimplement its API; we reproduce the audible result), Bink movie audio, voice chat, server-side sound selection.
Evidence sources: decompiler output of the 2009 binaries (`Engine.exe/*.c`, `.../CShell.dll/*.c`),
`Engine.exe.funcs.orig.tsv`, read-only capstone disassembly and PE import/constant reads of the ROM `Engine.exe`
(helpers in `staging/audio_2009/`), read-only ROM scans with `evidence/rezls.py` and
`evidence/romdata.py:entry` (counts only), PUBLIC knowledge of the RAD Miles Sound System 7 API semantics
and of the public EAX 2.0 property set, RIFF/WAVE and MPEG-1/2 Layer III header layouts.
Status: DRAFT

- Modules: `engine` = 2009 EU-test `Engine.exe` (sha256 `bb9a1e41b430a09ab8722e1de31f89dc53a00e5a96abbe236f65c4ceddcf574f`,
  image base 0x00460000); `cshell` = `Game/CShell.dll` (sha256 `2de3e2f4e1a762f0a484f427560acac62742341ff7ccf8d6b1e95cae73cb3b50`,
  image base 0x37100000). Every citation is `engine+0xRVA` / `cshell+0xRVA` (RVA = VA - image base).
- The mixer DLL shipped in the ROM root reports file version 7.0s in its version resource
  (sha256 `5cafd1f6910e6eddaacf6b33cc42ea93e2835e61dc9cb91c6f8aca79f6a02e9b`). Its exported entry points are named
  below by their public import names because those names are the interop surface; nothing of the DLL is reused.
- ROM data policy: no ROM strings, paths, file names or values are reproduced. Strings that the client looks up
  (attribute keys, profile keys, console variables, property names) are given as `sha256(utf8)[:16]` plus length;
  an implementation recovers the exact text from the user's ROM at run time by matching the hash. Only counts and
  sizes of ROM content are given. All identifiers in this spec (`AudioDevice`, `Voice`, `VoiceKind`, ...) are ours.
- Tag legend: `engine+0x..` / `cshell+0x..` = read from that function; PUBLIC = public API/format knowledge
  (reference named); UNPROVEN = not established from the binaries; COUNT = ROM scan result.

## Contents
1. Mixer entry points the engine imports
2. Output device setup
3. Voice (sound instance) model
4. Volume pipeline and sound classes
5. 3D positioning, distance law, listener
6. Occlusion, obstruction, ambient rules
7. Reverb rooms and sound filters
8. Background music (BGM)
9. The sound attribute table (per-sound records)
10. Audio assets in the ROM (WAV and MP3 histograms)
11. User-configurable settings (options screen, profile, console)
12. Reimplementation contract for our audio crate
13. Open questions
14. Server-started sounds and footsteps (spec-fold-8)

## 1. Mixer entry points the engine imports

`Engine.exe` imports exactly 36 functions from the mixer DLL (PE import table, IAT engine+0x2127f8..engine+0x212884;
COUNT by `staging/audio_2009/imports.py`). `CShell.dll` and `ClientFx.fxd` import none of them (CShell's only
multimedia import is one timer function) and reach sound only through the engine's interfaces. Their import
strings are not reproduced here: table 1.1 gives each one's IAT slot and the sha256[:16] and length of the exact
(decorated) import-table string, so an implementation can match them in the user's binary; the semantics are those
of the public MSS 7 API reference for the matching entry. Call sites were
found by scanning `.text` for indirect calls through each IAT slot (`staging/audio_2009/xref.py`; engine+0xec180..
engine+0xec2d0 is a block of jump thunks, one per import, not listed as a user).

| group | import (our name; exact import string by hash in 1.1) | engine call sites (containing function) | what the engine uses it for |
|---|---|---|---|
| lifetime | `mixer_set_provider_dir` | 0xdcda0 | provider folder = a 3-char relative directory (sha `5f330c671741ab40`, len 3) that matches the ROM's provider subfolder |
| lifetime | `mixer_startup`, `mixer_shutdown` | 0xdcda0; 0xdb9d0, 0xdc540 | start / stop |
| lifetime | `mixer_set_preference` | 0xdcda0 | one call, preference 1 = 0x100 (section 2) |
| device | `driver_open`, `driver_close` | 0xdcda0, 0xdb260; 0xdb1e0, 0xdb2b0, 0xdb9d0, 0xdc540 | two output drivers (section 2) |
| device | `driver_set_master`, `driver_get_master` | 0xd90a0, 0xda1b0, 0xdb130, 0xdb260, 0xdbc70, 0xdcda0; 0xd90f0 | master gain of each driver |
| voices | `voice_alloc`, `voice_release` | 0xdcda0; 0xdb9d0, 0xdc540 | voice pool, allocated once at start |
| voices | `voice_bind_file` | 0xa6610 | bind an in-memory file image (WAV or MP3) to a voice |
| voices | `voice_set_loops` | 0xa6610 | 0 (= loop forever) for looping sounds |
| voices | `voice_start`, `voice_stop`, `voice_resume`, `voice_status` | 0xdc190; 0xa6270, 0xa62d0, 0xa68a0, 0xdcda0; 0xa6320; 0xa62d0, 0xa6320, 0xdbc70 | start, stop, pause by stop + resume |
| voices | `voice_set_gain_pan`, `voice_set_levels` | 0xd9b90, 0xda1d0, 0xdb750, 0xdc190; 0xa63a0 | two independent gain stages (section 4) |
| voices | `voice_seek_ms`, `voice_tell_ms` | 0xda450; 0xda330 | seek / query by milliseconds |
| voices | `voice_set_reverb_send` | 0xdc190 | dry 1.0 / wet 1.0 when reverb is active (section 7) |
| 3D | `voice_set_pos3d`, `voice_set_dist3d` | 0xa6aa0, 0xa6b20, 0xdbc70; 0xa6aa0, 0xa6b20 | voice position, min/max distance |
| 3D | `listener_set_pos`, `listener_set_orient` | 0xdbc70 | listener from the camera |
| 3D | `driver_set_rolloff` | 0xd8e30, 0xdcda0 | rolloff factor, 0.0 by default (section 5) |
| rooms | `driver_set_room`, `driver_get_room` | 0xd9e10; 0xd9e10, 0xdcda0 | preset reverb room; `driver_get_room() != -1` = reverb available |
| rooms | `driver_set_reverb`, `driver_set_reverb_levels`, `driver_filter_prop` | 0xd9e10 | custom reverb parameters (section 7) |
| music | `stream_open`, `stream_close`, `stream_start`, `stream_set_loops`, `stream_voice` | 0xda060; 0xd8d70, 0xd8da0, 0xda000, 0xdabc0, 0xdb100, 0xdb9d0, 0xdc540, 0xdcda0; 0xda060 | file streaming for BGM (section 8) |

Consequences of what is NOT imported (absence in the import table is PROVEN; the audible meaning is from the
public MSS 7 API set): no per-voice playback-rate call, so pitch is never changed (section 3.6); no listener or
voice velocity call, so there is no doppler (section 5.4); no occlusion, obstruction, low-pass or cone call
(section 6); no speaker-configuration or 3D-provider call (the MSS 7 software 3D path of the digital driver is
used); no MIDI/XMIDI, no CD audio, no voice/codec entry point.

### 1.1 Import slots
| IAT rva | our name | import name sha (len) | arg bytes |
|---|---|---|---|
| engine+0x2127f8 | `driver_set_room` | `8676d252a91cc085` (20) | 8 |
| engine+0x2127fc | `driver_get_room` | `04f7e15ef645578a` (16) | 4 |
| engine+0x212800 | `voice_start` | `02270ccee781b57c` (19) | 4 |
| engine+0x212804 | `stream_start` | `beac25d8c89d16b1` (19) | 4 |
| engine+0x212808 | `stream_set_loops` | `feb44ee662671605` (28) | 8 |
| engine+0x21280c | `stream_open` | `a702f0ba90ebd198` (19) | 12 |
| engine+0x212810 | `voice_tell_ms` | `2f2490299bc66a6c` (26) | 12 |
| engine+0x212814 | `voice_seek_ms` | `25291bb37ba5353a` (29) | 8 |
| engine+0x212818 | `driver_close` | `706c5cfd69230ee2` (27) | 4 |
| engine+0x21281c | `driver_open` | `7898e6321c9e1b88` (27) | 16 |
| engine+0x212820 | `mixer_shutdown` | `3e6381a7b77a38d7` (15) | 0 |
| engine+0x212824 | `voice_release` | `2feb906122dbea59` (28) | 4 |
| engine+0x212828 | `listener_set_orient` | `9d662eacba4443c6` (35) | 28 |
| engine+0x21282c | `listener_set_pos` | `a5fdfa278d5ee90c` (32) | 16 |
| engine+0x212830 | `voice_set_reverb_send` | `f76a3d64e85a54d7` (32) | 12 |
| engine+0x212834 | `driver_set_reverb` | `074688e6aed3a829` (33) | 16 |
| engine+0x212838 | `driver_filter_prop` | `21ebff4e082fea63` (37) | 20 |
| engine+0x21283c | `voice_set_gain_pan` | `979a1e510ed9d277` (29) | 12 |
| engine+0x212840 | `driver_get_master` | `27fd828d251c627a` (34) | 4 |
| engine+0x212844 | `driver_set_master` | `4abb6411ffb05ed3` (38) | 8 |
| engine+0x212848 | `driver_set_rolloff` | `4648af56825a5435` (28) | 8 |
| engine+0x21284c | `stream_close` | `17e44bca9ae0be16` (19) | 4 |
| engine+0x212850 | `voice_set_pos3d` | `6bb9172095f83de4` (30) | 16 |
| engine+0x212854 | `voice_set_dist3d` | `ff3547c31330e102` (31) | 16 |
| engine+0x212858 | `voice_bind_file` | `c575215c74d0b678` (23) | 12 |
| engine+0x21285c | `voice_set_loops` | `67fda84f9482a43d` (28) | 8 |
| engine+0x212860 | `voice_set_levels` | `4260ca3f12f54d56` (32) | 12 |
| engine+0x212864 | `voice_resume` | `ae67e0ce47ac8ed1` (20) | 4 |
| engine+0x212868 | `voice_status` | `12db1f91af4ec87d` (20) | 4 |
| engine+0x21286c | `voice_stop` | `fca621f0f4125b90` (18) | 4 |
| engine+0x212870 | `mixer_set_provider_dir` | `e12ef5c7339c9f0b` (27) | 4 |
| engine+0x212874 | `mixer_startup` | `d01ebcd57ace64f5` (14) | 0 |
| engine+0x212878 | `mixer_set_preference` | `803f5b02e71262c6` (21) | 8 |
| engine+0x21287c | `voice_alloc` | `60b0e682284b2990` (29) | 4 |
| engine+0x212880 | `driver_set_reverb_levels` | `5a2f50c8bf95277f` (40) | 12 |
| engine+0x212884 | `stream_voice` | `6e8de4c75d6d87cd` (27) | 4 |

## 2. Output device setup

Sound manager construction (engine+0xdcda0; one global instance, pointer at VA 0x6e3bd4, getter engine+0xd8e20):

1. Provider folder set, mixer started, preference 1 set to 0x100 (engine+0xdcda0). PUBLIC (MSS 7 headers): preference
   index 1 is the mixer channel count; 0x100 = 256 mixing channels. The value is PROVEN, the meaning PUBLIC.
2. Two output drivers are opened with identical arguments `(44100 Hz, 16 bits, channel spec 0x10, flags 0)`
   (engine+0xdcda0; the music player re-opens its own with the same arguments at engine+0xdb260):
   - `music_driver`: owned by the embedded music player (manager offset +0x40c, driver at +0x424); master gain set
     to 1.0 on open.
   - `sfx_driver`: global at VA 0x6e3bd0; all sound effects use it.
   PUBLIC (MSS 7): channel spec 0x10 = "use the operating system's speaker configuration". So the device runs at
   44.1 kHz, 16-bit, with the OS speaker layout; the engine never forces stereo or a speaker count.
3. `reverb_available = (driver_get_room(sfx_driver) != -1)` (engine+0xdcda0, stored at manager +0x57c). When available the
   reverb parameters are reset to defaults and applied once (engine+0xd9ca0 then engine+0xd9e10; section 7).
4. Rolloff factor of `sfx_driver` set from the manager field +0xb98, which the constructor zeroes, so the initial
   rolloff is 0.0 (engine+0xdcda0). See section 5.2.
5. Voice pool: up to 255 sample handles are allocated on `sfx_driver` and pushed on a free list; allocation stops at
   the first failure and the reached count is kept (engine+0xdcda0, loop bound 0xff, count at manager +0x578).

Initialisation from the client (engine+0xdc760, called with a settings block from the client shell):
- sets `initialised` (+0x576) and `enabled` (+0x577) to 1; logs a requested software voice count clamped to 255
  (the pool size of step 5 is not changed by it; UNPROVEN whether anything else reads it);
- records an output sample rate and bit depth from the block for a format log line only (format tag 1, 2 channels);
  the drivers keep the 44.1 kHz / 16-bit setup of step 2;
- a "convert 16 to 8 bit" flag (block byte +0x110 bit 0, manager +0xaa4) is stored; where it is honoured is UNPROVEN;
- a velocity scale (block float at +0x11c; used only for the unsent listener velocity, section 5.4);
- the initial SFX volume (block u16 at +0x118, clamped to 100) is applied through the SFX master (section 4.3).

Run-time gates: every play request returns early unless `initialised` and `enabled` (engine+0xdc190, engine+0xdc3a0).
`enabled` follows a console flag each frame: on change the manager pauses (manager vtable +0x68, engine+0xd9190: every
active voice gets its pause method) or resumes (+0x6c, engine+0xd91d0) (engine+0x17460).

Window minimise (engine+0xdbc70, each update): while the main window is iconic the SFX master is set to 0 and the
music driver master to 0; on restore the stored SFX volume and music volume are re-applied (engine+0xd90a0,
engine+0xdb130). Our client must do the same (mute while minimised, restore exactly).

## 3. Voice (sound instance) model

### 3.1 Play request
Both the client shell and server-driven object sounds reach the engine as a play request block (engine+0xdc190
reads it; client-local entry engine+0xdc3a0). Offsets are into that block; names are ours.

| off | type | our name | meaning | evidence |
|---|---|---|---|---|
| 0x000 | u32 | `flags` | table 3.2 | engine+0xdc190 |
| 0x004 | char[0x104] | `file` | resource path of the sound file | cshell+0x1b1e20 copies up to 0x104 bytes; engine+0xdc3a0 resolves it through the resource manager |
| 0x110 | u32 | `handle_slot` | in: object id (u16) of the emitting object; out: the voice pointer when a handle is requested | engine+0xa6610 |
| 0x114 | u8 | `priority` | stored on the voice (+0x20, and +0x24 = priority + 1.0) | engine+0xa6610 |
| 0x118 | f32 | `outer_radius` | distance where the sound is gone | engine+0xa6610, engine+0xa6aa0 |
| 0x11c | f32 | `inner_radius` | distance up to which the sound is at full volume | same |
| 0x120 | u8 | `volume` | 0..100, used when flag `VOLUME` is set; voice default 100 | engine+0xa6610, engine+0xa6510 |
| 0x124 | f32 | `pitch` | used when flag `PITCH` is set; voice default 1.0 | engine+0xa6610 |
| 0x128 | f32[3] | `position` | world position (ambient and positional voices) | engine+0xa6aa0, engine+0xa6b20 |
| 0x138 | u8 | `class` | sound class 0..15 (section 4.2) | engine+0xa6610 (voice +0x78), engine+0xa63a0 |

### 3.2 Request flags (our names; bit values PROVEN)
| bit | our name | effect | evidence |
|---|---|---|---|
| 0x0001 | `AMBIENT` | ambient voice kind | engine+0xdc190 (kind selection) |
| 0x0002 | `POSITIONAL` | positional (3D) voice kind; ignored if `AMBIENT` is set | engine+0xdc190 |
| 0x0004 | `LOOP` | loop forever (mixer loop count 0); a start offset beyond the length wraps | engine+0xa6610 |
| 0x0008 | (cleared by client-local play) | UNPROVEN | engine+0xdc3a0 |
| 0x0010 | `WANT_HANDLE` | caller keeps a handle; a request with 0x20 or 0x400 but without 0x10 is rejected; the attribute table's loop key sets 0x14 (loop + handle) | engine+0xdc190 (rejects 0x420 without 0x10), engine+0xdbc70 |
| 0x0020 | (needs handle) | UNPROVEN meaning | engine+0xdc190 |
| 0x0040 | `VOLUME` | use request `volume` | engine+0xa6610 |
| 0x0080 | `PITCH` | use request `pitch` | engine+0xa6610 |
| 0x0100 | `REVERB` | set by the attribute table's reverb key (section 9); engine use UNPROVEN | cshell+0x2a1140 |
| 0x0200 | `CLIENT_ONLY` | voice is not tied to a server object id; forced by client-local play | engine+0xdc3a0, engine+0xa6610 |
| 0x0400 | (needs handle) | UNPROVEN meaning | engine+0xdc190 |
| 0x0800 | `LOCAL_IF_OWN` | play as a local (2D) voice when the manager's "listener is the local player" flag (+0xa80) is set | engine+0xdc190, engine+0xdaee0 |
| 0x1000 | `ONCE_PER_BUFFER` | on release, the sound buffer is unloaded when no other voice uses it | engine+0xa68a0 |
| 0x2000 | `NO_REVERB` | clears the voice's reverb-send flag (voice +0x88, default 1) | engine+0xdc190, engine+0xa6510 |

### 3.3 Voice kinds
`kind` is stored at voice +0x0c (constructors engine+0xa6510, engine+0xa6bc0, engine+0xa6c90).

| kind | value | chosen when | start-up behaviour | evidence |
|---|---|---|---|---|
| `Local` | 0 | neither `AMBIENT` nor `POSITIONAL`, or `LOCAL_IF_OWN` applies | plain 2D voice at centre pan; nominal radii inner 5.0, outer 10.0 (constants at VA 0x67846c / 0x67cf04), no mixer 3D call, no distance law | engine+0xa69e0 |
| `Ambient` | 1 | `AMBIENT` | 3D position set once from the request; mixer distances `max = outer + 0.01`, `min = inner`, auto wet attenuation on; engine distance law each frame (5.3) | engine+0xa6aa0 |
| `Positional` | 2 | `POSITIONAL` | as `Ambient`, and the 3D position is re-sent every update (5.1) | engine+0xa6b20, engine+0xdbc70 |

### 3.4 Start sequence (engine+0xdc190)
1. Gates of section 2. Resolve the sound buffer (load or reuse; engine+0xdabf0). Reject if unavailable.
2. Take a voice object of the chosen kind from its pool; initialise it from the request (kind init, which calls the
   common init engine+0xa6610).
3. Active list capacity is 255 (manager +0xa20 must be < 0xff); otherwise the voice is released and the request fails.
4. If a mixer handle was obtained: volume/pan to (1.0, 0.5), then for `Ambient`/`Positional` the distance law is
   applied once (engine+0xdb750), then `voice_start`; if reverb is active and the voice's reverb flag is set,
   reverb levels (1.0 dry, 1.0 wet).

### 3.5 Voice limit, priority, stealing
- 255 mixer handles at most (section 2, step 5) and 255 active voices (3.4 step 3).
- When the free handle list is empty the voice is still created and tracked, but it has no mixer handle and is
  silent; the engine logs an error (engine+0xa6610, engine+0xdcd40). No priority-based stealing was found in the
  engine sound manager: `priority` is stored but no reader was located. UNPROVEN whether the client shell culls by
  priority before requesting.

### 3.6 Pitch
The common init computes a per-voice pitch: `pitch = request_pitch_or_1.0 * (1 + U(-v, +v))` with `v` from the
sound buffer (+0x2c) and `U` from `rand()/32767` (engine+0xa6610; a result <= 0 becomes 1.0). The value is stored
(voice +0x40) but the engine imports no playback-rate entry point (section 1), so pitch changes are never heard.
Reimplementation: ignore pitch (play at the file's own rate) to match the original. A mod switch may enable it.

### 3.7 Start offset, looping, completion
- Start offset: the request may carry a start time (voice +0x34); if it exceeds the buffer length (a value converted from the buffer; unit assumed ms, UNPROVEN) a non-looping
  voice is marked finished immediately, a looping voice wraps modulo the length (engine+0xa6610).
- Looping: mixer loop count 0 (engine+0xa6610).
- Completion (engine+0xdbc70, every update): a voice whose mixer status is no longer "playing" (PUBLIC MSS 7 status
  4 = playing) is either kept (handle requested and not client-only; the owning object is notified) or released.
- Pause / resume per voice: pause stops the mixer sample if playing and counts nested pauses; resume restarts it
  when the count reaches zero and the status is "stopped" (PUBLIC status 8) (engine+0xa62d0, engine+0xa6320).
- Seek by milliseconds and position query exist only for client-only voices (engine+0xda450, engine+0xda330).

### 3.8 In-memory vs streamed
All sound effects are in memory: the whole file image is loaded into a sound buffer and bound with
`voice_bind_file` (engine+0xa6610, pointer from buffer +0x44 or from its parent buffer). Only music is streamed
from disk (section 8). The mixer decodes WAV directly and MP3 through its provider in the provider folder (PUBLIC).

## 4. Volume pipeline and sound classes

### 4.1 Gain stages of one effect voice
The mixer multiplies independent stages (PUBLIC, MSS 7: volume/pan, volume levels and driver master are separate
linear factors). The engine drives them as follows.

| stage | set by | value | evidence |
|---|---|---|---|
| A: voice volume/pan | start: (1.0, pan 0.5); `Ambient`/`Positional`: distance law each update | `A = law(d) * voice_volume / 100` (5.3); pan always 0.5 | engine+0xdc190, engine+0xdb750 |
| B: voice volume levels (L = R) | voice "set volume" method | `B = floor(floor(v * class_gain * m) * 127 / 100) * 0.01` | engine+0xa63a0 |
| C: SFX driver master | SFX volume setting | `C = fx_level / 100` | engine+0xd90a0 |

In stage B, `v` is the voice volume (0..100; the "get volume" method returns the voice byte +0x28, engine+0xa6380),
`class_gain` is the class volume (4.2), and `m = fx_level / 100` when the class is flagged "master-scaled", else 1.0
(engine+0xa63a0 with engine+0xd9260, engine+0xd92a0, engine+0xd9130). Both floors are truncations (float-to-int
conversion helper engine+0x1de5e4), the 127/100 step is integer arithmetic. Consequences an implementation must
reproduce:
- At v = 100, class gain 1.0, m = 1.0 the level is 1.27, i.e. above unity; whether the mixer clamps that is
  UNPROVEN. Our mixer: apply 1.27 and clip at the output stage, then compare against captures of the original.
- For master-scaled classes the SFX setting acts twice (B and C): effective gain proportional to `fx_level^2`.
  The speech class is master-scaled only through C.
- Volume quantisation: the integer step means 1 % resolution on the product.

When stage B is first applied to a new voice is UNPROVEN (no direct call from the start sequence was found; the
manager's "re-apply volumes" method engine+0xd92d0 calls get/set volume on every active voice, and the client shell
calls it after changing class gains, cshell+0x1e95a0). Until a capture says otherwise, our mixer computes B at
voice start as well.

### 4.2 Sound classes
The manager keeps 16 classes (index 0..15): `gain` f32 at +0xb48 + 4*i (default 1.0) and `master_scaled` u8 at
+0xb88 + i (default 1) (engine+0xdcda0). Setter (manager vtable +0x70, engine+0xd9210): rejects index > 15 or gain
outside [0, 1]. Getters engine+0xd9260 (gain, 1.0 for bad index) and engine+0xd92a0 (flag, 1 for bad index).
The client shell assigns only three classes, then re-applies all voices (cshell+0x1e95a0):

| class | our name | gain source (profile key, section 11) | master-scaled |
|---|---|---|---|
| 0 | `Default` | default multiplier (sha `fb1627408915be99`, len 22), profile default 0.75 (cshell+0x1ecef0) | yes |
| 1 | `Weapons` | weapons multiplier (sha `15a7676adee0abd1`, len 22), default 1.0 | yes |
| 2 | `Speech` | speech multiplier (sha `e9c37bf66ba9e9b3`, len 21), default 1.0 | no |

The class of a voice comes from request byte 0x138. Which sounds carry class 1 or 2 is decided by the caller
(client shell / server object); the mapping is UNPROVEN here.

### 4.3 Masters
- SFX volume: manager vtable +0x34 (engine+0xd90a0) clamps the u16 to <= 100, stores it (+0xabc, default 100) and sets
  the SFX driver master to `value * 0.01`. Getter +0x30 (engine+0xd90f0) reads the driver master back as `* 100`.
- Music volume: engine+0xdb130 clamps the stored copy to 0..100 (+0xabe, default 100) and sets the music driver master
  to `value * 0.01` using the unclamped argument. Exposed to the client as engine+0x106b0; getter engine+0xd9bd0.
- Minimise mute of section 2 sets both drivers to 0 and restores them.

## 5. 3D positioning, distance law, listener

### 5.1 Listener
- Manager byte +0xa80 (`listener_follows_camera`, default 1, engine+0xdcda0) is set through the engine's listener
  method (engine+0xdaee0, client interface table entry at VA 0x6826f8), which can also store an explicit listener
  position (+0xa38) and "ears" position (+0xa44).
- Each update with the flag set (engine+0xdbc70): the listener position is the active camera's position (camera
  object reached through VA 0x6dd228 +0xa44 then +0x78; position at +0xc8/+0xcc/+0xd0) and its rotation (+0xd4) is
  turned into right, up and forward vectors by engine+0x5f9a0. Without a camera: position (0,0,0), forward (0,0,1),
  up (0,1,0), right (1,0,0).
- Sent to the mixer as position and orientation `(face = forward, up = up)` (engine+0xdbc70). PUBLIC (MSS 7): the
  mixer's 3D space is left-handed with +Y up, which matches the engine's identity orientation (forward +Z,
  right +X); UNPROVEN beyond that match.
- Rate: the listener is pushed to the mixer at most every 100 ms (`next = now + 100` with the multimedia timer,
  engine+0xdbc70). `Positional` voice positions are pushed on every update call.
- The distance law (5.3) measures from manager +0xa44..+0xa4c. The update copies the current listener position
  there (it doubles as the previous position for the velocity of 5.4) before the voice loop, so in camera mode the
  law uses the current listener position, unthrottled; the explicit listener method can set it separately.

### 5.2 Mixer-side distances and rolloff
- `Ambient`/`Positional`: `voice_set_dist3d(max = outer + 0.01, min = inner, auto_wet = 1)`
  (engine+0xa6aa0, engine+0xa6b20; constant 0.01 at VA 0x675b54).
- Rolloff factor: 0.0 from construction (section 2, step 4). PUBLIC (MSS 7): the rolloff factor scales the mixer's
  own distance attenuation, so 0.0 disables it; the mixer then only spatialises (pans) and applies its distance-based
  wet attenuation. Changeable by a console command (sha `0b86e6732726b013`, len 15) that the engine registers if
  missing (engine+0x16f00, handler engine+0xd8e60 -> engine+0xd8e30) and by an engine export (engine+0x10640).
  Whether the client shell or any config issues it is UNPROVEN.
- Mixer behaviour beyond `max` distance (muted or held) is mixer-internal: UNPROVEN for this build; PUBLIC MSS 7
  documentation describes the maximum distance as where the sample stops being audible. Our mixer: silent beyond
  `outer + 0.01`.

### 5.3 Engine distance law (engine+0xdb750; radii squared at init, engine+0xa6610 tail with pow(x, 2.0))
```
r_in2  = inner^2          (voice +0x7c)
r_out2 = outer^2          (voice +0x80)
span2  = (outer - inner)^2 (voice +0x84)   -- note: not outer^2 - inner^2
d2     = |listener(+0xa44) - voice_position|^2
if r_in2 < d2 < r_out2:                    -- strict on both ends
    t = (d2 - r_in2) / span2
    A = (1 - t) * voice_volume / 100       -- stage A of 4.1, pan 0.5
else:
    A is left unchanged                     -- keeps the last value (1.0 at start)
```
Evaluated once at start (after A = 1.0) and on every update for `Ambient`/`Positional` voices whose mixer status
is "playing" (engine+0xdbc70). Consequences to reproduce:
- With inner = 0 the law is `1 - (d/outer)^2` (quadratic, not inverse-distance).
- With inner > 0, A reaches 0 at `d = sqrt(inner^2 + (outer - inner)^2)` which is inside `outer`, then goes negative
  until `outer`; the mixer's handling of a negative volume is UNPROVEN (assume clamp to 0).
- A voice that starts closer than `inner` or farther than `outer` plays at stage A = 1.0, not at its volume
  byte, until it enters the open interval. A voice that leaves the interval keeps the last A. Beyond `outer` the
  mixer distance cut-off (5.2) is what silences it.
- `Local` voices never use this law.

### 5.4 Doppler
The update computes a listener velocity (position delta * velocity scale * 1000 / elapsed ms, manager +0xa50..)
and `Positional` voices keep a velocity field, but no velocity is ever passed to the mixer (no velocity import,
section 1). Doppler is off. Our mixer: no doppler.

## 6. Occlusion, obstruction, ambient rules

- The manager exposes set/get methods for per-voice occlusion and obstruction and for voice position (they validate
  the voice, then dispatch to voice methods; engine+0xd9510, engine+0xd95d0, engine+0xd96a0, engine+0xd9760,
  engine+0xd9830, engine+0xd9900). The voice methods for occlusion/obstruction are stubs: `Local` and `Ambient`
  return "unsupported" (1), `Positional` returns success and stores nothing (vtables at VA 0x67de70 / 0x67df00 /
  0x67df48, slots 7..10 = engine+0xa6580..0xa65b0 and engine+0xa64b0..0xa64e0). No mixer call is made. There is no
  ray-cast or geometry-based occlusion in the sound manager. Our mixer: none, by default.
- `Ambient` voices: position is set once at start; they are not re-positioned by the update (only `Positional` are).
  Their distance law still runs each update against the moving listener. A helper presents an ambient or local
  voice as "listener forward * distance" (engine+0xa6c00, engine+0xa6a40); its caller is UNPROVEN.
- Moving an emitter: the position setter of `Positional` voices stores the new point (and optionally resets the
  stored velocity) (engine+0xa6d00); the next update sends it.
- Engine-side culling: none found besides the 255 limits (section 3.5) and the mixer's max-distance cut-off.
- Sound "play chance", random path choice and min/max delay (section 9) are applied by the caller that issues the
  request, not by the engine manager (they are fields of the client shell's record; the engine block of 3.1 has no
  such fields).

## 7. Reverb rooms and sound filters

### 7.1 Engine side
Two modes, chosen by the "select filter" method (manager vtable +0x54, engine+0xdafa0, args `(name, custom)`):
- preset mode (`custom = 0`): `name` is matched against a table of 26 environment names (pointer table VA 0x6d3020,
  loop bound 0x1a, compare through an imported string compare, engine+0xd9fc0). The names are the 26 standard
  environment presets of the public EAX 2.0 listener model (PUBLIC; same order as the public preset enumeration),
  index = public preset number. Index stored at +0x580; no match = error 0x3d.
- custom mode (`custom != 0`): `name` must equal a fixed 13-byte marker (sha `96b004e89ddd0859`, len 13; compared
  with length 0xe at engine+0xdafa0). The parameter set is reset to defaults (engine+0xd9ca0) and can then be edited
  property by property (manager vtable +0x58, engine+0xd99d0, `(property_name, float)`; 12 names, table 7.2).

"Enable" (manager vtable +0x5c, engine+0xdb060, `(on)`): needs `reverb_available` (section 2); with `on`, a selected
filter and a valid room it applies (engine+0xd9e10) and sets `reverb_active` (+0xac2); otherwise, if active, it
resets defaults, applies them and clears `reverb_active`.

Apply (engine+0xd9e10):
- preset mode: `driver_set_room(sfx_driver, index)` when it differs from the current room type.
- custom mode: `driver_set_reverb(sfx_driver, decay_time, reflections_delay, damping)` with damping from
  manager +0xab8 (constructor value 1.0), `driver_set_reverb_levels(1.0, 1.0)`, then 12 driver properties on
  the output filter (the public EAX 2 listener properties, names hashed in 7.2).

Per voice: at start, if `reverb_active` and the voice's reverb flag (+0x88; cleared by request flag 0x2000) are set,
`voice_set_reverb_send(dry 1.0, wet 1.0)` (engine+0xdc190). Voices already playing when reverb changes are not
updated (no such call elsewhere; PROVEN by call-site scan, section 1). The mixer additionally attenuates the wet
send with distance because `auto_wet = 1` (5.2; PUBLIC).

### 7.2 Custom parameters, defaults (engine+0xd9ca0) and property names (engine+0xd99d0)
| field | manager off | default (PROVEN) | property-name sha (len) set via +0x58 | output-filter property sha (len) used by apply |
|---|---|---|---|---|
| room (mB, int) | +0xad8 | -1000 | `911ea89c43d9dbb8` (4) | `20e20bb6e711391e` (9) |
| room HF (mB, int) | +0xadc | -100 | `aa92aba75de67839` (6) | `a45eb81914eb7ccc` (12) |
| room rolloff | +0xb34 | 0.0 | `6664ff6f15f32e42` (17) | `fc3f79af5643cc68` (17) |
| decay time (s) | +0xae4 | 1.49 | `91be3cd56c87dab4` (9) | `89e448e4acc3066a` (14) |
| decay HF ratio | +0xae8 | 0.83 | `34531847ac9d3ff4` (12) | `d9f2f6f9afec7b9a` (19) |
| reflections (mB, int) | +0xaf0 | -2602 | `9e676ed4a2c9d570` (11) | `a428b935b73909b1` (16) |
| reflections delay (s) | +0xaf4 | 0.007 | `13c27bc4789a3f4c` (16) | `8a332816a561e4c7` (22) |
| reverb (mB) | +0xb04 | 200 | `bf7d7f516e22891c` (6) | `3bb01f3c1e6105bd` (11) |
| reverb delay (s) | +0xb08 | 0.011 | `20483d5802064176` (11) | `b0f89a91882e5bc3` (17) |
| environment size (m) | +0xad0 | 7.5 | `1af851907331c0ed` (4) | `9a9b8848a9123d94` (21) |
| air absorption HF (mB) | +0xb28 | -5.0 | `f8aa08c7953dd1f8` (15) | `0b2c9f68c8509a43` (19) |
| diffusion | +0xad4 | 1.0 | `89ad3663bb60a8be` (9) | `043940559102e8de` (26) |

Other fields reset but not sent: environment index 12 (+0xac4), room LF 0, decay LF ratio 1.0, echo time 0.25,
modulation time 0.25, HF reference 5000.0, LF reference 250.0, flags 0xff. The sent defaults coincide with the
public EAX 2.0 "generic" environment values (PUBLIC), so "custom mode with no edits" sounds like the generic room.
Room, room HF and reflections are stored as integers (truncated) by the property setter.

### 7.3 Client shell side
- Records come from the filter attribute file (entry sha `d7aee08744699add`, len 16; loader cshell+0x29a4b0):
  per section a name key (sha `dcd1d5223f73b3a9`, len 4), the engine filter name (sha `26fcc8c79831fc5e`, len 10,
  up to 32 bytes), a type byte (sha `baaddf70fb5d432b`, len 4; passed as `custom`), and up to 13 pairs
  property-name / value (keys = prefix + decimal index 0..12; name prefix sha `60529803e0f91fc2`, len 3; value prefix sha
  `8e37953d23daca5f`, len 5; literals verified in the CShell image at VA 0x374f80d0 and in cshell+0x29a4b0). COUNT: 28
  sections; 27 carry only name + filter name (preset rooms), 1 carries a type and 13 pairs (the custom room).
- Selection (cshell+0x1fadd0): the console string variable "global filter" (sha `56e6fb14b55882de`, len 17,
  registered with default -1.0 at cshell+0x1f6fb0) names a record; if the console flag "filters on"
  (sha `09e9c8feced2671a`, len 12) is non-zero the shell calls select(filter name, type), sets each pair, then
  enable(all pairs accepted). Command-line flag "disable filters" (sha `b123fa1cd7fa1694`, len 19) forces the console
  flag to 0 (cshell+0x1b3370). Whether world volumes switch filters at run time (a second console variable sha
  `ad2f8722791c177e`, len 15 is registered next to it) is UNPROVEN.

## 8. Background music (BGM)

### 8.1 Engine music player (manager +0x40c; methods engine+0xda000..engine+0xdb2b0)
State: two stream slots `current` and `next` (each: stream handle, its mixer sample, gain), the crossfade start
time and duration, and the current path (engine+0xdb180 constructor, engine+0xda000 reset).

`play(path, loop, fade_seconds)` (client export engine+0x10650 -> engine+0xdb0d0, which converts
`fade_ms = trunc(fade_seconds * 1000)`, then engine+0xda060):
1. Ignore if the player has no driver or `path` is null; if a stream is playing and `path` equals the current path
   case-insensitively, do nothing (no restart).
2. If nothing is playing or `fade_ms == 0`: drop any `next`, close `current`, open the new stream into `current`.
   Otherwise: close any old `next`, open the new stream into `next`.
3. Open with the mixer's file-stream call on the music driver; on failure nothing else changes.
4. Loop count: `loop != 0` -> 0 (forever); `loop == 0` -> 1 (play once). Start the stream.
5. Initial gain of the new stream: 1.0 when `fade_ms == 0`, else 0.0. Remember the path.

Per update (engine+0xda1d0, from engine+0xdbc70; multimedia timer):
- during the fade window `t = (now - start) / fade_ms` in [0, 1):
  with a `next` stream: `current` gain `1 - t`, `next` gain `t` (linear crossfade);
  without one (fade-in from silence): `current` gain `t`.
- after the window: with a `next` stream it replaces `current` (old one closed) at gain 1.0; without one, a
  `current` gain <= 0.99 is snapped to 1.0.
- Gains are clamped to [0, 1] and applied as the stream sample's volume (pan 0.5) (engine+0xd9b90).

`stop` (engine+0x10670 -> engine+0xdb100): closes both streams immediately (no fade-out), clears the path.
Pause / resume exports (engine+0x10680, engine+0x10690 -> engine+0xdb110 / engine+0xdb120) are empty (return at once).
Music volume: section 4.3 (driver master, independent of the SFX master and of the stream gains).

### 8.2 Who plays what (client shell)
The shell reads a file path from the client attribute file (entry sha `6a25f4d8dc9c0e1a`, len 15), section sha
`c26b3ed4ce70e202` (len 9), through cshell+0x1b2c00, then calls its music object's `play(path, loop, fade)` (the
object's method +0x88; that it forwards to the engine export of 8.1 is consistent with the arguments but the link is
UNPROVEN) or `stop` (+0x84).

| key (sha, len) | caller | arguments | when |
|---|---|---|---|
| `da12ca4109dab384`, 16 | cshell+0x118bc0, cshell+0x11a300 | loop, fade 1.0 s | game launch from the lobby, loading screen |
| `da12ca4109dab384`, 16 | cshell+0x1630a0 | once, no fade | another loading path (UNPROVEN which) |
| `2537f0c81ab4149f`, 14 | cshell+0x1fba00 | loop, fade 1.0 s | screen state 9 (waiting room) |
| `00d4060b7bde3a74`, 10 | cshell+0x1fba00 | loop, fade 1.0 s | any other screen state except 0 and 7 (front-end menus); state 0 stops, state 7 keeps the current music |
| `c8fb4e436daa42f9`, 17 | cshell+0x28dd40, cshell+0xbe0c0 | loop, fade 1.0 s | the infection-style game mode start |
| `4e474505112a31a9`, 20 | cshell+0xbe570 | stop, then once, no fade | end of that mode's round |

COUNT (`staging/audio_2009/bgmkeys.py`, `bgmref.py`): the 5 keys resolve to 4 distinct files, all in `Game/BGM`;
the waiting-room key and the front-end key name the same file. In-match music during normal modes: none found
(no other key, no engine path). Of the 14 BGM files, 4 are referenced by the attribute file; the other 10 are not
referenced by any attribute entry, mode-setup entry or the four client binaries; 3 of those 10 file stems occur as
byte strings inside the world archive (how a world would play them is UNPROVEN; the engine has no other mp3 path).

### 8.3 Formats of the BGM files (COUNT, `staging/audio_2009/mp3hist.py`, first frame header of each file)
| count | format |
|---|---|
| 14 | files, 14,216,048 bytes in total; 9 start with an ID3v2 tag |
| 13 | MPEG-1 Layer III, 44100 Hz: 6 joint stereo 128 kbit/s, 4 joint stereo 192, 1 stereo 128, 1 stereo 192, 1 stereo 256 |
| 1 | MPEG-2 Layer III, 16000 Hz, mono, 16 kbit/s |
Decoding is done by the mixer's MP3 provider in the provider folder (PUBLIC). Our crate: any MP3 decoder that handles
MPEG-1/2 Layer III and skips ID3v2; resample to the 44.1 kHz device rate.

## 9. The sound attribute table (per-sound records)

Source: the sound attribute file (entry sha `c181c4259c96130d`, len 14, in `Game/ATTRIBUTES.rez`; file-name literal
used by cshell+0x2e4710), parsed per section by cshell+0x2a1140 into a record that the shell turns into a play
request (3.1). Keys and the request bits they set:

| key (sha, len) | type, default | record field (ours) | effect on the request | evidence |
|---|---|---|---|---|
| `5f7953f7c9b6ba16`, 6 | string | `parent` | the parser follows the parent chain to its root and stores the root name; whether values are inherited inside the attribute reader is UNPROVEN | cshell+0x2a1140 |
| `62fa5a5b0d3c3188`, 4 + index | string list | `paths[i]` | candidate files, read while present (index 0, 1, ...) | cshell+0x2a1140 |
| `81d27ef6d5033c3e`, 6 + index | f32, 1.0 | `weights[i]` | normalised into a cumulative distribution over `paths` (weighted random choice) | cshell+0x2a1140 |
| `487cc23156cc7e95`, 11 | f32, 0 | `inner_radius` | request 0x11c | cshell+0x2a1140 |
| `4bf718bcf0e0bbd2`, 11 | f32, 0 | `outer_radius` | request 0x118 | cshell+0x2a1140 |
| `bbd74991af4f3380`, 10 | f32, 0 | `play_chance` | caller-side gate | cshell+0x2a1140 |
| `6af11cc594045511`, 5 | f32, 0 | `pitch` | `PITCH` (0x80) when != 1.0 (inaudible, 3.6) | cshell+0x2a1140 |
| `b10fb966d72063f5`, 6 | int, 0 | `volume` (byte) | `VOLUME` (0x40) when != 100 | cshell+0x2a1140 |
| `d60dbba079223254`, 8 | int, 0 | `priority` | request 0x114 | cshell+0x2a1140 |
| `a39c62e1b8f1921c`, 14 | int, 0 | `local` | when 0, `POSITIONAL` (0x2) is set; when non-zero, no kind bit (a `Local` voice) | cshell+0x2a1140 |
| `f839d86487378c00`, 15 | int, 0 | `local_if_own` | `LOCAL_IF_OWN` (0x800) | cshell+0x2a1140 |
| `89da4bf6f173830a`, 9 | int, 0 | `looping` | `LOOP` + `WANT_HANDLE` (0x14) | cshell+0x2a1140 |
| `7f18fba9c0a29353`, 11 | int, 0 | `reverb` | 0x100 | cshell+0x2a1140 |
| `598d218bd6f5b953`, 8 | int, 0 | `once_per_buffer` | 0x1000 | cshell+0x2a1140 |
| `ea11484d788f3b4d`, 7 | int, 0 | `lip_sync` | caller-side | cshell+0x2a1140 |
| `23bd417edb0ad64f`, 8 / `1c862110d5ef07c3`, 8 | f32, 0 | `min_delay` / `max_delay` | caller-side repeat delay | cshell+0x2a1140 |

Parent fallback is required by the data. COUNT (`staging/audio_2009/inherit.py`): 213 sections name a parent,
there are only 3 distinct parents, all of them sections of the same file, and every chain terminates. Resolved
through the parent chain, 216 of 217 sections obtain a volume, both radii, the local key and the priority; read
directly, only 9 have a volume, which with the reader's default 0 would set `VOLUME` with 0 and silence them.
So the attribute readers used here (cshell+0x29f370 int, cshell+0x29f510 float, cshell+0x29f6a0 string) must fall
back to the parent section; their internals were not traced (mechanism UNPROVEN, requirement PROVEN by the data).
Our reader: look up the key in the section, then in its parent chain, then use the default.

COUNT (`staging/audio_2009/bute.py`): 217 sections. Key occurrences: parent 213, path index 0: 213, 1: 62, 2: 52,
3: 26, 4: 10, 5: 4, 6..8: 1 each; inner and outer radius 181 each; once-per-buffer 22; volume 9; pitch 4; looping 4;
play chance 4; local 3; local-if-own 3; reverb 3; priority 3. No section uses the weight, lip-sync or delay keys
(defaults apply).

Other sound-related attribute entries (COUNT, structure only):
| entry (sha, len) | sections | content shape | used by |
|---|---|---|---|
| `d7aee08744699add`, 16 | 28 | sound filters (7.3) | cshell+0x29a4b0 |
| `afc92e8801fac2f4`, 13 | 1 | one directory key (sha `a14bac75fafd623a`, len 8) | cshell+0x29e740 |
| `c461bae336fdce11`, 17 | - | radio-message sounds | file-name literal in the shell (cshell+0x2e4710 group) |
| `b1d0ddf6234d2b31`, 17 | - | auto-cast sounds | same |
| `9e8b878f3b82f725`, 13 | 2 | AI dialogue keys | no client reader found (server side) |
| `e6df283cfd916cf0`, 9 | 17 | dynamic-music sets (moods per set) | no engine reader; legacy, unused by the BGM path (UNPROVEN whether the shell's music object still loads it) |
| `6a25f4d8dc9c0e1a`, 15 | - | carries the BGM keys (8.2) and many interface sound keys | cshell+0x1b2c00 |

## 10. Audio assets in the ROM (WAV and MP3 histograms)

WAV (COUNT, `staging/audio_2009/wavhist.py`: walks every `Game/*.rez` directory with `rezls.walk`, reads at most 4 KiB
of each entry with extension WAV, parses the RIFF `fmt ` and `data` chunk headers; read-only):

| count | WAVE format (tag, channels, rate, bits) |
|---|---|
| 1,529 | WAV entries in total, all valid RIFF/WAVE, all format tag 1 (integer PCM), all 16-bit |
| 828 | mono, 44100 Hz |
| 675 | mono, 22050 Hz |
| 21 | stereo, 44100 Hz |
| 2 | mono, 11025 Hz |
| 1 | stereo, 16000 Hz |
| 1 | mono, 16000 Hz |
| 1 | mono, 5000 Hz |

So the earlier audit note "PCM16 / 44.1 kHz mono" holds for only 54 % of the entries; 44 % are 22.05 kHz mono and
our mixer must resample (any rate) and handle stereo. No ADPCM, no 8-bit, no float, no WAVE_FORMAT_EXTENSIBLE.
Durations from `data` length / byte rate: min 0.027 s, median 0.711 s, 90th percentile 2.574 s, max 131.9 s;
78 entries exceed 5 s and 11 exceed 30 s (still played from memory, section 3.8). WAV is the only audio type among the 27
distinct entry extensions of all archives (COUNT); MP3 exists only as the 14 loose BGM files (8.3). Three `.bik` movies carry their
own audio (out of scope).

WAV entries per archive (archive file name as sha, length):
| archive sha | len | WAV entries |
|---|---|---|
| `2388ed76aa8b84db` | 13 | 175 |
| `a29da623dcc57574` | 6 | 108 |
| `1429a6c7062b2374` | 11 | 4 |
| `216bc69ddf11fddd` | 15 | 172 |
| `d4ed72e479b50cb5` | 16 | 43 |
| `b40704ec2ad4333f` | 20 | 219 |
| `e1aff55447eb7070` | 21 | 48 |
| `2ba045824b747280` | 18 | 18 |
| `9418134a78c579a2` | 15 | 56 |
| `20838b5d2507f503` | 19 | 8 |
| `6e97650957128eee` | 19 | 85 |
| `2dba1625b2b95b75` | 15 | 41 |
| `39bc1d2b240092d9` | 19 | 10 |
| `4155ea24bcfe080c` | 16 | 113 |
| `f9f3ee9d066ae56b` | 15 | 84 |
| `fe2498eadb4a2591` | 21 | 47 |
| `2fce4a77793191f8` | 7 | 136 |
| `c7c7888fcbdbface` | 10 | 162 |

## 11. User-configurable settings (options screen, profile, console)

### 11.1 Options screen (sound page; page id string sha `b3514c28b8c7ef7e`, len 17; built at cshell+0x151510)
Four controls, all backed by console variables (read cshell+0x151aa0, applied cshell+0x151bf0):

| control | console variable (sha, len) | type, default when unset | applied as |
|---|---|---|---|
| effects volume | `2827d45df3bb7cac`, 11 | int 0..100, 100 | SFX master (4.3) = value, or 0 when effects are disabled |
| effects on/off | `0fcc24cf755dc13c`, 11 | bool, 1 | as above |
| music volume | `71ba43c050d8ad45`, 11 | int 0..100, 100 | music volume = value, or 0 when music is disabled (through the shell's music object, cshell+0x151bf0) |
| music on/off | `676657ff4ce8a4b4`, 11 | bool, 1 | as above |

Applying writes the four console variables, sets the music volume (0 if disabled), sets the SFX master through the
engine sound interface (+0x34 of the interface returned by the engine's +0x14 accessor), and saves.

### 11.2 Profile section (persisted per user)
Section sha `46124e023603420f` (len 5) of the user profile; written by cshell+0x1e92b0, read by cshell+0x1ecef0,
pushed to console variables and the engine by cshell+0x1ec690 and cshell+0x1e95a0.

| key (sha, len) | type | default on read (cshell+0x1ecef0) | effect |
|---|---|---|---|
| `2827d45df3bb7cac`, 11 | int | 0 | effects volume (11.1) |
| `71ba43c050d8ad45`, 11 | int | 0 | music volume (11.1); forced to 0 when the "music allowed" start-up flag is clear |
| `b3f675e47c4fab82`, 11 | bool | 1 | effects on/off (stored under this profile key, mapped to the console variable of 11.1) |
| `eebd38d581fdb8be`, 11 | bool | 1 | music on/off (same) |
| `fb1627408915be99`, 22 | f32 | 0.75 | class 0 gain (4.2) |
| `15a7676adee0abd1`, 22 | f32 | 1.0 | class 1 gain |
| `e9c37bf66ba9e9b3`, 21 | f32 | 1.0 | class 2 gain |
| `b913979e1883378e`, 10 | int | 0 | "16-bit sound" switch; a change triggers a sound re-initialisation (cshell+0x1ec690 -> cshell+0x1a7e80); its audible effect is UNPROVEN (the engine drivers stay 16-bit, section 2) |
| `bb2efc1d7fa01e40`, 12 | int | 0 | music quality switch passed to the shell's music object (cshell+0x29a1a0); audible effect UNPROVEN |

Note the asymmetry: a profile without the volume keys yields 0 (silent) through cshell+0x1ecef0, while the
options page defaults to 100 through the console variables. Which value wins on a fresh profile is UNPROVEN
(the ROM ships a `Profiles` folder; its contents were not read for this spec).

### 11.3 Console variables and start-up flags read by the shell (cshell+0x1b3370, cshell+0x1b1e20)
| name (sha, len) | role |
|---|---|
| `af936edcba3a0520`, 12 | disable music (clears the "music allowed" bit) |
| `30c8747270e5405e`, 12 | disable sound (clears the "sound allowed" bit) |
| `b123fa1cd7fa1694`, 19 | disable sound filters (7.3) |
| `1cbb8c9e6b691332`, 20 | disable hardware sound (bit kept by the shell; no engine use found, UNPROVEN) |
| `f92bdf8275a358ae`, 11 | sound enable (set to 1 when allowed and unset); the shell's local-sound helper plays nothing when it is 0 (cshell+0x1b1e20) |
| `a78d013317155e3d`, 11 | music enable (set to 1 when allowed and unset) |
| `09e9c8feced2671a`, 12 | filters on (7.3) |
| `56e6fb14b55882de`, 17 | global filter name (7.3) |
| `0b86e6732726b013`, 15 | engine console command: set rolloff factor (5.2) |

The shell's local-sound helper (cshell+0x1b1e20) also shows how UI sounds are requested: path copied (<= 0x104
bytes), the `POSITIONAL` bit cleared (so UI sounds are `Local`), `VOLUME` set when the given volume is below 100,
then the engine play entry.

## 12. Reimplementation contract for our audio crate

Behaviour our own mixer (for example on `cpal`) reproduces by default; each item cites the section above. Items
marked "mod switch" may be improved behind a server-enforced setting, never silently.

1. Device: 44.1 kHz, 16-bit-equivalent float mix, OS speaker layout (2). Mute both buses while the window is
   minimised, restore exactly (2).
2. Buses: one effects bus with master `sfx/100`, one music bus with master `music/100` (4.3).
3. Voices: up to 255 audible effect voices; the 256th and later are tracked but silent; no stealing (3.5). Active
   list cap 255 (3.4).
4. Kinds: `Local` (2D, centre), `Ambient` (positioned once), `Positional` (re-positioned each update) (3.3).
5. Gains: stage A (distance law with its quirks, 5.3), stage B (class/master formula with the x1.27 and
   double-master quirks, 4.1), stage C (bus master). Clip at the output.
6. 3D: spatialise relative to a left-handed listener (+Y up, +Z forward) taken from the camera, listener pushed at
   10 Hz for panning, distance law against the current listener (5.1). No mixer-side distance rolloff unless the
   rolloff console command sets a non-zero factor (then an inverse-distance law between inner and outer+0.01; mod
   switch, exact curve UNPROVEN). Silent beyond outer + 0.01 (5.2).
7. No doppler, no pitch change, no occlusion/obstruction (3.6, 5.4, 6). Mod switches allowed.
8. Reverb: 26 public room presets plus one custom parameter set (EAX 2 listener model, 7.2); a voice gets the wet
   send only if reverb was active when it started (7.1); wet send attenuated with distance.
9. Music: single-file MP3 streaming with path de-duplication, linear crossfade / fade-in over `fade` seconds, loop or
   once, hard stop, pause/resume no-ops (8.1). BGM choice by front-end state and mode (8.2).
10. Data: sound records with parent fallback (9); WAV PCM16 at any rate, mono or stereo (10); MP3 MPEG-1/2 L3 (8.3).
11. Settings: honour the console variables and profile keys of 11 by hash; same defaults.

Test hooks (for the differential harness, no ROM data in fixtures): a pure function for stage A (5.3) and stage B
(4.1) with table tests at the boundaries (d^2 = inner^2, d^2 = outer^2, inner = 0, v = 100, class 0.75), a
crossfade timeline test (8.1), and a voice-pool exhaustion test (3.5).

## 13. Open questions (UNPROVEN, to settle by capture or further reading)
1. When stage B (volume levels) is first applied to a new voice (4.1).
2. Mixer handling of level > 1.0 and of negative volume (4.1, 5.3); mixer behaviour beyond max distance (5.2).
3. Meaning of request flags 0x08, 0x20, 0x400; engine use of 0x100 (3.2).
4. Who culls by `priority` and applies play chance / delays (3.5, 6, 9).
5. Which callers assign sound classes 1 and 2 (4.2).
6. Whether anything issues the rolloff console command (5.2).
7. Whether world volumes switch reverb filters at run time (7.3).
8. Effect of the 16-bit switch, the music-quality switch and the convert-16-to-8 init flag (2, 11.2).
9. Which profile/console default wins on a fresh profile (11.2).
10. Whether the 10 unreferenced BGM files are reachable (8.2).
11. Start-offset unit and buffer length source (3.7).
12. The internals of the attribute readers' parent fallback (9).

## 14. Server-started sounds and footsteps (folded by spec-fold-8 from aa-triage-sounds / aa-fix-sounds, 2026-10-25)

Evidence of this section: the reference stack captures tdm1, tdm2, tdm4 and menu-frida6 on SNOWVALLEY (REFERENCE: the forms the real client accepted), the Rust-stack capture rust4
(OURS), read-only capstone disassembly and clean decompiler output of the user's CShell.dll (DISASM / EVIDENCE), and counts of the ROM clip markers (COUNT). Values are not copied.

### 14.1 World sound objects become tracked sounds

The ambient sounds of a world are not started by the client from the world file (the client scans the world's objects for light classes only: `FIRETEAM_2009.md` 3.3,
`FMT_DAT85.md`). They arrive as tracked sounds of `S2C_OBJ_GROUP` (`UDP_SESSION_2009_B.md` 8.3), one per switched-on world sound object, in the world-load group (REFERENCE,
ACCEPTED: 11 records on every SNOWVALLEY link of four captures; field rules in B 8.3). Their request flags (section 3.2) are 0x0130 always (`WANT_HANDLE`, the unproven 0x0020,
`REVERB`) plus `AMBIENT` or `POSITIONAL`, `LOOP` and `VOLUME` from the object's properties, so the ambient voice kind and the distance law of section 5 apply with the object's
radii (`inner = inner_ratio x outer / 255`). A server that sends none leaves the world silent apart from the client's local sounds (rust4: no tracked sound; the user heard
only the sprint sound; PROVEN-CAPTURE of the absence, OBSERVED report). COUNT (aa-fix-sounds): 150 objects of the sound class on 16 of 21 worlds, 129 switched on; an
attach switch is set on 13 objects of other maps (its "at the listener" use is UNPROVEN). DeCombat: DSV-56 / DSV-57 (PENDING).

### 14.2 Footsteps come from animation-key markers of the character clips

- The character FX's model-key handler (cshell+0x2057a0; two method-table slots point at it) compares each key name case-insensitively with the footstep key string (CShell
  data VA 0x374efe38, referenced only at cshell+0x205855) and on a match calls the footstep player cshell+0x204f60 (calls at cshell+0x20588f / +0x2058b5); the second foot key (the ground-print key)
  (VA 0x374efe28, compare at cshell+0x2058bf) is the next branch (DISASM).
- The footstep player cshell+0x204f60 (EVIDENCE, clean C): if the FX is the local player's (FX +0x40 set) and the keyed object is the client's own object, it uses the mover
  (global VA 0x3759bf60, +0x10): it returns when the mover's +0xac flag is set or the ground probe cshell+0x1fe1f0 finds nothing, and takes the surface from mover +0x50 (the
  surface the 0x64 trailer sends, `MOVEMENT_2009.md` 12.2); otherwise it traces from the left / right foot socket. It then plays the surface's footstep sound (cshell+0x202870).
  **So the local player's own footsteps come from the animation of its own character object**, and a remote player's from that player's streamed animation. Other callers:
  the jump notice cshell+0x1ff0b0 (a step at take-off, call at cshell+0x1ff22e) and cshell+0x1fc760 (no reference found).
- COUNT (the default body and its child model; aa-triage-sounds 4.1): the run lower-body clips carry two footstep markers each (for example at 50 and 350 ms), the sprint-run
  clips two, the jump and landing clips one; the walk and crouch clips carry only the ground-print markers; the stand clips and the main clip none. A walking or crouching character
  is therefore silent and a running one steps (ROM-gated test over 33 player rigs, aa-fix-sounds).
- On the wire (REFERENCE vs OURS; aa-triage-sounds 4.2): the reference stack streamed the user's own character to its own client (tdm1 4,652 entries; tdm4 on both links) with the lower
  tracker following the control word (run forward 37; run diagonals, sides and back 41 / 40 / 39 / 38 / 36 / 42; walk forward 47; crouch forward 18; sprint = run forward, by
  combined index on the default body); our rust4 server streamed the walk-forward clip 47 for every moving state and never streamed the user's own object to its owner (it kept
  the create's clip 0), so neither the bot's nor the user's steps were heard (the user's report). The client computed its surface the same on both stacks (trailer surface
  71 / 10), so the surface was not the cause. DeCombat: the DSV-28 refinement (clip by control word) and the DSV-23 change (own object streamed by default) of aa-sounds
  (PENDING).
- Open: the meaning of request flag 0x0020; a world sound that starts switched off and its start command (none on SNOWVALLEY); the reference stack's pain, fall and bounce sounds
  (B 8.3; DSV-58 not implemented); audible confirmation with the real client.

## Appendix I: fold record (spec-fold-8, 2026-10-25)

Sources: `reports/{aa-triage-sounds, aa-fix-sounds}.md` and the rows DSV-23 / DSV-28 / DSV-56..58 (`DC_DEFAULTS.md` Wave AA, PENDING). New section 14
(contents line added); sections 1-13 unchanged.
