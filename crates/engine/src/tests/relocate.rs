//! Moving things on the Map view: `photo.setLocation` (translate a group, stack it, sidecars,
//! undo / redo, hostile params) and `map.moveAttachment` (to a marker or a new one, name
//! collisions, undo / redo moving the file back and forth, failures leaving everything as it was).

use std::path::{Path, PathBuf};

use serde_json::json;

use lightcraft_catalog::{MarkerId, Op, PhotoId};

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-relocate-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, seed: u8) {
    let (w, h) = (40usize, 30usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 5) as u8, (i / w * 7) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn set_gps(s: &mut Session, id: PhotoId, gps: Option<(f64, f64)>, accuracy: Option<f64>) {
    let mut meta = s.catalog.photo(id).unwrap().meta.clone();
    meta.gps = gps;
    meta.gps_accuracy = accuracy;
    meta.gps_altitude = Some(12.5);
    meta.gps_direction = Some(90.0);
    s.commit("setup", Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
}

fn gps(s: &Session, id: PhotoId) -> (f64, f64) {
    s.catalog.photo(id).unwrap().meta.gps.unwrap()
}

#[test]
fn moving_a_group_keeps_its_arrangement_and_is_one_undo_step() {
    let mut s = Session::with_demo();
    let ids = s.visible_cloned();
    let (a, b, c, d) = (ids[0], ids[1], ids[2], ids[3]);
    set_gps(&mut s, a, Some((40.0, -75.0)), Some(6.0));
    set_gps(&mut s, b, Some((40.0, -74.9996)), None);
    set_gps(&mut s, c, Some((40.0004, -75.0)), None);
    set_gps(&mut s, d, None, None);
    let r = s.execute("photo.setLocation", &json!({"ids": [a.0, b.0, c.0, d.0], "lat": 37.82, "lon": -122.478})).unwrap();
    assert_eq!((r["moved"].as_u64(), r["skipped"].as_u64(), r["mode"].as_str()), (Some(3), Some(1), Some("translate")), "{r}");
    let (pa, pb, pc) = (gps(&s, a), gps(&s, b), gps(&s, c));
    // the centroid is on the point
    let (clat, clon) = ((pa.0 + pb.0 + pc.0) / 3.0, (pa.1 + pb.1 + pc.1) / 3.0);
    assert!((clat - 37.82).abs() < 2e-6 && (clon + 122.478).abs() < 2e-6, "{clat} {clon}");
    // b is still east of a by the same longitude, c north of a
    assert!(((pb.1 - pa.1) - 0.0004).abs() < 1e-6 && (pb.0 - pa.0).abs() < 1e-6, "{pa:?} {pb:?}");
    assert!(pc.0 > pa.0 && (pc.1 - pa.1).abs() < 1e-6);
    // the accuracy went (placed by hand), altitude and heading stayed; the one without a location has none still
    let m = &s.catalog.photo(a).unwrap().meta;
    assert_eq!((m.gps_accuracy, m.gps_altitude, m.gps_direction), (None, Some(12.5), Some(90.0)));
    assert_eq!(s.catalog.photo(d).unwrap().meta.gps, None);
    // one undo step
    assert_eq!(s.undo_step().unwrap(), "Move 3 Photos");
    assert_eq!((gps(&s, a), gps(&s, b)), ((40.0, -75.0), (40.0, -74.9996)));
    assert_eq!(s.catalog.photo(a).unwrap().meta.gps_accuracy, Some(6.0));
    s.redo_step().unwrap();
    assert_eq!(gps(&s, a), pa);
}

#[test]
fn stack_puts_every_photo_on_the_point_and_gives_one_to_photos_without() {
    let mut s = Session::with_demo();
    let ids = s.visible_cloned();
    set_gps(&mut s, ids[0], Some((40.0, -75.0)), None);
    set_gps(&mut s, ids[1], None, None);
    let r = s.execute("photo.setLocation", &json!({"ids": [ids[0].0, ids[1].0], "lat": 41.5, "lon": -76.25, "mode": "stack"})).unwrap();
    assert_eq!((r["moved"].as_u64(), r["skipped"].as_u64()), (Some(2), Some(0)), "{r}");
    assert_eq!((gps(&s, ids[0]), gps(&s, ids[1])), ((41.5, -76.25), (41.5, -76.25)));
    // the same again changes nothing and adds no undo step
    let undo = s.undo.len();
    let r = s.execute("photo.setLocation", &json!({"ids": [ids[0].0], "lat": 41.5, "lon": -76.25, "mode": "stack"})).unwrap();
    assert_eq!((r["moved"].as_u64(), r["unchanged"].as_u64()), (Some(0), Some(1)));
    assert_eq!(s.undo.len(), undo);
    // one photo: "Move Photo"
    s.execute("photo.setLocation", &json!({"ids": [ids[0].0], "lat": 41.0, "lon": -76.0})).unwrap();
    assert_eq!(s.undo_step().unwrap(), "Move Photo");
    // a latitude past Web Mercator's edge is held at it
    s.execute("photo.setLocation", &json!({"ids": [ids[0].0], "lat": 89.9, "lon": 0.0, "mode": "stack"})).unwrap();
    assert!(gps(&s, ids[0]).0 <= crate::cmd::relocate::MAX_LAT + 1e-9);
}

#[test]
fn hostile_location_params_are_errors_not_panics() {
    let mut s = Session::with_demo();
    let ids = s.visible_cloned();
    let id = ids[0].0;
    set_gps(&mut s, ids[0], Some((40.0, -75.0)), None);
    set_gps(&mut s, ids[1], None, None);
    let before = s.undo.len();
    for bad in [
        json!({}),
        json!({"ids": [], "lat": 1, "lon": 2}),
        json!({"ids": "1", "lat": 1, "lon": 2}),
        json!({"ids": [999_999], "lat": 1, "lon": 2}),
        json!({"ids": [-1], "lat": 1, "lon": 2}),
        json!({"ids": [1.5], "lat": 1, "lon": 2}),
        json!({"ids": [id]}),
        json!({"ids": [id], "lat": "north", "lon": 2}),
        json!({"ids": [id], "lat": 91, "lon": 2}),
        json!({"ids": [id], "lat": 1, "lon": -181}),
        json!({"ids": [id], "lat": 1, "lon": f64::MAX}),
        json!({"ids": [id], "lat": null, "lon": 2}),
        json!({"ids": [id], "lat": 1, "lon": 2, "mode": "scatter"}),
        json!({"ids": [id], "lat": 1, "lon": 2, "mode": 3}),
        // translate needs a photo with a location to move
        json!({"ids": [ids[1].0], "lat": 1, "lon": 2}),
        json!({"ids": vec![id; 100_001], "lat": 1, "lon": 2}),
    ] {
        assert!(s.execute("photo.setLocation", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.undo.len(), before, "nothing changed");
    assert_eq!(gps(&s, ids[0]), (40.0, -75.0));
    // a repeated id moves once
    let r = s.execute("photo.setLocation", &json!({"ids": [id, id], "lat": 1.0, "lon": 2.0})).unwrap();
    assert_eq!(r["moved"], 1);
}

#[test]
fn moving_photos_writes_the_sidecar_never_the_original() {
    let root = temp_dir("sidecar");
    let lib = root.join("library");
    let shots = root.join("shots");
    write_png(&shots.join("pier.png"), 5);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [shots.join("pier.png").to_string_lossy()], "mode": "add"})).unwrap();
    let id = PhotoId(r["imported"][0].as_u64().unwrap());
    let original = std::fs::read(shots.join("pier.png")).unwrap();
    s.xmp.auto_write = true;
    s.execute("photo.setLocation", &json!({"ids": [id.0], "lat": 37.8199, "lon": -122.4783, "mode": "stack"})).unwrap();
    let xmp = std::fs::read_to_string(shots.join("pier.xmp")).expect("the sidecar was written");
    assert!(xmp.contains("GPSLatitude") && xmp.contains("37,49"), "{xmp}");
    assert_eq!(std::fs::read(shots.join("pier.png")).unwrap(), original, "the original is untouched");
    let _ = std::fs::remove_dir_all(root);
}

/// A library on disk with marker 1 holding `inspection.pdf` and `notes.txt`.
fn with_marker(tag: &str) -> (Session, PathBuf, u64, u64, u64) {
    let root = temp_dir(tag);
    let lib = root.join("library");
    let src = root.join("site");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("inspection.pdf"), b"%PDF-1.4\n%%EOF\n").unwrap();
    std::fs::write(src.join("notes.txt"), b"bearing 3").unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    let paths = [src.join("inspection.pdf").to_string_lossy().into_owned(), src.join("notes.txt").to_string_lossy().into_owned()];
    let r = s.execute("map.attach", &json!({"lat": 37.8, "lon": -122.4, "paths": paths})).unwrap();
    let m = r["marker"].as_u64().unwrap();
    let (pdf, txt) = (r["attached"][0]["id"].as_u64().unwrap(), r["attached"][1]["id"].as_u64().unwrap());
    (s, lib, m, pdf, txt)
}

