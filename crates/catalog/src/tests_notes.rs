//! Photo inspection logs in the catalog: the op and its inverse, validation, persistence, and
//! older libraries.

use crate::journal::{LOG, SNAPSHOT, VERSION};
use crate::notes::{Condition, Note, NoteKind};
use crate::*;

fn photo(c: &mut Catalog) -> PhotoId {
    let id = c.alloc_photo_id();
    c.apply(Op::AddPhoto { photo: Box::new(Photo::new(id, Source::File { path: format!("/p/{}.jpg", id.0) }, "p.jpg", "JPEG", 4, 3, "2026-01-01")) })
        .unwrap();
    id
}

fn entry(id: u64) -> Note {
    Note {
        id,
        kind: NoteKind::Inspection,
        element: "Bearing".into(),
        condition: Some(Condition::Poor),
        text: "Bearing 3 squeals under load".into(),
        date: "2026-10-28".into(),
        created: "2026-10-28T10:00:00".into(),
        modified: "2026-10-28T10:00:00".into(),
    }
}

#[test]
fn set_notes_adds_edits_and_clears_with_inverses() {
    let mut c = Catalog::new();
    let id = photo(&mut c);
    assert!(c.notes(id).is_empty());
    let inv = c.apply(Op::SetNotes { id, notes: vec![entry(1)] }).unwrap();
    assert_eq!(inv, Op::SetNotes { id, notes: Vec::new() });
    assert_eq!(c.notes(id), &[entry(1)]);
    let mut edited = entry(1);
    edited.condition = Some(Condition::Critical);
    let inv2 = c.apply(Op::SetNotes { id, notes: vec![edited.clone(), entry(2)] }).unwrap();
    assert_eq!(c.notes(id).len(), 2);
    c.apply(inv2).unwrap();
    assert_eq!(c.notes(id), &[entry(1)], "undo puts the old log back");
    let inv3 = c.apply(Op::SetNotes { id, notes: Vec::new() }).unwrap();
    assert!(c.notes(id).is_empty());
    assert_eq!(c.all_notes().count(), 0, "an empty log keeps nothing");
    c.apply(inv3).unwrap();
    assert_eq!(c.notes(id), &[entry(1)]);
    // the op names its photo (sidecars, Local records)
    let mut seen = Vec::new();
    Op::SetNotes { id, notes: Vec::new() }.photo_ids(&mut |p| seen.push(p));
    assert_eq!(seen, vec![id]);
}

#[test]
fn a_bad_log_is_refused_and_changes_nothing() {
    let mut c = Catalog::new();
    let id = photo(&mut c);
    c.apply(Op::SetNotes { id, notes: vec![entry(1)] }).unwrap();
    let before = c.clone();
    let revision = c.revision;
    let mut bad_date = entry(2);
    bad_date.date = "2026-13-01".into();
    let mut long = entry(2);
    long.text = "x".repeat(notes::MAX_TEXT + 1);
    let mut element = entry(2);
    element.element = String::new();
    for notes in [vec![entry(1), bad_date], vec![long], vec![element], vec![entry(1), entry(1)]] {
        assert!(c.apply(Op::SetNotes { id, notes }).is_err());
    }
    assert_eq!(c, before);
    assert_eq!(c.revision, revision);
}

#[test]
fn notes_round_trip_through_the_snapshot_and_the_log() {
    let mut c = Catalog::new();
    let id = photo(&mut c);
    let op = Op::SetNotes { id, notes: vec![entry(1), entry(2)] };
    let line = Catalog::op_to_log_line(&op);
    assert_eq!(serde_json::from_str::<Op>(line.trim()).unwrap(), op);
    c.apply(op.clone()).unwrap();
    let back = Catalog::from_snapshot(&c.to_snapshot()).unwrap();
    assert_eq!(back.notes(id), c.notes(id));
    assert_eq!(back.to_snapshot(), c.to_snapshot());
    let store = MemStore::new();
    let (mut j, _, _) = Journal::open(Box::new(store.clone())).unwrap();
    j.append(&[Op::AddPhoto { photo: Box::new((**c.photo(id).unwrap()).clone()) }, op]).unwrap();
    drop(j);
    let (_, loaded, _) = Journal::open(Box::new(store)).unwrap();
    assert_eq!(loaded.notes(id).len(), 2);
}

/// A library without logs writes nothing about them, so its snapshot is what it was.
#[test]
fn a_catalog_without_notes_serializes_as_before() {
    let mut c = Catalog::new();
    photo(&mut c);
    assert!(!c.to_snapshot().contains("notes"));
}

/// A v10 library (markers, before logs) opens unchanged, with no logs, and is upgraded to v11.
#[test]
fn v10_library_loads_unchanged_and_is_upgraded() {
    let mut c = Catalog::new();
    let id = photo(&mut c);
    let mid = c.alloc_marker_id();
    c.apply(Op::SetMarker { id: mid, marker: Some(Box::new(Marker::new(mid, 37.8, -122.4, "2026-10-01T00:00:00"))) }).unwrap();
    let base = c.clone();
    let rating = Op::SetRating { id, rating: 3 };
    c.apply(rating.clone()).unwrap();
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":10,\"seq\":2,\"catalog\":{}}}\n", base.to_snapshot()).into_bytes());
    m.set(LOG, format!("{}\n", crate::journal::encode_record(3, &rating)).into_bytes());
    let (_, loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(loaded.to_snapshot(), c.to_snapshot());
    assert_eq!(loaded.marker(mid).map(|m| m.id), Some(mid));
    assert!(loaded.notes(id).is_empty());
    assert_eq!(r.upgraded_from, Some(10));
    let v: serde_json::Value = serde_json::from_slice(&m.get(SNAPSHOT).unwrap()).unwrap();
    assert_eq!(v["version"].as_u64(), Some(u64::from(VERSION)));
    assert_eq!(VERSION, 11);
}
