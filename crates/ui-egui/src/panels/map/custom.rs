//! The user's own tile service (Settings ▸ Map ▸ Custom, `map.setCustomSource`): a URL template
//! with `{z}`, `{x}`, `{y}` and optionally `{key}`, an API key, the deepest zoom it has tiles for
//! and the attribution its terms ask for. The key is saved with the other settings (`ui.json`, in
//! plain text) but never shown: `Debug` hides it, and so does the control channel's view of the
//! settings ([`hide_key`]).

use lightcraft_tiles::TileSource;
use lightcraft_tiles::source::{CustomSpec, MAX_CUSTOM_ZOOM};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::MapTiles;
use crate::LightcraftApp;

/// What the control channel shows (and `ui.set` takes back unchanged) in place of an API key.
pub const HIDDEN_KEY: &str = "(hidden)";

/// Settings ▸ Map ▸ Custom.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomTiles {
    /// Shown on the map's tile buttons.
    pub name: String,
    /// `https://…/{z}/{x}/{y}…`, `{key}` where the API key goes.
    pub url: String,
    pub api_key: String,
    /// The deepest zoom the service has tiles for; deeper views scale them up.
    pub max_native_zoom: u8,
    /// Shown on the map whenever its tiles are drawn (the service's terms say what).
    pub attribution: String,
}

impl Default for CustomTiles {
    fn default() -> Self {
        CustomTiles { name: String::new(), url: String::new(), api_key: String::new(), max_native_zoom: 19, attribution: String::new() }
    }
}

impl std::fmt::Debug for CustomTiles {
    /// Everything but the API key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomTiles")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("api_key", &if self.api_key.is_empty() { "" } else { HIDDEN_KEY })
            .field("max_native_zoom", &self.max_native_zoom)
            .field("attribution", &self.attribution)
            .finish()
    }
}

impl CustomTiles {
    /// The tile source, or what to fix.
    pub fn source(&self) -> Result<TileSource, String> {
        TileSource::custom(CustomSpec {
            name: &self.name,
            url_template: &self.url,
            api_key: &self.api_key,
            max_native_zoom: self.max_native_zoom,
            attribution: &self.attribution,
        })
    }

    /// The name on the map's buttons: the one given (shortened), else "Custom".
    pub fn label(&self) -> String {
        let n = self.name.trim();
        if n.is_empty() { crate::i18n::tr("Custom").to_string() } else { n.chars().take(24).collect() }
    }
}

/// Hide the custom tile source's API key in the UI state as the control channel shows it
/// (`ui.inspect`, `ui.set`'s reply).
pub fn hide_key(ui_state: &mut Value) {
    if let Some(k) = ui_state.pointer_mut("/settings/mapCustom/apiKey")
        && k.as_str().is_some_and(|s| !s.is_empty())
    {
        *k = Value::String(HIDDEN_KEY.into());
    }
}

/// `ui.set` was handed the state with the key hidden ([`hide_key`]): keep the key it had.
pub fn keep_hidden_key(before: &CustomTiles, after: &mut CustomTiles) {
    if after.api_key == HIDDEN_KEY {
        after.api_key.clone_from(&before.api_key);
    }
}

/// `map.setCustomSource {name?, url?, apiKey?, maxNativeZoom?, attribution?, use?}`: change the
/// fields given (the others stay), check the result and, with `use: true`, show it. The reply has
/// whether there is a key, never the key.
pub(super) fn set_custom_source(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let mut c = app.ui.settings.map_custom.clone();
    let text = |k: &str| -> Result<Option<String>, String> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.trim().to_string())),
            Some(_) => Err(format!("map.setCustomSource: `{k}` must be text")),
        }
    };
    if let Some(v) = text("name")? {
        c.name = v;
    }
    if let Some(v) = text("url")? {
        c.url = v;
    }
    if let Some(v) = text("apiKey")? {
        c.api_key = v;
    }
    if let Some(v) = text("attribution")? {
        c.attribution = v;
    }
    if let Some(v) = p.get("maxNativeZoom").filter(|v| !v.is_null()) {
        let Some(z) = v.as_u64().and_then(|z| u8::try_from(z).ok()).filter(|z| *z <= MAX_CUSTOM_ZOOM) else {
            return Err(format!("map.setCustomSource: `maxNativeZoom` must be a whole number from 0 to {MAX_CUSTOM_ZOOM}"));
        };
        c.max_native_zoom = z;
    }
    let use_it = match p.get("use") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("map.setCustomSource: `use` must be true or false".into()),
    };
    let source = c.source().map_err(|e| format!("map.setCustomSource: {e}"))?;
    app.ui.settings.map_custom = c;
    if use_it {
        app.ui.settings.map_tiles = MapTiles::Custom;
    }
    Ok(json!({
        "name": source.name,
        "maxNativeZoom": source.max_native_zoom,
        "hasKey": source.has_key(),
        "source": app.ui.settings.map_tiles.id(),
    }))
}