fn folder(lib: &Path, m: u64) -> PathBuf {
    lib.join("Attachments").join(m.to_string())
}

#[test]
fn an_attachment_moves_to_a_new_marker_with_its_file_and_undo_moves_it_back() {
    let (mut s, lib, m, pdf, _) = with_marker("newmarker");
    let r = s.execute("map.moveAttachment", &json!({"marker": m, "attachment": pdf, "to": {"lat": 37.81, "lon": -122.41}})).unwrap();
    assert_eq!(r["created"], true, "{r}");
    let to = r["marker"].as_u64().unwrap();
    assert_ne!(to, m);
    let new = s.catalog.marker(MarkerId(to)).unwrap();
    assert_eq!((new.lat, new.lon, new.attachments.len()), (37.81, -122.41, 1));
    assert_eq!(new.attachments[0].file_name, "inspection.pdf");
    assert_eq!(new.attachments[0].path, format!("Attachments/{to}/inspection.pdf"));
    assert_eq!(s.catalog.marker(MarkerId(m)).unwrap().attachments.len(), 1, "it left the old marker");
    assert!(folder(&lib, to).join("inspection.pdf").is_file(), "the file moved");
    assert!(!folder(&lib, m).join("inspection.pdf").exists());
    // one undo step: the new marker goes, the file goes back
    assert_eq!(s.undo_step().unwrap(), "Move Attachment");
    assert!(s.catalog.marker(MarkerId(to)).is_none());
    let old = s.catalog.marker(MarkerId(m)).unwrap();
    assert_eq!(old.attachments.len(), 2);
    assert!(folder(&lib, m).join("inspection.pdf").is_file() && !folder(&lib, to).join("inspection.pdf").exists());
    let listed = s.execute("map.markers", &json!({"id": m})).unwrap();
    assert!(listed["markers"][0]["attachments"].as_array().unwrap().iter().all(|a| a["exists"] == true), "{listed}");
    // redo moves it again
    s.redo_step().unwrap();
    assert!(folder(&lib, to).join("inspection.pdf").is_file() && !folder(&lib, m).join("inspection.pdf").exists());
    assert_eq!(s.catalog.marker(MarkerId(to)).unwrap().attachments.len(), 1);
    let _ = std::fs::remove_dir_all(lib.parent().unwrap());
}

