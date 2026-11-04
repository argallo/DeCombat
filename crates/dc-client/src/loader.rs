//! Model loads away from the frame thread: a small pool of loader threads reads a model file
//! and its child models from the user's ROM, decodes its textures, reads its render styles and uploads the result
//! (mesh, every LOD, textures, pipelines) through a [`ModelUploader`]; the frame thread only sends requests, takes
//! finished models and makes per-object instances of them ([`dc_render::ModelPipelines::instantiate`], a few
//! small uniform buffers).
//!
//! Our own engineering (no 2009 behaviour is involved: the 2009 engine's own resource threading is not modelled).
//! The rules are DC-DEFAULT CLI-260 .. CLI-262 (see the staged DC_DEFAULTS rows): a model is requested once per
//! distinct (file, textures, styles) upload key, the caches are shared by every loader thread and the frame
//! thread, and a blocking mode does the same work inline on the calling thread (the default of the headless and
//! offscreen paths, so their frames and PNGs stay the same as before).
// DC-DEFAULT: CLI-260 (proposed), see DC_DEFAULTS.md

use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

use dc_render::{
    ModelFiles, ModelInstance, ModelLook, ModelUploader, RenderStyle, RomTextures, StyleChoice,
    StyleLibrary, TextureImage, TextureIndex,
};
use dc_rez::RezVfs;

use crate::world::ModelRef;

/// What one model upload is made from: the model file with its texture list, the render-style names of its slots,
/// and the colour a texture that does not resolve becomes (`None`: the missing-texture checker). Two requests with
/// the same key share one upload.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UploadKey {
    pub model: ModelRef,
    pub styles: Vec<String>,
    pub fallback: Option<[u8; 4]>,
}

/// A finished upload: the model as uploaded (never drawn itself: objects draw instances of it) and its files.
#[derive(Clone)]
pub struct Loaded {
    pub template: Arc<ModelInstance>,
    pub files: Arc<ModelFiles>,
}

/// A cache by normalised name; each entry is filled once (a second reader waits for the first).
type Cache<T> = Mutex<HashMap<String, Arc<OnceLock<T>>>>;

/// The ROM caches every loader thread and the frame thread share. Each entry is loaded once: a second thread that
/// asks for an entry being loaded waits for it.
pub struct Shared {
    pub vfs: Arc<RezVfs>,
    pub index: Arc<TextureIndex>,
    files: Cache<Option<Arc<ModelFiles>>>,
    images: Cache<Option<TextureImage>>,
    styles: Mutex<StyleLibrary>,
    /// Model files that did not load (counted once per path).
    pub failures: AtomicU32,
    /// The ROM weapon table a `@camo:` skin reads its base texture's name from (VAR-3).
    base_rows: std::sync::OnceLock<Option<dc_bute::WeaponsTable>>,
}

fn key_of(path: &str) -> String {
    path.trim().replace('/', "\\").to_ascii_uppercase()
}

fn slot<T>(map: &Cache<T>, k: &str) -> Arc<OnceLock<T>> {
    let mut m = dc_hangdump::crumb_wait!("loader: lock the cache map", map.lock())
        .unwrap_or_else(|e| e.into_inner());
    m.entry(k.to_string()).or_default().clone()
}

impl Shared {
    pub fn new(vfs: Arc<RezVfs>, index: Arc<TextureIndex>) -> Shared {
        Shared {
            vfs,
            index,
            files: Mutex::new(HashMap::new()),
            images: Mutex::new(HashMap::new()),
            styles: Mutex::new(StyleLibrary::new()),
            failures: AtomicU32::new(0),
            base_rows: std::sync::OnceLock::new(),
        }
    }

