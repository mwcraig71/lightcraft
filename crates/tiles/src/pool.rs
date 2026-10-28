//! [`TilePool`]: the native [`TileProvider`]. Two worker threads per tile server (the OSM usage
//! policy asks for few parallel connections), started on the first request for that server. A
//! request goes to the disk cache first; a fresh tile there is used as is, a stale one is checked
//! with the server (`If-None-Match`), and if the server can't be reached the stale tile is still
//! drawn (cached areas stay usable offline). A tile asked for twice is fetched once; tiles that
//! left the view are dropped from the queue, and their in-flight request is cancelled.
//!
//! Decoded tiles are kept in a small memory LRU; the UI keeps its own textures.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::cache::{DiskCache, Entry, MAX_TILE_BYTES, keep_for, now_secs};
use crate::{TileId, TileImage, TileProvider, TileSource, TileState};

/// What a server answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    Tile {
        body: Vec<u8>,
        etag: Option<String>,
        cache_control: Option<String>,
    },
    /// 304: the cached copy is still right.
    NotModified {
        cache_control: Option<String>,
    },
    /// 404 / 204: no tile here.
    NoTile,
}

/// Fetches one URL (the real one speaks HTTPS; tests use a fake).
pub trait Fetcher: Send + Sync {
    /// GET `url`, conditional on `etag` when given. Gives up when `cancel` is raised.
    fn fetch(&self, url: &str, etag: Option<&str>, cancel: &AtomicBool) -> Result<Fetched, String>;
}

/// Workers per tile server.
const WORKERS_PER_SOURCE: usize = 2;
/// Decoded tiles kept in memory.
const MEMORY_TILES: usize = 160;
/// When a tile failed (network down, server error), ask again after this.
const RETRY_FAILED: Duration = Duration::from_secs(20);
/// A tile that would not decode, or that the server doesn't have, is asked for again after this.
const RETRY_BAD: Duration = Duration::from_secs(600);

