//! Drone orthomosaics on the Map view, headless: adding one shows its build in the activity
//! stack, then draws its tiles (with map tiles off: offline imagery), the Layers panel lists it,
//! and hiding or removing it takes it off the map.

use std::time::Duration;

use lightcraft_ortho::testdata::{Spec, write_geotiff};
use serde_json::json;

use crate::LightcraftApp;
use crate::headless::Headless;
use crate::panels::map::MapTiles;
use crate::state::ViewMode;

const T: Duration = Duration::from_secs(60);

fn widget(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r)
}

struct Scratch(std::path::PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn an_added_orthomosaic_builds_in_the_activity_stack_then_draws_over_the_map() {
    let dir = Scratch(std::env::temp_dir().join(format!("lc-ui-ortho-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let tif = dir.0.join("deck.tif");
    let (e, n) = lightcraft_ortho::Crs::from_epsg(32610).unwrap().from_lat_lon(37.8199, -122.4783).unwrap();
    // 2048 px at 3 cm (61 m): enough tiles that the build is still running when the row is looked for
    let size = 2048;
    let half = f64::from(size) * 0.03 / 2.0;
    write_geotiff(&tif, &Spec::utm(size, size, 32610, 0.03, (e - half, n + half)), |x, y| {
        if (x / 128 + y / 128) % 2 == 0 { [230, 120, 40, 255] } else { [40, 90, 200, 255] }
    })
    .unwrap();

    let mut session = lightcraft_engine::Session::with_demo();
    session.orthomosaics.root_override = Some(dir.0.join("lib"));
    let app = LightcraftApp::new(session, crate::Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.settle(Duration::from_secs(20));
    h.app.ui.settings.map_tiles = MapTiles::Off;

    let r = h.request("engine.execute", json!({"command": "map.addOrthomosaic", "params": {"path": tif.to_string_lossy()}}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["result"]["id"], 1, "{r}");
    assert_eq!(h.app.ui.view, ViewMode::Map, "the map opens on it");
    let rows = h.app.session.activity.list();
    let building = rows.iter().find(|t| t.kind == "orthomosaic");
    let ready = h.app.session.orthomosaics().status(1) == lightcraft_engine::orthomosaic::Status::Ready;
    assert!(building.is_some_and(|t| t.cancellable && t.label.contains("orthomosaic")) || ready, "{rows:?}");
    h.step();
    // the footprint is on the map from the start
    assert!(widget(&h, "mapOverlay:1").is_some());

    assert!(h.app.session.wait_for_orthomosaics(T), "the build finishes");
    assert!(h.app.session.activity.list().iter().all(|t| t.kind != "orthomosaic"), "its row goes");
    assert!(h.step_until(T, |h| widget(h, "mapOverlayTiles:1").is_some()), "its tiles are drawn with map tiles off");
    let r = widget(&h, "mapOverlay:1").unwrap();
    let map = widget(&h, "map:view").unwrap();
    assert!(r.width() > map.width() * 0.3, "fitted to it: {r:?} in {map:?}");

    // the Layers panel lists it; hiding it leaves the outline only
    let r = h.request("engine.execute", json!({"command": "map.layers", "params": {"open": true}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(widget(&h, "map:layers").is_some());
    assert!(widget(&h, "mapOrthoVisible:1").is_some());
    assert!(h.painted_text().iter().any(|t| t.contains("deck")), "{:?}", h.painted_text());
    let r = h.request("engine.execute", json!({"command": "orthomosaic.set", "params": {"id": 1, "visible": false}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(widget(&h, "mapOverlayTiles:1").is_none(), "hidden");

    // zoomed far out it is an outline only
    h.request("engine.execute", json!({"command": "orthomosaic.set", "params": {"id": 1, "visible": true}}), T);
    h.app.ui.map.zoom = 6.0;
    h.step();
    h.step();
    assert!(widget(&h, "mapOverlay:1").is_some() && widget(&h, "mapOverlayTiles:1").is_none());

    // removed: off the map, tiles deleted
    let tiles = h.app.session.orthomosaics().tile_dir(1).unwrap();
    let r = h.request("engine.execute", json!({"command": "orthomosaic.remove", "params": {"id": 1}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(widget(&h, "mapOverlay:1").is_none());
    assert!(!tiles.exists());
    assert!(tif.exists(), "the GeoTIFF stays");
}

#[test]
fn a_bad_file_is_an_error_not_a_crash() {
    let dir = Scratch(std::env::temp_dir().join(format!("lc-ui-ortho-bad-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    std::fs::write(dir.0.join("bad.tif"), b"II*\0\x08\0\0\0junk").unwrap();
    let mut session = lightcraft_engine::Session::with_demo();
    session.orthomosaics.root_override = Some(dir.0.join("lib"));
    let app = LightcraftApp::new(session, crate::Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1000.0, 700.0], 1.0);
    h.settle(Duration::from_secs(20));
    let r = h.request("engine.execute", json!({"command": "map.addOrthomosaic", "params": {"path": dir.0.join("bad.tif").to_string_lossy()}}), T);
    assert_eq!(r["ok"], false, "{r}");
    let r = h.request("engine.execute", json!({"command": "map.fitOrthomosaic", "params": {"id": 4}}), T);
    assert_eq!(r["ok"], false, "{r}");
}
