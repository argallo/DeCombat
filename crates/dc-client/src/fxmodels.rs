//! The drawn instances of the effect models ([`crate::fxkeys::FxModel`]): the pieces of a breaking crate, the drum a
//! drum's death group launches. [`crate::characters::ModelScene::update_fx_models`] loads each model through the
//! scene's upload cache and keeps one instance per live model key; this set holds them, places them every frame and
//! forgets those whose key ended.
// DC-DEFAULT: CLI-362 (proposed), see crate::fx::FxRunner::models_at

use std::collections::{BTreeMap, BTreeSet};

use dc_render::{AnimSet, LightSet, ModelInstance, Tracker};
use glam::Vec3;

use crate::fxkeys::FxModel;
use crate::world::ModelRef;

/// The live effect-model instances.
#[derive(Default)]
pub struct FxModelSet {
    live: BTreeMap<u64, (ModelRef, ModelInstance)>,
    failed: BTreeSet<u64>,
    /// Effect models drawn in the last update.
    pub drawn: u32,
}

impl FxModelSet {
    /// gpu-recover: the instances of a lost GPU device are put away; the effect models upload again.
    pub fn gpu_reset(&mut self) {
        crate::gpulost::retire(std::mem::take(&mut self.live));
        self.failed.clear();
    }
}

impl std::fmt::Debug for FxModelSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FxModelSet")
            .field("live", &self.live.len())
            .field("failed", &self.failed.len())
            .field("drawn", &self.drawn)
            .finish()
    }
}

impl FxModelSet {
    /// Forgets the instances whose key is no longer in `list`.
    pub fn keep(&mut self, list: &[FxModel]) {
        let has = |k: &u64| list.iter().any(|m| m.id == *k);
        self.live.retain(|k, _| has(k));
        self.failed.retain(has);
        self.drawn = 0;
    }

    /// True when the key has no instance of its model yet (and its model did not fail before).
    pub fn needs(&self, m: &FxModel) -> bool {
        !self.failed.contains(&m.id) && self.live.get(&m.id).is_none_or(|l| l.0 != m.model)
    }

    pub fn insert(&mut self, m: &FxModel, inst: ModelInstance) {
        self.live.insert(m.id, (m.model.clone(), inst));
    }

    pub fn fail(&mut self, id: u64) {
        self.live.remove(&id);
        self.failed.insert(id);
    }

    /// Places one model: its origin at the key's place, the key's rotation, posed by its first clip at the key's
    /// clip time ([`key_trackers`]; the bind pose for a model without clips, like the ROM's breaking pieces), lit at
    /// the middle of its drawn bounds (of the posed vertices when a clip moves it).
    // DC-DEFAULT: CLI-362 (proposed) the light set is taken at the middle of the model's bounds (the anchor of a crate
    // piece lies on the floor, where the sun test of the light set saw no sky and the piece drew black)
    pub fn place(&mut self, m: &FxModel, lights: &dyn Fn(Vec3, f32) -> LightSet) {
        let Some((_, inst)) = self.live.get_mut(&m.id) else {
            return;
        };
        inst.position = m.pos;
        inst.rotation = m.rot;
        let tr = key_trackers(m, &inst.anim);
        inst.set_pose(&tr);
        let bounds = if tr.is_empty() {
            inst.bounds.map(|(lo, hi)| (Vec3::from(lo), Vec3::from(hi)))
        } else {
            inst.posed_bounds()
        };
        let mid = bounds.map_or(Vec3::ZERO, |(lo, hi)| (lo + hi) * 0.5);
        inst.lights = lights(m.pos + m.rot * mid, inst.radius);
        self.drawn += 1;
    }

    /// The instances to draw.
    pub fn instances(&self) -> impl Iterator<Item = &ModelInstance> {
        self.live.values().map(|l| &l.1)
    }
}

/// The trackers a model key poses its model with: its first clip at the key's clip time ([`crate::fxkeys::ModelAnim`]),
/// none (the bind pose) for a model without clips. Without this the 2009 train of JUNK_FLEA (a 6,000-unit model whose
/// clip carries it along the track 2,600 units away) sat at its bind pose at the effect's origin, its roof showing
/// through the ground by the stairs, lit as underground (black).
// EVIDENCE: clientfx+0x17be0, clientfx+0x177a0 (see ModelAnim::clip_ms)
// DC-DEFAULT: CLI-599 (proposed)
pub fn key_trackers(m: &FxModel, anim: &AnimSet) -> Vec<Tracker> {
    match anim.clip_length_ms(0) {
        Some(len) if len > 0 => vec![Tracker::main(0, m.anim.clip_ms(m.elapsed, m.life, len))],
        _ => Vec::new(),
    }
}
