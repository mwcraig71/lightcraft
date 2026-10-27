//! Tiles on disk: `<dir>/<source>/<z>/<x>/<y>.tile`, each the server's bytes after a small
//! header (when it expires, its `ETag`). A tile with an empty body records that the server has
//! no tile there. The cache is bounded in bytes: past the limit the least recently used tiles
//! (oldest modification time; a read touches it) are deleted until it is 10 % under.
//!
//! Only files this cache writes (`*.tile` at the expected depth) are ever deleted, and symbolic
//! links are never followed, so pointing it at a shared folder can't lose other files.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::mercator::TileId;

const MAGIC: &[u8; 4] = b"LCT1";
const EXT: &str = "tile";
/// A tile file larger than this is not a tile (and is not read).
pub const MAX_TILE_BYTES: u64 = 4 * 1024 * 1024;
/// Default size limit.
pub const DEFAULT_LIMIT: u64 = 500 * 1024 * 1024;
const TAG: &str = "CACHEDIR.TAG";
const TAG_TEXT: &str =
    "Signature: 8a477f597d28d172789f06886806bc55\n# LightCraft map tile cache; safe to delete.\n# See https://bford.info/cachedir/\n";

/// A cached tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The server's bytes (PNG/JPEG); empty when the server has no tile here.
    pub body: Vec<u8>,
    pub etag: Option<String>,
    /// Seconds since 1970 after which the tile should be checked with the server again.
    pub expires: u64,
}

impl Entry {
    pub fn is_fresh(&self, now: u64) -> bool {
        now < self.expires
    }
}

/// Seconds since 1970 now.
pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The disk tile cache.
pub struct DiskCache {
    dir: PathBuf,
    limit: Mutex<u64>,
    /// Bytes on disk; counted on first use.
    used: Mutex<Option<u64>>,
}

impl DiskCache {
    pub fn new(dir: &Path, limit: u64) -> DiskCache {
        DiskCache { dir: dir.to_path_buf(), limit: Mutex::new(limit), used: Mutex::new(None) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, source: &str, t: TileId) -> PathBuf {
        self.dir.join(source).join(t.z.to_string()).join(t.x.to_string()).join(format!("{}.{EXT}", t.y))
    }

    /// The cached tile, touching it so it counts as recently used.
    pub fn get(&self, source: &str, t: TileId) -> Option<Entry> {
        let p = self.path(source, t);
        let meta = fs::symlink_metadata(&p).ok()?;
        if !meta.is_file() || meta.len() > MAX_TILE_BYTES {
            return None;
        }
        let bytes = fs::read(&p).ok()?;
        let entry = decode(&bytes)?;
        if let Ok(f) = fs::File::options().write(true).open(&p) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(entry)
    }

    /// Store a tile (replacing any older copy), then evict if over the limit.
    pub fn put(&self, source: &str, t: TileId, e: &Entry) -> std::io::Result<()> {
        let p = self.path(source, t);
        let parent = p.parent().ok_or_else(|| std::io::Error::other("tile path has no folder"))?;
        let made = !self.dir.exists();
        fs::create_dir_all(parent)?;
        if made {
            let _ = fs::write(self.dir.join(TAG), TAG_TEXT);
        }
        let bytes = encode(e);
        let old = fs::symlink_metadata(&p).ok().filter(|m| m.is_file()).map_or(0, |m| m.len());
        let tmp = p.with_extension("part");
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
        }
        if let Err(err) = fs::rename(&tmp, &p) {
            let _ = fs::remove_file(&tmp);
            return Err(err);
        }
        let used = {
            let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
            let now = match *used {
                Some(u) => u.saturating_sub(old).saturating_add(bytes.len() as u64),
                // the scan already sees the new file
                None => self.scan().iter().map(|f| f.1).sum(),
            };
            *used = Some(now);
            now
        };
        if used > self.limit() {
            self.evict();
        }
        Ok(())
    }

    /// Push a cached tile's expiry forward (the server said it hasn't changed).
    pub fn refresh(&self, source: &str, t: TileId, expires: u64) {
        if let Some(mut e) = self.get(source, t) {
            e.expires = expires;
            let _ = self.put(source, t, &e);
        }
    }

