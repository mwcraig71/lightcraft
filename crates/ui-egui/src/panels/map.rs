//! Map: every photo in the grid's current list that has a location, as a pin on a map, in
//! capture order with a thin track line. A click on a pin selects the photo, a double click
//! opens it; selecting a photo in the filmstrip brings its pin on screen. Pins close together
//! on screen merge into a cluster with a count (a click zooms to it); far out, each site is one
//! dot with its name. See docs/map.md.
//!
//! Tiles (OpenStreetMap streets, USGS or Esri imagery, or a tile service of the user's own) come
//! from [`crate::Services::map_tiles`] and only once the user has turned them on (Settings ▸ Map,
//! or the prompt the first time): until then, and offline, the pins draw on a plain background
//! with a scale bar.
//!
//! - `custom`: the user's own tile service (its settings, `map.setCustomSource`).
//! - `pins`: which photos have a pin, their order, sites and screen-space clustering.
//! - `widget`: the slippy map (pan, zoom, tiles, scale bar, attribution, controls).
//! - `view`: the view around it (status line, filmstrip, prompts).
//! - `mini`: the Info panel's mini-map of the active photo's place.
//! - `markers`: the user's markers with attached files, and their card.
//! - `ortho`: drone orthomosaics drawn on the map, the Layers panel.
//! - `photo`: a photo shown large in the map's place (`map.showPhoto`) beside its inspection log.
//! - `notes`: the inspection log's list and editor, the pins' log badges.
//! - `moving`: moving photos, markers and attached files to another place (right-click ▸ Move…).

mod custom;
mod markers;
mod mini;
mod moving;
mod notes;
mod ortho;
mod photo;
mod pins;
mod settings;
mod view;
mod widget;

use std::borrow::Cow;

use lightcraft_catalog::PhotoId;
use lightcraft_tiles::TileSource;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::state::ViewMode;

pub use custom::{CustomTiles, HIDDEN_KEY, hide_key, keep_hidden_key};
pub use markers::{MarkerUi, drop_hint, poll as poll_markers, take_drop};
pub use mini::{MINI_MAP_H, mini_map};
pub use moving::{MoveUi, cancel as cancel_move};
pub use notes::{NotesUi, info_section as notes_section};
pub use photo::{close_photo, film_double_click, photo_mode, step_photo};
pub use pins::{Pin, cluster};
pub use settings::settings_tab;
pub use view::{show, tiles_loading};

/// Deepest zoom the view goes to (about 4 cm a point): past a source's deepest tiles they are
/// scaled up, so neighbouring photos on a deck still separate.
pub const MAX_ZOOM: f64 = 22.0;
pub const MIN_ZOOM: f64 = 1.0;
/// Zoom a single photo (or a cluster of photos at one spot) is shown at.
pub const PHOTO_ZOOM: f64 = 17.0;

/// The Map view's place, saved with the UI state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MapState {
    /// Centre in Web Mercator world coordinates (0..1).
    pub center: [f64; 2],
    pub zoom: f64,
    /// Draw the line through the pins in capture order.
    pub track: bool,
    /// Fit every pin on screen at the next frame (opening the view, Fit All).
    #[serde(skip, default)]
    pub fit_pending: bool,
    /// The set of pins last fitted (a new folder or filter fits again). Saved, so restoring the
    /// UI state (`ui.set`, the next launch) keeps the place instead of fitting again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fitted: Option<u64>,
    /// The active photo whose pin was last brought on screen.
    #[serde(skip)]
    pub followed: Option<u64>,
    /// The photos a click on a cluster or site narrowed the filmstrip to (empty: all of them).
    /// Saved like `fitted`, so restoring the UI state (`ui.set`) doesn't widen it again.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub focus: Vec<u64>,
    /// The markers' card and context menu (not saved).
    #[serde(skip)]
    pub markers: MarkerUi,
    /// The photo shown large in the map's place with its inspection log (`map.showPhoto`; `None`:
    /// the map). Saved like `focus`, so restoring the UI state (`ui.set`) keeps it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo: Option<u64>,
    /// The inspection log's editor (not saved).
    #[serde(skip)]
    pub notes: NotesUi,
    /// Moving photos, a marker or a file: the placing mode and the notice after (not saved).
    #[serde(skip)]
    pub moving: MoveUi,
}