    /// VAR-3: the texture a `@camo:` skin names, painted from the ROM texture of its base row and the pack's pattern.
    /// `None` when any part is missing: the gun then draws with the key's fallback, and the match goes on.
    fn camo_image(&self, c: &crate::camo::CamoPath<'_>) -> Option<TextureImage> {
        if c.mode != "paint" {
            eprintln!("camo: unknown mode {:?}", c.mode);
            return None;
        }
        let rows = self
            .base_rows
            .get_or_init(|| dc_bute::WeaponsTable::from_vfs(&self.vfs).ok());
        let path = rows
            .as_ref()?
            .weapon(c.base_row)?
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(c.base_key))
            .map(|(_, v)| v.trim().trim_matches('"').to_string())?;
        let pattern = self.pattern_image(c.pattern)?;
        let base = self.image(&path)?;
        let mut painted = crate::camo::paint(&base, &pattern, &c.keep)?;
        if let Some(fx) = c.fx {
            let moved = match c.fx_pattern {
                Some(p) => self.pattern_image(p)?,
                None => pattern,
            };
            painted.fx = crate::camo::fx_of(&base, &moved, &c.keep, fx).map(Arc::new);
        }
        Some(painted)
    }

    /// A camo's pattern: a pack's file, else a texture of the ROM.
    fn pattern_image(&self, path: &str) -> Option<TextureImage> {
        match crate::modassets::is_mod_path(path) {
            true => Self::mod_image(path),
            false => self.image(path),
        }
    }

    /// A pack's own texture, from the installed mod layer (the texture index holds the ROM's files only).
    fn mod_image(path: &str) -> Option<TextureImage> {
        let bytes = crate::modassets::read_layered(path)?;
        match dc_tex::DtxFile::parse(&bytes) {
            Ok(file) => dc_render::decode_dtx(&file),
            Err(e) => {
                eprintln!("camo: {path}: {e}");
                None
            }
        }
    }

    /// The model file and its child models, read once per path (`None`: it does not load).
    pub fn files(&self, path: &str) -> Option<Arc<ModelFiles>> {
        // gate T1: a second thread asking for the same path waits here for the first one's read
        let s = slot(&self.files, &key_of(path));
        dc_hangdump::crumb_wait!(
            "loader: model files (once per path; may wait for another loader)",
            s.get_or_init(|| {
                let f = ModelFiles::load(&self.vfs, path).ok().map(Arc::new);
                if f.is_none() {
                    self.failures.fetch_add(1, Ordering::Relaxed);
                }
                f
            })
        )
        .clone()
    }

    /// The model file if it has been read already (`Some(None)`: it did not load); never blocks on a load.
    pub fn files_cached(&self, path: &str) -> Option<Option<Arc<ModelFiles>>> {
        let m = self.files.lock().unwrap_or_else(|e| e.into_inner());
        m.get(&key_of(path)).and_then(|s| s.get().cloned())
    }

    /// The decoded texture of a name, once per name (`None`: empty name, missing or unreadable).
    pub fn image(&self, name: &str) -> Option<TextureImage> {
        if name.is_empty() {
            return None;
        }
        let s = slot(&self.images, &key_of(name));
        dc_hangdump::crumb_wait!(
            "loader: texture image (once per name; may wait for another loader)",
            s.get_or_init(|| {
                let img = match crate::camo::parse(name) {
                    Some(c) => self.camo_image(&c),
                    None => RomTextures::new(&self.index).image(name.as_bytes()),
                };
                // perf-g1: the shared image cache keeps every decoded image (no eviction): a pool gauge
                if let Some(i) = &img {
                    let bytes: usize = i.levels.iter().map(Vec::len).sum();
                    dc_render::perf::gauge(dc_render::perf::Gauge::DecodedImageBytes, bytes as i64);
                }
                img
            })
        )
        .clone()
    }

    /// Reads, decodes and uploads one model with `upload` (a [`ModelUploader`] on a loader thread, the renderer on
    /// the frame thread): the files, the textures (an unresolved one: the key's fallback colour, else the checker),
    /// the render styles. `None` when the model file does not load.
    // SPEC: RENDER_2009.md §7.3 (each piece drawn with the style of its slot); EVIDENCE: engine+0x1b9390 (a missing
    // slot or style: the engine default)
    pub fn build(
        &self,
        key: &UploadKey,
        upload: &dyn Fn(&ModelFiles, ModelLook<'_>) -> ModelInstance,
    ) -> Option<Loaded> {
        let files = self.files(&key.model.model)?;
        let fallback = key.fallback.map(|c| TextureImage::rgba(2, 2, c.repeat(4)));
        let images: Vec<Option<TextureImage>> = key
            .model
            .textures
            .iter()
            .map(|n| self.image(n).or_else(|| fallback.clone()))
            .collect();
        let list: Vec<Option<Arc<RenderStyle>>> =
            dc_hangdump::crumb_wait!("loader: lock the render styles", self.styles.lock())
                .unwrap_or_else(|e| e.into_inner())
                .list(&self.vfs, &key.styles);
        let inst = upload(
            &files,
            ModelLook {
                textures: &images,
                styles: StyleChoice::List(&list),
            },
        );
        Some(Loaded {
            template: Arc::new(inst),
            files,
        })
    }
}

/// A request to the loader threads.
enum Job {
    Upload(UploadKey),
    /// Only the files of a model (for a clip-name map).
    Files(String),
}

/// What a loader thread hands back.
pub enum Done {
    Upload(UploadKey, Option<Loaded>),
    Files(String),
}

/// The loader threads of one device. Dropping it closes the job queue; the threads end after their current job.
pub struct Loader {
    jobs: mpsc::Sender<Job>,
    done: mpsc::Receiver<Done>,
    /// Requests sent and not yet handed back.
    outstanding: usize,
    pub threads: usize,
}

/// How many loader threads: a quarter of the machine's hardware threads, 1 to 3 (a character is up to three model
/// uploads; more threads would only compete with the frame thread). `DC_LOAD_THREADS=N` overrides (1..8).
// DC-DEFAULT: CLI-261 (proposed), see DC_DEFAULTS.md
pub fn loader_threads() -> usize {
    if let Some(n) = std::env::var("DC_LOAD_THREADS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        return n.clamp(1, 8);
    }
    std::thread::available_parallelism()
        .map(|n| (n.get() / 4).clamp(1, 3))
        .unwrap_or(1)
}

