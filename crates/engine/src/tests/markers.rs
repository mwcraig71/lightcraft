//! Map markers with attached files: `map.addMarker`, `map.attach`, `map.detach`,
//! `map.removeMarker`, undo / redo, hostile params, and the markers in `photo.exportLocations`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use lightcraft_catalog::{MarkerId, PhotoId};

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-markers-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A small procedural PNG (distinct per seed).
fn write_png(path: &Path, seed: u8) {
    let (w, h) = (40usize, 30usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 5) as u8, (i / w * 7) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A library on disk and a folder of files to attach: a "PDF", a text note, a Markdown note, a
/// spreadsheet-named file, and a photo.
fn setup(tag: &str) -> (Session, PathBuf, PathBuf) {
    let root = temp_dir(tag);
    let lib = root.join("library");
    let src = root.join("site");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("inspection.pdf"), b"%PDF-1.4\n% LightCraft test\n%%EOF\n").unwrap();
    std::fs::write(src.join("notes.txt"), b"Bearing 3 squeals").unwrap();
    std::fs::write(src.join("deck.md"), b"# Deck\n\n- joint 4 open").unwrap();
    std::fs::write(src.join("loads.xlsx"), b"PK\x03\x04 not really a workbook").unwrap();
    write_png(&src.join("pier.png"), 3);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    (s, lib, src)
}

fn p(dir: &Path, name: &str) -> String {
    dir.join(name).to_string_lossy().into_owned()
}

fn markers(s: &mut Session) -> Vec<Value> {
    s.execute("map.markers", &json!({})).unwrap()["markers"].as_array().unwrap().clone()
}

