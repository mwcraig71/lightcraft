//! Map markers with attached files (`map.addMarker`, `map.attach`…): a light record of a site,
//! such as a bridge, kept with the library. A marker is a point (latitude, longitude) with a
//! title, a plain-text note, documents attached to it and the photos attached at that spot.
//!
//! - **Files are copied into the library folder**, `<library>/Attachments/<marker id>/<name>`, so
//!   they travel with the library and the original can move or go. A name already taken there
//!   gets `-1`, `-2`… (never overwritten); a copy is written whole or not at all (a failed or
//!   cancelled copy removes what it wrote). A missing or unreadable source, a folder, or a full
//!   disk is reported for that file and the others are still attached.
//! - **An attached photo becomes a photo at that spot**: an image LightCraft reads is imported
//!   like any photo (`library.import`, copied into the library's `Originals/`), its location is
//!   set to the marker's point in the catalog (and its XMP sidecar, never the original's EXIF),
//!   and it is listed on the marker. One already in the library is linked and moved there.
//! - **Removing keeps the files.** Detaching an attachment or deleting a marker changes only the
//!   catalog record (one undo step); the copy stays in `Attachments/<marker id>/`, so undo and
//!   redo always find it. LightCraft never deletes an attached file.
//! - **Opening** goes through [`openable`]: only a document or picture type (PDF, spreadsheets,
//!   text, Markdown, Office files, images…) inside the marker's folder is handed to the system's
//!   default app; anything else (a program, a script, a link pointing out of the library) can
//!   only be shown in its folder.
//!
//! Big copies made for the UI run on a worker ([`stage`]) under an activity row; `map.attach
//! {staged}` then records them.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use lightcraft_catalog::markers::{MAX_NOTE, MAX_TITLE, safe_relative};
use lightcraft_catalog::{Attachment, AttachmentKind, Marker, MarkerId, Op, PhotoId};

use super::{CommandSpec, always, bad, cmd};
use crate::{EngineError, Result, Session};

/// The folder attachments are kept in, inside the library folder.
pub const FOLDER: &str = "Attachments";
/// Most paths one `map.attach` takes.
pub const MAX_PATHS: usize = 500;
/// Copy buffer.
const CHUNK: usize = 1 << 20;

/// What may be handed to the system's default app (lower case, without the dot): documents and
/// pictures, never programs, scripts, links or macro-enabled Office files.
pub const OPENABLE: &[&str] = &[
    "pdf", "xlsx", "xls", "ods", "csv", "tsv", "numbers", "docx", "doc", "odt", "rtf", "pages", "pptx", "ppt", "odp", "key", "txt", "text", "log",
    "md", "markdown", "json", "xml", "kml", "kmz", "geojson", "gpx", "dxf", "dwg", "ifc", "png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "webp",
    "heic", "heif", "mp4", "mov",
];

/// The library folder, when the library is a folder on disk (attachments are kept there).
pub fn library_dir(s: &Session, c: &str) -> Result<PathBuf> {
    match s.library.as_ref() {
        Some(l) if l.on_disk => Ok(l.dir.clone()),
        _ => Err(bad(c, "attached files are kept in the library folder: open a library on disk first")),
    }
}

/// `<library>/Attachments/<id>`.
pub fn marker_dir(library: &Path, id: MarkerId) -> PathBuf {
    library.join(FOLDER).join(id.0.to_string())
}

/// The file an attachment's relative path names inside `library`.
pub fn absolute(library: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(library.to_path_buf(), |p, part| p.join(part))
}

/// A file name that is safe on every system: no separators or characters Windows refuses, no
/// control characters, no trailing dots or spaces, not a reserved device name, at most 120
/// characters (the extension kept). Empty → `attachment`.
pub fn clean_name(name: &str) -> String {
    let mut s: String = name.chars().filter(|c| !c.is_control()).map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    while s.ends_with(['.', ' ']) {
        s.pop();
    }
    let s = s.trim_start().to_string();
    let s = if s.is_empty() || s.chars().all(|c| c == '.') { "attachment".to_string() } else { s };
    let (stem, ext) = split_ext(&s);
    let stem: String = stem.chars().take(100).collect();
    let ext: String = ext.chars().take(20).collect();
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3"];
    let stem = if reserved.iter().any(|r| r.eq_ignore_ascii_case(stem.trim())) { format!("_{stem}") } else { stem };
    if ext.is_empty() { stem } else { format!("{stem}.{ext}") }
}

