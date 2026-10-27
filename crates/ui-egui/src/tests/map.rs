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
    h.app.widgets.iter().filter(|(w, _)| w.starts_with("mapPin:") || w.starts_with("mapCluster:")).map(|(w, _)| w.clone()).collect()
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
fn close_pins_cluster_when_zoomed_out() {
    let tiles = Arc::new(FakeTiles::default());
    let mut h = demo(&tiles);
    bridge(&mut h);
    open_map(&mut h);
    h.app.ui.map.zoom = 10.0;
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
    let r = h.request("ui.set", json!({"map": {"zoom": 10.0}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.ui.map.zoom, 10.0);
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