/// A source's id and a tile (the built-in sources' ids borrow, so a lookup doesn't allocate).
type Key = (Cow<'static, str>, TileId);

struct Job {
    source: Arc<TileSource>,
    tile: TileId,
}

#[derive(Default)]
struct State {
    /// Frame number; `end_frame` advances it.
    frame: u64,
    /// The frame each queued or in-flight tile was last asked for in.
    wanted: HashMap<Key, u64>,
    queue: VecDeque<Job>,
    in_flight: HashMap<Key, Arc<AtomicBool>>,
    /// Decoded tiles and the tick each was last used at.
    memory: HashMap<Key, (Arc<TileImage>, u64)>,
    tick: u64,
    /// Tiles not to ask for again before the instant.
    missing: HashMap<Key, Instant>,
    started: HashSet<Cow<'static, str>>,
}

struct Shared {
    fetcher: Box<dyn Fetcher>,
    cache: Option<DiskCache>,
    state: Mutex<State>,
    work: Condvar,
    requests: AtomicU64,
    stop: AtomicBool,
}

/// The native tile provider.
pub struct TilePool {
    shared: Arc<Shared>,
}

impl TilePool {
    /// Tiles over HTTPS, cached in `cache_dir` (none: memory only) up to `cache_limit` bytes.
    pub fn new(cache_dir: Option<&Path>, cache_limit: u64) -> TilePool {
        Self::with_fetcher(Box::new(HttpFetcher), cache_dir, cache_limit)
    }

    pub fn with_fetcher(fetcher: Box<dyn Fetcher>, cache_dir: Option<&Path>, cache_limit: u64) -> TilePool {
        TilePool {
            shared: Arc::new(Shared {
                fetcher,
                cache: cache_dir.map(|d| DiskCache::new(d, cache_limit)),
                state: Mutex::new(State::default()),
                work: Condvar::new(),
                requests: AtomicU64::new(0),
                stop: AtomicBool::new(false),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn start_workers(&self, st: &mut State, source: &TileSource) {
        if !st.started.insert(source.id.clone()) {
            return;
        }
        for i in 0..WORKERS_PER_SOURCE {
            let shared = Arc::clone(&self.shared);
            let id = source.id.clone();
            let spawned = std::thread::Builder::new().name(format!("lc-tiles-{}-{i}", source.id)).spawn(move || worker(&shared, &id));
            if let Err(e) = spawned {
                log::warn!("map tiles: could not start a worker: {e}");
            }
        }
    }
}

impl Drop for TilePool {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        let st = self.lock();
        for c in st.in_flight.values() {
            c.store(true, Ordering::Relaxed);
        }
        drop(st);
        self.shared.work.notify_all();
    }
}

impl TileProvider for TilePool {
    fn get(&self, source: &TileSource, tile: TileId) -> TileState {
        let key = (source.id.clone(), tile);
        let mut st = self.lock();
        st.tick += 1;
        let tick = st.tick;
        if let Some((img, used)) = st.memory.get_mut(&key) {
            *used = tick;
            return TileState::Ready(Arc::clone(img));
        }
        if let Some(until) = st.missing.get(&key) {
            if Instant::now() < *until {
                return TileState::Missing;
            }
            st.missing.remove(&key);
        }
        let frame = st.frame;
        let known = st.wanted.insert(key, frame).is_some();
        if !known {
            st.queue.push_back(Job { source: Arc::new(source.clone()), tile });
            self.start_workers(&mut st, source);
            drop(st);
            self.shared.work.notify_all();
        }
        TileState::Pending
    }

    fn end_frame(&self) {
        let mut st = self.lock();
        let frame = st.frame;
        let State { wanted, queue, in_flight, .. } = &mut *st;
        queue.retain(|j| wanted.get(&(j.source.id.clone(), j.tile)) == Some(&frame));
        for (k, cancel) in in_flight.iter() {
            if wanted.get(k) != Some(&frame) {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        let live: HashSet<Key> = queue.iter().map(|j| (j.source.id.clone(), j.tile)).chain(in_flight.keys().cloned()).collect();
        wanted.retain(|k, f| *f == frame && live.contains(k));
        st.frame = frame.wrapping_add(1);
    }

    fn requests(&self) -> u64 {
        self.shared.requests.load(Ordering::Relaxed)
    }

    fn cache_bytes(&self) -> u64 {
        self.shared.cache.as_ref().map_or(0, DiskCache::bytes)
    }

    fn set_cache_limit(&self, bytes: u64) {
        if let Some(c) = &self.shared.cache {
            c.set_limit(bytes);
        }
    }

    fn clear_cache(&self) -> Result<u64, String> {
        self.shared.cache.as_ref().map_or(Ok(0), DiskCache::clear)
    }
}

/// What came of one job.
enum Outcome {
    Image(Arc<TileImage>),
    /// No tile, or undecodable: don't ask again for a while.
    Bad,
    /// Network or server trouble: ask again soon.
    Failed,
    Cancelled,
}

fn worker(shared: &Shared, source_id: &str) {
    loop {
        let (job, cancel) = {
            let mut st = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if shared.stop.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(i) = st.queue.iter().position(|j| j.source.id == source_id) {
                    let Some(job) = st.queue.remove(i) else { continue };
                    let cancel = Arc::new(AtomicBool::new(false));
                    st.in_flight.insert((job.source.id.clone(), job.tile), Arc::clone(&cancel));
                    break (job, cancel);
                }
                st = shared.work.wait(st).unwrap_or_else(PoisonError::into_inner);
            }
        };
        // a bad tile must not take the worker down with it
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| load(shared, &job.source, job.tile, &cancel))).unwrap_or(Outcome::Failed);
        let key = (job.source.id.clone(), job.tile);
        let mut st = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.in_flight.remove(&key);
        st.wanted.remove(&key);
        match outcome {
            Outcome::Image(img) => {
                st.tick += 1;
                let tick = st.tick;
                st.memory.insert(key, (img, tick));
                if st.memory.len() > MEMORY_TILES
                    && let Some(oldest) = st.memory.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| k.clone())
                {
                    st.memory.remove(&oldest);
                }
            }
            Outcome::Bad => {
                st.missing.insert(key, Instant::now() + RETRY_BAD);
            }
            Outcome::Failed => {
                st.missing.insert(key, Instant::now() + RETRY_FAILED);
            }
            Outcome::Cancelled => {}
        }
    }
}

fn load(shared: &Shared, source: &TileSource, tile: TileId, cancel: &AtomicBool) -> Outcome {
    let now = now_secs();
    let id: &str = &source.id;
    let cached = shared.cache.as_ref().and_then(|c| c.get(id, tile));
    if let Some(e) = &cached
        && e.is_fresh(now)
    {
        return decoded(&e.body);
    }
    if cancel.load(Ordering::Relaxed) {
        return Outcome::Cancelled;
    }
    shared.requests.fetch_add(1, Ordering::Relaxed);
    let expires = |cc: Option<&str>| now.saturating_add(keep_for(cc).as_secs());
    let fetched = shared.fetcher.fetch(&source.url(tile), cached.as_ref().and_then(|e| e.etag.as_deref()), cancel);
    // the server's "no imagery here" picture is no tile (a coarser one stands in)
    let fetched = match fetched {
        Ok(Fetched::Tile { etag: Some(tag), .. }) if source.no_data_etag == Some(tag.as_str()) => Ok(Fetched::NoTile),
        other => other,
    };
    match fetched {
        Ok(Fetched::Tile { body, etag, cache_control }) => {
            let entry = Entry { body, etag, expires: expires(cache_control.as_deref()) };
            if let Some(c) = &shared.cache
                && let Err(e) = c.put(id, tile, &entry)
            {
                log::warn!("map tiles: could not cache a tile: {e}");
            }
            decoded(&entry.body)
        }
        Ok(Fetched::NotModified { cache_control }) => match cached {
            Some(e) => {
                if let Some(c) = &shared.cache {
                    c.refresh(id, tile, expires(cache_control.as_deref()));
                }
                decoded(&e.body)
            }
            None => Outcome::Failed,
        },
        Ok(Fetched::NoTile) => {
            if let Some(c) = &shared.cache {
                let _ = c.put(id, tile, &Entry { body: Vec::new(), etag: None, expires: expires(None) });
            }
            Outcome::Bad
        }
        Err(_) if cancel.load(Ordering::Relaxed) => Outcome::Cancelled,
        Err(e) => match cached {
            // offline or the server is down: an old tile beats none
            Some(old) => decoded(&old.body),
            None => {
                // never the API key: errors may quote the URL
                log::debug!("map tiles: {id}: {}", source.redact(&e));
                Outcome::Failed
            }
        },
    }
}

fn decoded(body: &[u8]) -> Outcome {
    if body.is_empty() {
        return Outcome::Bad;
    }
    match decode(body) {
        Some(img) => Outcome::Image(Arc::new(img)),
        None => Outcome::Bad,
    }
}

/// PNG/JPEG tile bytes to RGBA.
pub fn decode(body: &[u8]) -> Option<TileImage> {
    let t = lightcraft_codecs::decode_thumbnail(body, 512).ok()?;
    let (width, height) = (t.image.width, t.image.height);
    if width == 0 || height == 0 {
        return None;
    }
    let rgba = t.image.data.iter().flatten().copied().collect();
    Some(TileImage { width, height, rgba })
}

/// Tiles over HTTPS with [`lightcraft_fetch::http`], with LightCraft's User-Agent.
pub struct HttpFetcher;

impl Fetcher for HttpFetcher {
    fn fetch(&self, url: &str, etag: Option<&str>, cancel: &AtomicBool) -> Result<Fetched, String> {
        use lightcraft_fetch::http;
        let limits = http::Limits { connect: Duration::from_secs(10), stall: Duration::from_secs(15), cancel };
        let mut url = http::Url::parse(url).map_err(|e| e.to_string())?;
        for _ in 0..4 {
            let mut headers = vec![("User-Agent", crate::user_agent())];
            if let Some(tag) = etag {
                headers.push(("If-None-Match", tag.to_string()));
            }
            let mut resp = http::get(&url, &headers, &limits).map_err(|e| e.to_string())?;
            let cache_control = resp.header("cache-control").map(str::to_string);
            match resp.status {
                200 => {
                    let tag = resp.header("etag").map(str::to_string);
                    if resp.content_length().is_some_and(|n| n > MAX_TILE_BYTES) {
                        return Err("tile too large".into());
                    }
                    let mut body = Vec::new();
                    let mut buf = [0u8; 16 * 1024];
                    loop {
                        let n = resp.read(&mut buf, &limits).map_err(|e| e.to_string())?;
                        if n == 0 {
                            break;
                        }
                        body.extend_from_slice(buf.get(..n).unwrap_or_default());
                        if body.len() as u64 > MAX_TILE_BYTES {
                            return Err("tile too large".into());
                        }
                    }
                    return Ok(Fetched::Tile { body, etag: tag, cache_control });
                }
                304 => return Ok(Fetched::NotModified { cache_control }),
                204 | 404 => return Ok(Fetched::NoTile),
                301 | 302 | 303 | 307 | 308 => {
                    let loc = resp.header("location").ok_or("redirect without a location")?;
                    url = url.join(loc).map_err(|e| e.to_string())?;
                }
                s => return Err(format!("HTTP {s} from {}", url.display())),
            }
        }
        Err("too many redirects".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{CustomSpec, ESRI_IMAGERY, OSM, USGS_IMAGERY};

    fn png() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 4, 4);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[200u8; 64]).unwrap();
        }
        out
    }

    /// Answers from a script; counts calls; can hold a call until it is cancelled.
    #[derive(Default)]
    struct Fake {
        answer: Mutex<Option<Result<Fetched, String>>>,
        calls: Mutex<Vec<(String, Option<String>)>>,
        hold: AtomicBool,
        saw_cancel: Arc<AtomicBool>,
    }

    impl Fetcher for Arc<Fake> {
        fn fetch(&self, url: &str, etag: Option<&str>, cancel: &AtomicBool) -> Result<Fetched, String> {
            self.calls.lock().unwrap().push((url.to_string(), etag.map(str::to_string)));
            if self.hold.load(Ordering::Relaxed) {
                let t = Instant::now();
                while !cancel.load(Ordering::Relaxed) && t.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                self.saw_cancel.store(cancel.load(Ordering::Relaxed), Ordering::Relaxed);
                return Err("cancelled".into());
            }
            self.answer.lock().unwrap().clone().unwrap_or(Err("no answer".into()))
        }
    }

    fn wait(pool: &TilePool, src: &TileSource, t: TileId) -> TileState {
        let start = Instant::now();
        loop {
            match pool.get(src, t) {
                TileState::Pending if start.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(5)),
                s => return s,
            }
        }
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lc-pool-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    const T: TileId = TileId { z: 5, x: 3, y: 9 };

    #[test]
    fn fetches_once_then_serves_from_memory_and_disk() {
        let fake = Arc::new(Fake::default());
        *fake.answer.lock().unwrap() = Some(Ok(Fetched::Tile { body: png(), etag: Some("e1".into()), cache_control: None }));
        let dir = tmp("once");
        let pool = TilePool::with_fetcher(Box::new(Arc::clone(&fake)), Some(&dir), 1 << 20);
        assert!(matches!(pool.get(&OSM, T), TileState::Pending));
        // asked again while loading: still one request
        assert!(matches!(pool.get(&OSM, T), TileState::Pending | TileState::Ready(_)));
        let TileState::Ready(img) = wait(&pool, &OSM, T) else { panic!("no tile") };
        assert_eq!((img.width, img.height, img.rgba.len()), (4, 4, 64));
        assert_eq!(pool.requests(), 1);
        assert_eq!(fake.calls.lock().unwrap()[0].0, "https://tile.openstreetmap.org/5/3/9.png");
        assert!(pool.cache_bytes() > 0);
        drop(pool);
        // a new pool (the next run) uses the fresh disk copy without asking the server
        let pool = TilePool::with_fetcher(Box::new(Arc::clone(&fake)), Some(&dir), 1 << 20);
        assert!(matches!(wait(&pool, &OSM, T), TileState::Ready(_)));
        assert_eq!(pool.requests(), 0);
        assert!(pool.clear_cache().unwrap() > 0);
        assert_eq!(pool.cache_bytes(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_tiles_revalidate_and_are_used_offline() {
        let fake = Arc::new(Fake::default());
        let dir = tmp("stale");
        let cache = DiskCache::new(&dir, 1 << 20);
        cache.put(&USGS_IMAGERY.id, T, &Entry { body: png(), etag: Some("old".into()), expires: 1 }).unwrap();
        // the network is down: the stale tile is drawn anyway
        *fake.answer.lock().unwrap() = Some(Err("could not connect".into()));
        let pool = TilePool::with_fetcher(Box::new(Arc::clone(&fake)), Some(&dir), 1 << 20);
        assert!(matches!(wait(&pool, &USGS_IMAGERY, T), TileState::Ready(_)));
        assert_eq!(fake.calls.lock().unwrap()[0].1.as_deref(), Some("old"), "conditional request");
        // back online, 304: the tile's expiry moves forward
        *fake.answer.lock().unwrap() = Some(Ok(Fetched::NotModified { cache_control: Some("max-age=999999".into()) }));
        let pool = TilePool::with_fetcher(Box::new(Arc::clone(&fake)), Some(&dir), 1 << 20);
        assert!(matches!(wait(&pool, &USGS_IMAGERY, T), TileState::Ready(_)));
        assert!(cache.get(&USGS_IMAGERY.id, T).unwrap().is_fresh(now_secs()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_no_data_picture_counts_as_no_tile() {
        let fake = Arc::new(Fake::default());
        let tag = ESRI_IMAGERY.no_data_etag.map(str::to_string);
        *fake.answer.lock().unwrap() = Some(Ok(Fetched::Tile { body: png(), etag: tag, cache_control: None }));
        let pool = TilePool::with_fetcher(Box::new(Arc::clone(&fake)), None, 0);
        let deep = TileId { z: 19, x: 3, y: 9 };
        assert!(matches!(wait(&pool, &ESRI_IMAGERY, deep), TileState::Missing), "Esri's grey placeholder is not imagery");
        assert!(fake.calls.lock().unwrap()[0].0.ends_with("/World_Imagery/MapServer/tile/19/9/3"));
        // the same picture from another server is a tile
        assert!(matches!(wait(&pool, &OSM, deep), TileState::Ready(_)));
    }

    #[test]
    fn a_custom_source_is_fetched_with_its_key_and_cached_under_its_id() {
        let fake = Arc::new(Fake::default());
        *fake.answer.lock().unwrap() = Some(Ok(Fetched::Tile { body: png(), etag: None, cache_control: None }));
        let dir = tmp("custom");
        let pool = TilePool::with_fetcher(Box::new(Arc::clone(&fake)), Some(&dir), 1 << 20);
        let spec = CustomSpec {
            name: "Mine",
            url_template: "https://t.example.com/{z}/{x}/{y}.jpg?key={key}",
            api_key: "k3y",
            max_native_zoom: 20,
            attribution: "Me",
        };
        let src = TileSource::custom(spec).unwrap();
        assert!(matches!(wait(&pool, &src, T), TileState::Ready(_)));
        assert_eq!(fake.calls.lock().unwrap()[0].0, "https://t.example.com/5/3/9.jpg?key=k3y");
        let folder = dir.join(src.id.as_ref());
        assert!(folder.join("5").join("3").join("9.tile").is_file(), "{folder:?}");
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| !n.contains("k3y")), "{names:?}");
        // another template: other tiles (a fresh request)
        let other = TileSource::custom(CustomSpec { url_template: "https://t.example.com/v2/{z}/{x}/{y}.jpg?key={key}", ..spec }).unwrap();
        assert!(matches!(wait(&pool, &other, T), TileState::Ready(_)));
        assert_eq!(pool.requests(), 2);
        drop(pool);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_tile_and_garbage_are_missing_and_remembered() {
        let fake = Arc::new(Fake::default());
        *fake.answer.lock().unwrap() = Some(Ok(Fetched::NoTile));
        let dir = tmp("missing");
        let pool = TilePool::with_fetcher(Box::new(Arc::clone(&fake)), Some(&dir), 1 << 20);
        assert!(matches!(wait(&pool, &OSM, T), TileState::Missing));
        assert!(matches!(pool.get(&OSM, T), TileState::Missing));
        assert_eq!(pool.requests(), 1);
        let other = TileId { z: 5, x: 4, y: 9 };
        *fake.answer.lock().unwrap() = Some(Ok(Fetched::Tile { body: b"not an image".to_vec(), etag: None, cache_control: None }));
        assert!(matches!(wait(&pool, &OSM, other), TileState::Missing));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tiles_that_leave_the_view_are_dropped_and_cancelled() {
        let fake = Arc::new(Fake::default());
        fake.hold.store(true, Ordering::Relaxed);
        let pool = TilePool::with_fetcher(Box::new(Arc::clone(&fake)), None, 0);
        // more tiles than workers: some queue
        for x in 0..6 {
            let _ = pool.get(&OSM, TileId { z: 8, x, y: 1 });
        }
        pool.end_frame();
        let start = Instant::now();
        while fake.calls.lock().unwrap().len() < 2 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        // the next frame wants none of them
        pool.end_frame();
        let start = Instant::now();
        while !fake.saw_cancel.load(Ordering::Relaxed) && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(fake.saw_cancel.load(Ordering::Relaxed), "in-flight request cancelled");
        std::thread::sleep(Duration::from_millis(100));
        // only the two in-flight tiles were ever requested; the queued four were dropped
        assert_eq!(fake.calls.lock().unwrap().len(), 2);
        assert!(pool.lock().queue.is_empty());
    }
}