/// `report.final.pdf` → (`report.final`, `pdf`); a leading dot is part of the stem.
fn split_ext(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(i) if i > 0 => (name.get(..i).unwrap_or(name).to_string(), name.get(i + 1..).unwrap_or("").to_string()),
        _ => (name.to_string(), String::new()),
    }
}

/// A file copied into a marker's folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Copied {
    /// The source's file name (cleaned).
    pub file_name: String,
    /// Relative to the library folder, `/` separated.
    pub rel: String,
    pub size: u64,
}

/// Copy `src` into `dir` (created if needed) under its own name, or the first of `name-1.ext`,
/// `name-2.ext`… that is free. Never overwrites; what a failed or cancelled copy wrote is removed.
/// `rel_dir` is `dir` relative to the library (`Attachments/3`). `progress` gets the bytes copied
/// so far; `cancelled` is asked between chunks.
pub fn copy_into(src: &Path, dir: &Path, rel_dir: &str, progress: &dyn Fn(u64), cancelled: &dyn Fn() -> bool) -> std::result::Result<Copied, String> {
    let meta = std::fs::metadata(src).map_err(|e| format!("can't read it: {e}"))?;
    if meta.is_dir() {
        return Err("it is a folder: attach the files in it".into());
    }
    if !meta.is_file() {
        return Err("it isn't a file".into());
    }
    let mut from = std::fs::File::open(src).map_err(|e| format!("can't read it: {e}"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    let original = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = clean_name(&original);
    let (stem, ext) = split_ext(&name);
    let mut chosen = None;
    for n in 0..10_000u32 {
        let candidate = match (n, ext.is_empty()) {
            (0, _) => name.clone(),
            (_, true) => format!("{stem}-{n}"),
            (_, false) => format!("{stem}-{n}.{ext}"),
        };
        let dest = dir.join(&candidate);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&dest) {
            Ok(f) => {
                chosen = Some((candidate, dest, f));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("can't write into {}: {e}", dir.display())),
        }
    }
    let Some((candidate, dest, mut to)) = chosen else { return Err(format!("no free name for {name} in {}", dir.display())) };
    let fail = |dest: &Path, why: String| {
        let _ = std::fs::remove_file(dest);
        Err(why)
    };
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        if cancelled() {
            drop(to);
            return fail(&dest, "cancelled".into());
        }
        let n = match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                drop(to);
                return fail(&dest, format!("can't read it: {e}"));
            }
        };
        let Some(chunk) = buf.get(..n) else { break };
        if let Err(e) = to.write_all(chunk) {
            drop(to);
            return fail(&dest, write_error(&e));
        }
        done = done.saturating_add(n as u64);
        progress(done);
    }
    if let Err(e) = to.sync_all() {
        drop(to);
        return fail(&dest, write_error(&e));
    }
    Ok(Copied { file_name: name, rel: format!("{rel_dir}/{candidate}"), size: done })
}

fn write_error(e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::StorageFull { "the disk is full".into() } else { format!("can't write the copy: {e}") }
}

/// One file [`stage`] handled: copied, or why not.
#[derive(Clone, Debug)]
pub struct Staged {
    pub source: String,
    pub result: std::result::Result<Copied, String>,
}

/// Copy `paths` into marker `id`'s folder of `library` off the UI thread, reporting bytes to
/// `task` and stopping when it is cancelled. `map.attach {marker, staged}` records the copies.
pub fn stage(library: &Path, id: MarkerId, paths: &[String], task: &crate::activity::TaskHandle) -> Vec<Staged> {
    let dir = marker_dir(library, id);
    let rel_dir = format!("{FOLDER}/{}", id.0);
    let total: u64 = paths.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
    task.progress(0, total);
    let mut base = 0u64;
    let mut out = Vec::new();
    for p in paths {
        let name = Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        task.detail(&name);
        let result = copy_into(Path::new(p), &dir, &rel_dir, &|n| task.progress(base.saturating_add(n), total), &|| task.is_cancelled());
        if let Ok(c) = &result {
            base = base.saturating_add(c.size);
        }
        out.push(Staged { source: p.clone(), result });
    }
    out
}