#[test]
fn attach_copies_documents_dedupes_names_and_imports_photos_at_the_point() {
    let (mut s, lib, src) = setup("attach");
    let r = s.execute("map.addMarker", &json!({"lat": 37.8199, "lon": -122.4783, "title": "South tower", "note": "Paint loss"})).unwrap();
    let id = r["id"].as_u64().unwrap();
    let r = s
        .execute(
            "map.attach",
            &json!({"marker": id, "paths": [p(&src, "inspection.pdf"), p(&src, "notes.txt"), p(&src, "notes.txt"), p(&src, "deck.md"), p(&src, "loads.xlsx"), p(&src, "pier.png")]}),
        )
        .unwrap();
    let attached = r["attached"].as_array().unwrap();
    assert_eq!(attached.len(), 5, "{r}");
    let names: Vec<&str> = attached.iter().map(|a| a["path"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            format!("Attachments/{id}/inspection.pdf"),
            format!("Attachments/{id}/notes.txt"),
            format!("Attachments/{id}/notes-1.txt"),
            format!("Attachments/{id}/deck.md"),
            format!("Attachments/{id}/loads.xlsx"),
        ]
    );
    let kinds: Vec<&str> = attached.iter().map(|a| a["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["pdf", "text", "text", "markdown", "spreadsheet"]);
    // the copies are in the library, byte for byte
    assert_eq!(
        std::fs::read(lib.join("Attachments").join(id.to_string()).join("inspection.pdf")).unwrap(),
        std::fs::read(src.join("inspection.pdf")).unwrap()
    );
    assert!(lib.join("Attachments").join(id.to_string()).join("notes-1.txt").is_file());
    // the photo became a photo of the library at the marker's point, linked to the marker
    let photos = r["photos"].as_array().unwrap();
    assert_eq!(photos.len(), 1, "{r}");
    let ph = PhotoId(photos[0].as_u64().unwrap());
    let photo = s.catalog.photo(ph).unwrap();
    assert_eq!(photo.meta.gps, Some((37.8199, -122.4783)));
    match &photo.source {
        lightcraft_catalog::Source::File { path } => assert!(Path::new(path).starts_with(lib.join("Originals")), "{path}"),
        other => panic!("{other:?}"),
    }
    let m = s.catalog.marker(MarkerId(id)).unwrap();
    assert_eq!(m.photos, vec![ph]);
    assert_eq!(m.attachments.len(), 5);
    // the originals are left as they were
    assert!(src.join("pier.png").is_file() && src.join("notes.txt").is_file());
    // one undo step takes all of it back (the copies stay on disk), redo brings it again
    let label = s.undo_step().unwrap();
    assert!(label.starts_with("Attach"), "{label}");
    assert!(s.catalog.marker(MarkerId(id)).unwrap().attachments.is_empty());
    assert!(s.catalog.photo(ph).is_none());
    assert!(lib.join("Attachments").join(id.to_string()).join("inspection.pdf").is_file(), "undo never deletes the copy");
    s.redo_step().unwrap();
    assert_eq!(s.catalog.marker(MarkerId(id)).unwrap().attachments.len(), 5);
    assert_eq!(s.catalog.photo(ph).unwrap().meta.gps, Some((37.8199, -122.4783)));
    // it all survives reopening the library
    let listed = markers(&mut s);
    assert_eq!(listed[0]["attachments"][0]["exists"], true);
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    let m = s.catalog.marker(MarkerId(id)).unwrap();
    assert_eq!((m.title.as_str(), m.note.as_str(), m.attachments.len(), m.photos.len()), ("South tower", "Paint loss", 5, 1));
    let _ = std::fs::remove_dir_all(lib.parent().unwrap());
}

#[test]
fn a_photo_already_in_the_library_is_linked_and_moved_to_the_point() {
    let (mut s, lib, src) = setup("dup");
    let r = s.execute("library.import", &json!({"paths": [p(&src, "pier.png")]})).unwrap();
    let existing = r["imported"][0].as_u64().unwrap();
    let r = s.execute("map.attach", &json!({"lat": 40.0, "lon": -75.0, "paths": [p(&src, "pier.png")]})).unwrap();
    assert_eq!(r["photos"], json!([existing]), "{r}");
    assert_eq!(s.catalog.photo(PhotoId(existing)).unwrap().meta.gps, Some((40.0, -75.0)));
    // a new marker was made at the point
    let id = r["marker"].as_u64().unwrap();
    let m = s.catalog.marker(MarkerId(id)).unwrap();
    assert_eq!((m.lat, m.lon, m.photos.clone()), (40.0, -75.0, vec![PhotoId(existing)]));
    let _ = std::fs::remove_dir_all(lib.parent().unwrap());
}

#[test]
fn detach_and_remove_are_undoable_and_keep_the_files() {
    let (mut s, lib, src) = setup("detach");
    let id = s.execute("map.addMarker", &json!({"lat": 1.0, "lon": 2.0})).unwrap()["id"].as_u64().unwrap();
    let r = s.execute("map.attach", &json!({"marker": id, "paths": [p(&src, "inspection.pdf"), p(&src, "notes.txt")]})).unwrap();
    let pdf = r["attached"][0]["id"].as_u64().unwrap();
    let file = lib.join("Attachments").join(id.to_string()).join("inspection.pdf");
    s.execute("map.detach", &json!({"marker": id, "attachment": pdf})).unwrap();
    assert_eq!(s.catalog.marker(MarkerId(id)).unwrap().attachments.len(), 1);
    assert!(file.is_file(), "the copy stays for undo");
    assert_eq!(s.undo_step().unwrap(), "Remove Attachment");
    assert!(s.catalog.marker(MarkerId(id)).unwrap().attachment(pdf).is_some());
    s.redo_step().unwrap();
    assert!(s.catalog.marker(MarkerId(id)).unwrap().attachment(pdf).is_none());
    s.execute("map.updateMarker", &json!({"id": id, "title": "Pier 3", "note": "scour"})).unwrap();
    assert_eq!(s.catalog.marker(MarkerId(id)).unwrap().title, "Pier 3");
    let r = s.execute("map.removeMarker", &json!({"id": id})).unwrap();
    assert_eq!(r["attachments"], 1);
    assert!(s.catalog.marker(MarkerId(id)).is_none());
    assert!(lib.join("Attachments").join(id.to_string()).join("notes.txt").is_file());
    assert_eq!(s.undo_step().unwrap(), "Delete Marker");
    let m = s.catalog.marker(MarkerId(id)).unwrap();
    assert_eq!((m.title.as_str(), m.attachments.len()), ("Pier 3", 1));
    s.undo_step().unwrap();
    assert_eq!(s.catalog.marker(MarkerId(id)).unwrap().title, "", "the edit was its own step");
    let _ = std::fs::remove_dir_all(lib.parent().unwrap());
}

#[test]
fn hostile_params_are_errors_not_panics() {
    let (mut s, lib, src) = setup("hostile");
    for bad in [
        json!({}),
        json!({"lat": "north", "lon": 2}),
        json!({"lat": 91, "lon": 0}),
        json!({"lat": 1, "lon": f64::MAX}),
        json!({"lat": 1}),
        json!({"lat": 1, "lon": 2, "title": 5}),
        json!({"lat": 1, "lon": 2, "title": "x".repeat(500)}),
    ] {
        assert!(s.execute("map.addMarker", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.catalog.markers().count(), 0);
    let id = s.execute("map.addMarker", &json!({"lat": 1.0, "lon": 2.0})).unwrap()["id"].as_u64().unwrap();
    for bad in [
        json!({"marker": id}),
        json!({"marker": id, "paths": "notes.txt"}),
        json!({"marker": id, "paths": [1, 2]}),
        json!({"marker": 999, "paths": [p(&src, "notes.txt")]}),
        json!({"marker": -1, "paths": [p(&src, "notes.txt")]}),
        json!({"paths": [p(&src, "notes.txt")]}),
        json!({"marker": id, "paths": [p(&src, "missing.pdf")]}),
        json!({"marker": id, "paths": [src.to_string_lossy()]}),
        json!({"marker": id, "staged": [{"path": "../../outside.txt"}]}),
        json!({"marker": id, "staged": [{"path": format!("Attachments/{}/x.txt", id + 1)}]}),
        json!({"marker": id, "paths": vec!["a"; 501]}),
    ] {
        assert!(s.execute("map.attach", &bad).is_err(), "{bad}");
    }
    // one bad file among good ones is reported, the rest attached
    let r = s.execute("map.attach", &json!({"marker": id, "paths": [p(&src, "missing.pdf"), p(&src, "notes.txt")]})).unwrap();
    assert_eq!((r["attached"].as_array().unwrap().len(), r["errors"].as_array().unwrap().len()), (1, 1), "{r}");
    for bad in [json!({"marker": id}), json!({"marker": id, "attachment": 77}), json!({"marker": id, "photo": "x"}), json!({"id": id})] {
        assert!(s.execute("map.detach", &bad).is_err(), "{bad}");
    }
    for bad in [json!({}), json!({"id": "1"}), json!({"id": 999})] {
        assert!(s.execute("map.removeMarker", &bad).is_err(), "{bad}");
        assert!(s.execute("map.updateMarker", &bad).is_err(), "{bad}");
    }
    assert!(s.execute("map.updateMarker", &json!({"id": id, "lat": 100, "lon": 0})).is_err());
    assert!(s.execute("map.attachmentPath", &json!({"marker": id, "attachment": 999})).is_err());
    // attachments need a library on disk; markers alone don't
    let mut mem = Session::new();
    let m = mem.execute("map.addMarker", &json!({"lat": 1.0, "lon": 2.0})).unwrap()["id"].as_u64().unwrap();
    let e = mem.execute("map.attach", &json!({"marker": m, "paths": [p(&src, "notes.txt")]})).unwrap_err();
    assert!(e.to_string().contains("library"), "{e}");
    let _ = std::fs::remove_dir_all(lib.parent().unwrap());
}

/// Only documents and pictures inside the marker's folder are handed to the default app.
#[test]
fn only_documents_in_the_library_are_openable() {
    let (mut s, lib, src) = setup("openable");
    std::fs::write(src.join("run.bat"), b"echo hi").unwrap();
    let r = s.execute("map.attach", &json!({"lat": 1.0, "lon": 2.0, "paths": [p(&src, "inspection.pdf"), p(&src, "run.bat")]})).unwrap();
    let id = r["marker"].as_u64().unwrap();
    let pdf = s.execute("map.attachmentPath", &json!({"marker": id, "attachment": r["attached"][0]["id"]})).unwrap();
    assert_eq!(pdf["openable"], true, "{pdf}");
    assert!(Path::new(pdf["path"].as_str().unwrap()).starts_with(&lib));
    let bat = s.execute("map.attachmentPath", &json!({"marker": id, "attachment": r["attached"][1]["id"]})).unwrap();
    assert_eq!(bat["openable"], false, "{bat}");
    assert!(bat["reason"].as_str().unwrap().contains("Show in Folder"));
    let _ = std::fs::remove_dir_all(lib.parent().unwrap());
}

/// Big copies for the UI: staged on a worker, then recorded by `map.attach {staged}`.
#[test]
fn staged_copies_are_recorded() {
    let (mut s, lib, src) = setup("staged");
    let id = s.execute("map.addMarker", &json!({"lat": 1.0, "lon": 2.0})).unwrap()["id"].as_u64().unwrap();
    let guard = s.activity.start("attach", "Attaching files", crate::activity::Cancel::Yes);
    let staged = crate::cmd::markers::stage(&lib, MarkerId(id), &[p(&src, "loads.xlsx")], &guard.handle());
    assert_eq!(s.activity.list().len(), 1, "the copy shows in the activity stack");
    drop(guard);
    let c = staged[0].result.clone().unwrap();
    let r = s.execute("map.attach", &json!({"marker": id, "staged": [{"path": c.rel, "fileName": c.file_name}]})).unwrap();
    assert_eq!(r["attached"][0]["kind"], "spreadsheet", "{r}");
    assert_eq!(s.catalog.marker(MarkerId(id)).unwrap().attachments.len(), 1);
    let _ = std::fs::remove_dir_all(lib.parent().unwrap());
}

/// `photo.exportLocations` carries the markers as `kind: marker` points.
#[test]
fn export_locations_includes_markers() {
    let mut s = Session::with_demo();
    let id = s.visible_cloned()[0];
    let mut meta = s.catalog.photo(id).unwrap().meta.clone();
    meta.gps = Some((40.0, -75.0));
    s.commit("test", lightcraft_catalog::Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
    let m = s.execute("map.addMarker", &json!({"lat": 40.001, "lon": -75.002, "title": "Pier <2>", "note": "Scour at the nose"})).unwrap()["id"]
        .as_u64()
        .unwrap();
    let r = s.execute("photo.exportLocations", &json!({"ids": [id.0]})).unwrap();
    assert_eq!(r["markers"], 1, "{r}");
    let g: Value = serde_json::from_str(r["text"].as_str().unwrap()).unwrap();
    let marker = g["features"].as_array().unwrap().iter().find(|f| f["properties"]["kind"] == "marker").unwrap().clone();
    assert_eq!(marker["geometry"], json!({"type": "Point", "coordinates": [-75.002, 40.001]}));
    assert_eq!(marker["properties"]["name"], "Pier <2>");
    assert_eq!(marker["properties"]["id"], m);
    assert_eq!(marker["properties"]["note"], "Scour at the nose");
    let kml = s.execute("photo.exportLocations", &json!({"ids": [id.0], "format": "kml"})).unwrap();
    let k = kml["text"].as_str().unwrap();
    assert!(k.contains("<name>Markers</name>") && k.contains("<name>Pier &lt;2&gt;</name>") && k.contains("<value>marker</value>"), "{k}");
    // without markers
    let r = s.execute("photo.exportLocations", &json!({"ids": [id.0], "markers": false})).unwrap();
    assert_eq!(r["markers"], 0);
    assert!(!r["text"].as_str().unwrap().contains("\"marker\""));
}
