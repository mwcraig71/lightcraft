//! The tile servers the Map view can show. Each is a URL template with `{z}`, `{x}` and `{y}`
//! (the template fixes the order: USGS and Esri put the row before the column), the zooms it
//! serves, and the attribution that must stay visible on the map.
//!
//! The built-in servers are statics; a tile service of the user's own ([`TileSource::custom`]) is
//! an owned value built from its settings. Its API key (substituted for `{key}`) is private to
//! the source: [`TileSource::url`] is the only place it appears, `Debug` hides it, and
//! [`TileSource::redact`] takes it out of any text (errors) before it is logged.

use std::borrow::Cow;
use std::fmt;

use crate::mercator::TileId;

/// One tile server.
#[derive(Clone, PartialEq, Eq)]
pub struct TileSource {
    /// Stable id (cache folder, texture and request keys). A custom source's id is
    /// `custom-<hash of its URL template>`, so a changed template never shows the old one's tiles.
    pub id: Cow<'static, str>,
    /// Name shown in menus (English for the built-in ones; the UI translates it).
    pub name: Cow<'static, str>,
    /// The URL with `{z}`, `{x}`, `{y}` (and `{key}`), as given: never with the key filled in.
    pub url_template: Cow<'static, str>,
    pub min_zoom: u8,
    /// Deepest zoom the view may use with this source.
    pub max_zoom: u8,
    /// Deepest zoom the server has tiles for; deeper views scale these up (overzoom).
    pub max_native_zoom: u8,
    /// Shown bottom-right on the map whenever the source's tiles are drawn.
    pub attribution: Cow<'static, str>,
    /// The `ETag` of the "no data here" picture the server sends where it has no imagery (Esri's
    /// grey "Map data not yet available"): such a tile counts as missing, so a coarser one stands in.
    pub no_data_etag: Option<&'static str>,
    /// The API key substituted for `{key}` (custom sources only).
    key: Option<String>,
}

/// OpenStreetMap's standard tiles. Their usage policy: a User-Agent naming the app, honour the
/// cache headers, no bulk download or offline areas, attribution always visible.
pub static OSM: TileSource = TileSource {
    id: Cow::Borrowed("osm"),
    name: Cow::Borrowed("Street (OpenStreetMap)"),
    url_template: Cow::Borrowed("https://tile.openstreetmap.org/{z}/{x}/{y}.png"),
    min_zoom: 0,
    max_zoom: 19,
    max_native_zoom: 19,
    attribution: Cow::Borrowed("© OpenStreetMap contributors"),
    no_data_etag: None,
    key: None,
};

/// USGS The National Map orthoimagery (US and Puerto Rico; public domain). Tiles to zoom 16,
/// scaled up beyond so pins at neighbouring piers separate.
pub static USGS_IMAGERY: TileSource = TileSource {
    id: Cow::Borrowed("usgs-imagery"),
    name: Cow::Borrowed("Imagery (USGS, United States)"),
    url_template: Cow::Borrowed("https://basemap.nationalmap.gov/arcgis/rest/services/USGSImageryOnly/MapServer/tile/{z}/{y}/{x}"),
    min_zoom: 0,
    max_zoom: 19,
    max_native_zoom: 16,
    attribution: Cow::Borrowed("USDA, USGS The National Map: Orthoimagery"),
    no_data_etag: None,
    key: None,
};

/// Esri World Imagery (worldwide). Real imagery reaches zoom 19 across the US and Europe (zoom 21
/// in some metro areas, such as over the Golden Gate Bridge); deeper, and where there is no
/// imagery, the server answers with a grey "Map data not yet available" JPEG (always the same
/// `ETag`), so tiles past 19 are scaled up instead. The attribution is the service's
/// `copyrightText`, after "Esri" as its terms ask. See docs/map.md for the licence caveat.
pub static ESRI_IMAGERY: TileSource = TileSource {
    id: Cow::Borrowed("esri-imagery"),
    name: Cow::Borrowed("Imagery (Esri, worldwide)"),
    url_template: Cow::Borrowed("https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}"),
    min_zoom: 0,
    max_zoom: 19,
    max_native_zoom: 19,
    attribution: Cow::Borrowed("Esri — Source: Esri, Vantor, Earthstar Geographics, and the GIS User Community"),
    no_data_etag: Some("\"vvvvvvvvvvvvf\""),
    key: None,
};