/// Whether an attachment may be handed to the system's default app: a document or picture type
/// ([`OPENABLE`]), a regular file, inside the library's attachments folder even after links are
/// followed. Returns the absolute path, or why not.
pub fn openable(library: &Path, a: &Attachment) -> std::result::Result<String, String> {
    if !safe_relative(&a.path) {
        return Err(format!("{} isn't in the library's attachments folder", a.file_name));
    }
    let path = absolute(library, &a.path);
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    if !OPENABLE.contains(&ext.as_str()) {
        return Err(format!("{} isn't a document or picture type LightCraft opens; use Show in Folder", a.file_name));
    }
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(format!("{} isn't a plain file, so it isn't opened", a.file_name)),
        Err(e) => return Err(format!("{} is missing from the library ({e})", a.file_name)),
    }
    let root = library.join(FOLDER).canonicalize().map_err(|e| format!("{}: {e}", a.file_name))?;
    let real = path.canonicalize().map_err(|e| format!("{}: {e}", a.file_name))?;
    if !real.starts_with(&root) {
        return Err(format!("{} points out of the library, so it isn't opened", a.file_name));
    }
    let abs = std::path::absolute(&path).map_err(|e| format!("{}: {e}", a.file_name))?;
    Ok(abs.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------- params

fn marker_id(p: &Value, key: &str, c: &str) -> Result<MarkerId> {
    match p.get(key) {
        Some(v) => v.as_u64().map(MarkerId).ok_or_else(|| bad(c, format!("`{key}` must be a marker id (a whole number)"))),
        None => Err(bad(c, format!("missing `{key}` (a marker id)"))),
    }
}

fn marker_of(s: &Session, id: MarkerId, c: &str) -> Result<Marker> {
    s.catalog.marker(id).cloned().ok_or_else(|| bad(c, format!("no marker {}", id.0)))
}

fn lat_lon(p: &Value, c: &str, required: bool) -> Result<Option<(f64, f64)>> {
    let num = |k: &str| -> Result<Option<f64>> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_f64().filter(|x| x.is_finite()).map(Some).ok_or_else(|| bad(c, format!("`{k}` must be a number"))),
        }
    };
    match (num("lat")?, num("lon")?) {
        (Some(lat), Some(lon)) => {
            if lat.abs() > 90.0 || lon.abs() > 180.0 {
                return Err(bad(c, "latitude must be within ±90 and longitude within ±180"));
            }
            Ok(Some((lat, lon)))
        }
        (None, None) if !required => Ok(None),
        _ => Err(bad(c, "give both `lat` and `lon`")),
    }
}

/// An optional text param, trimmed at the ends, at most `max` characters.
fn text(p: &Value, key: &str, max: usize, c: &str) -> Result<Option<String>> {
    match p.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(String::new())),
        Some(Value::String(t)) => {
            let t = t.trim().to_string();
            if t.chars().count() > max {
                return Err(bad(c, format!("`{key}` is longer than {max} characters")));
            }
            Ok(Some(t))
        }
        Some(_) => Err(bad(c, format!("`{key}` must be text"))),
    }
}

fn paths(p: &Value, key: &str, c: &str) -> Result<Vec<String>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => {
            if a.len() > MAX_PATHS {
                return Err(bad(c, format!("at most {MAX_PATHS} files at a time")));
            }
            a.iter()
                .map(|v| {
                    v.as_str()
                        .map(str::trim)
                        .filter(|x| !x.is_empty())
                        .map(str::to_string)
                        .ok_or_else(|| bad(c, format!("`{key}` must be file paths")))
                })
                .collect()
        }
        Some(_) => Err(bad(c, format!("`{key}` must be a list of file paths"))),
    }
}

// ---------------------------------------------------------------- results

