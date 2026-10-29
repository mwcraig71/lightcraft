//! A photo's inspection log (`photo.notes`, `photo.addNote`, `photo.updateNote`,
//! `photo.removeNote`): dated entries about what the photo shows, each with a kind (inspection,
//! repair, preservation), the structural element, an optional condition and free text. See
//! [`lightcraft_catalog::notes`] for the record; every change here is one undo step
//! ([`lightcraft_catalog::Op::SetNotes`] with the photo's whole log).
//!
//! With XMP sidecars written automatically, the photo's sidecar gets a plain-text summary of the
//! log in `lc:inspectionLog` (LightCraft's own namespace; the caption is never touched). The
//! original file is never changed.

use serde_json::{Value, json};

use lightcraft_catalog::notes::{self, Condition, ELEMENTS, MAX_TEXT, Note, NoteKind};
use lightcraft_catalog::{Op, PhotoId};

use super::{CommandSpec, always, bad, cmd};
use crate::{Result, Session};

/// An entry as the commands return it.
pub fn note_json(n: &Note) -> Value {
    json!({
        "id": n.id,
        "kind": n.kind.id(),
        "element": n.element,
        "condition": n.condition.map(Condition::id),
        "text": n.text,
        "date": n.date,
        "created": n.created,
        "modified": n.modified,
    })
}

/// A photo's log as the commands return it: newest first, with the worst current condition.
pub fn log_json(s: &Session, id: PhotoId) -> Value {
    let log = s.catalog.notes(id);
    let entries: Vec<Value> = notes::newest_first(log).into_iter().map(note_json).collect();
    json!({"id": id.0, "notes": entries, "worst": notes::worst_condition(log).map(Condition::id), "summary": notes::summary(log)})
}

/// The photo the call names (`id`), else the active one; it must be in the library.
fn photo(s: &Session, p: &Value, c: &str) -> Result<PhotoId> {
    let id = match super::photo_id(p, c)? {
        Some(id) => id,
        None => s.active().ok_or_else(|| bad(c, "no photo: give `id` (a photo id) or select one"))?,
    };
    if s.catalog.photo(id).is_none() {
        return Err(bad(c, format!("no photo {}", id.0)));
    }
    Ok(id)
}

fn entry_id(p: &Value, c: &str) -> Result<u64> {
    match p.get("note") {
        Some(v) => v.as_u64().ok_or_else(|| bad(c, "`note` must be an entry id (a whole number)")),
        None => Err(bad(c, "missing `note` (the entry's id)")),
    }
}

fn kind(p: &Value, c: &str) -> Result<Option<NoteKind>> {
    match p.get("kind") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(k)) => NoteKind::parse(k).map(Some).ok_or_else(|| bad(c, "`kind` must be inspection, repair or preservation")),
        Some(_) => Err(bad(c, "`kind` must be inspection, repair or preservation")),
    }
}

/// `None`: not given; `Some(None)`: cleared (null or ""); `Some(Some(c))`: set.
fn condition(p: &Value, c: &str) -> Result<Option<Option<Condition>>> {
    match p.get("condition") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(Some(None)),
        Some(Value::String(s)) => {
            Condition::parse(s).map(|x| Some(Some(x))).ok_or_else(|| bad(c, "`condition` must be good, fair, poor or critical (or null)"))
        }
        Some(_) => Err(bad(c, "`condition` must be good, fair, poor or critical (or null)")),
    }
}

fn element(p: &Value, c: &str) -> Result<Option<String>> {
    match p.get("element") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(e)) => notes::normalize_element(e).map(Some).map_err(|why| bad(c, why)),
        Some(_) => Err(bad(c, "`element` must be text (Deck, Bearing, Pier…, or Other: what it is)")),
    }
}

fn text(p: &Value, c: &str) -> Result<Option<String>> {
    match p.get("text") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(String::new())),
        Some(Value::String(t)) => {
            let t = t.trim().replace("\r\n", "\n");
            if t.chars().count() > MAX_TEXT {
                return Err(bad(c, format!("`text` is longer than {MAX_TEXT} characters")));
            }
            Ok(Some(t))
        }
        Some(_) => Err(bad(c, "`text` must be text")),
    }
}

fn date(p: &Value, c: &str) -> Result<Option<String>> {
    match p.get("date") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(d)) => {
            let d = d.trim();
            notes::check_date(d).map_err(|why| bad(c, why))?;
            Ok(Some(d.to_string()))
        }
        Some(_) => Err(bad(c, "`date` must be a date written YYYY-MM-DD")),
    }
}

/// Today, as the session's clock has it (`YYYY-MM-DD`; the app's clock is the system's, UTC).
/// A clock that says something else is refused: the call then has to give the date.
pub fn today(s: &Session, c: &str) -> Result<String> {
    let now = (s.clock)();
    match now.get(..10) {
        Some(d) if notes::check_date(d).is_ok() => Ok(d.to_string()),
        _ => Err(bad(c, "give `date` (YYYY-MM-DD): the clock doesn't say what day it is")),
    }
}

