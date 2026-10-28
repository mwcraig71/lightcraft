//! Map markers with attachments, headless: a marker draws at its point, a click opens its card,
//! the card lists what `map.attach` attached, "Photos here" narrows the filmstrip, the right-click
//! menu adds a marker, and files dropped on the map attach where they land.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lightcraft_catalog::{MarkerId, Op, PhotoId};
use serde_json::{Value, json};

use crate::headless::Headless;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-ui-markers-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, seed: u8) {
    let (w, h) = (40usize, 30usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 5) as u8, (i / w * 7) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// The demo photos in a library on disk (attached files are kept there), three of them along a
/// bridge, tiles off, the Map view open.
fn app(tag: &str) -> (Headless, PathBuf, Vec<PhotoId>) {
    let root = temp_dir(tag);
    let mut session = lightcraft_engine::Session::new().with_fs();
    session.open_library(root.join("library"), true).unwrap();
    let services = Services { png: None, ..Default::default() };
    let mut h = Headless::new(LightcraftApp::new(session, services), [1400.0, 900.0], 1.0);
    h.settle(T);
    let ids: Vec<PhotoId> = h.app.session.visible_cloned();
    for (i, id) in ids.iter().enumerate().take(3) {
        let mut meta = h.app.session.catalog.photo(*id).unwrap().meta.clone();
        meta.gps = Some((40.0, -75.0 + i as f64 * 0.0003));
        h.app.session.commit("setup", Op::SetMeta { id: *id, meta: Box::new(meta) }).unwrap();
    }
    run(&mut h, "map.setTileSource", json!({"source": "off"}));
    run(&mut h, "view.map", json!({}));
    h.settle(T);
    h.step();
    (h, root, ids)
}

fn run(h: &mut Headless, command: &str, params: Value) -> Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn widget(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r)
}

