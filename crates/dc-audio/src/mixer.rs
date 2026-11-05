//! Software mixer: up to 255 effect voices, an effects group and a music group, 44.1 kHz stereo f32 out.
// SPEC: AUDIO_2009.md §2 (device), §3 (voice model), §4 (levels), §5 (distance law, listener), §12 (contract)

use std::sync::Arc;

use crate::bgm::BgmPlayer;
use crate::decode::Pcm;
use crate::law::{self, ClassTable, Radii};
use crate::reverb::ReverbState;
use crate::{MAX_VOICES, OUTPUT_RATE};

/// Voice kinds. // SPEC: AUDIO_2009.md §3.3
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceKind {
    /// Plain 2D voice at centre pan, no distance law.
    Local,
    /// 3D position set once; distance law each update.
    Ambient,
    /// 3D position re-sent every update; distance law each update.
    Positional,
}

/// Mixer-side distance cut-off margin: `max = outer + 0.01`. // SPEC: AUDIO_2009.md §5.2
pub const OUTER_MARGIN: f32 = 0.01;
/// The highest voice level (stage B) the mixer applies by default. The engine's level formula gives 1.27 at volume
/// 100 and class gain 1.0 (SPEC AUDIO_2009.md §4.1: the integer step `* 127 / 100` then `* 0.01`); whether the
/// 2009 mixer clamps a level above 1.0 is internal to it (UNPROVEN, §13.2). Ours saturates at unity, so a full
/// weapons voice plays at 1.0, not 1.27 (2.1 dB lower), and the class-0 level 0.95 is unchanged.
// DC-DEFAULT: CLI-202 (proposed) voice level ceiling 1.0, see DC_DEFAULTS.md
pub const LEVEL_CEILING: f32 = 1.0;
/// The listener is pushed to the panner at most this often. // SPEC: AUDIO_2009.md §5.1
pub const LISTENER_PUSH_S: f64 = 0.100;
/// Nominal radii of a `Local` voice. // SPEC: AUDIO_2009.md §3.3
pub const LOCAL_RADII: (f32, f32) = (5.0, 10.0);

/// Listener: position and right/up/forward basis in the left-handed +Y-up space.
/// // SPEC: AUDIO_2009.md §5.1
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub pos: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
}

impl Default for Listener {
    /// No camera: origin, forward +Z, up +Y, right +X. // SPEC: AUDIO_2009.md §5.1
    fn default() -> Self {
        Self {
            pos: [0.0; 3],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            forward: [0.0, 0.0, 1.0],
        }
    }
}

impl Listener {
    /// Builds the basis from a camera position and its forward and up vectors (right = up x forward in a
    /// left-handed system). Inputs need not be normalised.
    pub fn from_camera(pos: [f32; 3], forward: [f32; 3], up: [f32; 3]) -> Self {
        let f = norm(forward);
        let r = norm(cross(up, f));
        let u = cross(f, r);
        Self {
            pos,
            right: r,
            up: u,
            forward: f,
        }
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l > 1e-9 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}
fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    dot(d, d)
}

/// A play request (the audible fields of the request block). // SPEC: AUDIO_2009.md §3.1
#[derive(Clone, Debug)]
pub struct PlayRequest {
    pub pcm: Arc<Pcm>,
    pub kind: VoiceKind,
    pub looped: bool,
    /// Keep the voice (finished, silent) until `release` is called. Request flag WANT_HANDLE.
    pub want_handle: bool,
    /// 0..=100 voice volume (default 100).
    pub volume: u8,
    pub inner_radius: f32,
    pub outer_radius: f32,
    pub position: [f32; 3],
    /// Sound class 0..15.
    pub class: u8,
    /// Per-voice reverb flag (cleared by NO_REVERB). Default true.
    pub reverb: bool,
}