/// Every built-in source, in menu order.
pub static ALL: [&TileSource; 3] = [&OSM, &USGS_IMAGERY, &ESRI_IMAGERY];

/// The built-in source with this id.
pub fn by_id(id: &str) -> Option<&'static TileSource> {
    ALL.iter().copied().find(|s| s.id == id)
}

/// Deepest zoom a custom source may claim to have tiles for.
pub const MAX_CUSTOM_ZOOM: u8 = 22;
/// Longest URL template, API key, name or attribution accepted (characters).
const MAX_FIELD: usize = 2048;

/// What the user typed for a tile service of their own (Settings ▸ Map ▸ Custom).
#[derive(Clone, Copy, Debug)]
pub struct CustomSpec<'a> {
    pub name: &'a str,
    /// `https://…{z}…{x}…{y}…`, optionally with `{key}`.
    pub url_template: &'a str,
    /// Substituted for `{key}`; may be empty when the template has none.
    pub api_key: &'a str,
    pub max_native_zoom: u8,
    pub attribution: &'a str,
}

impl fmt::Debug for TileSource {
    /// Everything but the API key (only whether there is one).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TileSource")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("url_template", &self.url_template)
            .field("min_zoom", &self.min_zoom)
            .field("max_zoom", &self.max_zoom)
            .field("max_native_zoom", &self.max_native_zoom)
            .field("attribution", &self.attribution)
            .field("no_data_etag", &self.no_data_etag)
            .field("key", &self.key.as_ref().map(|_| "<hidden>"))
            .finish()
    }
}

impl fmt::Display for TileSource {
    /// The source's name, for messages and logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// Check a custom URL template: `https://` with a host, `{z}`, `{x}` and `{y}` each once, no
/// other `{…}` than `{key}`, no spaces. The error says what to fix.
pub fn validate_template(template: &str) -> Result<(), String> {
    let t = template.trim();
    if t.is_empty() {
        return Err("Enter the tile URL template.".into());
    }
    if t.chars().count() > MAX_FIELD {
        return Err(format!("The URL template is too long (over {MAX_FIELD} characters)."));
    }
    let Some(rest) = t.get(..8).filter(|s| s.eq_ignore_ascii_case("https://")).and_then(|_| t.get(8..)) else {
        return Err("The URL template must start with https:// (tiles are only loaded over a secure connection).".into());
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if host.is_empty() || host.contains('{') {
        return Err("The URL template needs a server name after https:// (no {…} in it).".into());
    }
    if t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("The URL template must not contain spaces.".into());
    }
    for p in ["{z}", "{x}", "{y}"] {
        match t.matches(p).count() {
            0 => {
                return Err(format!(
                    "The URL template needs {p} (the template uses {{z}} for the zoom, {{x}} for the column and {{y}} for the row)."
                ));
            }
            1 => {}
            _ => return Err(format!("The URL template has {p} more than once.")),
        }
    }
    // every `{…}` must be one of ours
    let mut s = t;
    while let Some(open) = s.find('{') {
        let after = s.get(open..).unwrap_or_default();
        let Some(close) = after.find('}') else { return Err("The URL template has a { without a matching }.".into()) };
        let name = after.get(..=close).unwrap_or_default();
        if !matches!(name, "{z}" | "{x}" | "{y}" | "{key}") {
            return Err(format!("The URL template has {name}: only {{z}}, {{x}}, {{y}} and {{key}} are filled in."));
        }
        s = after.get(close + 1..).unwrap_or_default();
    }
    if t.contains('}') && t.matches('}').count() != t.matches('{').count() {
        return Err("The URL template has a } without a matching {.".into());
    }
    Ok(())
}

/// FNV-1a, 64-bit: a hash that stays the same across runs and builds (the cache folder's name).
fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// `s` with every byte outside the URL-safe set percent-encoded (an API key in a query string).
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl TileSource {
    /// A tile service of the user's own. Checks the template ([`validate_template`]), that a
    /// template with `{key}` has a key, and that the name and attribution are filled in.
    pub fn custom(spec: CustomSpec<'_>) -> Result<TileSource, String> {
        validate_template(spec.url_template)?;
        let template = spec.url_template.trim();
        let key = spec.api_key.trim();
        if template.contains("{key}") && key.is_empty() {
            return Err("The URL template has {key}: enter the API key.".into());
        }
        if key.chars().count() > MAX_FIELD || key.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("The API key must not contain spaces.".into());
        }
        let attribution = spec.attribution.trim();
        if attribution.is_empty() {
            return Err("Enter the attribution the tile service asks you to show (for example \"© Mapbox © OpenStreetMap\").".into());
        }
        let name = spec.name.trim();
        let name = if name.is_empty() { "Custom" } else { name };
        let short = |s: &str| s.chars().take(MAX_FIELD).collect::<String>();
        let max_native_zoom = spec.max_native_zoom.min(MAX_CUSTOM_ZOOM);
        Ok(TileSource {
            id: Cow::Owned(format!("custom-{:016x}", fnv1a(template))),
            name: Cow::Owned(short(name)),
            url_template: Cow::Owned(template.to_string()),
            min_zoom: 0,
            max_zoom: max_native_zoom.max(19),
            max_native_zoom,
            attribution: Cow::Owned(short(attribution)),
            no_data_etag: None,
            key: (!key.is_empty()).then(|| key.to_string()),
        })
    }