#[test]
fn an_attachment_moved_onto_a_marker_with_that_name_gets_a_free_one() {
    let (mut s, lib, m, _, txt) = with_marker("collide");
    let other = s.execute("map.addMarker", &json!({"lat": 1.0, "lon": 2.0})).unwrap()["id"].as_u64().unwrap();
    // the other marker already has a notes.txt (and a stray notes-1.txt sits in its folder)
    std::fs::create_dir_all(folder(&lib, other)).unwrap();
    std::fs::write(folder(&lib, other).join("notes-1.txt"), b"not ours").unwrap();
    let theirs = lib.parent().unwrap().join("theirs");
    std::fs::create_dir_all(&theirs).unwrap();
    std::fs::write(theirs.join("notes.txt"), b"theirs").unwrap();
    s.execute("map.attach", &json!({"marker": other, "paths": [theirs.join("notes.txt").to_string_lossy()]})).unwrap();
    let r = s.execute("map.moveAttachment", &json!({"marker": m, "attachment": txt, "to": other})).unwrap();
    assert_eq!((r["created"].as_bool(), r["marker"].as_u64()), (Some(false), Some(other)), "{r}");
    assert_eq!(r["path"], format!("Attachments/{other}/notes-2.txt"));
    assert_eq!(std::fs::read(folder(&lib, other).join("notes-2.txt")).unwrap(), b"bearing 3");
    assert_eq!(std::fs::read(folder(&lib, other).join("notes.txt")).unwrap(), b"theirs", "nothing overwritten");
    assert_eq!(std::fs::read(folder(&lib, other).join("notes-1.txt")).unwrap(), b"not ours");
    let moved = s.catalog.marker(MarkerId(other)).unwrap();
    assert_eq!(moved.attachments.len(), 2);
    assert_eq!(moved.attachments[1].file_name, "notes.txt", "it keeps its own name on the card");
    s.undo_step().unwrap();
    assert_eq!(std::fs::read(folder(&lib, m).join("notes.txt")).unwrap(), b"bearing 3");
    assert!(!folder(&lib, other).join("notes-2.txt").exists());
    let _ = std::fs::remove_dir_all(lib.parent().unwrap());
}