impl PlayRequest {
    pub fn local(pcm: Arc<Pcm>) -> Self {
        Self {
            pcm,
            kind: VoiceKind::Local,
            looped: false,
            want_handle: false,
            volume: 100,
            inner_radius: LOCAL_RADII.0,
            outer_radius: LOCAL_RADII.1,
            position: [0.0; 3],
            class: 0,
            reverb: true,
        }
    }
    pub fn positional(pcm: Arc<Pcm>, position: [f32; 3], inner: f32, outer: f32) -> Self {
        Self {
            kind: VoiceKind::Positional,
            position,
            inner_radius: inner,
            outer_radius: outer,
            ..Self::local(pcm)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VoiceId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayError {
    /// The 255-entry active list is full. // SPEC: AUDIO_2009.md §3.4 step 3
    ActiveListFull,
}

#[derive(Clone, Debug)]
struct Voice {
    id: VoiceId,
    req: PlayRequest,
    radii: Radii,
    /// Stage A.
    a: f32,
    /// Holds one of the mixer handles; a voice without one is tracked but silent. // SPEC: AUDIO_2009.md §3.5
    has_handle: bool,
    /// Reverb wet send was granted at start. // SPEC: AUDIO_2009.md §7.1
    wet: bool,
    pos: f64,
    finished: bool,
    pause_count: u32,
    /// Beyond `outer + 0.01` of the listener: silent. // SPEC: AUDIO_2009.md §5.2
    cut: bool,
}

impl Voice {
    fn playing(&self) -> bool {
        !self.finished && self.pause_count == 0 && self.has_handle
    }
}

/// The mixer. Not real-time safe by itself (it allocates in `play`); the device layer owns it behind a lock.
pub struct Mixer {
    voices: Vec<Voice>,
    next_id: u32,
    free_handles: usize,
    pub classes: ClassTable,
    sfx_level: u16,
    music_level: u16,
    minimised: bool,
    /// Listener for the engine distance law: the camera's current position, unthrottled.
    law_pos: [f32; 3],
    /// Listener as last pushed to the panner (10 Hz).
    pan_listener: Listener,
    next_push_s: f64,
    camera: Option<Listener>,
    /// The stereo law of the voices (CLI-200).
    pan_law: PanLaw,
    /// Highest stage-B level a voice gets (`None`: the engine's 1.27 at volume 100 passes through) (CLI-202).
    level_ceiling: Option<f32>,
    pub reverb: ReverbState,
    pub bgm: BgmPlayer,
    scratch_music: Vec<f32>,
    scratch_gains: Vec<f32>,
    /// Rate of the rendered output in Hz: 44.1 kHz as the 2009 drivers (SPEC §2), or the output device's own rate
    /// when it cannot run at 44.1 kHz; every voice converts from its file rate straight to this rate.
    out_rate: u32,
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

impl Mixer {
    pub fn new() -> Self {
        Self::with_handles(MAX_VOICES)
    }

    /// A mixer whose handle pool holds only `handles` (<= 255) entries, to exercise exhaustion.
    pub fn with_handles(handles: usize) -> Self {
        Self {
            voices: Vec::new(),
            next_id: 1,
            free_handles: handles.min(MAX_VOICES),
            classes: ClassTable::default(),
            sfx_level: 100,
            music_level: 100,
            minimised: false,
            law_pos: [0.0; 3],
            pan_listener: Listener::default(),
            next_push_s: 0.0,
            camera: None,
            pan_law: PanLaw::default(),
            level_ceiling: Some(LEVEL_CEILING),
            reverb: ReverbState::default(),
            bgm: BgmPlayer::new(),
            scratch_music: Vec::new(),
            scratch_gains: Vec::new(),
            out_rate: OUTPUT_RATE,
        }
    }

    /// Sets the rate `render` produces (Hz, clamped to 8000..=384000). The voices keep their position in their
    /// files, so a change mid-voice keeps the pitch and the remaining duration.
    pub fn set_output_rate(&mut self, rate: u32) {
        self.out_rate = rate.clamp(8_000, 384_000);
    }
    pub fn output_rate(&self) -> u32 {
        self.out_rate
    }

    // ---- masters (SPEC §4.3) ------------------------------------------------------------------------

    /// SFX volume 0..=100 (clamped), stored and applied as the SFX master `v * 0.01`.
    pub fn set_effects_volume(&mut self, v: u16) {
        self.sfx_level = v.min(100);
    }
    pub fn effects_volume(&self) -> u16 {
        self.sfx_level
    }
    pub fn set_music_volume(&mut self, v: u16) {
        self.music_level = v.min(100);
    }
    pub fn music_volume(&self) -> u16 {
        self.music_level
    }
    /// Window minimised: both masters read 0 until restored; stored volumes are untouched.
    /// // SPEC: AUDIO_2009.md §2 "Window minimise"
    pub fn set_minimised(&mut self, m: bool) {
        self.minimised = m;
    }
    fn sfx_master(&self) -> f32 {
        if self.minimised {
            0.0
        } else {
            law::master(self.sfx_level)
        }
    }
    fn music_master(&self) -> f32 {
        if self.minimised {
            0.0
        } else {
            law::master(self.music_level)
        }
    }

    // ---- listener (SPEC §5.1) -----------------------------------------------------------------------

    /// Sets the camera the listener follows; `None` = no camera (origin, identity basis).
    pub fn set_camera(&mut self, cam: Option<Listener>) {
        self.camera = cam;
    }

    /// The listener last pushed to the panner.
    pub fn pan_listener(&self) -> Listener {
        self.pan_listener
    }

    /// Sets the highest stage-B level ([`LEVEL_CEILING`] by default; `None` lets the 1.27 through).
    pub fn set_level_ceiling(&mut self, c: Option<f32>) {
        self.level_ceiling = c;
    }
    pub fn level_ceiling(&self) -> Option<f32> {
        self.level_ceiling
    }

    /// Sets the stereo law of the voices ([`PanLaw`]).
    pub fn set_pan_law(&mut self, law: PanLaw) {
        self.pan_law = law;
    }
    pub fn pan_law(&self) -> PanLaw {
        self.pan_law
    }
    /// `(left, right)` pan gains a voice has against the pushed listener; `(1, 1)` for a `Local` voice.
    pub fn pan_of(&self, id: VoiceId) -> Option<(f32, f32)> {
        self.idx(id).map(|i| {
            let v = &self.voices[i];
            match v.req.kind {
                VoiceKind::Local => self.pan_law.local(),
                _ => self.pan_law.gains(&self.pan_listener, v.req.position),
            }
        })
    }

    // ---- voices -------------------------------------------------------------------------------------

    pub fn active_voices(&self) -> usize {
        self.voices.len()
    }
    pub fn free_handles(&self) -> usize {
        self.free_handles
    }
    /// Voices that currently hold a handle and are audible candidates.
    pub fn playing_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.playing()).count()
    }

    /// Start sequence. // SPEC: AUDIO_2009.md §3.4
    pub fn play(&mut self, req: PlayRequest) -> Result<VoiceId, PlayError> {
        if self.voices.len() >= MAX_VOICES {
            return Err(PlayError::ActiveListFull);
        }
        let id = VoiceId(self.next_id);
        self.next_id += 1;
        let has_handle = self.free_handles > 0;
        if has_handle {
            self.free_handles -= 1;
        }
        let wet = has_handle && self.reverb.active && req.reverb;
        let mut v = Voice {
            id,
            radii: Radii::new(req.inner_radius, req.outer_radius),
            a: 1.0,
            has_handle,
            wet,
            pos: 0.0,
            finished: false,
            pause_count: 0,
            cut: false,
            req,
        };
        if v.req.kind != VoiceKind::Local {
            self.apply_law(&mut v);
        }
        self.voices.push(v);
        Ok(id)
    }

    fn apply_law(&self, v: &mut Voice) {
        let d2 = dist2(self.law_pos, v.req.position);
        v.a = law::stage_a(v.a, d2, v.radii, v.req.volume);
        let max = v.req.outer_radius + OUTER_MARGIN;
        v.cut = d2 > max * max;
    }

    fn idx(&self, id: VoiceId) -> Option<usize> {
        self.voices.iter().position(|v| v.id == id)
    }

    /// Stops and frees a voice (and its handle).
    pub fn release(&mut self, id: VoiceId) {
        if let Some(i) = self.idx(id) {
            let v = self.voices.remove(i);
            if v.has_handle {
                self.free_handles += 1;
            }
        }
    }

    /// Stops and frees every effect voice (their handles back to the pool); the music player is no voice and keeps
    /// its streams. Returns the voices released.
    // EVIDENCE: engine+0xd8eb0 (every entry of the active instance list gets its stop call), engine+0x19d00 (the
    // world teardown calls it after the world's objects are gone); the music streams are not in that list (the
    // stream calls of AUDIO_2009.md §8.1)
    pub fn release_all(&mut self) -> usize {
        let n = self.voices.len();
        for v in self.voices.drain(..) {
            if v.has_handle {
                self.free_handles += 1;
            }
        }
        n
    }

    /// Voices of the active list that loop (tests, reports).
    pub fn looping_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.req.looped).count()
    }

