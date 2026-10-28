//! Drone orthomosaics drawn on the Map view: the library's list of them, their map tiles and the
//! background jobs that build those tiles (see docs/map.md → Orthomosaics).
//!
//! An orthomosaic belongs to the library (a bridge project), so the list is kept in the library's
//! folder, `Orthomosaics/orthomosaics.json`, beside each one's tiles
//! (`Orthomosaics/<id>/<z>/<x>/<y>.jpg|png`). The GeoTIFF itself is referenced in place (they're
//! huge); its size and modification time are recorded, so a changed file builds its tiles again
//! and a missing one is reported (its tiles are still drawn). A library that isn't on disk (the
//! demo) keeps them in a folder under the system's temporary directory for the session.
//!
//! The list is read when the session first looks at it for a library, so nothing is added to the
//! catalog and older libraries open unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_ortho::Info;
use serde::{Deserialize, Serialize};

use crate::Session;
use crate::activity::{Activity, Cancel};

/// The folder in a library.
pub const FOLDER: &str = "Orthomosaics";
const LIST: &str = "orthomosaics.json";
const MANIFEST: &str = "build.json";
/// Orthomosaics kept per library.
pub const MAX: usize = 200;

/// One orthomosaic as the library records it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overlay {
    pub id: u64,
    pub name: String,
    /// The GeoTIFF, referenced in place.
    pub path: PathBuf,
    /// Its size and modification time (ms since 1970) when the tiles were planned.
    pub size: u64,
    pub modified_ms: u64,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default = "full")]
    pub opacity: f32,
    pub info: Info,
}

fn yes() -> bool {
    true
}
fn full() -> f32 {
    1.0
}

/// Where an orthomosaic's tiles are.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    /// Tiles being made: `done` of `total` looked at.
    Building {
        done: u64,
        total: u64,
    },
    Ready,
    /// The build failed or was cancelled (`orthomosaic.rebuild` tries again).
    Failed {
        error: String,
    },
}

/// What the Map view draws from.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub overlay: Overlay,
    pub status: Status,
    /// The GeoTIFF isn't where it was (the tiles made before are still drawn).
    pub missing: bool,
    /// The tile folder (`<z>/<x>/<y>.jpg|png`).
    pub dir: PathBuf,
}

#[derive(Default)]
struct Shared {
    status: HashMap<u64, Status>,
    /// Bumped whenever a status changes (the map re-reads tiles it found absent).
    generation: u64,
    /// The build each orthomosaic's status belongs to (its cancel flag): an older build that
    /// was replaced or removed never overwrites it.
    owner: HashMap<u64, Arc<AtomicBool>>,
}

/// The session's orthomosaics (`Session::orthomosaics`).
#[derive(Default)]
pub struct Orthomosaics {
    list: Vec<Overlay>,
    missing: HashMap<u64, bool>,
    shared: Arc<Mutex<Shared>>,
    jobs: HashMap<u64, Arc<AtomicBool>>,
    /// The library the list belongs to, and its folder.
    loaded: Option<(Arc<()>, PathBuf, bool)>,
    /// Tests: keep the folder here instead.
    pub root_override: Option<PathBuf>,
}

