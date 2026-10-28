//! Settings ▸ Map: which tiles the Map view loads, a tile service of the user's own, the tile
//! cache's size, Clear Cache.

use egui::RichText;
use serde_json::json;

use super::{CACHE_CHOICES_MB, MapTiles};
use crate::LightcraftApp;
use crate::dialogs::settings::{choices, heading, hint, row};
use crate::i18n::{tr, tr_format};
use crate::text_field::TextField;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

pub fn settings_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, t, tr("Map tiles"));
    row(ui, t, tr("Under the pins"), |ui| {
        let mut tiles = if app.ui.settings.map_tiles == MapTiles::Ask { MapTiles::Off } else { app.ui.settings.map_tiles };
        let custom = app.ui.settings.map_custom.label();
        let opts = [
            (MapTiles::Off, tr("Off")),
            (MapTiles::Street, tr("Street (OpenStreetMap)")),
            (MapTiles::Imagery, tr("Imagery (USGS, US)")),
            (MapTiles::Esri, tr("Imagery (Esri, worldwide)")),
            (MapTiles::Custom, custom.as_str()),
        ];
        if choices(ui, "settingsMapTiles", &opts, &mut tiles) {
            app.ui.settings.map_tiles = tiles;
        }
    });
    hint(
        ui,
        t,
        tr(
            "Off draws the pins on a plain background and makes no network requests. The others download map tiles, which tells the tile server (OpenStreetMap, the USGS, Esri or your own) which area you look at, never which photos you have. Esri's imagery is the sharpest built in (to about 30 cm a pixel in much of the world, deeper in some cities); USGS covers the United States only.",
        ),
    );
    custom_source(app, ui, t);
    heading(ui, t, tr("Tile cache"));
    row(ui, t, tr("Keep up to"), |ui| {
        let labels: Vec<(u32, String)> =
            CACHE_CHOICES_MB.iter().map(|mb| (*mb, if *mb >= 1000 { format!("{} GB", mb / 1000) } else { format!("{mb} MB") })).collect();
        let opts: Vec<(u32, &str)> = labels.iter().map(|(v, l)| (*v, l.as_str())).collect();
        choices(ui, "settingsMapCache", &opts, &mut app.ui.settings.map_cache_mb);
    });
    match app.services.map_tiles.clone() {
        Some(tiles) => {
            let used = tiles.cache_bytes();
            let used = tr_format!("In use: {mb} MB", mb = used / 1_000_000);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(used).size(11.5).color(t.text_dim));
                if text_button(ui, "settingsMapClear", "Clear Cache", false).clicked() {
                    let _ = app.run("map.clearCache", json!({}));
                }
            });
        }
        None => hint(ui, t, tr("This build loads no map tiles.")),
    }
    hint(ui, t, tr("Areas viewed before still draw from the cache when there is no connection."));
}

/// Settings ▸ Map ▸ Custom tile source: name, URL template, API key, deepest zoom, attribution,
/// and whether that makes a usable source (or what to fix).
fn custom_source(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, t, tr("Custom tile source"));
    hint(
        ui,
        t,
        tr(
            "Your own tile service: a state's orthoimagery, Esri's ArcGIS Location Platform, Mapbox Satellite or another XYZ server. The URL needs {z}, {x} and {y}; {key} is replaced by the API key.",
        ),
    );
    let c = &mut app.ui.settings.map_custom;
    row(ui, t, tr("Name"), |ui| {
        TextField::singleline("field:mapCustomName", &mut c.name).hint(tr("Custom")).label(tr("Custom tile source name")).width(220.0).show(ui);
    });
    row(ui, t, tr("URL template"), |ui| {
        draft_field(ui, "field:mapCustomUrl", &mut c.url, "https://…/{z}/{x}/{y}.png?key={key}", "Custom tile source URL template", false);
    });
    row(ui, t, tr("API key"), |ui| {
        draft_field(ui, "field:mapCustomKey", &mut c.api_key, "Only if the URL has {key}", "Custom tile source API key", true);
    });
    row(ui, t, tr("Deepest zoom"), |ui| {
        let r = ui.add(egui::DragValue::new(&mut c.max_native_zoom).range(0..=lightcraft_tiles::source::MAX_CUSTOM_ZOOM));
        register(ui, "field:mapCustomZoom", r.rect);
        ui.label(RichText::new(tr("the deepest zoom the service has tiles for; deeper views scale them up")).size(11.0).color(t.text_dim));
    });
    row(ui, t, tr("Attribution"), |ui| {
        TextField::singleline("field:mapCustomAttribution", &mut c.attribution)
            .hint(tr("Required: the credit the service asks you to show"))
            .label(tr("Custom tile source attribution"))
            .width(360.0)
            .show(ui);
    });
    let status = if c.url.trim().is_empty() { None } else { Some(c.source()) };
    match status {
        None => {}
        Some(Ok(_)) => {
            let ready = ui.label(RichText::new(tr("Ready: choose it under the pins above, or on the map.")).size(11.5).color(t.accent));
            register(ui, "text:mapCustomStatus", ready.rect);
        }
        Some(Err(e)) => {
            let bad = ui.label(RichText::new(tr(&e)).size(11.5).color(t.caution));
            register(ui, "text:mapCustomStatus", bad.rect);
        }
    }
    hint(
        ui,
        t,
        tr(
            "The API key is saved in LightCraft's settings file (ui.json) in plain text, never logged and never shown to scripts. Follow the service's terms: most ask for an account, a key and their attribution on the map.",
        ),
    );
}

/// A one-line field whose text reaches `value` only when the edit ends (Return or leaving the
/// field), so a half-typed URL or key never loads tiles.
fn draft_field(ui: &mut egui::Ui, widget: &str, value: &mut String, hint: &str, label: &str, password: bool) {
    let key = egui::Id::unique("lc-map-settings-draft").with(widget);
    let editing = crate::text_field::was_editing(ui.ctx(), widget);
    let mut draft = ui.data(|d| d.get_temp::<String>(key)).filter(|_| editing).unwrap_or_else(|| value.clone());
    let r = TextField::singleline(widget, &mut draft).hint(tr(hint)).label(tr(label)).width(360.0).password(password).show(ui);
    if r.committed() {
        *value = draft.trim().to_string();
    }
    // (kept only while the edit is under way: a key typed in is not left lying around)
    if r.editing {
        ui.data_mut(|d| d.insert_temp(key, draft));
    } else {
        ui.data_mut(|d| d.remove::<String>(key));
    }
}