    pub fn limit(&self) -> u64 {
        *self.limit.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set_limit(&self, bytes: u64) {
        *self.limit.lock().unwrap_or_else(PoisonError::into_inner) = bytes;
        if self.bytes() > bytes {
            self.evict();
        }
    }

    /// Bytes on disk.
    pub fn bytes(&self) -> u64 {
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(u) = *used {
            return u;
        }
        let u = self.scan().iter().map(|f| f.1).sum();
        *used = Some(u);
        u
    }

    /// Every tile file: path, size, last use.
    fn scan(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let mut out = Vec::new();
        walk(&self.dir, 0, &mut out);
        out
    }

    /// Delete least recently used tiles until 10 % under the limit.
    fn evict(&self) {
        let target = self.limit() / 10 * 9;
        let mut files = self.scan();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort_by_key(|f| f.2);
        for (p, len, _) in files {
            if total <= target {
                break;
            }
            if fs::remove_file(&p).is_ok() {
                total = total.saturating_sub(len);
            }
        }
        *self.used.lock().unwrap_or_else(PoisonError::into_inner) = Some(total);
    }

    /// Delete every cached tile (and the folders left empty); returns the bytes freed.
    pub fn clear(&self) -> Result<u64, String> {
        let mut freed = 0u64;
        let mut failed = None;
        for (p, len, _) in self.scan() {
            match fs::remove_file(&p) {
                Ok(()) => freed = freed.saturating_add(len),
                Err(e) => failed = Some(format!("{}: {e}", p.display())),
            }
        }
        remove_empty_dirs(&self.dir, 0);
        *self.used.lock().unwrap_or_else(PoisonError::into_inner) = None;
        match failed {
            Some(e) => Err(format!("some cached tiles could not be deleted ({e})")),
            None => Ok(freed),
        }
    }
}

/// Collect `*.tile` files at depth 4 (`source/z/x/y.tile`) without following links.
fn walk(dir: &Path, depth: u8, out: &mut Vec<(PathBuf, u64, SystemTime)>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() && depth < 3 {
            walk(&p, depth + 1, out);
        } else if ft.is_file()
            && depth == 3
            && p.extension().is_some_and(|x| x == EXT)
            && let Ok(m) = e.metadata()
        {
            out.push((p, m.len(), m.modified().unwrap_or(UNIX_EPOCH)));
        }
    }
}

fn remove_empty_dirs(dir: &Path, depth: u8) {
    if depth >= 3 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            let p = e.path();
            remove_empty_dirs(&p, depth + 1);
            // fails (and is kept) unless empty
            let _ = fs::remove_dir(&p);
        }
    }
}

fn encode(e: &Entry) -> Vec<u8> {
    let etag = e.etag.as_deref().unwrap_or_default().as_bytes();
    let etag = etag.get(..etag.len().min(1024)).unwrap_or_default();
    let mut out = Vec::with_capacity(16 + etag.len() + e.body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&e.expires.to_le_bytes());
    out.extend_from_slice(&(etag.len() as u16).to_le_bytes());
    out.extend_from_slice(etag);
    out.extend_from_slice(&e.body);
    out
}

fn decode(b: &[u8]) -> Option<Entry> {
    if b.get(..4)? != MAGIC {
        return None;
    }
    let expires = u64::from_le_bytes(b.get(4..12)?.try_into().ok()?);
    let n = usize::from(u16::from_le_bytes(b.get(12..14)?.try_into().ok()?));
    let end = 14usize.checked_add(n)?;
    let etag = std::str::from_utf8(b.get(14..end)?).ok().filter(|s| !s.is_empty()).map(str::to_string);
    Some(Entry { body: b.get(end..)?.to_vec(), etag, expires })
}