    /// Nested pause. // SPEC: AUDIO_2009.md §3.7
    pub fn pause(&mut self, id: VoiceId) {
        if let Some(i) = self.idx(id) {
            self.voices[i].pause_count += 1;
        }
    }
    pub fn resume(&mut self, id: VoiceId) {
        if let Some(i) = self.idx(id) {
            let v = &mut self.voices[i];
            v.pause_count = v.pause_count.saturating_sub(1);
        }
    }
    pub fn is_finished(&self, id: VoiceId) -> bool {
        self.idx(id)
            .map(|i| self.voices[i].finished)
            .unwrap_or(true)
    }
    /// Moves a `Positional` voice; the next update applies it. // SPEC: AUDIO_2009.md §6
    pub fn set_voice_position(&mut self, id: VoiceId, p: [f32; 3]) {
        if let Some(i) = self.idx(id) {
            if self.voices[i].req.kind == VoiceKind::Positional {
                self.voices[i].req.position = p;
            }
        }
    }
    /// Current stage A of a voice (tests, UI).
    pub fn stage_a_of(&self, id: VoiceId) -> Option<f32> {
        self.idx(id).map(|i| self.voices[i].a)
    }
    /// Total linear gain a voice has right now (before pan): `A * B * C`, 0 when cut or silent.
    pub fn gain_of(&self, id: VoiceId) -> Option<f32> {
        self.idx(id).map(|i| self.voice_gain(&self.voices[i]))
    }
    /// Whether a voice was granted the reverb wet send.
    pub fn wet_of(&self, id: VoiceId) -> Option<bool> {
        self.idx(id).map(|i| self.voices[i].wet)
    }

    fn voice_gain(&self, v: &Voice) -> f32 {
        if !v.playing() || v.cut {
            return 0.0;
        }
        // Stage B is recomputed here, which is what the shell's "re-apply volumes" call does; the spec
        // leaves the first application of B to a new voice open (§13.1), we apply it from the start.
        let b = self
            .classes
            .level(v.req.volume, v.req.class as usize, self.sfx_level);
        let b = self.level_ceiling.map_or(b, |c| b.min(c));
        law::voice_gain(v.a, b, self.sfx_master())
    }

    // ---- update (call once per game frame) -----------------------------------------------------------

