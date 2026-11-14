# The audio mix: limiter, voice caps, trims, and what ducking would take

How dc-audio keeps a busy match from clipping, and the next steps that are designed but not built. Rules and numbers:
DC_DEFAULTS.md CLI-1955 (Sound Mixer groups), CLI-1960 (limiter), CLI-1961 (voice caps), CLI-1962 (trims). Status: the
three built parts are implemented and unit-tested, with an offline render as proof; nobody has listened to them in a
match yet, and all three are off or neutral by default, so the mix is exactly as before until someone switches them on.

```
voice -> distance law (A) -> voice/class level (B) -> effects master (C) -> group volume (player) -> group trim
      \__ per group: voice cap (steal the least audible, 5 ms fade) ___________________________________/
all voices summed + music after its master -> limiter (ceiling, hold, release) -> clamp [-1, 1] -> device
```

## 1. What exists

| part | where | default | setters (dc-audio `Mixer`) |
|---|---|---|---|
| Group volume (the player's Sound Mixer slider, 13 rows) | `set_group_volume` | 100 | `set_group_volume(group: u8, v: u16)`, `group_volume(group) -> u16` |
| Group trim (a designer's level, under the player's slider) | `set_group_trim_db` | 0 dB, exact unity | `set_group_trim_db(group: u8, db: f32)` (-60..+12), `group_trim_db(group) -> f32` |
| Voice cap per group | `set_group_cap` | none | `set_group_cap(group: u8, cap: Option<u16>)`, `group_cap(group) -> Option<u16>`, `group_live_voices(group) -> usize`, `group_steals(group) -> u64`, `group_refused(group) -> u64` |
| Output limiter | `set_limiter_enabled` | off, ceiling -1 dBFS | `set_limiter_enabled(bool)`, `limiter_enabled()`, `set_limiter_ceiling_db(f32)` (-6..0), `limiter_ceiling_db()`, `limiter_reduction_db() -> f32`, `limiter_meter() -> Arc<LimiterMeter>` |

`LimiterMeter` is a set of atomics the audio callback writes once per buffer: `reduction_db()` (the last buffer's gain
reduction, for a live "limiter -x dB" readout), `limited_frames()`, `clipped_samples()`, `enabled()`, and
`take_window()` (largest reduction since the last call). A panel reads it without taking the mixer's lock.

The groups are the client's (`dc-client` `soundmix::SoundGroup`): own fire, own handling, others' fire, others'
handling, flybys, hit markers, own steps, others' steps, voices, breath, world (explosions / map), announcer, menu,
and impacts. Impacts (bullets hitting surfaces, the most frequent voice in a match) have a group of their own for the
cap and the trim but no row on the Sound Mixer page: their player volume follows the "Explosions / Map" slider, as
before they had a group.

The client switches: `--limiter on|off|<dB>`, `--voice-caps on|off`, and `AudioConfig::{limiter,
limiter_ceiling_db, voice_caps}`. The device's status line ends in `| limiter on limited <frames> max reduction <dB>`.

## 2. Why the limiter is built this way

- **Zero latency, hard knee, instant attack.** A frame above the ceiling sets the gain to exactly `ceiling / peak`;
  the output never exceeds the ceiling. Below the ceiling, with the gain at 1.0, the sample is not touched: a mix
  that never reaches the ceiling is bit-exact with the limiter off. A look-ahead (a few ms of delay, gain ramped down
  before the peak) sounds smoother on the first millisecond of a transient but delays every sound and is no longer
  sample-identical; a soft knee changes signals below the ceiling. Both were rejected for that reason.
- **Non-finite samples** (an infinity or a NaN from a broken voice) are not limited and do not move the gain: they
  reach the clamp exactly as with the limiter off.
- **Switching it off** while it holds the gain down lets the gain finish its release (no click), then bypasses it.
- **Hold 10 ms, release 200 ms.** Automatic fire is 60..100 ms between shots; a release longer than that gives a
  burst one steady reduction instead of a dip per shot (pumping).
- **After effects + music.** Only the full sum can be kept from clipping. The music dips with a limited peak for the
  hold plus the release; that is deliberate.
- **Cost.** About 1 us per 512-frame stereo buffer limiting every frame in a release build (under 0.01 % of the
  buffer's 10.7 ms at 48 kHz; about 10 us in a debug build).

Offline proof (`crates/dc-audio/tests/mix_burst_render.rs`, synthesized shots, 2 s, 600 rounds a minute):

| shooters | clipped samples, off | peak, off | clipped, on | peak, on | max reduction |
|---|---|---|---|---|---|
| 1 | 1248 | 1.000 | 0 | 0.891 | 3.7 dB |
| 4 | 9308 | 1.000 | 0 | 0.891 | 7.2 dB |
| 8 | 28917 | 1.000 | 0 | 0.891 | 11.2 dB |
| 16 | 39102 | 1.000 | 0 | 0.891 | 12.3 dB |

A reduction above about 6 dB is audible as the whole mix ducking. The fix for that is less summed energy (voice caps,
trims, quieter source files with headroom), not a harder limiter.

## 3. Voice caps

A cap per group; a new voice in a full group takes the place of the group's least audible voice (its level at the
listener after the distance law, voice level, masters and group volume; the oldest of equals), which fades out over
5 ms. A new voice quieter than every voice of the full group is not started. Loops are never counted or stolen. A cap
set below a group's live voices (a panel moving it) fades the least audible ones at once until the group is at the cap.
A voice released during its fade finishes the fade first. Groups are separate, so a remote shot can never take the
place of the player's own gun, reloads or steps (the own-fire, own-handling and own-steps groups are never capped).
The player's own bullet impacts are in the impacts group with everyone's: with the caps on, a nearer impact of another
player can take the place of a farther own impact. Ranking own impacts first would need the effects stage to mark
them; not done. Bullet impacts are logged with the label `one_shot` as before their group existed. The client's caps when on:
others' fire 24, others' handling 16, flybys 8, others' steps 24, world 24, impacts 32; the player's own sounds,
voices, breath, hit markers, announcer and menu: none. A fire sound already cuts its emitter's previous one, so
others' fire holds about one voice per shooter.

## 4. Next steps (not built)

### 4.1 Player-facing settings (small: about 1 day)

The Sound Mixer page already holds the player's per-group volume. A designer's trim must stay separate from it (the
player's setting is always applied on top of the designer's level), so trims belong in a mod or a server-sent mix
profile, not on the page. Wiring: a `[sound.trims]` table (group key -> dB) read where `AudioConfig` is built, applied
with `set_group_trim_db`. The limiter switch and the caps switch could join the Sound tab as two checkboxes.

### 4.2 Ducking (medium: about 2-3 days with tests)

Rule: when a "loud event" starts (the own gun's shot, an explosion within its inner radius), dip a set of groups by a
few dB for a fraction of a second: ambience (world loops), music, distant others' fire. Proposal: -4 dB, attack 5 ms,
hold 80 ms, release 250 ms; retriggered by each shot, so automatic fire holds the dip. Implementation: a per-group
duck gain in the mixer, smoothed per buffer (a one-pole ramp toward the target, so no zipper noise), multiplied where
the trim is; a `duck(groups: mask, db, hold_s)` call from the sound director when it starts a qualifying voice. It is
deterministic, allocation-free and testable offline like the limiter. Risk: too much ducking makes the player's own
gun feel isolated; the depth needs a listening test.

### 4.3 An HDR-style loudness window (large: about 1-2 weeks)

The idea (used by several shooters): every sound has a loudness in dB at the listener; the mixer keeps a window
(about 40-60 dB wide) whose top follows the loudest sound playing now, and sounds below the bottom of the window are
culled or faded. A quiet footstep is audible in a quiet room and disappears under a firefight, and the sum never
needs a limiter to stay sane. What it would take: a loudness value per sound (an authored dB value or one measured
from the file at load: RMS or peak of its first 100 ms), the window's top tracking with its own attack and release,
the per-voice gain relative to the window top, and a culling rule for voices below the window that re-admits them
when the window falls. It replaces the fixed 2009-style levels with a relative mix, so it changes how every sound
balances against every other: for the emulator it would be an opt-in mode, never the default.

### 4.4 Headroom in the source sounds

Many effects are normalized near full scale, so two of them already clip. A per-group trim of -3..-6 dB on the most
frequent groups (impacts, others' fire, flybys) is the cheapest headroom; the limiter then catches the rare peaks.