/// How long a response may be kept before asking again, from its `Cache-Control` header:
/// its `max-age`, but never less than 7 days (the OSM tile usage policy's floor) nor more than a year.
pub fn keep_for(cache_control: Option<&str>) -> Duration {
    const WEEK: u64 = 7 * 24 * 3600;
    let max_age = cache_control
        .and_then(|v| v.split(',').find_map(|d| d.trim().strip_prefix("max-age=").and_then(|n| n.trim_matches('"').parse::<u64>().ok())));
    Duration::from_secs(max_age.unwrap_or(WEEK).clamp(WEEK, 365 * 24 * 3600))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-tiles-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn t(x: u32) -> TileId {
        TileId { z: 12, x, y: 7 }
    }

    #[test]
    fn stores_and_reads_back_with_etag_and_expiry() {
        let d = tmp("rw");
        let c = DiskCache::new(&d, DEFAULT_LIMIT);
        let e = Entry { body: vec![1, 2, 3], etag: Some("\"abc\"".into()), expires: 1234 };
        c.put("osm", t(1), &e).unwrap();
        assert_eq!(c.get("osm", t(1)), Some(e));
        assert!(c.get("osm", t(2)).is_none());
        assert!(c.get("usgs-imagery", t(1)).is_none());
        assert!(d.join(TAG).is_file());
        c.refresh("osm", t(1), 99_999);
        assert_eq!(c.get("osm", t(1)).unwrap().expires, 99_999);
        assert!(!c.get("osm", t(1)).unwrap().is_fresh(100_000));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_tile_is_an_empty_body() {
        let d = tmp("empty");
        let c = DiskCache::new(&d, DEFAULT_LIMIT);
        c.put("osm", t(3), &Entry { body: Vec::new(), etag: None, expires: 5 }).unwrap();
        assert_eq!(c.get("osm", t(3)).unwrap().body, Vec::<u8>::new());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_files_read_as_missing() {
        let d = tmp("corrupt");
        let c = DiskCache::new(&d, DEFAULT_LIMIT);
        let p = c.path("osm", t(1));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        for bytes in [&b""[..], b"LCT1", b"LCT1\0\0\0\0\0\0\0\0\xff\xff", b"PNG nonsense here"] {
            fs::write(&p, bytes).unwrap();
            assert!(c.get("osm", t(1)).is_none());
        }
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn evicts_least_recently_used_over_the_limit() {
        let d = tmp("lru");
        // each entry is 1000 body bytes + 14 header bytes
        let c = DiskCache::new(&d, 3500);
        let e = |n: u8| Entry { body: vec![n; 1000], etag: None, expires: 0 };
        for x in 0..3 {
            c.put("osm", t(x), &e(x as u8)).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        // tile 0 is used again, so tile 1 is now the oldest
        assert!(c.get("osm", t(0)).is_some());
        std::thread::sleep(Duration::from_millis(20));
        c.put("osm", t(3), &e(3)).unwrap();
        assert!(c.bytes() <= 3500 / 10 * 9);
        assert!(c.get("osm", t(1)).is_none(), "least recently used went first");
        assert!(c.get("osm", t(3)).is_some());
        // lowering the limit evicts at once
        c.set_limit(1100);
        assert!(c.bytes() <= 990);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn clear_deletes_only_tiles() {
        let d = tmp("clear");
        let c = DiskCache::new(&d, DEFAULT_LIMIT);
        c.put("osm", t(1), &Entry { body: vec![0; 10], etag: None, expires: 0 }).unwrap();
        let keep = d.join("notes.txt");
        fs::write(&keep, "mine").unwrap();
        assert_eq!(c.clear().unwrap(), 24);
        assert_eq!(c.bytes(), 0);
        assert!(keep.is_file());
        assert!(!d.join("osm").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn keeps_at_least_a_week() {
        let week = 7 * 24 * 3600;
        assert_eq!(keep_for(None).as_secs(), week);
        assert_eq!(keep_for(Some("max-age=60")).as_secs(), week);
        assert_eq!(keep_for(Some("public, max-age=2592000")).as_secs(), 2_592_000);
        assert_eq!(keep_for(Some("max-age=99999999999")).as_secs(), 365 * 24 * 3600);
        assert_eq!(keep_for(Some("no-cache, max-age=abc")).as_secs(), week);
    }
}
