//! Moving things on the Map view, headless: the right-click's Move… items on a pin, a selection,
//! a cluster and a marker; the placing mode's banner; Esc, a right-click and Cancel leaving
//! everything as it was; a click placing the photos with the map kept where it is, and Undo; a
//! marker card's Move To… onto another marker and onto the empty map.

use std::path::PathBuf;
use std::time::Duration;

use lightcraft_catalog::{MarkerId, Op, PhotoId};
use serde_json::{Value, json};

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-ui-moving-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The demo photos in a library on disk, three of them along a bridge (~25 m apart), tiles off,
/// the Map view open and zoomed in so each is its own pin.
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
        meta.gps_accuracy = Some(5.0);
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

fn key(h: &mut Headless, k: &str) {
    let r = h.request("ui.key", json!({"key": k}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

fn text(h: &Headless) -> String {
    h.painted_text().join("\n")
}

fn gps(h: &Headless, id: PhotoId) -> (f64, f64) {
    h.app.session.catalog.photo(id).unwrap().meta.gps.unwrap()
}

fn pin(h: &Headless, id: PhotoId) -> egui::Rect {
    widget(h, &format!("mapPin:{}", id.0)).unwrap_or_else(|| {
        panic!("pin {} drawn: {:?}", id.0, h.app.widgets.iter().map(|w| &w.0).filter(|w| w.starts_with("map")).collect::<Vec<_>>())
    })
}

/// Open the right-click menu at `at`; what is painted then.
fn menu_at(h: &mut Headless, at: egui::Pos2) -> String {
    click(h, at, true);
    text(h)
}

fn close_menu(h: &mut Headless) {
    key(h, "Escape");
}

#[test]
fn right_click_offers_move_on_a_pin_a_selection_a_cluster_and_a_marker() {
    let (mut h, root, ids) = app("menu");
    // one pin
    let at = pin(&h, ids[0]).center();
    let t = menu_at(&mut h, at);
    assert!(t.contains("Move Photo…"), "{t}");
    assert!(widget(&h, "mapMenu:movePhotos").is_some());
    close_menu(&mut h);
    // a pin of one of the selected photos moves the selection
    run(&mut h, "library.select", json!({"ids": [ids[0].0, ids[1].0, ids[2].0], "mode": "replace"}));
    h.step();
    let at = pin(&h, ids[1]).center();
    let t = menu_at(&mut h, at);
    assert!(t.contains("Move 3 Selected Photos…"), "{t}");
    close_menu(&mut h);
    run(&mut h, "library.select", json!({"ids": [], "mode": "replace"}));
    // zoomed out, the three are a cluster
    let r = h.request("ui.set", json!({"map": {"zoom": 15.0}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    let cluster = widget(&h, "mapCluster:3").expect("one cluster of three");
    let t = menu_at(&mut h, cluster.center());
    assert!(t.contains("Move 3 Photos…"), "{t}");
    // its item starts the placing mode with the three
    let item = widget(&h, "mapMenu:movePhotos").unwrap();
    click(&mut h, item.center(), false);
    assert!(widget(&h, "map:moveBanner").is_some());
    assert!(text(&h).contains("Click the new location for 3 photos"), "{}", text(&h));
    run(&mut h, "map.cancelMove", json!({}));
    h.step();
    // a marker: Move Marker…
    let vp = h.app.ui.map.markers.viewport.unwrap();
    let (lat, lon) = {
        let (x, y) = vp.world_at(vp.rect.center() + egui::vec2(200.0, 120.0));
        lightcraft_tiles::mercator::from_world(x, y)
    };
    let m = run(&mut h, "map.addMarker", json!({"lat": lat, "lon": lon}))["id"].as_u64().unwrap();
    h.step();
    h.step();
    let mk = widget(&h, &format!("mapMarker:{m}")).unwrap();
    let t = menu_at(&mut h, mk.center());
    assert!(t.contains("Move Marker…") && !t.contains("Move Photo"), "{t}");
    assert!(widget(&h, "mapMenu:moveMarker").is_some());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn esc_a_right_click_or_cancel_leave_everything_as_it_was() {
    let (mut h, root, ids) = app("cancel");
    let before: Vec<(f64, f64)> = ids.iter().take(3).map(|id| gps(&h, *id)).collect();
    let (center, zoom, undo) = (h.app.ui.map.center, h.app.ui.map.zoom, h.app.session.undo.len());
    let photos = json!([ids[0].0, ids[1].0, ids[2].0]);
    let r = run(&mut h, "map.beginMove", json!({"photos": photos}));
    assert_eq!((r["count"].as_u64(), r["located"].as_u64()), (Some(3), Some(3)), "{r}");
    h.step();
    h.step();
    assert!(widget(&h, "map:moveBanner").is_some(), "the banner is up");
    assert!(widget(&h, "button:mapMoveStack").is_some() && widget(&h, "button:mapMoveCancel").is_some());
    // the ghost follows the pointer
    let map = widget(&h, "map:view").unwrap();
    let r = h.request("ui.move", json!({"x": map.left() + 300.0, "y": map.top() + 400.0}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(widget(&h, "map:moveGhost").is_some());
    key(&mut h, "Escape");
    assert!(h.app.ui.map.moving.placing.is_none(), "Esc cancels");
    assert!(widget(&h, "map:moveBanner").is_none());
    assert_eq!(h.app.ui.view, crate::state::ViewMode::Map, "and stays on the map");
    // a right-click cancels too, without opening the menu
    run(&mut h, "map.beginMove", json!({"photos": photos}));
    h.step();
    click(&mut h, map.center(), true);
    assert!(h.app.ui.map.moving.placing.is_none(), "a right-click cancels");
    assert!(widget(&h, "mapMenu:movePhotos").is_none() && widget(&h, "mapMenu:addMarker").is_none());
    // and the banner's Cancel
    run(&mut h, "map.beginMove", json!({"photos": photos}));
    h.step();
    h.step();
    click_widget(&mut h, "button:mapMoveCancel");
    assert!(h.app.ui.map.moving.placing.is_none());
    let after: Vec<(f64, f64)> = ids.iter().take(3).map(|id| gps(&h, *id)).collect();
    assert_eq!(after, before, "nothing moved");
    assert_eq!(h.app.session.undo.len(), undo);
    assert_eq!((h.app.ui.map.center, h.app.ui.map.zoom), (center, zoom), "the map didn't move");
    // hostile params
    for bad in [
        json!({}),
        json!({"photos": []}),
        json!({"photos": [999_999]}),
        json!({"photos": "x"}),
        json!({"marker": 77}),
        json!({"photos": [ids[0].0], "marker": 1}),
        json!({"attachment": {"marker": 1}}),
    ] {
        let r = h.request("engine.execute", json!({"command": "map.beginMove", "params": bad}), T);
        assert_eq!(r["ok"], false, "{bad}: {r}");
    }
    for bad in [json!({"lat": 1.0, "lon": 2.0}), json!({"lat": f64::NAN, "lon": 2.0}), json!({"lat": "x"})] {
        let r = h.request("engine.execute", json!({"command": "map.place", "params": bad}), T);
        assert_eq!(r["ok"], false, "nothing is being placed / bad numbers: {bad}: {r}");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_click_places_the_group_keeps_the_map_where_it_is_and_undo_takes_it_back() {
    let (mut h, root, ids) = app("place");
    let before: Vec<(f64, f64)> = ids.iter().take(3).map(|id| gps(&h, *id)).collect();
    let (center, zoom) = (h.app.ui.map.center, h.app.ui.map.zoom);
    run(&mut h, "map.beginMove", json!({"photos": [ids[0].0, ids[1].0, ids[2].0]}));
    h.step();
    let map = widget(&h, "map:view").unwrap();
    let at = egui::pos2(map.left() + 250.0, map.top() + 200.0);
    click(&mut h, at, false);
    assert!(h.app.ui.map.moving.placing.is_none(), "placed");
    let vp = h.app.ui.map.markers.viewport.unwrap();
    let (x, y) = vp.world_at(at);
    let (lat, lon) = lightcraft_tiles::mercator::from_world(x, y);
    let after: Vec<(f64, f64)> = ids.iter().take(3).map(|id| gps(&h, *id)).collect();
    let (clat, clon) = (after.iter().map(|g| g.0).sum::<f64>() / 3.0, after.iter().map(|g| g.1).sum::<f64>() / 3.0);
    assert!((clat - lat).abs() < 1e-5 && (clon - lon).abs() < 1e-5, "the centroid went to the click: {clat} {clon} vs {lat} {lon}");
    // still in a row, the same distance apart
    assert!(((after[1].1 - after[0].1) - (before[1].1 - before[0].1)).abs() < 1e-6, "{after:?}");
    assert_eq!(h.app.session.catalog.photo(ids[0]).unwrap().meta.gps_accuracy, None, "placed by hand");
    h.step();
    h.step();
    assert_eq!((h.app.ui.map.center, h.app.ui.map.zoom), (center, zoom), "no refit");
    // the pins are drawn where the click was
    let p = pin(&h, ids[1]).center();
    assert!(p.distance(at) < 2.0, "{p:?} vs {at:?}");
    assert!(text(&h).contains("Moved 3 photos"), "{}", text(&h));
    assert_eq!(h.app.session.undo.last().unwrap().label, "Move 3 Photos");
    click_widget(&mut h, "button:mapMoveUndo");
    let back: Vec<(f64, f64)> = ids.iter().take(3).map(|id| gps(&h, *id)).collect();
    assert_eq!(back, before, "Undo put them back");
    h.step();
    assert_eq!((h.app.ui.map.center, h.app.ui.map.zoom), (center, zoom), "no refit after Undo either");
    // Stack at one point
    run(&mut h, "map.beginMove", json!({"photos": [ids[0].0, ids[1].0]}));
    h.step();
    h.step();
    click_widget(&mut h, "button:mapMoveStack");
    assert!(h.app.ui.map.moving.stack);
    run(&mut h, "map.place", json!({"lat": 40.001, "lon": -75.001}));
    assert_eq!((gps(&h, ids[0]), gps(&h, ids[1])), ((40.001, -75.001), (40.001, -75.001)));
    // a marker moves with its files (map.updateMarker)
    let m = run(&mut h, "map.addMarker", json!({"lat": 40.0, "lon": -75.0}))["id"].as_u64().unwrap();
    run(&mut h, "map.beginMove", json!({"marker": m}));
    run(&mut h, "map.place", json!({"lat": 40.0002, "lon": -75.0004}));
    let mk = h.app.session.catalog.marker(MarkerId(m)).unwrap();
    assert_eq!((mk.lat, mk.lon), (40.0002, -75.0004));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn move_to_puts_an_attachment_on_another_marker_or_a_new_one() {
    let (mut h, root, _) = app("attachment");
    let src = root.join("site");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("inspection.pdf"), b"%PDF-1.4\n%%EOF\n").unwrap();
    std::fs::write(src.join("notes.txt"), b"bearing 3").unwrap();
    let vp = h.app.ui.map.markers.viewport.unwrap();
    let point = |dx: f32, dy: f32| {
        let (x, y) = vp.world_at(vp.rect.center() + egui::vec2(dx, dy));
        lightcraft_tiles::mercator::from_world(x, y)
    };
    let (a_lat, a_lon) = point(-300.0, -150.0);
    let (b_lat, b_lon) = point(250.0, 150.0);
    let paths = [src.join("inspection.pdf").to_string_lossy().into_owned(), src.join("notes.txt").to_string_lossy().into_owned()];
    let r = run(&mut h, "map.attach", json!({"lat": a_lat, "lon": a_lon, "paths": paths}));
    let a = r["marker"].as_u64().unwrap();
    let pdf = r["attached"][0]["id"].as_u64().unwrap();
    let txt = r["attached"][1]["id"].as_u64().unwrap();
    let b = run(&mut h, "map.addMarker", json!({"lat": b_lat, "lon": b_lon, "title": "Pier 4"}))["id"].as_u64().unwrap();
    run(&mut h, "map.openMarker", json!({"id": a}));
    h.step();
    h.step();
    click_widget(&mut h, &format!("button:markerMove-{pdf}"));
    assert!(text(&h).contains("Click a marker, or a point for a new marker, for inspection.pdf"), "{}", text(&h));
    // a click on marker B
    let mb = widget(&h, &format!("mapMarker:{b}")).unwrap();
    click(&mut h, mb.center(), false);
    assert!(h.app.ui.map.moving.placing.is_none());
    let lib = root.join("library").join("Attachments");
    let on_b = h.app.session.catalog.marker(MarkerId(b)).unwrap();
    assert_eq!(on_b.attachments.len(), 1, "it is on marker B");
    assert!(lib.join(b.to_string()).join("inspection.pdf").is_file() && !lib.join(a.to_string()).join("inspection.pdf").exists());
    assert_eq!(h.app.session.catalog.marker(MarkerId(a)).unwrap().attachments.len(), 1);
    assert!(text(&h).contains("Moved inspection.pdf to marker"), "{}", text(&h));
    // the other file onto the empty map: a new marker there
    run(&mut h, "map.openMarker", json!({"id": a}));
    h.step();
    h.step();
    click_widget(&mut h, &format!("button:markerMove-{txt}"));
    let map = widget(&h, "map:view").unwrap();
    let at = egui::pos2(map.left() + 220.0, map.bottom() - 140.0);
    click(&mut h, at, false);
    let markers: Vec<_> = h.app.session.catalog.markers().cloned().collect();
    assert_eq!(markers.len(), 3, "a new marker for it");
    let new = markers.iter().find(|m| m.id.0 != a && m.id.0 != b).unwrap();
    assert_eq!(new.attachments.len(), 1);
    assert!(lib.join(new.id.0.to_string()).join("notes.txt").is_file());
    let vp = h.app.ui.map.markers.viewport.unwrap();
    let placed = vp.screen_of(lightcraft_tiles::mercator::to_world(new.lat, new.lon));
    assert!(placed.distance(at) < 1.5, "{placed:?} vs {at:?}");
    // undo: the file goes back
    run(&mut h, "edit.undo", json!({}));
    assert!(lib.join(a.to_string()).join("notes.txt").is_file());
    assert!(h.app.session.catalog.marker(new.id).is_none());
    let _ = std::fs::remove_dir_all(root);
}
