//! A photo's inspection log: `photo.addNote`, `photo.updateNote`, `photo.removeNote`,
//! `photo.notes`, undo / redo, hostile params, the XMP sidecar's summary and the log in
//! `photo.exportLocations`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use lightcraft_catalog::{Op, PhotoId};

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-notes-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, seed: u8) {
    let (w, h) = (40usize, 24usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 6) as u8, (i / w * 9) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn demo() -> (Session, u64) {
    let mut s = Session::with_demo();
    let id = s.visible_cloned()[0].0;
    (s, id)
}

fn add(s: &mut Session, p: Value) -> Value {
    s.execute("photo.addNote", &p).unwrap()
}

#[test]
fn add_edit_and_delete_are_each_one_undo_step() {
    let (mut s, id) = demo();
    let r = add(
        &mut s,
        json!({"id": id, "kind": "inspection", "element": "bearing", "condition": "poor", "text": "Squeals under load", "date": "2026-09-01"}),
    );
    let n1 = r["note"]["id"].as_u64().unwrap();
    assert_eq!(r["note"]["element"], "Bearing", "normalized: {r}");
    assert_eq!(r["note"]["condition"], "poor");
    // the date defaults to today (the session's clock)
    let r = add(&mut s, json!({"id": id, "kind": "repair", "element": "Lighting pole", "text": "Replaced lamp"}));
    let n2 = r["note"]["id"].as_u64().unwrap();
    assert_eq!(r["note"]["date"], crate::cmd::notes::today(&s, "t").unwrap());
    assert_eq!(r["note"]["element"], "Other: Lighting pole");
    assert_eq!(r["note"]["condition"], Value::Null);
    let log = s.execute("photo.notes", &json!({"id": id})).unwrap();
    assert_eq!(log["notes"].as_array().unwrap().len(), 2);
    assert_eq!(log["notes"][0]["id"], n2, "newest first: {log}");
    assert_eq!(log["worst"], "poor");
    assert!(log["summary"].as_str().unwrap().contains("Bearing · Poor: Squeals under load"), "{log}");
    // edit: one field changes, the rest stays
    let r = s.execute("photo.updateNote", &json!({"id": id, "note": n1, "condition": "critical"})).unwrap();
    assert_eq!((r["changed"].as_bool(), r["note"]["text"].as_str()), (Some(true), Some("Squeals under load")));
    // the same values again change nothing and add no step
    let steps = s.undo.len();
    let r = s.execute("photo.updateNote", &json!({"id": id, "note": n1, "condition": "critical"})).unwrap();
    assert_eq!(r["changed"], false);
    assert_eq!(s.undo.len(), steps);
    // a condition can be cleared
    s.execute("photo.updateNote", &json!({"id": id, "note": n2, "condition": null, "text": "Lamp replaced"})).unwrap();
    s.execute("photo.removeNote", &json!({"id": id, "note": n1})).unwrap();
    assert_eq!(s.catalog.notes(PhotoId(id)).len(), 1);
    // undo walks back one change at a time
    assert_eq!(s.undo_step().unwrap(), "Delete Log Entry");
    assert_eq!(s.catalog.notes(PhotoId(id)).len(), 2);
    assert_eq!(s.undo_step().unwrap(), "Edit Log Entry");
    assert_eq!(s.undo_step().unwrap(), "Edit Log Entry");
    assert_eq!(s.catalog.notes(PhotoId(id)).iter().find(|n| n.id == n1).unwrap().condition, Some(lightcraft_catalog::Condition::Poor));
    assert_eq!(s.undo_step().unwrap(), "Add Log Entry");
    assert_eq!(s.undo_step().unwrap(), "Add Log Entry");
    assert!(s.catalog.notes(PhotoId(id)).is_empty());
    s.redo_step().unwrap();
    assert_eq!(s.catalog.notes(PhotoId(id)).len(), 1);
}

#[test]
fn the_active_photo_is_the_default() {
    let (mut s, id) = demo();
    s.execute("library.select", &json!({"ids": [id]})).unwrap();
    add(&mut s, json!({"element": "Deck", "text": "Cracking"}));
    assert_eq!(s.catalog.notes(PhotoId(id)).len(), 1);
    assert_eq!(s.execute("photo.notes", &json!({})).unwrap()["id"], id);
}

#[test]
fn hostile_params_are_refused_and_change_nothing() {
    let (mut s, id) = demo();
    add(&mut s, json!({"id": id, "element": "Deck"}));
    let before = s.catalog.notes(PhotoId(id)).to_vec();
    let steps = s.undo.len();
    let long = "x".repeat(lightcraft_catalog::notes::MAX_TEXT + 1);
    for p in [
        json!({"id": id}),
        json!({"id": id, "element": ""}),
        json!({"id": id, "element": 7}),
        json!({"id": id, "element": "Deck\nDrop table"}),
        json!({"id": id, "element": "x".repeat(300)}),
        json!({"id": id, "element": "Deck", "kind": "demolition"}),
        json!({"id": id, "element": "Deck", "kind": 3}),
        json!({"id": id, "element": "Deck", "condition": "awful"}),
        json!({"id": id, "element": "Deck", "text": long}),
        json!({"id": id, "element": "Deck", "text": ["a"]}),
        json!({"id": id, "element": "Deck", "date": "2026-02-30"}),
        json!({"id": id, "element": "Deck", "date": "yesterday"}),
        json!({"id": id, "element": "Deck", "date": 20261028}),
        json!({"id": 999_999, "element": "Deck"}),
        json!({"id": "1", "element": "Deck"}),
        json!({"id": -1, "element": "Deck"}),
    ] {
        assert!(s.execute("photo.addNote", &p).is_err(), "{p}");
    }
    for p in [
        json!({"id": id}),
        json!({"id": id, "note": 99}),
        json!({"id": id, "note": "1"}),
        json!({"id": id, "note": 1, "date": "2026-13-01"}),
        json!({"id": id, "note": 1, "element": ""}),
    ] {
        assert!(s.execute("photo.updateNote", &p).is_err(), "{p}");
    }
    assert!(s.execute("photo.removeNote", &json!({"id": id, "note": 42})).is_err());
    assert!(s.execute("photo.removeNote", &json!({"id": id})).is_err());
    assert_eq!(s.catalog.notes(PhotoId(id)), before.as_slice());
    assert_eq!(s.undo.len(), steps);
}

#[test]
fn a_full_log_takes_no_more() {
    let (mut s, id) = demo();
    let full: Vec<lightcraft_catalog::Note> = (1..=lightcraft_catalog::notes::MAX_NOTES as u64)
        .map(|i| lightcraft_catalog::Note {
            id: i,
            kind: lightcraft_catalog::NoteKind::Inspection,
            element: "Deck".into(),
            condition: None,
            text: String::new(),
            date: "2026-10-01".into(),
            created: String::new(),
            modified: String::new(),
        })
        .collect();
    s.commit("setup", Op::SetNotes { id: PhotoId(id), notes: full }).unwrap();
    let e = s.execute("photo.addNote", &json!({"id": id, "element": "Deck"})).unwrap_err().to_string();
    assert!(e.contains("as many as a log holds"), "{e}");
}

#[test]
fn the_sidecar_gets_a_summary_and_the_caption_stays() {
    let src = temp_dir("sidecar");
    write_png(&src.join("pier.png"), 4);
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id.0;
    s.execute("photo.setMeta", &json!({"ids": [id], "caption": "South pier from the east"})).unwrap();
    s.execute("library.xmpPreferences", &json!({"autoWrite": true})).unwrap();
    let original = std::fs::read(src.join("pier.png")).unwrap();
    add(&mut s, json!({"id": id, "element": "Pier", "condition": "fair", "text": "Spalling <2 cm> & rust", "date": "2026-10-02"}));
    let xmp = std::fs::read_to_string(src.join("pier.xmp")).expect("the sidecar was written");
    assert!(xmp.contains("<lc:inspectionLog>2026-10-02 · Inspection · Pier · Fair: Spalling &lt;2 cm&gt; &amp; rust</lc:inspectionLog>"), "{xmp}");
    assert!(xmp.contains("South pier from the east"), "the caption is kept: {xmp}");
    assert_eq!(std::fs::read(src.join("pier.png")).unwrap(), original, "the original is never changed");
    // an emptied log takes the summary out again
    let n = s.catalog.notes(PhotoId(id))[0].id;
    s.execute("photo.removeNote", &json!({"id": id, "note": n})).unwrap();
    let xmp = std::fs::read_to_string(src.join("pier.xmp")).unwrap();
    assert!(!xmp.contains("inspectionLog"), "{xmp}");
    assert!(xmp.contains("South pier from the east"));
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn exported_locations_carry_the_log() {
    let (mut s, id) = demo();
    let mut meta = s.catalog.photo(PhotoId(id)).unwrap().meta.clone();
    meta.gps = Some((37.8199, -122.4783));
    s.commit("setup", Op::SetMeta { id: PhotoId(id), meta: Box::new(meta) }).unwrap();
    add(&mut s, json!({"id": id, "element": "Cable/Suspender", "condition": "critical", "text": "Broken wire", "date": "2026-10-03"}));
    add(&mut s, json!({"id": id, "kind": "preservation", "element": "Railing/Barrier", "text": "Repainted", "date": "2026-10-04"}));
    let r = s.execute("photo.exportLocations", &json!({"ids": [id], "format": "geojson"})).unwrap();
    let g: Value = serde_json::from_str(r["text"].as_str().unwrap()).unwrap();
    let props = &g["features"][0]["properties"];
    assert_eq!(props["condition"], "critical");
    let notes = props["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0]["kind"], "preservation", "newest first: {props}");
    assert_eq!(notes[1]["element"], "Cable/Suspender");
    assert_eq!(notes[1]["text"], "Broken wire");
    let r = s.execute("photo.exportLocations", &json!({"ids": [id], "format": "kml"})).unwrap();
    let k = r["text"].as_str().unwrap();
    assert!(k.contains("<description>2026-10-04 · Preservation · Railing/Barrier: Repainted\n2026-10-03 · Inspection · Cable/Suspender · Critical: Broken wire</description>"), "{k}");
    assert!(k.contains("<Data name=\"notes\"><value>2026-10-04"), "{k}");
    assert!(k.contains("<Data name=\"condition\"><value>critical</value></Data>"), "{k}");
    // a photo without a log has no notes at all
    let other = s.visible_cloned()[1].0;
    let mut meta = s.catalog.photo(PhotoId(other)).unwrap().meta.clone();
    meta.gps = Some((37.82, -122.48));
    s.commit("setup", Op::SetMeta { id: PhotoId(other), meta: Box::new(meta) }).unwrap();
    let r = s.execute("photo.exportLocations", &json!({"ids": [other], "format": "geojson"})).unwrap();
    let g: Value = serde_json::from_str(r["text"].as_str().unwrap()).unwrap();
    assert!(g["features"][0]["properties"].get("notes").is_none());
}

#[test]
fn the_log_survives_reopening_the_library() {
    let root = temp_dir("reopen");
    let lib = root.join("library");
    let mut s = Session::new().with_fs();
    s.open_library(&lib, true).unwrap();
    let id = s.visible_cloned()[0].0;
    add(&mut s, json!({"id": id, "element": "Joint", "condition": "poor", "text": "Open 4 cm", "date": "2026-10-05"}));
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    let log = s.execute("photo.notes", &json!({"id": id})).unwrap();
    assert_eq!(log["notes"][0]["text"], "Open 4 cm", "{log}");
    let _ = std::fs::remove_dir_all(&root);
}
