//! Settings ▸ Map: which tiles the Map view loads, the tile cache's size, Clear Cache.

use serde_json::json;

use super::{CACHE_CHOICES_MB, MapTiles};
use crate::LightcraftApp;
use crate::dialogs::settings::{choices, heading, hint, row};
use crate::i18n::{tr, tr_format};
use crate::theme::Tokens;
use crate::widgets::text_button;

pub fn settings_tab(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, t, tr("Map tiles"));
    row(ui, t, tr("Under the pins"), |ui| {
        let mut tiles = if app.ui.settings.map_tiles == MapTiles::Ask { MapTiles::Off } else { app.ui.settings.map_tiles };
        let opts = [(MapTiles::Off, tr("Off")), (MapTiles::Street, tr("Street (OpenStreetMap)")), (MapTiles::Imagery, tr("Imagery (USGS, US)"))];
        if choices(ui, "settingsMapTiles", &opts, &mut tiles) {
            app.ui.settings.map_tiles = tiles;
        }
    });
    hint(
        ui,
        t,
        tr(
            "Off draws the pins on a plain background and makes no network requests. Street and Imagery download map tiles, which tells OpenStreetMap or the USGS which area you look at (never which photos you have).",
        ),
    );
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