impl Default for MapState {
    fn default() -> Self {
        MapState {
            center: [0.5, 0.5],
            zoom: 2.0,
            track: true,
            fit_pending: true,
            fitted: None,
            followed: None,
            focus: Vec::new(),
            markers: MarkerUi::default(),
            photo: None,
            notes: NotesUi::default(),
            moving: MoveUi::default(),
        }
    }
}

/// Map tiles, chosen in Settings ▸ Map. `Ask` until the user has chosen: no tiles are loaded
/// then (LightCraft makes no network request the user didn't turn on).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MapTiles {
    #[default]
    Ask,
    Off,
    Street,
    /// USGS The National Map (United States).
    Imagery,
    /// Esri World Imagery.
    Esri,
    /// The user's own tile service ([`CustomTiles`]).
    Custom,
}

impl MapTiles {
    pub const ALL: [MapTiles; 5] = [MapTiles::Off, MapTiles::Street, MapTiles::Imagery, MapTiles::Esri, MapTiles::Custom];

    /// The tile server, when tiles are on; `None` too for a custom source that isn't set up
    /// ([`CustomTiles::source`] says why).
    pub fn source(self, custom: &CustomTiles) -> Option<Cow<'static, TileSource>> {
        match self {
            MapTiles::Street => Some(Cow::Borrowed(&lightcraft_tiles::source::OSM)),
            MapTiles::Imagery => Some(Cow::Borrowed(&lightcraft_tiles::source::USGS_IMAGERY)),
            MapTiles::Esri => Some(Cow::Borrowed(&lightcraft_tiles::source::ESRI_IMAGERY)),
            MapTiles::Custom => custom.source().ok().map(Cow::Owned),
            MapTiles::Ask | MapTiles::Off => None,
        }
    }

    /// Command / control id.
    pub fn id(self) -> &'static str {
        match self {
            MapTiles::Ask => "ask",
            MapTiles::Off => "off",
            MapTiles::Street => "street",
            MapTiles::Imagery => "imagery",
            MapTiles::Esri => "esri",
            MapTiles::Custom => "custom",
        }
    }

    pub fn from_id(id: &str) -> Option<MapTiles> {
        [MapTiles::Ask, MapTiles::Off, MapTiles::Street, MapTiles::Imagery, MapTiles::Esri, MapTiles::Custom].into_iter().find(|m| m.id() == id)
    }

    /// Menu / button label (English; translated where shown). Where the custom source is set
    /// up, its own name is shown instead ([`CustomTiles::label`]).
    pub fn label(self) -> &'static str {
        match self {
            MapTiles::Ask | MapTiles::Off => "Off",
            MapTiles::Street => "Street",
            MapTiles::Imagery => "Imagery",
            MapTiles::Esri => "Esri",
            MapTiles::Custom => "Custom",
        }
    }
}

/// Disk cache size choices (MB) for Settings ▸ Map.
pub const CACHE_CHOICES_MB: [u32; 5] = [100, 250, 500, 1000, 2000];
pub const DEFAULT_CACHE_MB: u32 = 500;

/// `view.map` and the `map.*` commands; `None` for any other id.
pub(crate) fn run(app: &mut LightcraftApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if let Some(r) = markers::run(app, id, p) {
        return Some(r);
    }
    if let Some(r) = ortho::run(app, id, p) {
        return Some(r);
    }
    if let Some(r) = photo::run(app, id, p) {
        return Some(r);
    }
    if let Some(r) = moving::run(app, id, p) {
        return Some(r);
    }
    Some(match id {
        "view.map" if p.get("photo").is_some_and(|v| !v.is_null()) => open_at_photo(app, p.get("photo")),
        "view.map" => {
            app.ui.view = ViewMode::Map;
            app.ui.map.fit_pending = true;
            app.ui.map.focus.clear();
            let pins = pins::collect(app);
            Ok(json!({"photos": pins.total, "located": pins.pins.len()}))
        }
        "map.fitAll" => {
            app.ui.map.fit_pending = true;
            app.ui.map.focus.clear();
            Ok(Value::Null)
        }
        "map.focus" => focus(app, p.get("ids")),
        "map.toggleTrack" => match p.get("on") {
            None => {
                app.ui.map.track = !app.ui.map.track;
                Ok(json!({"on": app.ui.map.track}))
            }
            Some(Value::Bool(on)) => {
                app.ui.map.track = *on;
                Ok(json!({"on": on}))
            }
            Some(v) => Err(format!("map.toggleTrack: `on` must be true or false, not {v}")),
        },
        "map.setTileSource" => {
            let Some(tiles) = p.get("source").and_then(Value::as_str).and_then(MapTiles::from_id).filter(|t| *t != MapTiles::Ask) else {
                return Some(Err("map.setTileSource: `source` must be off, street, imagery, esri or custom".into()));
            };
            if tiles == MapTiles::Custom
                && let Err(e) = app.ui.settings.map_custom.source()
            {
                let why = format!("map.setTileSource: the custom tile source isn't set up (Settings ▸ Map ▸ Custom, or map.setCustomSource): {e}");
                return Some(Err(why));
            }
            app.ui.settings.map_tiles = tiles;
            Ok(json!({"source": tiles.id()}))
        }
        "map.setCustomSource" => custom::set_custom_source(app, p),
        "map.clearCache" => clear_cache(app),
        _ => return None,
    })
}