/// A marker as the commands return it: attachments with their absolute path and whether the file
/// is there (when the library is on disk), and the photos still in the library.
pub fn marker_json(s: &Session, m: &Marker) -> Value {
    let lib = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
    let attachments: Vec<Value> = m
        .attachments
        .iter()
        .map(|a| {
            let mut v = json!({"id": a.id, "fileName": a.file_name, "path": a.path, "size": a.size, "kind": a.kind.id(), "added": a.added});
            if let Some(lib) = &lib {
                let abs = absolute(lib, &a.path);
                v["exists"] = json!(abs.is_file());
                v["file"] = json!(abs.to_string_lossy());
            }
            v
        })
        .collect();
    let photos: Vec<u64> = m.photos.iter().filter(|id| s.catalog.photo(**id).is_some_and(|p| !p.deleted)).map(|id| id.0).collect();
    json!({
        "id": m.id.0, "lat": m.lat, "lon": m.lon, "title": m.title, "note": m.note,
        "created": m.created, "modified": m.modified, "attachments": attachments, "photos": photos,
    })
}

// ---------------------------------------------------------------- commands

fn add_marker(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "map.addMarker";
    let Some((lat, lon)) = lat_lon(p, C, true)? else { return Err(bad(C, "give `lat` and `lon`")) };
    let title = text(p, "title", MAX_TITLE, C)?.unwrap_or_default();
    let note = text(p, "note", MAX_NOTE, C)?.unwrap_or_default();
    let id = new_marker(s, lat, lon, title, note)?;
    let m = marker_of(s, id, C)?;
    Ok(json!({"id": id.0, "marker": marker_json(s, &m)}))
}

fn new_marker(s: &mut Session, lat: f64, lon: f64, title: String, note: String) -> Result<MarkerId> {
    let id = s.catalog.alloc_marker_id();
    let mut m = Marker::new(id, lat, lon, &(s.clock)());
    m.title = title;
    m.note = note;
    s.commit("Add Marker", Op::SetMarker { id, marker: Some(Box::new(m)) })?;
    Ok(id)
}

fn update_marker(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "map.updateMarker";
    let id = marker_id(p, "id", C)?;
    let mut m = marker_of(s, id, C)?;
    let before = m.clone();
    if let Some(t) = text(p, "title", MAX_TITLE, C)? {
        m.title = t;
    }
    if let Some(n) = text(p, "note", MAX_NOTE, C)? {
        m.note = n;
    }
    if let Some((lat, lon)) = lat_lon(p, C, false)? {
        m.lat = lat;
        m.lon = lon;
    }
    let changed = m != before;
    if changed {
        m.modified = (s.clock)();
        s.commit("Edit Marker", Op::SetMarker { id, marker: Some(Box::new(m.clone())) })?;
    }
    Ok(json!({"changed": changed, "marker": marker_json(s, &m)}))
}

fn remove_marker(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "map.removeMarker";
    let id = marker_id(p, "id", C)?;
    let m = marker_of(s, id, C)?;
    s.commit("Delete Marker", Op::SetMarker { id, marker: None })?;
    // the copies stay in Attachments/<id>/: undo brings the record back to them
    Ok(json!({"removed": id.0, "attachments": m.attachments.len(), "photos": m.photos.len()}))
}

fn detach(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "map.detach";
    let id = marker_id(p, "marker", C)?;
    let mut m = marker_of(s, id, C)?;
    let (label, what) = match (p.get("attachment"), p.get("photo")) {
        (Some(a), None) => {
            let a = a.as_u64().ok_or_else(|| bad(C, "`attachment` must be an attachment id (a whole number)"))?;
            let before = m.attachments.len();
            m.attachments.retain(|x| x.id != a);
            if m.attachments.len() == before {
                return Err(bad(C, format!("marker {} has no attachment {a}", id.0)));
            }
            ("Remove Attachment", json!({"attachment": a}))
        }
        (None, Some(ph)) => {
            let ph = ph.as_u64().ok_or_else(|| bad(C, "`photo` must be a photo id (a whole number)"))?;
            let before = m.photos.len();
            m.photos.retain(|x| x.0 != ph);
            if m.photos.len() == before {
                return Err(bad(C, format!("photo {ph} isn't attached to marker {}", id.0)));
            }
            ("Remove Photo from Marker", json!({"photo": ph}))
        }
        _ => return Err(bad(C, "give `attachment` (an attachment id) or `photo` (a photo id)")),
    };
    m.modified = (s.clock)();
    s.commit(label, Op::SetMarker { id, marker: Some(Box::new(m.clone())) })?;
    Ok(json!({"detached": what, "marker": marker_json(s, &m)}))
}