    /// Per-update step. `now_s` is a monotonic clock in seconds. // SPEC: AUDIO_2009.md §5.1, §5.3, §3.7
    pub fn update(&mut self, now_s: f64) {
        // Listener: camera or the default basis.
        let cam = self.camera.unwrap_or_default();
        self.law_pos = cam.pos;
        // A clock that went back (the match clock restarts with every match while the mixer lives on across them)
        // pushes at once: otherwise the panner keeps the previous match's listener until the new clock catches up.
        // DC-DEFAULT: CLI-1540 (proposed) the 100 ms throttle re-arms when the update clock goes backwards
        if now_s >= self.next_push_s || now_s + LISTENER_PUSH_S < self.next_push_s {
            self.pan_listener = cam;
            self.next_push_s = now_s + LISTENER_PUSH_S;
        }
        // Voice loop: distance law for playing 3D voices; completion handling.
        for i in 0..self.voices.len() {
            let mut v = std::mem::replace(&mut self.voices[i], dummy_voice());
            if v.req.kind != VoiceKind::Local && v.playing() {
                self.apply_law(&mut v);
            }
            self.voices[i] = v;
        }
        self.release_finished();
        self.bgm.update(now_s);
    }

    /// The completion half of [`Mixer::update`] alone: finished voices without a kept handle leave the active
    /// list and give their handle back. A loop without game frames (the front end) calls this so its interface
    /// sounds do not fill the list (no clock, no listener step).
    pub fn release_finished(&mut self) {
        let mut i = 0;
        while i < self.voices.len() {
            let v = &self.voices[i];
            if v.finished && !v.req.want_handle {
                if v.has_handle {
                    self.free_handles += 1;
                }
                self.voices.remove(i);
            } else {
                i += 1;
            }
        }
    }

    // ---- rendering -----------------------------------------------------------------------------------

    /// Mixes `frames` stereo frames into `out` (interleaved L/R, `out.len() >= 2 * frames`), overwriting it.
    /// Effects and music are summed after their masters and clipped to [-1, 1].
    /// // SPEC: AUDIO_2009.md §12 items 2, 5
    pub fn render(&mut self, out: &mut [f32], frames: usize) {
        let out = &mut out[..2 * frames];
        out.fill(0.0);
        let pan_l = self.pan_listener;
        let law = self.pan_law;
        let rate = self.out_rate;
        // The gains are collected into a kept buffer: `render` runs in the device callback and should not allocate
        // once the buffers have grown.
        let mut gains = std::mem::take(&mut self.scratch_gains);
        gains.clear();
        gains.extend(self.voices.iter().map(|v| self.voice_gain(v)));
        for (v, &g) in self.voices.iter_mut().zip(gains.iter()) {
            if g == 0.0 {
                // A silent voice still advances in time so that it finishes when it would have.
                if v.playing() {
                    advance_only(v, frames, rate);
                }
                continue;
            }
            let (pl, pr) = match v.req.kind {
                VoiceKind::Local => law.local(),
                _ => law.gains(&pan_l, v.req.position),
            };
            mix_voice(v, out, frames, rate, g, pl, pr);
        }
        self.scratch_gains = gains;
        if self.music_master() > 0.0 || self.bgm.is_playing() {
            self.scratch_music.clear();
            self.scratch_music.resize(2 * frames, 0.0);
            self.bgm
                .render_add_at(&mut self.scratch_music, frames, rate);
            let m = self.music_master();
            for (o, s) in out.iter_mut().zip(self.scratch_music.iter()) {
                *o += s * m;
            }
        }
        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
    }
}

fn dummy_voice() -> Voice {
    Voice {
        id: VoiceId(0),
        req: PlayRequest::local(Arc::new(Pcm {
            rate: OUTPUT_RATE,
            channels: 1,
            samples: Vec::new(),
        })),
        radii: Radii::new(0.0, 0.0),
        a: 1.0,
        has_handle: false,
        wet: false,
        pos: 0.0,
        finished: true,
        pause_count: 0,
        cut: false,
    }
}

/// The stereo pan law of the 3D voices. The 2009 engine hands each 3D voice to the mixer with a position, the
/// listener basis, `min = inner`, `max = outer + 0.01`, rolloff 0.0, pan 0.5 and no velocity, cone or 3D provider
/// call (SPEC AUDIO_2009.md §1, §5.2, §5.4); the mixer's own spatialiser that turns this into two speaker levels is
/// internal to the third-party DLL and not reimplemented (UNPROVEN). So the law is ours:
/// constant power on the listener's right axis (`L^2 + R^2 = 2 centre^2`; a centred voice and every 2D voice at
/// `centre` on both ears), the quiet ear never below `floor` times the centre level (a sound at 90 degrees:
/// `floor` and `sqrt(2 - floor^2)` times `centre`), and the side share shrinking to the centre inside `near` units
/// (an emitter at the listener does not flip from ear to ear). Behind is mirrored onto the front (no front / back
/// cue, no HRTF), above and below pan to the centre. [`PanLaw::HARD`] is the law before CLI-200.
// DC-DEFAULT: CLI-200 (proposed) constant-power pan, centre 1/sqrt(2), floor 0.3, near 48 units, see DC_DEFAULTS.md
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanLaw {
    /// Level of the quiet ear for a sound at 90 degrees, relative to the centre (0..=1).
    pub floor: f32,
    /// Distance (world units) below which the side share is scaled by `distance / near`.
    pub near: f32,
    /// Level of both ears for a centred voice and for every `Local` voice (the mixer's centre pan 0.5).
    pub centre: f32,
}

