//! Map markers in the catalog: the op and its inverse, persistence, and older libraries.

use crate::journal::{LOG, SNAPSHOT, VERSION};
use crate::*;

fn marker(c: &mut Catalog) -> Marker {
    let id = c.alloc_marker_id();
    let mut m = Marker::new(id, 37.8199, -122.4783, "2026-10-28T10:00:00");
    m.title = "South tower".into();
    m.note = "Paint loss at the base".into();
    m.attachments.push(Attachment {
        id: 1,
        file_name: "inspection.pdf".into(),
        path: format!("Attachments/{}/inspection.pdf", id.0),
        size: 1234,
        kind: AttachmentKind::Pdf,
        added: "2026-10-28T10:01:00".into(),
    });
    m.photos.push(PhotoId(7));
    m
}

#[test]
fn set_marker_adds_replaces_and_removes_with_inverses() {
    let mut c = Catalog::new();
    let m = marker(&mut c);
    let id = m.id;
    let inv = c.apply(Op::SetMarker { id, marker: Some(Box::new(m.clone())) }).unwrap();
    assert_eq!(inv, Op::SetMarker { id, marker: None });
    assert_eq!(c.marker(id), Some(&m));
    let mut edited = m.clone();
    edited.title = "South tower, east face".into();
    let inv2 = c.apply(Op::SetMarker { id, marker: Some(Box::new(edited.clone())) }).unwrap();
    assert_eq!(c.marker(id).map(|m| m.title.as_str()), Some("South tower, east face"));
    c.apply(inv2).unwrap();
    assert_eq!(c.marker(id), Some(&m), "undo puts the old marker back");
    let inv3 = c.apply(Op::SetMarker { id, marker: None }).unwrap();
    assert_eq!(c.markers().count(), 0);
    c.apply(inv3).unwrap();
    assert_eq!(c.marker(id), Some(&m), "undo of a removal brings it back whole");
    // the next id is past the ones in use
    assert!(c.alloc_marker_id().0 > id.0);
}

#[test]
fn a_bad_marker_is_refused_and_changes_nothing() {
    let mut c = Catalog::new();
    let mut m = marker(&mut c);
    let before = c.clone();
    m.lon = 500.0;
    assert!(c.apply(Op::SetMarker { id: m.id, marker: Some(Box::new(m.clone())) }).is_err());
    m.lon = -122.0;
    let other = MarkerId(m.id.0 + 5);
    assert!(c.apply(Op::SetMarker { id: other, marker: Some(Box::new(m.clone())) }).is_err(), "filed under another id");
    m.attachments[0].path = "../../etc/passwd".into();
    assert!(c.apply(Op::SetMarker { id: m.id, marker: Some(Box::new(m)) }).is_err(), "a path out of the library");
    assert_eq!(c, before);
}

#[test]
fn markers_round_trip_through_the_snapshot_and_the_log() {
    let mut c = Catalog::new();
    let m = marker(&mut c);
    let op = Op::SetMarker { id: m.id, marker: Some(Box::new(m.clone())) };
    // the op as a log line reads back as itself
    let line = Catalog::op_to_log_line(&op);
    assert_eq!(serde_json::from_str::<Op>(line.trim()).unwrap(), op);
    c.apply(op).unwrap();
    let back = Catalog::from_snapshot(&c.to_snapshot()).unwrap();
    assert_eq!(back.marker(m.id), Some(&m));
    assert_eq!(back.to_snapshot(), c.to_snapshot());
    // through a journal on a store
    let store = MemStore::new();
    let (mut j, _, _) = Journal::open(Box::new(store.clone())).unwrap();
    j.append(&[Op::SetMarker { id: m.id, marker: Some(Box::new(m.clone())) }]).unwrap();
    drop(j);
    let (_, loaded, _) = Journal::open(Box::new(store)).unwrap();
    assert_eq!(loaded.marker(m.id), Some(&m));
}

/// A library without markers writes nothing about them, so its snapshot is what it was.
#[test]
fn a_catalog_without_markers_serializes_as_before() {
    let mut c = Catalog::new();
    let id = c.alloc_photo_id();
    c.apply(Op::AddPhoto { photo: Box::new(Photo::new(id, Source::File { path: "/p/a.jpg".into() }, "a.jpg", "JPEG", 4, 3, "2026-01-01")) }).unwrap();
    let snap = c.to_snapshot();
    assert!(!snap.contains("markers") && !snap.contains("next_marker"), "{snap}");
}

/// A v9 library (before markers) opens unchanged, with no markers, and is upgraded.
#[test]
fn v9_library_loads_without_markers_and_is_upgraded() {
    let mut c = Catalog::new();
    let id = c.alloc_photo_id();
    c.apply(Op::AddPhoto { photo: Box::new(Photo::new(id, Source::File { path: "/p/a.jpg".into() }, "a.jpg", "JPEG", 4, 3, "2026-01-01")) }).unwrap();
    let m = MemStore::new();
    m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":9,\"seq\":1,\"catalog\":{}}}\n", c.to_snapshot()).into_bytes());
    m.set(LOG, Vec::new());
    let (_, mut loaded, r) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(loaded.to_snapshot(), c.to_snapshot());
    assert_eq!(loaded.markers().count(), 0);
    assert_eq!(r.upgraded_from, Some(9));
    let v: serde_json::Value = serde_json::from_slice(&m.get(SNAPSHOT).unwrap()).unwrap();
    assert_eq!(v["version"].as_u64(), Some(u64::from(VERSION)));
    assert_eq!(loaded.alloc_marker_id(), MarkerId(1));
}

/// A damaged snapshot whose counter is behind its markers hands out a fresh id anyway.
#[test]
fn marker_ids_never_repeat_after_a_damaged_counter() {
    let mut c = Catalog::new();
    let m = marker(&mut c);
    c.apply(Op::SetMarker { id: m.id, marker: Some(Box::new(m.clone())) }).unwrap();
    let snap = c.to_snapshot().replace("\"next_marker\":2", "\"next_marker\":0");
    let mut back = Catalog::from_snapshot(&snap).unwrap();
    assert_ne!(back.alloc_marker_id(), m.id);
}