/// `map.attach`: see the module docs.
fn attach(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "map.attach";
    let files = paths(p, "paths", C)?;
    let staged = match p.get("staged") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) if a.len() <= MAX_PATHS => a.clone(),
        Some(_) => return Err(bad(C, format!("`staged` must be a list of at most {MAX_PATHS} copies"))),
    };
    if files.is_empty() && staged.is_empty() {
        return Err(bad(C, "no files: give `paths`"));
    }
    let lib = library_dir(s, C)?;
    let point = lat_lon(p, C, false)?;
    let undo0 = s.undo.len();
    let created = match (p.get("marker"), point) {
        (Some(_), _) => None,
        (None, Some((lat, lon))) => Some(new_marker(s, lat, lon, String::new(), String::new())?),
        (None, None) => return Err(bad(C, "give `marker` (a marker id), or `lat` and `lon` for a new marker there")),
    };
    let id = match created {
        Some(id) => id,
        None => marker_id(p, "marker", C)?,
    };
    let r = attach_to(s, &lib, id, &files, &staged);
    match r {
        Ok(v) => {
            let n = s.undo.len().saturating_sub(undo0);
            let count = v["attached"].as_array().map_or(0, Vec::len) + v["photos"].as_array().map_or(0, Vec::len);
            s.merge_undo(n, &format!("Attach {count} File{}", if count == 1 { "" } else { "s" }));
            Ok(v)
        }
        Err(e) => {
            // a marker made for these files goes again when none could be attached
            if let Some(id) = created
                && s.catalog.marker(id).is_some_and(|m| m.attachments.is_empty() && m.photos.is_empty())
            {
                let _ = s.undo_step();
                s.redo.clear();
            }
            Err(e)
        }
    }
}

