//! The Map view, headless, with a fake tile provider (never the network).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_tiles::{TileId, TileImage, TileProvider, TileSource, TileState};
use serde_json::json;

use crate::headless::Headless;
use crate::panels::map::MapTiles;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

/// Solid tiles; counts what it was asked for.
#[derive(Default)]
struct FakeTiles {
    requests: AtomicU64,
}

impl TileProvider for FakeTiles {
    fn get(&self, _: &'static TileSource, _: TileId) -> TileState {
        self.requests.fetch_add(1, Ordering::Relaxed);
        TileState::Ready(Arc::new(TileImage { width: 2, height: 2, rgba: [90, 140, 90, 255].repeat(4) }))
    }
    fn end_frame(&self) {}
    fn requests(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }
    fn cache_bytes(&self) -> u64 {
        0
    }
    fn set_cache_limit(&self, _: u64) {}
    fn clear_cache(&self) -> Result<u64, String> {
        Ok(0)
    }
}

fn demo(tiles: &Arc<FakeTiles>) -> Headless {
    let services = Services { png: None, map_tiles: Some(tiles.clone()), ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.settle(T);
    h
}

fn set_gps(h: &mut Headless, id: PhotoId, gps: Option<(f64, f64)>) {
    let mut meta = h.app.session.catalog.photo(id).unwrap().meta.clone();
    meta.gps = gps;
    h.app.session.commit("setup", Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
}

/// Three located photos along a bridge (~25 m apart) and the rest without a location.
fn bridge(h: &mut Headless) -> Vec<PhotoId> {
    let ids: Vec<PhotoId> = h.app.session.visible_cloned();
    assert!(ids.len() >= 4, "the demo has photos");
    for (i, id) in ids.iter().enumerate() {
        let gps = (i < 3).then(|| (40.0, -75.0 + i as f64 * 0.0003));
        set_gps(h, *id, gps);
    }
    ids
}

fn open_map(h: &mut Headless) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": "view.map"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    h.step();
    r
}

fn widget(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r)
}

fn pins(h: &Headless) -> Vec<String> {
    h.app
        .widgets
        .iter()
        .filter(|(w, _)| w.starts_with("mapPin:") || w.starts_with("mapCluster:") || w.starts_with("mapSite:"))
        .map(|(w, _)| w.clone())
        .collect()
}

