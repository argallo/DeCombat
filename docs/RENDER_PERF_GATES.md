# Render performance programme: bounded budgets and acceptance gates

This document is
binding for the dc-render / dc-client performance track. Direction is approved; implementation is NOT approved until
the budget ledger is filled from measured data (G1), frozen, and each gate is pass / fail. No optimization phase
merges on narrative evidence.

Provenance: ORIGINAL (our own engineering rules; no 2009 behaviour claims).

---

Approved as direction — implementation is **not** approved until the budget ledger below is filled, frozen, and each
gate is pass/fail. No optimization phase merges on narrative evidence.

## Rule

Every subsystem operates inside a declared bound. Exceeding a bound must trigger a defined, deterministic degradation
(defer, evict, use resident mip, drop by scripted rule) — never unbounded queue growth, allocation, or pipeline
creation. High-water marks are reported in telemetry and test output; CI fails if a high-water mark exceeds its
budget. Waivers require a written exception + correctness review, not an automatic rebaseline.

## Bounded Resource Budget Ledger — freeze before coding

Fill from min-spec target after Gate 1 baseline. Values left blank block implementation.

| Domain | Bounded quantity | Cap to pin | Overflow behavior |
|---|---|---|---|
| System RAM | Archive index, decoded-texture LRU, decompression scratch pool, streaming request queue | `___ MB` each, `___` max queued requests | Evict LRU / reject+placeholder, no growth |
| GPU static | BSP VB/IB, lightmap array, resident textures per level | `___ MB` | Level load fails explicitly |
| GPU dynamic/frame | Packed frame records, bone palettes, particle instances, HUD quads | `___ MB/frame`, `___` max bones/particles/quads | Hard caps in code; spawn/update clamps by ID order |
| Staging in-flight | Recycled chunks outstanding, bytes awaiting fence | `___ MB`, `___` chunks | Uploads deferred to next frame by priority |
| Upload traffic | Bytes copied/frame, copy ops/frame | `p95 ___ MB`, hard cap `___ MB` | Non-critical mips/uploads defer |
| CPU frame | Parse, decode jobs, pack, encode/submit | `p95 ___ ms`, `p99 ___ ms`, `0` hot-loop allocations | Decode thread pool + job queue capped at `___` |
| GPU frame | Per-pass timestamp time, total | `p95/p99 ___ ms` per pass on min-spec | Gate failure, not a tuning note |
| Pipelines | Pipelines per level, load-time compile, runtime creates | `___` max, `___ ms` load budget, `0` runtime creates | Unmapped material → fallback pipeline |
| Draw/bind state | Draws, bind-group switches, dynamic offsets / frame | Diagnostic caps `___/___/___` | Gate review if exceeded |
| Streaming | Time-to-resident, hitch attributable to streaming | `p95 ___ frames`, no frame `> ___ ms` from uploads | Scripted placeholder remains |

## Acceptance Gates

**G0 — Correctness contract frozen (blocks all work)**
Manifest records asset hashes, shader inputs, render settings, software versions, adapter/driver metadata. Tests
driven by explicit ticks + scripted inputs; scheduling-independent RNG streams; deterministic resource
initialization; capture hashes only intended texels (row padding stripped, format/dimensions in contract). Exact lane
vs hardware-coverage lane defined separately.
*Exit:* repeated runs and separate processes produce identical hashes in exact lane; a deliberate
arithmetic/ordering change is detected by the tests.

**G1 — Baseline measured, budgets pinned**
Profile the current renderer on representative stress scenes: worst-case BSP visibility, max skinned characters,
particle stress, HUD stress, cold streaming. Measure CPU parse/I/O/decode/pack/encode, GPU pass times, upload volume,
memory — including p95/p99 tails and hitches, not averages.
*Exit:* every blank in the ledger is filled from measured data + stated headroom and signed off.

**G2 — Resident rendering path**
Packed frame records + few large uploads (`write_buffer`/`write_buffer_with` first), shared geometry buffers with
range allocator, explicit pass ordering with translucency order preserved.
*Exit:* G0 hashes unchanged (or approved visual-change review); CPU/GPU/upload budgets met on G1 scenes; zero runtime
pipeline creates; zero hot-loop allocations.

**G3 — Staging & batching changes (only if G2 data justifies)**
StagingBelt/recycled chunks, BSP range compaction, texture arrays for compatible images (lightmaps first) — each
behind its own measurement.
*Exit:* improvement shown in p95/p99 frame or hitch time, not draw count alone; budgets still held; determinism gate
re-passed.

**G4 — Texture decoding change (separate approval)**
Native BC, integer compute decode, or cached CPU decode — chosen from G1 decode/bandwidth data, decoder equivalence
proven or rebaseline explicitly approved.
*Exit:* texel-level comparison vs current decoder documented; sRGB/alpha/mip/sampler semantics preserved or change
reviewed; memory/upload budget gain measured.

**G5 — Scripted streaming lane**
Scripted visibility/requests/completion ticks, mip transitions, eviction, failure cases. If real I/O misses a
scripted tick, the test waits. Fully-resident lane stays untouched.
*Exit:* logical schedule is machine-speed independent; hitch and time-to-resident budgets met; failure injection
produces placeholder, never crash or uninitialized sampling.

**G6 — Advanced paths (each independently gated, none pre-approved)**
Compute skinning, GPU particles, binding arrays/ (message truncated here)

---

## Gate status log (coordinator keeps this current)

| Gate | Status | Evidence |
|---|---|---|
| G0 | IMPLEMENTED on branch `perf-g0` (not merged; exit criteria met on this machine's i686 debug Vulkan lane) | Contract `docs/RENDER_DETERMINISM_CONTRACT.md`; manifest `dc_client::exact` (ROM read journal `dc_rom::journal`, GPU-object journal `dc_render::{shader_module, render_pipeline, sampler}`); texel hash `dc_render::texel_hash`; 7 exact-lane scenes, 242 hashes (`crates/dc-client/tests/exact_lane.rs`): identical over 3 in-process runs and 3 processes (one under 24 busy threads, one with 3 frames in flight and `DC_PROF_HUDFORCE=1`; profile mode without it composes the HUD only when due, recorded as the input `render_settings.hud_cadence`); `DC_DETERMINISM_MUTATE=arith` / `order` change them; goldens `crates/dc-client/tests/golden/exact_lane.json` written only by `tools/exact_golden.py --update`; content-keyed effect seeds (CLI-430, staged); `tools/pixhash.py` (master = client-fps = its base, pixel-exact on 2 scripted scenes + the CLI run). Report `reports/perf-g0.md` |
| G1 | NOT STARTED (render-profile branch 2026-10-26 07:30 measured means only, no p95 / p99, offscreen only) | |
| G2..G6 | BLOCKED on G0 + G1 | |