fn attach_to(s: &mut Session, lib: &Path, id: MarkerId, files: &[String], staged: &[Value]) -> Result<Value> {
    const C: &str = "map.attach";
    let mut m = marker_of(s, id, C)?;
    let dir = marker_dir(lib, id);
    let rel_dir = format!("{FOLDER}/{}", id.0);
    let now = (s.clock)();
    let mut errors: Vec<Value> = Vec::new();
    let mut attached: Vec<Attachment> = Vec::new();
    let mut images: Vec<String> = Vec::new();
    let room = lightcraft_catalog::markers::MAX_ATTACHMENTS.saturating_sub(m.attachments.len());
    let add = |m: &mut Marker, attached: &mut Vec<Attachment>, c: Copied| {
        let a = Attachment {
            id: m.next_attachment_id().max(attached.last().map_or(0, |x: &Attachment| x.id.saturating_add(1))),
            kind: AttachmentKind::of(&c.file_name),
            file_name: c.file_name,
            path: c.rel,
            size: c.size,
            added: now.clone(),
        };
        attached.push(a);
    };
    for f in files {
        let path = Path::new(f);
        if !path.exists() {
            errors.push(json!({"path": f, "error": "the file is missing"}));
            continue;
        }
        if crate::import::is_supported(path) && path.is_file() {
            images.push(f.clone());
            continue;
        }
        if attached.len() >= room {
            errors.push(json!({"path": f, "error": "the marker holds as many attachments as it can"}));
            continue;
        }
        match copy_into(path, &dir, &rel_dir, &|_| {}, &|| false) {
            Ok(c) => add(&mut m, &mut attached, c),
            Err(e) => errors.push(json!({"path": f, "error": e})),
        }
    }
    for st in staged {
        let rel = st.get("path").and_then(Value::as_str).unwrap_or_default();
        let ok = safe_relative(rel) && rel.strip_prefix(&rel_dir).is_some_and(|r| r.starts_with('/') && r.matches('/').count() == 1);
        let file = absolute(lib, rel);
        let Ok(meta) = std::fs::symlink_metadata(&file).map_err(|e| e.to_string()) else {
            errors.push(json!({"path": rel, "error": "the copy is missing"}));
            continue;
        };
        if !ok || !meta.is_file() {
            errors.push(json!({"path": rel, "error": format!("not a copy in this marker's folder ({rel_dir})")}));
            continue;
        }
        if m.attachments.iter().chain(attached.iter()).any(|a| a.path == rel) {
            continue;
        }
        if attached.len() >= room {
            errors.push(json!({"path": rel, "error": "the marker holds as many attachments as it can"}));
            continue;
        }
        let name = st.get("fileName").and_then(Value::as_str).map(clean_name).unwrap_or_else(|| clean_name(rel.rsplit('/').next().unwrap_or(rel)));
        add(&mut m, &mut attached, Copied { file_name: name, rel: rel.to_string(), size: meta.len() });
    }
    // photos: imported like any photo, then placed at the marker's point
    let mut photos: Vec<PhotoId> = Vec::new();
    if !images.is_empty() {
        match s.execute("library.import", &json!({"paths": images, "mode": "copy"})) {
            Ok(r) => {
                let ids = |k: &str| -> Vec<u64> { r[k].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default() };
                photos.extend(ids("imported").into_iter().chain(ids("restored")).map(PhotoId));
                for d in r["duplicates"].as_array().into_iter().flatten() {
                    match d["existing"].as_u64() {
                        Some(e) => photos.push(PhotoId(e)),
                        None => errors.push(json!({"path": d["path"], "error": "already in the library"})),
                    }
                }
                for f in r["failed"].as_array().into_iter().flatten() {
                    errors.push(
                        json!({"path": f[0], "error": format!("couldn't be imported as a photo: {}", f[1].as_str().unwrap_or("unknown error"))}),
                    );
                }
            }
            Err(e) => errors.extend(images.iter().map(|f| json!({"path": f, "error": format!("couldn't be imported as a photo: {e}")}))),
        }
    }
    photos.dedup();
    for ph in &photos {
        let Some(photo) = s.catalog.photo(*ph) else { continue };
        let mut meta = photo.meta.clone();
        if meta.gps != Some((m.lat, m.lon)) {
            meta.gps = Some((m.lat, m.lon));
            // the fix the file recorded was for another place
            meta.gps_accuracy = None;
            s.commit("Set Location", Op::SetMeta { id: *ph, meta: Box::new(meta) })?;
        }
    }
    let new_photos: Vec<PhotoId> = photos.iter().copied().filter(|ph| !m.photos.contains(ph)).collect();
    if attached.is_empty() && photos.is_empty() {
        let why = errors.first().and_then(|e| e["error"].as_str()).unwrap_or("nothing to attach").to_string();
        return Err(EngineError::BadParams { cmd: C.into(), msg: format!("nothing attached: {why}") });
    }
    m.attachments.extend(attached.iter().cloned());
    m.photos.extend(new_photos.iter().copied());
    m.modified = now;
    if !attached.is_empty() || !new_photos.is_empty() {
        s.commit("Attach Files", Op::SetMarker { id, marker: Some(Box::new(m.clone())) })?;
    }
    let attached_json: Vec<Value> =
        attached.iter().map(|a| json!({"id": a.id, "fileName": a.file_name, "path": a.path, "size": a.size, "kind": a.kind.id()})).collect();
    Ok(json!({
        "marker": id.0,
        "attached": attached_json,
        "photos": photos.iter().map(|p| p.0).collect::<Vec<_>>(),
        "errors": errors,
        "card": marker_json(s, &m),
    }))
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "map.markers";
    let only = match p.get("id") {
        None | Some(Value::Null) => None,
        Some(_) => Some(marker_id(p, "id", C)?),
    };
    let ms: Vec<Value> = s.catalog.markers().filter(|m| only.is_none_or(|o| o == m.id)).map(|m| marker_json(s, m)).collect();
    if only.is_some() && ms.is_empty() {
        return Err(bad(C, "no such marker"));
    }
    Ok(json!({"markers": ms}))
}