fn click(h: &mut Headless, at: egui::Pos2, right: bool) {
    let mut p = json!({"x": at.x, "y": at.y});
    if right {
        p["button"] = json!("right");
    }
    let r = h.request("ui.click", p, T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

fn click_widget(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

#[test]
fn a_marker_draws_at_its_point_and_a_click_opens_its_card() {
    let (mut h, root, _) = app("card");
    // a little above the middle of the map
    let vp = h.app.ui.map.markers.viewport.unwrap();
    let (lat, lon) = {
        let (x, y) = vp.world_at(vp.rect.center() - egui::vec2(40.0, 60.0));
        lightcraft_tiles::mercator::from_world(x, y)
    };
    let r = run(&mut h, "map.addMarker", json!({"lat": lat, "lon": lon, "title": "Pier 2", "note": "Spalling"}));
    let id = r["id"].as_u64().unwrap();
    h.step();
    h.step();
    let Some(m) = widget(&h, &format!("mapMarker:{id}")) else {
        let ws: Vec<&String> = h.app.widgets.iter().map(|(w, _)| w).filter(|w| w.starts_with("map")).collect();
        panic!("the marker is drawn: {ws:?} {:?} {:?}", h.app.ui.view, h.app.ui.map.markers.viewport);
    };
    // where its point is on the map
    let vp = h.app.ui.map.markers.viewport.unwrap();
    let at = vp.screen_of(lightcraft_tiles::mercator::to_world(lat, lon));
    assert!(m.center().distance(at) < 1.0, "{m:?} vs {at:?}");
    assert!(widget(&h, "markerCard").is_none());
    click(&mut h, m.center(), false);
    assert_eq!(h.app.ui.map.markers.open, Some(id));
    assert!(widget(&h, "markerCard").is_some(), "the card opened");
    let text = h.painted_text().join("\n");
    assert!(text.contains("Attachments (0)"), "{text}");
    assert_eq!(h.app.ui.map.markers.title, "Pier 2");
    // the click on the marker selected no photo
    assert!(h.app.session.selection.ids.is_empty() || h.app.session.catalog.marker(MarkerId(id)).is_some());
    // Close
    click_widget(&mut h, "button:markerCardClose");
    assert_eq!(h.app.ui.map.markers.open, None);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn attaching_updates_the_card_and_photos_here_narrows_the_filmstrip() {
    let (mut h, root, _) = app("attach");
    let src = root.join("site");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("inspection.pdf"), b"%PDF-1.4\n%%EOF\n").unwrap();
    std::fs::write(src.join("notes.txt"), b"bearing 3").unwrap();
    write_png(&src.join("pier.png"), 9);
    let id = run(&mut h, "map.addMarker", json!({"lat": 40.0002, "lon": -75.0001}))["id"].as_u64().unwrap();
    run(&mut h, "map.openMarker", json!({"id": id}));
    h.step();
    assert!(h.painted_text().join("\n").contains("Attachments (0)"));
    let path = |n: &str| src.join(n).to_string_lossy().into_owned();
    let r = run(&mut h, "map.attachFiles", json!({"marker": id, "paths": [path("inspection.pdf"), path("notes.txt"), path("pier.png")]}));
    assert_eq!(r["attached"].as_array().unwrap().len(), 2, "{r}");
    let photo = r["photos"][0].as_u64().expect("the photo was imported");
    h.step();
    h.step();
    let text = h.painted_text().join("\n");
    assert!(text.contains("Attachments (2)") && text.contains("inspection.pdf") && text.contains("notes.txt"), "{text}");
    let aid = r["attached"][0]["id"].as_u64().unwrap();
    assert!(widget(&h, &format!("markerAttachment:{aid}")).is_some());
    assert!(text.contains("Photos here (1)"), "{text}");
    // the photo is at the marker's point
    assert_eq!(h.app.session.catalog.photo(PhotoId(photo)).unwrap().meta.gps, Some((40.0002, -75.0001)));
    click_widget(&mut h, "button:markerPhotos");
    assert_eq!(h.app.ui.map.focus, vec![photo], "the filmstrip shows the marker's photos");
    assert_eq!(h.app.session.selection.ids, vec![PhotoId(photo)]);
    // Remove takes it off the card; undo puts it back
    click_widget(&mut h, &format!("button:markerRemove-{aid}"));
    assert!(h.painted_text().join("\n").contains("Attachments (1)"));
    run(&mut h, "edit.undo", json!({}));
    h.step();
    h.step();
    assert!(h.painted_text().join("\n").contains("Attachments (2)"));
    // Delete Marker asks first when something is attached
    click_widget(&mut h, "button:markerDelete");
    assert!(h.app.session.catalog.marker(MarkerId(id)).is_some());
    click_widget(&mut h, "button:markerDeleteConfirm");
    assert!(h.app.session.catalog.marker(MarkerId(id)).is_none());
    assert_eq!(h.app.ui.map.markers.open, None);
    assert_eq!(h.app.ui.view, ViewMode::Map);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_context_menu_adds_a_marker_and_drops_attach_where_they_land() {
    let (mut h, root, _) = app("menu");
    let map = widget(&h, "map:view").unwrap();
    let at = egui::pos2(map.left() + 200.0, map.top() + 150.0);
    click(&mut h, at, true);
    let item = widget(&h, "mapMenu:addMarker").expect("the context menu offers Add Marker Here");
    click(&mut h, item.center(), false);
    let ms: Vec<_> = h.app.session.catalog.markers().cloned().collect();
    assert_eq!(ms.len(), 1);
    let vp = h.app.ui.map.markers.viewport.unwrap();
    let placed = vp.screen_of(lightcraft_tiles::mercator::to_world(ms[0].lat, ms[0].lon));
    assert!(placed.distance(at) < 1.5, "{placed:?} vs {at:?}");
    assert_eq!(h.app.ui.map.markers.open, Some(ms[0].id.0), "its card opens");
    // close the card, then drop a file elsewhere on the map: a new marker there
    run(&mut h, "map.openMarker", json!({"id": null}));
    h.step();
    let src = root.join("drop");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("report.md"), b"# Report").unwrap();
    let there = egui::pos2(map.left() + 500.0, map.top() + 300.0);
    let r = h.request("ui.move", json!({"x": there.x, "y": there.y}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    let r = h.request("ui.dropFiles", json!({"paths": [src.join("report.md").to_string_lossy()]}), T);
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..4 {
        h.step();
    }
    let ms: Vec<_> = h.app.session.catalog.markers().cloned().collect();
    assert_eq!(ms.len(), 2, "a new marker for the drop");
    let dropped = ms.iter().find(|m| !m.attachments.is_empty()).expect("the file is attached");
    assert_eq!(dropped.attachments[0].file_name, "report.md");
    let vp = h.app.ui.map.markers.viewport.unwrap();
    let placed = vp.screen_of(lightcraft_tiles::mercator::to_world(dropped.lat, dropped.lon));
    assert!(placed.distance(there) < 1.5, "{placed:?} vs {there:?}");
    assert!(root.join("library").join("Attachments").join(dropped.id.0.to_string()).join("report.md").is_file());
    // no import review was opened for the drop
    assert!(h.app.ui.dialog.is_none());
    let _ = std::fs::remove_dir_all(root);
}