#[test]
fn a_failed_attachment_move_changes_nothing_and_hostile_params_are_errors() {
    let (mut s, lib, m, pdf, txt) = with_marker("hostile");
    let other = s.execute("map.addMarker", &json!({"lat": 1.0, "lon": 2.0})).unwrap()["id"].as_u64().unwrap();
    let undo = s.undo.len();
    for bad in [
        json!({}),
        json!({"marker": m, "attachment": pdf}),
        json!({"marker": m, "attachment": pdf, "to": m}),
        json!({"marker": m, "attachment": pdf, "to": 999}),
        json!({"marker": m, "attachment": pdf, "to": "x"}),
        json!({"marker": m, "attachment": pdf, "to": {"lat": 91, "lon": 0}}),
        json!({"marker": m, "attachment": pdf, "to": {"lat": "a", "lon": 0}}),
        json!({"marker": m, "attachment": pdf, "to": {"lat": 1}}),
        json!({"marker": m, "attachment": 77, "to": other}),
        json!({"marker": m, "attachment": -1, "to": other}),
        json!({"marker": 999, "attachment": pdf, "to": other}),
        json!({"marker": m, "to": other}),
    ] {
        assert!(s.execute("map.moveAttachment", &bad).is_err(), "{bad}");
    }
    // the file is missing from the library: an error, and the records stay as they were
    std::fs::remove_file(folder(&lib, m).join("notes.txt")).unwrap();
    let e = s.execute("map.moveAttachment", &json!({"marker": m, "attachment": txt, "to": other})).unwrap_err();
    assert!(e.to_string().contains("missing"), "{e}");
    assert_eq!(s.undo.len(), undo, "nothing changed");
    assert_eq!(s.catalog.marker(MarkerId(m)).unwrap().attachments.len(), 2);
    assert!(s.catalog.marker(MarkerId(other)).unwrap().attachments.is_empty());
    assert_eq!(s.catalog.markers().count(), 2, "no marker made for it");
    // a move whose file was deleted afterwards still undoes (the step must not jam the history)
    s.execute("map.moveAttachment", &json!({"marker": m, "attachment": pdf, "to": other})).unwrap();
    std::fs::remove_file(folder(&lib, other).join("inspection.pdf")).unwrap();
    assert_eq!(s.undo_step().unwrap(), "Move Attachment");
    assert!(s.catalog.marker(MarkerId(m)).unwrap().attachment(pdf).is_some());
    // markers in a library that isn't on disk: no files to move
    let mut mem = Session::new();
    let a = mem.execute("map.addMarker", &json!({"lat": 1.0, "lon": 2.0})).unwrap()["id"].as_u64().unwrap();
    assert!(mem.execute("map.moveAttachment", &json!({"marker": a, "attachment": 1, "to": {"lat": 1.0, "lon": 2.0}})).is_err());
    let _ = std::fs::remove_dir_all(lib.parent().unwrap());
}

#[test]
fn moving_a_marker_keeps_its_files_and_photos_where_they_are() {
    let (mut s, lib, m, pdf, _) = with_marker("marker");
    s.execute("map.updateMarker", &json!({"id": m, "lat": 37.9, "lon": -122.5})).unwrap();
    let mk = s.catalog.marker(MarkerId(m)).unwrap();
    assert_eq!((mk.lat, mk.lon), (37.9, -122.5));
    assert!(mk.attachment(pdf).is_some() && folder(&lib, m).join("inspection.pdf").is_file());
    assert_eq!(s.undo_step().unwrap(), "Edit Marker");
    let _ = std::fs::remove_dir_all(lib.parent().unwrap());
}