fn lock(m: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ListFile {
    version: u32,
    orthomosaics: Vec<Overlay>,
}

#[derive(Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
struct Manifest {
    size: u64,
    modified_ms: u64,
    complete: bool,
    tiles: u64,
    bytes: u64,
}

/// Size and modification time of a file, or `None` when it isn't there.
fn stamp(path: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let ms =
        m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    Some((m.len(), ms))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.part");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

impl Orthomosaics {
    /// The folder holding the list and the tiles.
    pub fn root(&self) -> Option<&Path> {
        self.loaded.as_ref().map(|(_, p, _)| p.as_path())
    }

    pub fn tile_dir(&self, id: u64) -> Option<PathBuf> {
        self.root().map(|r| r.join(id.to_string()))
    }

    /// Bumped whenever a build makes progress or ends.
    pub fn generation(&self) -> u64 {
        lock(&self.shared).generation
    }

    pub fn status(&self, id: u64) -> Status {
        lock(&self.shared).status.get(&id).cloned().unwrap_or(Status::Failed { error: "not built".into() })
    }

    pub fn overlays(&self) -> &[Overlay] {
        &self.list
    }

    pub fn get(&self, id: u64) -> Option<&Overlay> {
        self.list.iter().find(|o| o.id == id)
    }

    fn view(&self, o: &Overlay) -> View {
        View {
            overlay: o.clone(),
            status: self.status(o.id),
            missing: self.missing.get(&o.id).copied().unwrap_or(false),
            dir: self.tile_dir(o.id).unwrap_or_default(),
        }
    }

    pub fn views(&self) -> Vec<View> {
        self.list.iter().map(|o| self.view(o)).collect()
    }

    fn save(&self) -> Result<(), String> {
        let Some(root) = self.root() else { return Ok(()) };
        if self.list.is_empty() && !root.join(LIST).exists() {
            return Ok(());
        }
        let file = ListFile { version: 1, orthomosaics: self.list.clone() };
        let json = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
        write_atomic(&root.join(LIST), &json)
    }

    /// Stop every build (another library opened, or the session ends).
    fn cancel_all(&mut self) {
        for flag in self.jobs.values() {
            flag.store(true, Ordering::Relaxed);
        }
        self.jobs.clear();
    }
}

impl Drop for Orthomosaics {
    fn drop(&mut self) {
        self.cancel_all();
        // a session folder (no library on disk) goes with the session
        if let Some((_, root, false)) = &self.loaded {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

/// The folder for a library: `<library>/Orthomosaics`, or a session folder for one not on disk.
fn root_for(s: &Session) -> PathBuf {
    if let Some(r) = &s.orthomosaics.root_override {
        return r.clone();
    }
    match &s.library {
        Some(l) if l.on_disk => l.dir.join(FOLDER),
        // one per library opening, so another in-memory library starts empty
        _ => std::env::temp_dir().join(format!("lightcraft-orthomosaics-{}-{:x}", std::process::id(), Arc::as_ptr(&s.library_identity) as usize)),
    }
}

impl Session {
    /// The orthomosaics of the open library, read when the library changed since the last call.
    pub fn orthomosaics(&mut self) -> &Orthomosaics {
        self.sync_orthomosaics();
        &self.orthomosaics
    }

    /// What the Map view draws: every orthomosaic with its status and tile folder.
    pub fn orthomosaic_views(&mut self) -> Vec<View> {
        self.sync_orthomosaics();
        self.orthomosaics.views()
    }

    fn sync_orthomosaics(&mut self) {
        let identity = Arc::clone(&self.library_identity);
        let root = root_for(self);
        if self.orthomosaics.loaded.as_ref().is_some_and(|(id, r, _)| Arc::ptr_eq(id, &identity) && *r == root) {
            return;
        }
        self.orthomosaics.cancel_all();
        let on_disk = self.library.as_ref().is_some_and(|l| l.on_disk) || self.orthomosaics.root_override.is_some();
        self.orthomosaics.loaded = Some((identity, root.clone(), on_disk));
        self.orthomosaics.shared = Arc::new(Mutex::new(Shared::default()));
        self.orthomosaics.missing.clear();
        let list: Vec<Overlay> = match std::fs::read(root.join(LIST)) {
            Ok(bytes) => match serde_json::from_slice::<ListFile>(&bytes) {
                Ok(f) => f.orthomosaics.into_iter().take(MAX).collect(),
                Err(e) => {
                    log::warn!("orthomosaics: {}: {e}", root.join(LIST).display());
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        };
        self.orthomosaics.list = list;
        let ids: Vec<u64> = self.orthomosaics.list.iter().map(|o| o.id).collect();
        for id in ids {
            self.check_orthomosaic(id);
        }
    }

    /// Look at an orthomosaic's file and tiles: ready, missing, changed (rebuild) or unbuilt (build).
    fn check_orthomosaic(&mut self, id: u64) {
        let Some(o) = self.orthomosaics.get(id).cloned() else { return };
        let dir = self.orthomosaics.tile_dir(id).unwrap_or_default();
        let manifest: Option<Manifest> = std::fs::read(dir.join(MANIFEST)).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let built = manifest.as_ref().is_some_and(|m| m.complete && m.size == o.size && m.modified_ms == o.modified_ms);
        match stamp(&o.path) {
            None => {
                self.orthomosaics.missing.insert(id, true);
                let status =
                    if built { Status::Ready } else { Status::Failed { error: format!("the file isn't there any more: {}", o.path.display()) } };
                set_status(&self.orthomosaics.shared, id, status);
            }
            Some(st) => {
                self.orthomosaics.missing.insert(id, false);
                if built && st == (o.size, o.modified_ms) {
                    set_status(&self.orthomosaics.shared, id, Status::Ready);
                    return;
                }
                if st != (o.size, o.modified_ms) {
                    // the file changed: where it is may have too
                    match lightcraft_ortho::guarded("reading the orthomosaic", || lightcraft_ortho::inspect(&o.path)) {
                        Ok(info) => {
                            if let Some(e) = self.orthomosaics.list.iter_mut().find(|e| e.id == id) {
                                e.info = info;
                                e.size = st.0;
                                e.modified_ms = st.1;
                            }
                            if let Err(e) = self.orthomosaics.save() {
                                log::warn!("orthomosaics: {e}");
                            }
                        }
                        Err(e) => {
                            set_status(&self.orthomosaics.shared, id, Status::Failed { error: e.to_string() });
                            return;
                        }
                    }
                }
                let _ = std::fs::remove_dir_all(&dir);
                self.start_build(id);
            }
        }
    }

    /// Start (or restart) building an orthomosaic's tiles in the background.
    pub(crate) fn start_build(&mut self, id: u64) {
        let Some(o) = self.orthomosaics.get(id).cloned() else { return };
        let Some(dir) = self.orthomosaics.tile_dir(id) else { return };
        if let Some(old) = self.orthomosaics.jobs.remove(&id) {
            old.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.orthomosaics.jobs.insert(id, cancel.clone());
        let total = o.info.tile_count();
        lock(&self.orthomosaics.shared).owner.insert(id, cancel.clone());
        set_status(&self.orthomosaics.shared, id, Status::Building { done: 0, total });
        spawn_build(&self.activity, self.orthomosaics.shared.clone(), o, dir, cancel);
    }

    /// Tests: wait until no orthomosaic is building (or `timeout` passes).
    pub fn wait_for_orthomosaics(&mut self, timeout: std::time::Duration) -> bool {
        let start = web_time::Instant::now();
        loop {
            let building = lock(&self.orthomosaics.shared).status.values().any(|s| matches!(s, Status::Building { .. }));
            if !building {
                return true;
            }
            if start.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Add the orthomosaic at `path` (read its georeferencing, record it, start building its tiles).
    pub fn add_orthomosaic(&mut self, path: &Path, name: Option<&str>) -> Result<View, String> {
        self.sync_orthomosaics();
        if cfg!(target_arch = "wasm32") {
            return Err("orthomosaics aren't available in the web version".into());
        }
        if self.orthomosaics.list.len() >= MAX {
            return Err(format!("the library has {MAX} orthomosaics already; remove one first"));
        }
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(o) = self.orthomosaics.list.iter().find(|o| o.path == path) {
            return Err(format!("“{}” is on the map already (orthomosaic {})", o.name, o.id));
        }
        let (size, modified_ms) = stamp(&path).ok_or_else(|| format!("no such file: {}", path.display()))?;
        let info = lightcraft_ortho::guarded("reading the orthomosaic", || lightcraft_ortho::inspect(&path)).map_err(|e| e.to_string())?;
        let id = self.orthomosaics.list.iter().map(|o| o.id).max().unwrap_or(0) + 1;
        let name = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| format!("Orthomosaic {id}")));
        let overlay = Overlay { id, name, path, size, modified_ms, visible: true, opacity: 1.0, info };
        self.orthomosaics.list.push(overlay);
        self.orthomosaics.missing.insert(id, false);
        if let Some(dir) = self.orthomosaics.tile_dir(id) {
            let _ = std::fs::remove_dir_all(dir);
        }
        self.orthomosaics.save()?;
        self.start_build(id);
        let o = self.orthomosaics.get(id).ok_or("the orthomosaic went away")?;
        Ok(self.orthomosaics.view(o))
    }

    /// Show or hide an orthomosaic, change its opacity or name.
    pub fn set_orthomosaic(&mut self, id: u64, visible: Option<bool>, opacity: Option<f64>, name: Option<&str>) -> Result<View, String> {
        self.sync_orthomosaics();
        let o = self.orthomosaics.list.iter_mut().find(|o| o.id == id).ok_or_else(|| format!("no orthomosaic {id}"))?;
        if let Some(v) = visible {
            o.visible = v;
        }
        if let Some(op) = opacity {
            if !(op.is_finite() && (0.0..=1.0).contains(&op)) {
                return Err(format!("`opacity` must be from 0 to 1, not {op}"));
            }
            o.opacity = op as f32;
        }
        if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
            o.name = n.chars().take(200).collect();
        }
        self.orthomosaics.save()?;
        let o = self.orthomosaics.get(id).ok_or("the orthomosaic went away")?;
        Ok(self.orthomosaics.view(o))
    }

    /// Remove an orthomosaic from the map and delete its tiles (never the GeoTIFF).
    pub fn remove_orthomosaic(&mut self, id: u64) -> Result<(), String> {
        self.sync_orthomosaics();
        let at = self.orthomosaics.list.iter().position(|o| o.id == id).ok_or_else(|| format!("no orthomosaic {id}"))?;
        self.orthomosaics.list.remove(at);
        self.orthomosaics.missing.remove(&id);
        let job = self.orthomosaics.jobs.remove(&id);
        if let Some(flag) = &job {
            flag.store(true, Ordering::Relaxed);
        }
        {
            let mut shared = lock(&self.orthomosaics.shared);
            shared.status.remove(&id);
            shared.owner.remove(&id);
            shared.generation = shared.generation.wrapping_add(1);
        }
        self.orthomosaics.save()?;
        if let Some(dir) = self.orthomosaics.tile_dir(id) {
            // a running build stops at its next tile; delete what is there now, and once more when it has
            if std::fs::remove_dir_all(&dir).is_err() && dir.exists() {
                log::warn!("orthomosaics: couldn't delete {}", dir.display());
            }
            if job.is_some() {
                let _ = std::thread::Builder::new().name("lc-ortho-remove".into()).spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    let _ = std::fs::remove_dir_all(&dir);
                });
            }
        }
        Ok(())
    }

    /// Build an orthomosaic's tiles again (after a failure or a cancel, or to refresh them).
    pub fn rebuild_orthomosaic(&mut self, id: u64) -> Result<View, String> {
        self.sync_orthomosaics();
        let o = self.orthomosaics.get(id).cloned().ok_or_else(|| format!("no orthomosaic {id}"))?;
        if stamp(&o.path).is_none() {
            return Err(format!("the file isn't there any more: {}", o.path.display()));
        }
        // a changed file is re-read first; otherwise the tiles are made again from scratch
        if let Some(dir) = self.orthomosaics.tile_dir(id) {
            let _ = std::fs::remove_file(dir.join(MANIFEST));
        }
        self.check_orthomosaic(id);
        let o = self.orthomosaics.get(id).ok_or("the orthomosaic went away")?;
        Ok(self.orthomosaics.view(o))
    }
}

fn set_status(shared: &Arc<Mutex<Shared>>, id: u64, status: Status) {
    let mut s = lock(shared);
    s.status.insert(id, status);
    s.generation = s.generation.wrapping_add(1);
}

/// [`set_status`] from the build whose cancel flag is `owner`, if it is still the current one.
fn set_owned(shared: &Arc<Mutex<Shared>>, id: u64, owner: &Arc<AtomicBool>, status: Status) {
    let mut s = lock(shared);
    if s.owner.get(&id).is_some_and(|o| Arc::ptr_eq(o, owner)) {
        s.status.insert(id, status);
        s.generation = s.generation.wrapping_add(1);
    }
}

/// Worker threads for one build: all but one core, at most six.
fn build_threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get()).saturating_sub(1).clamp(1, 6)
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_build(activity: &Activity, shared: Arc<Mutex<Shared>>, o: Overlay, dir: PathBuf, cancel: Arc<AtomicBool>) {
    let guard = activity.start("orthomosaic", "Building orthomosaic map tiles", Cancel::Flag(cancel.clone()));
    guard.detail(&o.name);
    guard.progress(0, o.info.tile_count());
    let id = o.id;
    let fail_shared = shared.clone();
    let spawned = std::thread::Builder::new().name("lc-ortho-build".into()).spawn(move || {
        let handle = guard.handle();
        let progress = |done: u64, total: u64| {
            handle.progress(done, total);
            // the map looks again every few tiles
            if done.is_multiple_of(4) || done == total {
                set_owned(&shared, id, &cancel, Status::Building { done, total });
            }
        };
        let r = lightcraft_ortho::guarded("building the orthomosaic's tiles", || {
            lightcraft_ortho::build(&o.path, &o.info, &dir, build_threads(), &cancel, &progress)
        });
        // a newer build or a removal that took over keeps its status (`set_owned`)
        match r {
            Ok(built) => {
                let m = Manifest { size: o.size, modified_ms: o.modified_ms, complete: true, tiles: built.tiles, bytes: built.bytes };
                let status = match serde_json::to_vec_pretty(&m).map_err(|e| e.to_string()).and_then(|j| write_atomic(&dir.join(MANIFEST), &j)) {
                    Ok(()) => Status::Ready,
                    Err(e) => Status::Failed { error: e },
                };
                set_owned(&shared, id, &cancel, status);
            }
            Err(lightcraft_ortho::Error::Cancelled) => set_owned(&shared, id, &cancel, Status::Failed { error: "cancelled".into() }),
            Err(e) => set_owned(&shared, id, &cancel, Status::Failed { error: e.to_string() }),
        }
        drop(guard);
    });
    if let Err(e) = spawned {
        set_status(&fail_shared, id, Status::Failed { error: format!("couldn't start the build: {e}") });
    }
}

#[cfg(target_arch = "wasm32")]
fn spawn_build(_: &Activity, shared: Arc<Mutex<Shared>>, o: Overlay, _: PathBuf, _: Arc<AtomicBool>) {
    set_status(&shared, o.id, Status::Failed { error: "orthomosaics aren't available in the web version".into() });
}