fn attachment_path(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "map.attachmentPath";
    let id = marker_id(p, "marker", C)?;
    let m = marker_of(s, id, C)?;
    let aid = p.get("attachment").and_then(Value::as_u64).ok_or_else(|| bad(C, "`attachment` must be an attachment id (a whole number)"))?;
    let a = m.attachment(aid).ok_or_else(|| bad(C, format!("marker {} has no attachment {aid}", id.0)))?;
    let lib = library_dir(s, C)?;
    let file = absolute(&lib, &a.path);
    let mut r = json!({"path": file.to_string_lossy(), "exists": file.is_file(), "fileName": a.file_name});
    match openable(&lib, a) {
        Ok(_) => r["openable"] = json!(true),
        Err(why) => {
            r["openable"] = json!(false);
            r["reason"] = json!(why);
        }
    }
    Ok(r)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "map.addMarker",
            "Add Marker",
            [],
            None,
            "{lat, lon, title?, note?: plain text} → {id, marker} — a point on the Map view to attach files to (one undo step)",
            always,
            add_marker
        ),
        cmd!(
            "map.updateMarker",
            "Edit Marker",
            [],
            None,
            "{id, title?, note?, lat?, lon?} → {changed, marker} (attached photos keep their own location)",
            always,
            update_marker
        ),
        cmd!(
            "map.removeMarker",
            "Delete Marker",
            [],
            None,
            "{id} → {removed, attachments, photos} — the record only: the copied files stay in the library's Attachments/<id>/ so undo finds them; attached photos stay in the library",
            always,
            remove_marker
        ),
        cmd!(
            "map.attach",
            "Attach Files to Marker",
            [],
            None,
            "{marker? | lat?, lon? (a new marker there), paths: [file], staged?: [{path: Attachments/<id>/<name>, fileName?}]} → {marker, attached: [{id, fileName, path, size, kind}], photos: [photoId], errors: [{path, error}], card} — files are copied into <library>/Attachments/<marker>/ (a taken name gets -1, -2…); images LightCraft reads are imported as photos (copied into Originals/) with their location set to the marker's point (catalog and XMP sidecar, not the original) and listed on the marker; one undo step",
            always,
            attach
        ),
        cmd!(
            "map.detach",
            "Remove Attachment",
            [],
            None,
            "{marker, attachment} | {marker, photo} → {detached, marker} — the record only: the copy stays in the library (undo finds it), a photo stays in the library",
            always,
            detach
        ),
        cmd!(query "map.markers", "Map Markers", [], None, "{id?} → {markers: [{id, lat, lon, title, note, created, modified, attachments: [{id, fileName, path, size, kind, added, exists, file}], photos}]}", always, list),
        cmd!(
            query "map.attachmentPath",
            "Attachment File",
            [],
            None,
            "{marker, attachment} → {path, exists, fileName, openable, reason?} — where the library's copy is, and whether it is a type handed to the default app",
            always,
            attachment_path
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(clean_name("inspection.pdf"), "inspection.pdf");
        assert_eq!(clean_name("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(clean_name("CON.txt"), "_CON.txt");
        assert_eq!(clean_name("..."), "attachment");
        assert_eq!(clean_name("notes. "), "notes");
        assert_eq!(clean_name(""), "attachment");
        assert!(clean_name(&"x".repeat(500)).chars().count() <= 100);
    }

    #[test]
    fn copies_never_overwrite_and_a_missing_source_is_an_error() {
        let d = std::env::temp_dir().join(format!("lc-markers-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let src = d.join("notes.txt");
        std::fs::write(&src, b"hello").unwrap();
        let out = d.join("lib").join("Attachments").join("1");
        let a = copy_into(&src, &out, "Attachments/1", &|_| {}, &|| false).unwrap();
        let b = copy_into(&src, &out, "Attachments/1", &|_| {}, &|| false).unwrap();
        assert_eq!((a.rel.as_str(), b.rel.as_str()), ("Attachments/1/notes.txt", "Attachments/1/notes-1.txt"));
        assert_eq!(std::fs::read(out.join("notes-1.txt")).unwrap(), b"hello");
        assert!(copy_into(&d.join("gone.txt"), &out, "Attachments/1", &|_| {}, &|| false).is_err());
        assert!(copy_into(&d, &out, "Attachments/1", &|_| {}, &|| false).unwrap_err().contains("folder"));
        // a cancelled copy leaves nothing behind
        let e = copy_into(&src, &out, "Attachments/1", &|_| {}, &|| true).unwrap_err();
        assert_eq!(e, "cancelled");
        assert!(!out.join("notes-2.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