fn click_at(h: &mut Headless, at: egui::Pos2) {
    let r = h.request("ui.click", json!({"x": at.x, "y": at.y}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

#[test]
fn shows_a_pin_per_located_photo_and_counts_the_rest() {
    let tiles = Arc::new(FakeTiles::default());
    let mut h = demo(&tiles);
    let ids = bridge(&mut h);
    let r = open_map(&mut h);
    assert_eq!(h.app.ui.view, ViewMode::Map);
    assert_eq!(r["result"]["located"], 3, "{r}");
    let mut found = pins(&h);
    found.sort();
    let mut want: Vec<String> = ids[..3].iter().map(|i| format!("mapPin:{}", i.0)).collect();
    want.sort();
    assert_eq!(found, want, "fitted to three separate pins");
    let text = h.painted_text().join("\n");
    let without = ids.len() - 3;
    assert!(text.contains(&format!("{} photos · 3 with location", ids.len())), "{text}");
    assert!(text.contains(&format!("{without} without location")), "{text}");
    // the first time, no tiles until asked, and the question is on screen
    assert_eq!(h.app.ui.settings.map_tiles, MapTiles::Ask);
    assert!(widget(&h, "map:ask").is_some());
    assert_eq!(tiles.requests(), 0, "no tile requested before tiles are turned on");
}

#[test]
fn clicking_a_pin_selects_its_photo() {
    let tiles = Arc::new(FakeTiles::default());
    let mut h = demo(&tiles);
    let ids = bridge(&mut h);
    open_map(&mut h);
    let target = ids[1];
    let pin = widget(&h, &format!("mapPin:{}", target.0)).expect("pin");
    click_at(&mut h, pin.center());
    assert_eq!(h.app.session.active(), Some(target));
    assert_eq!(h.app.session.selection.ids, vec![target]);
    // "N without location" selects those photos
    let link = widget(&h, "map:without").expect("link");
    click_at(&mut h, link.center());
    assert_eq!(h.app.session.selection.ids, ids[3..].to_vec());
}

#[test]
fn far_out_the_photos_are_one_named_site_then_a_cluster() {
    let tiles = Arc::new(FakeTiles::default());
    let mut h = demo(&tiles);
    bridge(&mut h);
    open_map(&mut h);
    // far out: one dot named after the source (demo photos have no folder)
    h.app.ui.map.zoom = 10.0;
    h.step();
    h.step();
    let title = crate::i18n::source_title(&h.app.session);
    assert_eq!(pins(&h), vec![format!("mapSite:{title}")]);
    assert!(h.painted_text().iter().any(|t| t == &title), "the site is labelled");
    // a click on it zooms in to its photos
    let s = widget(&h, &format!("mapSite:{title}")).unwrap();
    click_at(&mut h, s.center());
    assert!(h.app.ui.map.zoom >= 15.0, "{}", h.app.ui.map.zoom);
    // closer: a cluster with a count
    h.app.ui.map.zoom = 15.0;
    h.step();
    h.step();
    assert_eq!(pins(&h), vec!["mapCluster:3".to_string()]);
    // a click on the cluster zooms in until they separate
    let c = widget(&h, "mapCluster:3").unwrap();
    click_at(&mut h, c.center());
    h.step();
    assert!(h.app.ui.map.zoom > 15.0, "{}", h.app.ui.map.zoom);
    assert_eq!(pins(&h).len(), 3, "{:?}", pins(&h));
}

#[test]
fn tiles_draw_with_attribution_only_once_turned_on() {
    let tiles = Arc::new(FakeTiles::default());
    let mut h = demo(&tiles);
    bridge(&mut h);
    let r = h.request("engine.execute", json!({"command": "map.setTileSource", "params": {"source": "off"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    open_map(&mut h);
    assert_eq!(tiles.requests(), 0, "tiles off: no requests");
    assert!(widget(&h, "map:ask").is_none(), "asked once only");
    assert!(widget(&h, "map:attribution").is_none());
    let r = h.request("engine.execute", json!({"command": "map.setTileSource", "params": {"source": "street"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(tiles.requests() > 0);
    assert!(widget(&h, "map:attribution").is_some());
    assert!(h.painted_text().iter().any(|t| t.contains("© OpenStreetMap contributors")));
    let r = h.request("engine.execute", json!({"command": "map.setTileSource", "params": {"source": "imagery"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(h.painted_text().iter().any(|t| t.contains("USGS The National Map")));
    let bad = h.request("engine.execute", json!({"command": "map.setTileSource", "params": {"source": "satellite"}}), T);
    assert_ne!(bad["ok"], true, "{bad}");
}

#[test]
fn the_filmstrip_selection_brings_its_pin_on_screen() {
    let tiles = Arc::new(FakeTiles::default());
    let mut h = demo(&tiles);
    let ids = bridge(&mut h);
    open_map(&mut h);
    // look far away, then pick a located photo elsewhere
    h.app.ui.map.center = [0.9, 0.9];
    h.step();
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [ids[2].0]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    let view = widget(&h, "map:view").unwrap();
    let pin = widget(&h, &format!("mapPin:{}", ids[2].0)).expect("pin drawn");
    assert!(view.contains(pin.center()));
}

#[test]
fn ui_set_moves_the_map_without_fitting_it_again() {
    let tiles = Arc::new(FakeTiles::default());
    let mut h = demo(&tiles);
    bridge(&mut h);
    open_map(&mut h);
    let r = h.request("ui.set", json!({"map": {"zoom": 15.0}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.ui.map.zoom, 15.0);
    assert_eq!(pins(&h), vec!["mapCluster:3".to_string()]);
    // Fit All does fit again
    let r = h.request("engine.execute", json!({"command": "map.fitAll"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(h.app.ui.map.zoom > 15.0);
}

#[test]
fn photos_with_a_heading_get_a_view_cone() {
    let tiles = Arc::new(FakeTiles::default());
    let mut h = demo(&tiles);
    let ids = bridge(&mut h);
    let mut meta = h.app.session.catalog.photo(ids[0]).unwrap().meta.clone();
    meta.gps_direction = Some(90.0);
    meta.gps_direction_magnetic = true;
    h.app.session.commit("setup", Op::SetMeta { id: ids[0], meta: Box::new(meta) }).unwrap();
    open_map(&mut h);
    assert!(widget(&h, &format!("mapCone:{}", ids[0].0)).is_some());
    assert!(widget(&h, &format!("mapCone:{}", ids[1].0)).is_none(), "no heading, no cone");
    // the hover card says which way, and that it is magnetic
    let pin = widget(&h, &format!("mapPin:{}", ids[0].0)).unwrap();
    let r = h.request("ui.move", json!({"x": pin.center().x, "y": pin.center().y}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(h.painted_text().iter().any(|t| t.contains("Heading 90° (magnetic)")), "{:?}", h.painted_text());
}

/// The demo with the Info panel open on `active`, tall enough for the GPS field to be on screen.
fn info_on(tiles: &Arc<FakeTiles>, active: impl Fn(&[PhotoId]) -> PhotoId) -> (Headless, Vec<PhotoId>) {
    let services = Services { png: None, map_tiles: Some(tiles.clone()), ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1400.0, 2000.0], 1.0);
    h.settle(T);
    let ids = bridge(&mut h);
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [active(&ids).0]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.app.ui.right = crate::state::RightPanel::Info;
    h.app.ui.view = ViewMode::PhotoGrid;
    h.step();
    h.step();
    (h, ids)
}

#[test]
fn the_info_panel_has_a_mini_map_for_a_located_photo() {
    let tiles = Arc::new(FakeTiles::default());
    let (mut h, ids) = info_on(&tiles, |ids| ids[0]);
    assert_eq!(h.app.ui.view, ViewMode::PhotoGrid);
    let mini = widget(&h, "info:miniMap").expect("a mini-map under the GPS field");
    let panel = widget(&h, "panel:right_panel").expect("the right panel");
    assert!((mini.height() - crate::panels::map::MINI_MAP_H).abs() < 0.5, "{mini:?}");
    assert!(mini.left() >= panel.left() && mini.right() <= panel.right() + 0.5, "{mini:?} in {panel:?}");
    // tiles not chosen yet: none drawn, and the mini-map doesn't ask
    assert_eq!(tiles.requests(), 0);
    assert!(widget(&h, "map:ask").is_none());
    assert!(widget(&h, "info:miniMapAttribution").is_none());
    // a photo without a location has none
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [ids[3].0]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.session.active(), Some(ids[3]));
    assert!(widget(&h, "info:miniMap").is_none(), "no location, no mini-map");
}

#[test]
fn the_mini_map_shows_the_accuracy_circle() {
    let tiles = Arc::new(FakeTiles::default());
    let (mut h, ids) = info_on(&tiles, |ids| ids[0]);
    assert!(widget(&h, "info:miniMapAccuracy").is_none(), "no positioning error, no circle");
    let mut meta = h.app.session.catalog.photo(ids[0]).unwrap().meta.clone();
    meta.gps_accuracy = Some(15.0);
    h.app.session.commit("setup", Op::SetMeta { id: ids[0], meta: Box::new(meta) }).unwrap();
    h.step();
    h.step();
    let mini = widget(&h, "info:miniMap").unwrap();
    let circle = widget(&h, "info:miniMapAccuracy").expect("a ±15 m circle at zoom 17");
    assert!(mini.contains(circle.center()), "{circle:?} in {mini:?}");
    assert!(circle.width() > 20.0, "{circle:?}");
}

#[test]
fn the_mini_map_draws_tiles_only_when_they_are_on() {
    let tiles = Arc::new(FakeTiles::default());
    let (mut h, _) = info_on(&tiles, |ids| ids[0]);
    let r = h.request("engine.execute", json!({"command": "map.setTileSource", "params": {"source": "off"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(widget(&h, "info:miniMap").is_some());
    assert_eq!(tiles.requests(), 0, "tiles off: no requests");
    let r = h.request("engine.execute", json!({"command": "map.setTileSource", "params": {"source": "street"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(tiles.requests() > 0, "tiles on: the mini-map draws them");
    let mini = widget(&h, "info:miniMap").unwrap();
    let a = widget(&h, "info:miniMapAttribution").expect("the attribution while tiles are drawn");
    assert!(mini.contains(a.center()), "{a:?} in {mini:?}");
    assert!(h.painted_text().iter().any(|t| t.contains("© OpenStreetMap contributors")));
}

#[test]
fn clicking_the_mini_map_opens_the_map_view_on_the_photo() {
    let tiles = Arc::new(FakeTiles::default());
    let (mut h, ids) = info_on(&tiles, |ids| ids[1]);
    // several photos selected, Info showing the active one (Target Photo)
    h.app.ui.settings.info_selected_photos = false;
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [ids[0].0, ids[1].0], "active": ids[1].0}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    let mini = widget(&h, "info:miniMap").expect("mini-map");
    click_at(&mut h, mini.center());
    assert_eq!(h.app.ui.view, ViewMode::Map);
    for _ in 0..4 {
        h.step();
    }
    let want = lightcraft_tiles::mercator::to_world(40.0, -75.0 + 0.0003);
    let m = &h.app.ui.map;
    assert_eq!(m.zoom, crate::panels::map::PHOTO_ZOOM, "not fitted to all the pins");
    assert!((m.center[0] - want.0).abs() < 1e-9 && (m.center[1] - want.1).abs() < 1e-9, "{:?} vs {want:?}", m.center);
    assert_eq!(h.app.session.active(), Some(ids[1]));
    assert_eq!(h.app.session.selection.ids, vec![ids[1]]);
    let view = widget(&h, "map:view").unwrap();
    // (at this zoom the bridge's photos, ~25 m apart, are one cluster around it)
    let near = pins(&h).iter().filter_map(|w| widget(&h, w)).map(|r| r.center().distance(view.center())).fold(f32::MAX, f32::min);
    assert!(near < 30.0, "its pin or cluster in the middle: {:?}", pins(&h));
}

#[test]
fn view_map_photo_param_checks_the_photo() {
    let tiles = Arc::new(FakeTiles::default());
    let mut h = demo(&tiles);
    let ids = bridge(&mut h);
    for (photo, why) in [(json!(ids[3].0), "has no location"), (json!(999_999), "no such photo"), (json!("x"), "must be a photo id")] {
        let r = h.request("engine.execute", json!({"command": "view.map", "params": {"photo": photo}}), T);
        assert_ne!(r["ok"], true, "{r}");
        assert!(r.to_string().contains(why), "{r}");
    }
    let r = h.request("engine.execute", json!({"command": "view.map", "params": {"photo": ids[2].0}}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["result"]["photo"], ids[2].0, "{r}");
    assert_eq!(h.app.ui.map.zoom, crate::panels::map::PHOTO_ZOOM);
}

#[test]
fn photos_with_a_positioning_error_get_an_accuracy_circle() {
    let tiles = Arc::new(FakeTiles::default());
    let mut h = demo(&tiles);
    let ids = bridge(&mut h);
    for (id, metres) in [(ids[0], 20.0), (ids[2], 0.5)] {
        let mut meta = h.app.session.catalog.photo(id).unwrap().meta.clone();
        meta.gps_accuracy = Some(metres);
        h.app.session.commit("setup", Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
    }
    open_map(&mut h);
    let r = h.request("ui.set", json!({"map": {"zoom": 18.0}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    // 20 m at 40° north and zoom 18 (about 0.457 m per point) is about 44 points each way
    let circle = widget(&h, &format!("mapAccuracy:{}", ids[0].0)).expect("accuracy circle drawn");
    assert!((circle.width() - 87.5).abs() < 2.0, "{circle:?}");
    let pin = widget(&h, &format!("mapPin:{}", ids[0].0)).unwrap();
    assert!(circle.center().distance(pin.center()) < 1.0, "centred on the pin");
    assert!(widget(&h, &format!("mapAccuracy:{}", ids[1].0)).is_none(), "no error recorded, no circle");
    assert!(widget(&h, &format!("mapAccuracy:{}", ids[2].0)).is_none(), "smaller than the pin, not drawn");
    // the hover card says how far off it may be
    let r = h.request("ui.move", json!({"x": pin.center().x, "y": pin.center().y}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(h.painted_text().iter().any(|t| t.contains("±20 m")), "{:?}", h.painted_text());
}

/// File ▸ Export Photo Locations… is in the menu, writes the photos shown (KML by the extension)
/// and says how many it wrote and left out.
#[test]
fn export_photo_locations_writes_kml_and_says_what_it_left_out() {
    let tiles = Arc::new(FakeTiles::default());
    let mut h = demo(&tiles);
    let ids = bridge(&mut h);
    let items = h.request("ui.menu.list", json!({}), T)["result"].to_string();
    assert!(items.contains("file.exportLocations"), "{items}");
    let dir = std::env::temp_dir().join(format!("lc-ui-locations-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sites.kml");
    let r = h.request("engine.execute", json!({"command": "file.exportLocations", "params": {"path": path.to_string_lossy()}}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["result"]["written"], 3, "{r}");
    let kml = std::fs::read_to_string(&path).unwrap();
    assert_eq!(kml.matches("<Placemark>").count(), 4, "three photos and the track: {kml}");
    let toast = h.app.ui.toast.clone().map(|t| t.0).unwrap_or_default();
    assert!(toast.contains("Exported 3 photo locations") && toast.contains(&format!("{} without a location", ids.len() - 3)), "{toast}");
    // a folder that isn't there: an error in the toast, not a panic
    let gone = dir.join("gone").join("x.kml");
    let r = h.request("engine.execute", json!({"command": "file.exportLocations", "params": {"path": gone.to_string_lossy()}}), T);
    assert_eq!(r["ok"], false, "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clicking_a_cluster_narrows_the_filmstrip_to_its_photos() {
    let tiles = Arc::new(FakeTiles::default());
    let mut h = demo(&tiles);
    let ids = bridge(&mut h);
    open_map(&mut h);
    let film = |h: &Headless| -> Vec<String> {
        let mut f: Vec<String> = h.app.widgets.iter().filter(|(w, _)| w.starts_with("film:")).map(|(w, _)| w.clone()).collect();
        f.sort();
        f
    };
    assert!(film(&h).len() > 3, "every photo in the strip at first: {:?}", film(&h));
    h.app.ui.map.zoom = 15.0;
    h.step();
    h.step();
    let c = widget(&h, "mapCluster:3").expect("one cluster of the three");
    click_at(&mut h, c.center());
    h.step();
    let mut want: Vec<String> = ids[..3].iter().map(|i| format!("film:{}", i.0)).collect();
    want.sort();
    assert_eq!(film(&h), want, "only the cluster's photos in the filmstrip");
    let mut sel = h.app.session.selection.ids.clone();
    sel.sort();
    let mut three = ids[..3].to_vec();
    three.sort();
    assert_eq!(sel, three, "and selected");
    assert!(h.painted_text().iter().any(|t| t.contains("Filmstrip: 3 photos here")), "{:?}", h.painted_text());
    // Show All brings every photo back
    let all = widget(&h, "map:focusClear").expect("Show All");
    click_at(&mut h, all.center());
    h.step();
    assert!(film(&h).len() > 3, "{:?}", film(&h));
    assert!(widget(&h, "map:focusClear").is_none());
    // and so does Fit All after narrowing again by command
    let r = h.request("engine.execute", json!({"command": "map.focus", "params": {"ids": [ids[0].0]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(film(&h), vec![format!("film:{}", ids[0].0)]);
    // moving the map through the UI state keeps it narrowed
    let r = h.request("ui.set", json!({"map": {"zoom": 16.0}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(film(&h), vec![format!("film:{}", ids[0].0)]);
    let r = h.request("engine.execute", json!({"command": "map.fitAll"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(film(&h).len() > 3);
    let bad = h.request("engine.execute", json!({"command": "map.focus", "params": {"ids": "all"}}), T);
    assert_ne!(bad["ok"], true, "{bad}");
}
