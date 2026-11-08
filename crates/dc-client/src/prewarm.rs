//! gpu-prewarm: the match window makes every GPU pipeline the match can need before its first frame, on the frame
//! thread: the model pipelines and samplers of every ROM render style (the server-announced ones such as the
//! infected styles included), the x-ray pass and the effect pass; the world, sky, HUD, overlay, debug-box and gamma
//! passes are made by their constructors at the same time. It runs at match load and again after an in-match device
//! or renderer rebuild. From then on a pipeline made is logged once as "made in play" ([`dc_render::warm`]); the
//! model loader threads make none (they hand the pipelines a load needs back to the frame thread).
//!
//! Why: the user's driver resets (2026-11-04) came at the first frames that drew newly loaded models, whose pipelines
//! were then made on loader threads while the frame thread submitted. Whether this ends the resets only the user's
//! play can show; on its own it removes first-use pipeline compiles from play.
// ORIGINAL: gpu-prewarm engineering (our own resource timing; no 2009 behaviour)

use std::time::Instant;

use dc_render::{GpuContext, PrewarmCount, WorldRenderer};

use crate::characters::ModelScene;
use crate::fx::FxStage;

/// What [`match_gpu`] made and how long it took.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Prewarm {
    pub models: PrewarmCount,
    /// The effect pass was made now.
    pub fx: bool,
    /// Pipelines and shader modules made in the call (every kind).
    pub creates: u64,
    pub ms: f64,
}

/// A load phase starts (match load, device or renderer rebuild) or the match ends: creates from now on are not
/// "made in play" until [`match_gpu`] ends.
pub fn load_begins() {
    dc_render::warm::end_play();
}

/// Makes the match's on-demand pipelines now with `renderer` (see the module doc) and declares the match in play.
/// `models`: the match's model scene (its ROM style list); `None` makes the built-in styles' pipelines only.
/// `fx` must already have the renderer's sample count ([`FxStage::set_msaa`]); `format` is the effect pass's target.
pub fn match_gpu(
    ctx: &GpuContext,
    renderer: &WorldRenderer,
    models: Option<&ModelScene>,
    fx: &mut FxStage,
    format: wgpu::TextureFormat,
) -> Prewarm {
    let t = Instant::now();
    let before = dc_render::warm::creates();
    dc_render::warm::restart_longest();
    let times0 = dc_render::warm::pipeline_times();
    let counts = match models {
        Some(m) => m.prewarm(ctx, renderer),
        None => renderer.prewarm(ctx, []),
    };
    let fx_made = fx.prewarm(ctx, format);
    let p = Prewarm {
        models: counts,
        fx: fx_made,
        creates: dc_render::warm::creates() - before,
        ms: t.elapsed().as_secs_f64() * 1000.0,
    };
    println!(
        "dc-client: GPU pipelines made before play: {} in {:.1} ms ({} model pipelines and {} samplers for {} render \
         styles, x-ray, effects); later ones are logged as \"pipeline made in play\"",
        p.creates, p.ms, counts.pipelines, counts.samplers, counts.styles
    );
    // CLI-1933: the compile time behind it (summed over the pre-warm threads) and the backend
    let times = dc_render::warm::pipeline_times();
    println!(
        "dc-client: pre-warm pipelines {} ({:?}): {:.0} ms of compiles, the longest {:.0} ms",
        times.count - times0.count,
        ctx.info.backend,
        times.total_ms - times0.total_ms,
        times.max_ms
    );
    dc_render::warm::begin_play();
    p
}