/// The default quiet-ear floor of [`PanLaw`]. // DC-DEFAULT: CLI-200 (proposed)
pub const PAN_FLOOR: f32 = 0.3;
/// The default near distance of [`PanLaw`]. // DC-DEFAULT: CLI-200 (proposed)
pub const PAN_NEAR: f32 = 48.0;
/// The default centre level of [`PanLaw`]: the equal-power centre `1 / sqrt(2)`, for the 2D voices as well.
// DC-DEFAULT: CLI-200 (proposed) the 2009 engine sets every voice to pan 0.5 (engine+0xdc190 start, engine+0xdb750
// distance law); the mixer's level per ear at that pan is internal (UNPROVEN), ours is the equal-power centre
pub const PAN_CENTRE: f32 = std::f32::consts::FRAC_1_SQRT_2;

impl Default for PanLaw {
    fn default() -> Self {
        PanLaw {
            floor: PAN_FLOOR,
            near: PAN_NEAR,
            centre: PAN_CENTRE,
        }
    }
}

impl PanLaw {
    /// The hard law the mixer had before CLI-200: full equal-power swing, one ear silent at 90 degrees.
    pub const HARD: PanLaw = PanLaw {
        floor: 0.0,
        near: 0.0,
        centre: 1.0,
    };

    /// `(left, right)` gains of a `Local` voice: the centre level on both ears.
    pub fn local(&self) -> (f32, f32) {
        (self.centre, self.centre)
    }

    /// `(left, right)` gains for an emitter at `p` heard by `l`; `(centre, centre)` straight ahead, `L^2 + R^2 =
    /// 2 centre^2` everywhere.
    pub fn gains(&self, l: &Listener, p: [f32; 3]) -> (f32, f32) {
        let d = [p[0] - l.pos[0], p[1] - l.pos[1], p[2] - l.pos[2]];
        let len = dot(d, d).sqrt();
        if len < 1e-4 {
            return self.local();
        }
        let mut x = (dot(d, l.right) / len).clamp(-1.0, 1.0);
        if self.near > 0.0 && len < self.near {
            x *= len / self.near;
        }
        let q = std::f32::consts::FRAC_PI_4;
        let floor = self.floor.clamp(0.0, 1.0);
        // the quiet ear's angle at |x| = 1: sqrt(2) cos(theta_max) = floor
        let theta_max = (floor / std::f32::consts::SQRT_2).acos();
        let theta = q + x * (theta_max - q);
        let k = std::f32::consts::SQRT_2 * self.centre;
        (theta.cos().max(0.0) * k, theta.sin().max(0.0) * k)
    }
}

/// Pan gains for a 3D voice relative to the pushed listener with the default law ([`PanLaw::default`]).
pub fn pan_gains(l: &Listener, p: [f32; 3]) -> (f32, f32) {
    PanLaw::default().gains(l, p)
}

fn advance_only(v: &mut Voice, frames: usize, out_rate: u32) {
    let n = v.req.pcm.frames();
    if n == 0 {
        v.finished = true;
        return;
    }
    let step = f64::from(v.req.pcm.rate) / f64::from(out_rate);
    v.pos += step * frames as f64;
    if v.pos >= n as f64 {
        if v.req.looped {
            v.pos %= n as f64;
        } else {
            v.finished = true;
        }
    }
}