fn set(s: &mut Session, id: PhotoId, label: &str, log: Vec<Note>, c: &str) -> Result<()> {
    if let Some(why) = notes::notes_problem(&log) {
        return Err(bad(c, why));
    }
    s.commit(label, Op::SetNotes { id, notes: log })
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let id = photo(s, p, "photo.notes")?;
    Ok(log_json(s, id))
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.addNote";
    let id = photo(s, p, C)?;
    let kind = kind(p, C)?.unwrap_or_default();
    let element = element(p, C)?.ok_or_else(|| bad(C, "missing `element` (Deck, Bearing, Pier…, or Other: what it is)"))?;
    let condition = condition(p, C)?.flatten();
    let text = text(p, C)?.unwrap_or_default();
    let date = match date(p, C)? {
        Some(d) => d,
        None => today(s, C)?,
    };
    let mut log = s.catalog.notes(id).to_vec();
    if log.len() >= notes::MAX_NOTES {
        return Err(bad(C, format!("photo {} already has {} entries, as many as a log holds", id.0, notes::MAX_NOTES)));
    }
    let now = (s.clock)();
    let n = Note { id: notes::next_id(&log), kind, element, condition, text, date, created: now.clone(), modified: now };
    log.push(n.clone());
    set(s, id, "Add Log Entry", log, C)?;
    Ok(json!({"id": id.0, "note": note_json(&n), "log": log_json(s, id)}))
}

fn update(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.updateNote";
    let id = photo(s, p, C)?;
    let nid = entry_id(p, C)?;
    let mut log = s.catalog.notes(id).to_vec();
    let Some(n) = log.iter_mut().find(|n| n.id == nid) else { return Err(bad(C, format!("photo {} has no log entry {nid}", id.0))) };
    let before = n.clone();
    if let Some(k) = kind(p, C)? {
        n.kind = k;
    }
    if let Some(e) = element(p, C)? {
        n.element = e;
    }
    if let Some(c) = condition(p, C)? {
        n.condition = c;
    }
    if let Some(t) = text(p, C)? {
        n.text = t;
    }
    if let Some(d) = date(p, C)? {
        n.date = d;
    }
    let changed = *n != before;
    if changed {
        n.modified = (s.clock)();
    }
    let out = note_json(n);
    if changed {
        set(s, id, "Edit Log Entry", log, C)?;
    }
    Ok(json!({"id": id.0, "changed": changed, "note": out}))
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.removeNote";
    let id = photo(s, p, C)?;
    let nid = entry_id(p, C)?;
    let mut log = s.catalog.notes(id).to_vec();
    let before = log.len();
    log.retain(|n| n.id != nid);
    if log.len() == before {
        return Err(bad(C, format!("photo {} has no log entry {nid}", id.0)));
    }
    set(s, id, "Delete Log Entry", log, C)?;
    Ok(json!({"id": id.0, "removed": nid, "left": s.catalog.notes(id).len()}))
}

fn elements(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({
        "elements": ELEMENTS,
        "kinds": NoteKind::ALL.iter().map(|k| k.id()).collect::<Vec<_>>(),
        "conditions": Condition::ALL.iter().map(|c| c.id()).collect::<Vec<_>>(),
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "photo.notes",
            "Inspection Log",
            [],
            None,
            "{id?: photo id (default: the active photo)} → {id, notes: [{id, kind, element, condition, text, date, created, modified}] newest first, worst: the worst current condition (each element's newest rating) | null, summary: plain text}",
            always,
            list
        ),
        cmd!(
            "photo.addNote",
            "Add Log Entry",
            [],
            None,
            "{id?, kind?: inspection | repair | preservation (default inspection), element: Deck | Wearing surface | Joint | Railing/Barrier | Girder/Beam | Floor beam | Stringer | Truss | Cable/Suspender | Bearing | Pier | Pier cap | Abutment | Wingwall | Footing/Foundation | Culvert | Drainage | Approach | Other[: free text], condition?: good | fair | poor | critical, text?: plain text (≤ 5000 characters), date?: YYYY-MM-DD (default today)} → {id, note, log} — one undo step; with automatic XMP sidecars, the sidecar's lc:inspectionLog gets the log's summary",
            always,
            add
        ),
        cmd!(
            "photo.updateNote",
            "Edit Log Entry",
            [],
            None,
            "{id?, note: entry id, kind?, element?, condition?: … | null (clears), text?, date?} → {id, changed, note} — one undo step",
            always,
            update
        ),
        cmd!("photo.removeNote", "Delete Log Entry", [], None, "{id?, note: entry id} → {id, removed, left} — one undo step", always, remove),
        cmd!(query "photo.noteChoices", "Inspection Log Choices", [], None, "{} → {elements, kinds, conditions}: what an entry may be", always, elements),
    ]
}
