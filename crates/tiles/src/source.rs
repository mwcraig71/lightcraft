//! The tile servers the Map view can show. Each is a URL template with `{z}`, `{x}` and `{y}`
//! (the template fixes the order: USGS puts the row before the column), the zooms it serves, and
//! the attribution that must stay visible on the map.

use crate::mercator::TileId;

/// One tile server.
#[derive(Debug, PartialEq, Eq)]
pub struct TileSource {
    /// Stable id (settings, cache folder, commands).
    pub id: &'static str,
    /// Name shown in menus (English; the UI translates it).
    pub name: &'static str,
    pub url_template: &'static str,
    pub min_zoom: u8,
    /// Deepest zoom the view may use with this source.
    pub max_zoom: u8,
    /// Deepest zoom the server has tiles for; deeper views scale these up (overzoom).
    pub max_native_zoom: u8,
    /// Shown bottom-right on the map whenever the source's tiles are drawn.
    pub attribution: &'static str,
}

/// OpenStreetMap's standard tiles. Their usage policy: a User-Agent naming the app, honour the
/// cache headers, no bulk download or offline areas, attribution always visible.
pub static OSM: TileSource = TileSource {
    id: "osm",
    name: "Street (OpenStreetMap)",
    url_template: "https://tile.openstreetmap.org/{z}/{x}/{y}.png",
    min_zoom: 0,
    max_zoom: 19,
    max_native_zoom: 19,
    attribution: "© OpenStreetMap contributors",
};

/// USGS The National Map orthoimagery (US and Puerto Rico; public domain). Tiles to zoom 16,
/// scaled up beyond so pins at neighbouring piers separate.
pub static USGS_IMAGERY: TileSource = TileSource {
    id: "usgs-imagery",
    name: "Imagery (USGS, United States)",
    url_template: "https://basemap.nationalmap.gov/arcgis/rest/services/USGSImageryOnly/MapServer/tile/{z}/{y}/{x}",
    min_zoom: 0,
    max_zoom: 19,
    max_native_zoom: 16,
    attribution: "USDA, USGS The National Map: Orthoimagery",
};

/// Every source, in menu order.
pub static ALL: [&TileSource; 2] = [&OSM, &USGS_IMAGERY];

/// The source with this id.
pub fn by_id(id: &str) -> Option<&'static TileSource> {
    ALL.iter().copied().find(|s| s.id == id)
}

impl TileSource {
    /// The URL of a tile.
    pub fn url(&self, t: TileId) -> String {
        self.url_template.replace("{z}", &t.z.to_string()).replace("{x}", &t.x.to_string()).replace("{y}", &t.y.to_string())
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
    }

    #[test]
    fn overzoom_uses_the_deepest_native_tile() {
        let t = TileId { z: 18, x: 4 * 1000 + 3, y: 4 * 2000 + 1 };
        let (n, (u, v, s)) = USGS_IMAGERY.native(t);
        assert_eq!(n, TileId { z: 16, x: 1000, y: 2000 });
        assert_eq!((u, v, s), (0.75, 0.25, 0.25));
        assert_eq!(OSM.native(t), (t, (0.0, 0.0, 1.0)));
    }

    #[test]
    fn tile_zoom_stays_in_range() {
        assert_eq!(OSM.tile_zoom(30.0), 19);
        assert_eq!(OSM.tile_zoom(-3.0), 0);
        assert_eq!(OSM.tile_zoom(f64::NAN), 0);
        assert_eq!(OSM.tile_zoom(12.6), 13);
        assert_eq!(by_id("usgs-imagery"), Some(&USGS_IMAGERY));
        assert_eq!(by_id("nope"), None);
    }
}