    /// Does the source carry an API key?
    pub fn has_key(&self) -> bool {
        self.key.is_some()
    }

    /// The URL of a tile (with the API key filled in: never log it, see [`TileSource::redact`]).
    pub fn url(&self, t: TileId) -> String {
        let url = self.url_template.replace("{z}", &t.z.to_string()).replace("{x}", &t.x.to_string()).replace("{y}", &t.y.to_string());
        match &self.key {
            Some(k) => url.replace("{key}", &url_encode(k)),
            None => url,
        }
    }

    /// `text` (an error, which may quote a URL) with the API key taken out, for logs and messages.
    pub fn redact(&self, text: &str) -> String {
        match &self.key {
            Some(k) => text.replace(&url_encode(k), "<key>").replace(k.as_str(), "<key>"),
            None => text.to_string(),
        }
    }

    /// The whole zoom whose tiles a view at `zoom` draws.
    pub fn tile_zoom(&self, zoom: f64) -> u8 {
        let z = if zoom.is_finite() { zoom.round().clamp(0.0, 255.0) as u8 } else { self.min_zoom };
        z.clamp(self.min_zoom, self.max_zoom)
    }

    /// The tile to fetch for `t` and the part of it to draw, as a `(u0, v0, size)` square in
    /// texture coordinates: `t` itself, or past [`TileSource::max_native_zoom`] its ancestor there.
    pub fn native(&self, t: TileId) -> (TileId, (f64, f64, f64)) {
        if t.z <= self.max_native_zoom { (t, (0.0, 0.0, 1.0)) } else { t.ancestor(t.z - self.max_native_zoom) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_put_row_and_column_where_each_server_wants_them() {
        let t = TileId { z: 15, x: 9372, y: 12534 };
        assert_eq!(OSM.url(t), "https://tile.openstreetmap.org/15/9372/12534.png");
        assert_eq!(USGS_IMAGERY.url(t), "https://basemap.nationalmap.gov/arcgis/rest/services/USGSImageryOnly/MapServer/tile/15/12534/9372");
        assert_eq!(ESRI_IMAGERY.url(t), "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/15/12534/9372");
    }

    #[test]
    fn overzoom_uses_the_deepest_native_tile() {
        let t = TileId { z: 18, x: 4 * 1000 + 3, y: 4 * 2000 + 1 };
        let (n, (u, v, s)) = USGS_IMAGERY.native(t);
        assert_eq!(n, TileId { z: 16, x: 1000, y: 2000 });
        assert_eq!((u, v, s), (0.75, 0.25, 0.25));
        assert_eq!(OSM.native(t), (t, (0.0, 0.0, 1.0)));
        // Esri: native to 19, scaled up past it
        let deep = TileId { z: 21, x: 4 * 7 + 1, y: 4 * 9 + 2 };
        assert_eq!(ESRI_IMAGERY.native(deep).0, TileId { z: 19, x: 7, y: 9 });
    }

    #[test]
    fn tile_zoom_stays_in_range() {
        assert_eq!(OSM.tile_zoom(30.0), 19);
        assert_eq!(OSM.tile_zoom(-3.0), 0);
        assert_eq!(OSM.tile_zoom(f64::NAN), 0);
        assert_eq!(OSM.tile_zoom(12.6), 13);
        assert_eq!(by_id("usgs-imagery"), Some(&USGS_IMAGERY));
        assert_eq!(by_id("esri-imagery"), Some(&ESRI_IMAGERY));
        assert_eq!(by_id("nope"), None);
    }

    const SECRET: &str = "AAPK-s3cr3t+key/=";

    fn spec(template: &str) -> CustomSpec<'_> {
        CustomSpec { name: "Esri Location Platform", url_template: template, api_key: SECRET, max_native_zoom: 21, attribution: "Esri, Vantor" }
    }

    #[test]
    fn a_custom_source_fills_in_the_key_and_never_shows_it() {
        let s =
            TileSource::custom(spec("https://ibasemaps-api.arcgis.com/arcgis/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}?token={key}"))
                .unwrap();
        let t = TileId { z: 20, x: 167_544, y: 405_130 };
        assert_eq!(
            s.url(t),
            "https://ibasemaps-api.arcgis.com/arcgis/rest/services/World_Imagery/MapServer/tile/20/405130/167544?token=AAPK-s3cr3t%2Bkey%2F%3D"
        );
        assert!(s.has_key());
        for shown in [format!("{s:?}"), format!("{s:#?}"), format!("{s}"), s.url_template.to_string(), s.id.to_string()] {
            assert!(!shown.contains("s3cr3t"), "{shown}");
        }
        assert!(format!("{s:?}").contains("<hidden>"));
        assert_eq!(s.to_string(), "Esri Location Platform");
        // an error quoting the URL comes out without the key, raw or encoded
        let e = format!("HTTP 498 from {} ({SECRET})", s.url(t));
        let r = s.redact(&e);
        assert!(!r.contains("s3cr3t") && r.contains("token=<key>"), "{r}");
        // deeper than its native zoom it is scaled up; the view may go to 22
        assert_eq!((s.max_native_zoom, s.max_zoom), (21, 21));
        assert_eq!(s.native(TileId { z: 22, x: 2, y: 2 }).0, TileId { z: 21, x: 1, y: 1 });
    }

    #[test]
    fn a_custom_source_is_cached_under_a_hash_of_its_template() {
        let a = TileSource::custom(spec("https://tiles.example.com/{z}/{x}/{y}.jpg?k={key}")).unwrap();
        let other_key = TileSource::custom(CustomSpec { api_key: "different", ..spec("https://tiles.example.com/{z}/{x}/{y}.jpg?k={key}") }).unwrap();
        let other_template = TileSource::custom(spec("https://tiles.example.com/v2/{z}/{x}/{y}.jpg?k={key}")).unwrap();
        // stable across runs (a fixed hash, not the process's random one) and safe as a folder name
        assert_eq!(a.id, format!("custom-{:016x}", fnv1a("https://tiles.example.com/{z}/{x}/{y}.jpg?k={key}")));
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
        assert!(a.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        assert_eq!(a.id, other_key.id, "the key is not part of the folder name");
        assert_ne!(a.id, other_template.id, "a new template gets new tiles");
        assert!(!a.id.contains("s3cr3t"));
        assert!(a.id.starts_with("custom-") && !ALL.iter().any(|b| b.id == a.id));
    }

    #[test]
    fn templates_are_checked() {
        for ok in [
            "https://tile.example.com/{z}/{x}/{y}.png",
            "HTTPS://example.com/tiles/{z}/{y}/{x}?token={key}",
            "https://api.mapbox.com/v4/mapbox.satellite/{z}/{x}/{y}@2x.jpg90?access_token={key}",
        ] {
            assert_eq!(validate_template(ok), Ok(()), "{ok}");
        }
        for (bad, why) in [
            ("", "Enter"),
            ("http://tile.example.com/{z}/{x}/{y}.png", "https://"),
            ("ftp://tile.example.com/{z}/{x}/{y}.png", "https://"),
            ("file:///C:/tiles/{z}/{x}/{y}.png", "https://"),
            ("https://tile.example.com/{z}/{x}.png", "{y}"),
            ("https://tile.example.com/{x}/{y}.png", "{z}"),
            ("https://tile.example.com/{z}/{z}/{x}/{y}.png", "more than once"),
            ("https://tile.example.com/{z}/{x}/{y}.png?s={s}", "{s}"),
            ("https://tile.example.com/{z}/{x}/{y}.png?r={-y}", "{-y}"),
            ("https://tile.example.com/{z}/{x}/{y}.png?{", "without a matching"),
            ("https://tile.example.com/{z}/{x}/{y}.png}", "without a matching"),
            ("https://{s}.tile.example.com/{z}/{x}/{y}.png", "server name"),
            ("https:///{z}/{x}/{y}.png", "server name"),
            ("https://tile.example.com/{z}/{x}/{y} .png", "spaces"),
            ("https://tile.example.com/{z}/{x}/{y}.png\n", ""),
        ] {
            let r = validate_template(bad);
            if bad.ends_with('\n') {
                assert_eq!(r, Ok(()), "surrounding whitespace is trimmed");
                continue;
            }
            let e = r.expect_err(bad);
            assert!(e.contains(why), "{bad}: {e}");
        }
        let long = format!("https://example.com/{}/{{z}}/{{x}}/{{y}}", "a".repeat(5000));
        assert!(validate_template(&long).unwrap_err().contains("too long"));
    }

    #[test]
    fn custom_sources_need_a_key_for_key_templates_and_an_attribution() {
        let t = "https://tiles.example.com/{z}/{x}/{y}.jpg?k={key}";
        assert!(TileSource::custom(CustomSpec { api_key: " ", ..spec(t) }).unwrap_err().contains("API key"));
        assert!(TileSource::custom(CustomSpec { api_key: "a b", ..spec(t) }).unwrap_err().contains("spaces"));
        assert!(TileSource::custom(CustomSpec { attribution: "  ", ..spec(t) }).unwrap_err().contains("attribution"));
        // no {key}: no key needed; a blank name reads "Custom"; zoom capped at 22
        let s =
            TileSource::custom(CustomSpec { name: "", api_key: "", max_native_zoom: 30, ..spec("https://t.example.org/{z}/{x}/{y}.png") }).unwrap();
        assert!(!s.has_key());
        assert_eq!((s.name.as_ref(), s.max_native_zoom, s.max_zoom), ("Custom", 22, 22));
        assert_eq!(s.url(TileId { z: 3, x: 1, y: 2 }), "https://t.example.org/3/1/2.png");
        let shallow = TileSource::custom(CustomSpec { max_native_zoom: 15, ..spec(t) }).unwrap();
        assert_eq!(shallow.max_zoom, 19, "the view still zooms in, scaling up its deepest tiles");
    }
}