fn mix_voice(
    v: &mut Voice,
    out: &mut [f32],
    frames: usize,
    out_rate: u32,
    gain: f32,
    pl: f32,
    pr: f32,
) {
    let pcm = v.req.pcm.clone();
    let n = pcm.frames();
    if n == 0 {
        v.finished = true;
        return;
    }
    let step = f64::from(pcm.rate) / f64::from(out_rate);
    let three_d = v.req.kind != VoiceKind::Local;
    for f in 0..frames {
        let mut i = v.pos as usize;
        if i >= n {
            if v.req.looped {
                v.pos %= n as f64;
                i = v.pos as usize;
            } else {
                v.finished = true;
                return;
            }
        }
        let frac = (v.pos - i as f64) as f32;
        let j = if i + 1 < n {
            i + 1
        } else if v.req.looped {
            0
        } else {
            i
        };
        let (l, r) = if three_d {
            let m0 = pcm.frame_mono(i);
            let m1 = pcm.frame_mono(j);
            let m = m0 + (m1 - m0) * frac;
            (m, m)
        } else {
            let (l0, r0) = pcm.frame_lr(i);
            let (l1, r1) = pcm.frame_lr(j);
            (l0 + (l1 - l0) * frac, r0 + (r1 - r0) * frac)
        };
        out[2 * f] += l * gain * pl;
        out[2 * f + 1] += r * gain * pr;
        v.pos += step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dc(len: usize, level: f32) -> Arc<Pcm> {
        Arc::new(Pcm {
            rate: OUTPUT_RATE,
            channels: 1,
            samples: vec![level; len],
        })
    }

    #[test]
    fn local_voice_pipeline_levels() {
        // the engine pipeline itself (SPEC §4.1): no level ceiling, centre pan at unity
        let mut m = Mixer::new();
        m.set_level_ceiling(None);
        m.set_pan_law(PanLaw::HARD);
        m.classes.set_gain(0, 1.0);
        let id = m.play(PlayRequest::local(dc(1000, 0.5))).unwrap();
        // class 0, fx 100: B = 1.27, C = 1.0 -> 1.27
        assert!((m.gain_of(id).unwrap() - 1.27).abs() < 1e-5);
        m.set_effects_volume(50);
        // B: floor(100*1*0.5)=50 -> 63 -> 0.63; C 0.5 -> 0.315
        assert!((m.gain_of(id).unwrap() - 0.315).abs() < 1e-5);
        let mut out = vec![0.0; 20];
        m.render(&mut out, 10);
        assert!((out[0] - 0.5 * 0.315).abs() < 1e-5);
        assert_eq!(out[0], out[1]);
    }

    #[test]
    fn default_level_ceiling_and_centre() {
        // CLI-202: a level above 1.0 saturates; CLI-200: a 2D voice plays at the equal-power centre on both ears
        let mut m = Mixer::new();
        assert_eq!(m.level_ceiling(), Some(LEVEL_CEILING));
        m.classes.set_gain(0, 1.0);
        let id = m.play(PlayRequest::local(dc(1000, 0.5))).unwrap();
        assert!(
            (m.gain_of(id).unwrap() - 1.0).abs() < 1e-6,
            "1.27 saturates at 1.0"
        );
        m.classes.set_gain(0, 0.75);
        assert!(
            (m.gain_of(id).unwrap() - 0.95).abs() < 1e-6,
            "0.95 is below the ceiling"
        );
        let mut out = vec![0.0; 20];
        m.render(&mut out, 10);
        assert!(
            (out[0] - 0.5 * 0.95 * PAN_CENTRE).abs() < 1e-5,
            "{}",
            out[0]
        );
        assert_eq!(out[0], out[1]);
        assert_eq!(m.pan_of(id), Some((PAN_CENTRE, PAN_CENTRE)));
        // effects volume 50 stays below the ceiling: B 0.46 (0.75 class), C 0.5
        m.set_effects_volume(50);
        assert!((m.gain_of(id).unwrap() - 0.46 * 0.5).abs() < 1e-5);
    }

    #[test]
    fn output_is_clipped() {
        let mut m = Mixer::new();
        m.set_level_ceiling(None);
        m.set_pan_law(PanLaw::HARD);
        m.play(PlayRequest::local(dc(100, 0.9))).unwrap();
        let mut out = vec![0.0; 20];
        m.render(&mut out, 10);
        assert_eq!(out[0], 1.0); // 0.9 * 1.27 > 1
    }

    #[test]
    fn positional_law_and_cutoff() {
        let mut m = Mixer::new();
        m.set_camera(Some(Listener::default()));
        let id = m
            .play(PlayRequest::positional(
                dc(100000, 0.1),
                [0.0, 0.0, 6.0],
                2.0,
                10.0,
            ))
            .unwrap();
        // d2 = 36: inside (4, 100): A = (1 - 32/64) * 1 = 0.5
        assert!((m.stage_a_of(id).unwrap() - 0.5).abs() < 1e-6);
        m.update(0.0);
        assert!((m.stage_a_of(id).unwrap() - 0.5).abs() < 1e-6);
        // move to d = 9.5: d2 = 90.25, A = 1 - 86.25/64 < 0 -> gain clamps to 0
        m.set_voice_position(id, [0.0, 0.0, 9.5]);
        m.update(0.1);
        assert!(m.stage_a_of(id).unwrap() < 0.0);
        assert_eq!(m.gain_of(id).unwrap(), 0.0);
        // beyond outer + 0.01: A keeps the last value, voice is cut
        m.set_voice_position(id, [0.0, 0.0, 11.0]);
        m.update(0.2);
        assert!(m.stage_a_of(id).unwrap() < 0.0);
        assert_eq!(m.gain_of(id).unwrap(), 0.0);
        // back inside the inner radius: A unchanged (no update), still the old value
        m.set_voice_position(id, [0.0, 0.0, 1.0]);
        let a = m.stage_a_of(id).unwrap();
        m.update(0.3);
        assert_eq!(m.stage_a_of(id).unwrap(), a);
    }

    #[test]
    fn start_outside_interval_plays_at_unity_a() {
        let mut m = Mixer::new();
        let id = m
            .play(PlayRequest::positional(
                dc(1000, 0.1),
                [0.0, 0.0, 1.0],
                2.0,
                10.0,
            ))
            .unwrap();
        assert_eq!(m.stage_a_of(id), Some(1.0));
    }

    #[test]
    fn a_clock_that_restarts_pushes_the_listener_at_once() {
        // the mixer outlives the match; the next match's clock starts at 0 again
        let mut m = Mixer::new();
        m.set_camera(Some(Listener::from_camera(
            [5000.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        )));
        m.update(600.0);
        assert_eq!(m.pan_listener().pos, [5000.0, 0.0, 0.0]);
        m.set_camera(Some(Listener::default()));
        m.update(0.0);
        assert_eq!(
            m.pan_listener(),
            Listener::default(),
            "pushed on the restarted clock"
        );
        // and the throttle runs on the new clock
        m.set_camera(Some(Listener::from_camera(
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0],
        )));
        m.update(0.05);
        assert_eq!(m.pan_listener(), Listener::default());
        m.update(0.1);
        assert_eq!(m.pan_listener().pos, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn pan_follows_listener_right_axis() {
        let mut m = Mixer::new();
        m.set_camera(Some(Listener::default()));
        m.update(0.0);
        m.play(PlayRequest::positional(
            dc(1000, 0.1),
            [150.0, 0.0, 0.0],
            200.0,
            400.0,
        ))
        .unwrap();
        let mut out = vec![0.0; 20];
        m.render(&mut out, 10);
        // CLI-200: the right ear louder, the left one at the floor (not silent)
        assert!(
            out[1] > out[0] * 4.0 && out[0] > 0.0,
            "right ear louder: {} {}",
            out[0],
            out[1]
        );
        // Listener push is throttled to 10 Hz: a camera turn within 100 ms is not seen by the panner.
        m.set_camera(Some(Listener::from_camera(
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        )));
        m.update(0.05);
        assert_eq!(m.pan_listener().forward, [0.0, 0.0, 1.0]);
        m.update(0.11);
        assert!((m.pan_listener().forward[0] - 1.0).abs() < 1e-6);
    }

    /// An emitter `d` units from the default listener (forward +Z, right +X) at `deg` degrees clockwise from
    /// the front (90 = right, 270 = left).
    fn at(deg: f32, d: f32) -> [f32; 3] {
        let r = deg.to_radians();
        [r.sin() * d, 0.0, r.cos() * d]
    }

    #[test]
    fn pan_law_front_side_back_and_floor() {
        let law = PanLaw::default();
        let l = Listener::default();
        let c = PAN_CENTRE;
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        // front and back: the centre level on both ears
        for deg in [0.0, 180.0] {
            let (pl, pr) = law.gains(&l, at(deg, 500.0));
            assert!(close(pl, c) && close(pr, c), "{deg}: {pl} {pr}");
        }
        // right and left: the quiet ear stays at the floor, the loud ear keeps the power (at most unity)
        let quiet = PAN_FLOOR * c;
        let loud = (2.0 - PAN_FLOOR * PAN_FLOOR).sqrt() * c;
        assert!(loud <= 1.0);
        let (pl, pr) = law.gains(&l, at(90.0, 500.0));
        assert!(close(pl, quiet) && close(pr, loud), "{pl} {pr}");
        let (pl, pr) = law.gains(&l, at(270.0, 500.0));
        assert!(close(pl, loud) && close(pr, quiet), "{pl} {pr}");
        // constant power and never below the floor at any angle and distance beyond `near`
        for deg in (0..360).step_by(15) {
            for d in [60.0, 300.0, 2000.0] {
                let (pl, pr) = law.gains(&l, at(deg as f32, d));
                assert!(close(pl * pl + pr * pr, 2.0 * c * c), "{deg} {d}");
                assert!(pl >= quiet - 1e-4 && pr >= quiet - 1e-4, "{deg} {d}");
            }
        }
        // 45 degrees right: between the centre and the side
        let (pl, pr) = law.gains(&l, at(45.0, 500.0));
        assert!(pl < c && pl > quiet && pr > c && pr < loud, "{pl} {pr}");
        // above the listener: centre
        let (pl, pr) = law.gains(&l, [0.0, 300.0, 0.0]);
        assert!(close(pl, c) && close(pr, c));
        // a unity centre: the same law scaled
        let wide = PanLaw { centre: 1.0, ..law };
        let (pl, pr) = wide.gains(&l, at(90.0, 500.0));
        assert!(close(pl, PAN_FLOOR) && close(pr, loud / c), "{pl} {pr}");
    }

    #[test]
    fn pan_law_narrows_near_the_listener_and_hard_is_the_old_law() {
        let law = PanLaw::default();
        let l = Listener::default();
        // at half the near distance to the right: half of the side swing
        let c = PAN_CENTRE;
        let (pl, pr) = law.gains(&l, at(90.0, PAN_NEAR * 0.5));
        let (fl, fr) = law.gains(&l, at(90.0, 1000.0));
        assert!(pl > fl && pl < c && pr < fr && pr > c, "{pl} {pr}");
        // at the listener: centre
        assert_eq!(law.gains(&l, [0.0; 3]), (c, c));
        // the hard law: one ear silent at 90 degrees, centre unity
        let (hl, hr) = PanLaw::HARD.gains(&l, at(90.0, 5.0));
        assert!(hl.abs() < 1e-6 && (hr - std::f32::consts::SQRT_2).abs() < 1e-5);
        let (hl, hr) = PanLaw::HARD.gains(&l, at(0.0, 5.0));
        assert!((hl - 1.0).abs() < 1e-5 && (hr - 1.0).abs() < 1e-5);
        // the mixer renders with its law and reports it per voice
        let mut m = Mixer::new();
        m.set_camera(Some(Listener::default()));
        m.update(0.0);
        let id = m
            .play(PlayRequest::positional(
                dc(1000, 0.1),
                at(90.0, 150.0),
                100.0,
                400.0,
            ))
            .unwrap();
        let (vl, vr) = m.pan_of(id).unwrap();
        assert!((vl - PAN_FLOOR * c).abs() < 1e-4 && vr > c);
        let mut out = vec![0.0; 20];
        m.render(&mut out, 10);
        assert!(out[0] > 0.0, "the left ear is not silent: {}", out[0]);
        assert!((out[0] / out[1] - vl / vr).abs() < 1e-4);
        m.set_pan_law(PanLaw::HARD);
        assert_eq!(m.pan_law(), PanLaw::HARD);
        m.render(&mut out, 10);
        assert!(out[0].abs() < 1e-6, "hard law: left silent");
        let local = m.play(PlayRequest::local(dc(1000, 0.1))).unwrap();
        assert_eq!(m.pan_of(local), Some((1.0, 1.0)));
    }

    #[test]
    fn voice_pool_exhaustion() {
        let mut m = Mixer::with_handles(3);
        let ids: Vec<_> = (0..5)
            .map(|_| m.play(PlayRequest::local(dc(10000, 0.1))).unwrap())
            .collect();
        assert_eq!(m.active_voices(), 5);
        assert_eq!(m.playing_voices(), 3);
        assert_eq!(m.gain_of(ids[4]), Some(0.0)); // tracked but silent
        m.release(ids[0]);
        assert_eq!(m.free_handles(), 1);
        // active list cap
        let mut m = Mixer::new();
        for _ in 0..MAX_VOICES {
            m.play(PlayRequest::local(dc(100, 0.1))).unwrap();
        }
        assert_eq!(m.free_handles(), 0);
        assert!(matches!(
            m.play(PlayRequest::local(dc(100, 0.1))),
            Err(PlayError::ActiveListFull)
        ));
    }

    #[test]
    fn finished_voices_release_unless_handle_kept() {
        let mut m = Mixer::new();
        let a = m.play(PlayRequest::local(dc(10, 0.1))).unwrap();
        let mut req = PlayRequest::local(dc(10, 0.1));
        req.want_handle = true;
        let b = m.play(req).unwrap();
        let mut out = vec![0.0; 200];
        m.render(&mut out, 100);
        m.update(0.0);
        assert!(m.is_finished(a)); // gone
        assert_eq!(m.active_voices(), 1);
        assert!(m.is_finished(b));
        m.release(b);
        assert_eq!(m.free_handles(), MAX_VOICES);
    }

    /// leave-match-audio: every voice, looped, kept-handle or plain, leaves the list and gives its handle back.
    #[test]
    fn release_all_frees_every_voice_and_handle() {
        let mut m = Mixer::new();
        let mut looped = PlayRequest::local(dc(10, 0.1));
        looped.looped = true;
        m.play(looped).unwrap();
        let mut kept = PlayRequest::local(dc(10, 0.1));
        kept.want_handle = true;
        m.play(kept).unwrap();
        m.play(PlayRequest::local(dc(100_000, 0.1))).unwrap();
        assert_eq!((m.active_voices(), m.looping_voices()), (3, 1));
        assert_eq!(m.release_all(), 3);
        assert_eq!((m.active_voices(), m.looping_voices()), (0, 0));
        assert_eq!(m.free_handles(), MAX_VOICES);
        assert_eq!(m.release_all(), 0);
    }

    #[test]
    fn minimise_mutes_and_restores() {
        let mut m = Mixer::new();
        m.set_music_volume(70);
        m.play(PlayRequest::local(dc(1000, 0.1))).unwrap();
        m.set_minimised(true);
        let mut out = vec![0.0; 20];
        m.render(&mut out, 10);
        assert!(out.iter().all(|s| *s == 0.0));
        m.set_minimised(false);
        assert_eq!(m.music_volume(), 70);
        m.render(&mut out, 10);
        assert!(out[0] > 0.0);
    }

    #[test]
    fn reverb_wet_only_if_active_at_start() {
        let mut m = Mixer::new();
        let a = m.play(PlayRequest::local(dc(100, 0.1))).unwrap();
        m.reverb.select = crate::reverb::ReverbSelect::Preset(0);
        m.reverb.enable(true);
        let b = m.play(PlayRequest::local(dc(100, 0.1))).unwrap();
        let mut req = PlayRequest::local(dc(100, 0.1));
        req.reverb = false;
        let c = m.play(req).unwrap();
        assert_eq!(
            (m.wet_of(a), m.wet_of(b), m.wet_of(c)),
            (Some(false), Some(true), Some(false))
        );
    }

    #[test]
    fn resamples_22k_to_44k() {
        let mut m = Mixer::new();
        let pcm = Arc::new(Pcm {
            rate: 22050,
            channels: 1,
            samples: vec![0.1; 50],
        });
        let id = m.play(PlayRequest::local(pcm)).unwrap();
        let mut out = vec![0.0; 2 * 150];
        m.render(&mut out, 150);
        // 50 source frames = 100 output frames
        assert!(out[2 * 90] != 0.0);
        assert_eq!(out[2 * 120], 0.0);
        m.update(0.0);
        assert!(m.is_finished(id));
    }
}