/// `map.focus {ids}`: narrow the filmstrip to these photos and select them (a click on a cluster
/// does this); `[]` or no `ids` shows every photo again.
fn focus(app: &mut LightcraftApp, ids: Option<&Value>) -> Result<Value, String> {
    let ids: Vec<u64> = match ids {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => {
            a.iter().map(|v| v.as_u64().ok_or_else(|| format!("map.focus: `ids` must be photo ids, not {v}"))).collect::<Result<_, _>>()?
        }
        Some(v) => return Err(format!("map.focus: `ids` must be a list of photo ids, not {v}")),
    };
    if !ids.is_empty() {
        app.run("library.select", json!({"ids": ids, "mode": "replace"}))?;
    }
    app.ui.map.focus = ids;
    Ok(json!({"focus": app.ui.map.focus.len()}))
}

/// `view.map {photo}`: select the photo and open the Map view centred on it at [`PHOTO_ZOOM`]
/// (the Info panel's mini-map), instead of fitting every pin.
fn open_at_photo(app: &mut LightcraftApp, photo: Option<&Value>) -> Result<Value, String> {
    let Some(id) = photo.and_then(Value::as_u64).map(PhotoId) else {
        return Err(format!("view.map: `photo` must be a photo id, not {}", photo.unwrap_or(&Value::Null)));
    };
    let Some(p) = app.session.catalog.photo(id) else {
        return Err(format!("view.map: no such photo {}", id.0));
    };
    let Some((lat, lon)) = p.meta.gps.filter(|g| pins::usable(*g)) else {
        return Err(format!("view.map: photo {} has no location", id.0));
    };
    app.run("library.select", json!({"ids": [id.0], "mode": "replace"}))?;
    app.ui.view = ViewMode::Map;
    let pins = pins::collect(app);
    let (x, y) = lightcraft_tiles::mercator::to_world(lat, lon);
    let m = &mut app.ui.map;
    m.center = [x, y];
    m.zoom = PHOTO_ZOOM;
    // these pins count as fitted, so the view doesn't fit them all on its first frame
    m.fit_pending = false;
    m.fitted = Some(pins.key);
    m.followed = Some(id.0);
    m.focus.clear();
    Ok(json!({"photos": pins.total, "located": pins.pins.len(), "photo": id.0}))
}

/// Delete the cached tiles on a background task (a large cache takes a while).
fn clear_cache(app: &mut LightcraftApp) -> Result<Value, String> {
    let Some(tiles) = app.services.map_tiles.clone() else {
        return Ok(json!({"freed": 0}));
    };
    let guard = app.session.activity.start("mapCache", "Clearing the map tile cache", lightcraft_engine::activity::Cancel::No);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let spawned = std::thread::Builder::new().name("lc-map-clear".into()).spawn(move || {
            let _guard = guard;
            if let Err(e) = tiles.clear_cache() {
                log::warn!("map: {e}");
            }
        });
        spawned.map_err(|e| format!("map.clearCache: {e}"))?;
        Ok(json!({"started": true}))
    }
    #[cfg(target_arch = "wasm32")]
    {
        drop(guard);
        tiles.clear_cache().map(|freed| json!({"freed": freed}))
    }
}