impl Loader {
    /// Starts `threads` loader threads uploading through `uploader`. `None` when no thread could be started (the
    /// caller then loads inline).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start(shared: Arc<Shared>, uploader: ModelUploader, threads: usize) -> Option<Loader> {
        let (jobs, rx) = mpsc::channel::<Job>();
        let (done_tx, done) = mpsc::channel::<Done>();
        let rx = Arc::new(Mutex::new(rx));
        let mut started = 0;
        for i in 0..threads.max(1) {
            let (rx, done_tx, shared, up) = (
                rx.clone(),
                done_tx.clone(),
                shared.clone(),
                uploader.clone(),
            );
            let spawned = std::thread::Builder::new()
                .name(format!("dc-model-loader-{i}"))
                .spawn(move || loop {
                    let job = match dc_hangdump::crumb_wait!(
                        "loader: wait for a job",
                        rx.lock().unwrap_or_else(|e| e.into_inner()).recv()
                    ) {
                        Ok(j) => j,
                        Err(_) => return,
                    };
                    // a load that panics counts as a model that does not load, so the request is still handed back
                    // (and `wait` cannot hang on it)
                    let out = match job {
                        Job::Upload(key) => {
                            let r = catch_unwind(AssertUnwindSafe(|| {
                                shared.build(&key, &|f, look| up.upload_look(f, look))
                            }))
                            .unwrap_or(None);
                            Done::Upload(key, r)
                        }
                        Job::Files(path) => {
                            let _ = catch_unwind(AssertUnwindSafe(|| shared.files(&path)));
                            Done::Files(path)
                        }
                    };
                    if dc_hangdump::crumb_wait!("loader: hand the result back", done_tx.send(out))
                        .is_err()
                    {
                        return;
                    }
                });
            if spawned.is_ok() {
                started += 1;
            }
        }
        (started > 0).then_some(Loader {
            jobs,
            done,
            outstanding: 0,
            threads: started,
        })
    }

    /// No threads on the web target: every load is inline.
    #[cfg(target_arch = "wasm32")]
    pub fn start(_: Arc<Shared>, _: ModelUploader, _: usize) -> Option<Loader> {
        None
    }

    fn send(&mut self, job: Job) -> bool {
        let ok = self.jobs.send(job).is_ok();
        self.outstanding += ok as usize;
        // perf-g1: the streaming request queue (the job channel itself is unbounded today)
        dc_render::perf::gauge(dc_render::perf::Gauge::LoaderJobs, ok as i64);
        ok
    }

    /// Queues an upload.
    pub fn upload(&mut self, key: UploadKey) -> bool {
        self.send(Job::Upload(key))
    }

    /// Queues a file read.
    pub fn read_files(&mut self, path: String) -> bool {
        self.send(Job::Files(path))
    }

    /// The finished requests so far (never blocks).
    pub fn poll(&mut self) -> Vec<Done> {
        let mut v = Vec::new();
        while let Ok(d) = self.done.try_recv() {
            v.push(d);
        }
        self.outstanding -= v.len().min(self.outstanding);
        dc_render::perf::gauge(dc_render::perf::Gauge::LoaderJobs, -(v.len() as i64));
        v
    }

    /// Every outstanding request, waiting for the ones still running.
    pub fn wait(&mut self) -> Vec<Done> {
        let mut v = self.poll();
        while self.outstanding > 0 {
            match dc_hangdump::crumb_wait!(
                "loader: wait for the outstanding loads",
                self.done.recv()
            ) {
                Ok(d) => {
                    v.push(d);
                    self.outstanding -= 1;
                    dc_render::perf::gauge(dc_render::perf::Gauge::LoaderJobs, -1);
                }
                Err(_) => {
                    self.outstanding = 0;
                }
            }
        }
        v
    }

    /// Requests sent and not yet handed back.
    pub fn outstanding(&self) -> usize {
        self.outstanding
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn what_crosses_threads_is_send_and_sync() {
        assert_send_sync::<Shared>();
        assert_send_sync::<ModelUploader>();
        assert_send_sync::<ModelInstance>();
        assert_send_sync::<Loaded>();
    }

    #[test]
    fn thread_count_is_bounded() {
        let n = loader_threads();
        assert!((1..=8).contains(&n));
    }

    #[test]
    fn upload_keys_tell_looks_apart() {
        let m = |t: &str| ModelRef {
            model: "M\\a.ltb".into(),
            textures: vec![t.into()],
        };
        let k = |t: &str, f: Option<[u8; 4]>| UploadKey {
            model: m(t),
            styles: Vec::new(),
            fallback: f,
        };
        let mut set = std::collections::HashSet::new();
        set.insert(k("x.dtx", None));
        set.insert(k("x.dtx", None));
        set.insert(k("y.dtx", None));
        set.insert(k("x.dtx", Some([1, 2, 3, 4])));
        assert_eq!(set.len(), 3);
    }
}
