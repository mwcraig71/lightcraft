//! Web Mercator (EPSG:3857) as slippy maps use it. A point is kept as *world* coordinates in
//! `0..1` on both axes (x east from the antimeridian, y south from the top edge at
//! [`MAX_LAT`]); at zoom `z` the world is `256 · 2^z` pixels wide, and tile `(x, y)` covers
//! world pixels `256·x .. 256·(x+1)`. Zoom may be fractional (the view scales the nearest whole
//! zoom's tiles).

/// Tile edge in pixels.
pub const TILE: f64 = 256.0;
/// Latitude of the square map's top and bottom edge.
pub const MAX_LAT: f64 = 85.051_128_779_806_59;
/// Deepest zoom any view uses.
pub const MAX_ZOOM: u8 = 22;
/// Equatorial circumference / 256 at zoom 0: metres per pixel at the equator.
const M_PER_PX_Z0: f64 = 156_543.033_928_041;

/// One tile: zoom, column, row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    /// The tile `levels` zooms up that contains this one, and where this one sits in it as a
    /// `(u0, v0, size)` square in `0..1` texture coordinates.
    pub fn ancestor(self, levels: u8) -> (TileId, (f64, f64, f64)) {
        let levels = levels.min(self.z).min(31);
        let parent = TileId { z: self.z - levels, x: self.x >> levels, y: self.y >> levels };
        let n = f64::from(1u32 << levels);
        let mask = (1u32 << levels) - 1;
        let size = 1.0 / n;
        (parent, (f64::from(self.x & mask) * size, f64::from(self.y & mask) * size, size))
    }
}

/// Tiles across one side of the world at zoom `z` (2^z).
pub fn tiles_across(z: u8) -> u32 {
    1u32 << z.min(31)
}

/// World pixels per world unit at (possibly fractional) `zoom`.
pub fn scale(zoom: f64) -> f64 {
    TILE * zoom.clamp(0.0, f64::from(MAX_ZOOM)).exp2()
}

/// World coordinates of a latitude/longitude in degrees; latitude is clamped to ±[`MAX_LAT`],
/// longitude wrapped into −180..180. Non-finite input gives the world's centre.
pub fn to_world(lat: f64, lon: f64) -> (f64, f64) {
    if !lat.is_finite() || !lon.is_finite() {
        return (0.5, 0.5);
    }
    let lat = lat.clamp(-MAX_LAT, MAX_LAT).to_radians();
    let lon = (lon + 180.0).rem_euclid(360.0) - 180.0;
    let x = (lon + 180.0) / 360.0;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0;
    (x, y.clamp(0.0, 1.0))
}

/// Latitude/longitude in degrees of world coordinates (x wraps; y is clamped to the map).
pub fn from_world(x: f64, y: f64) -> (f64, f64) {
    let x = if x.is_finite() { x.rem_euclid(1.0) } else { 0.5 };
    let y = if y.is_finite() { y.clamp(0.0, 1.0) } else { 0.5 };
    let lon = x * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
    (lat, lon)
}

/// The tile at zoom `z` that holds world point `(x, y)`.
pub fn tile_at(x: f64, y: f64, z: u8) -> TileId {
    let n = tiles_across(z);
    let cell = |v: f64| {
        let v = if v.is_finite() { v } else { 0.5 };
        // as-casts saturate; the clamp keeps the last row/column for v == 1.0
        ((v * f64::from(n)).floor() as i64).clamp(0, i64::from(n) - 1) as u32
    };
    TileId { z, x: cell(x.rem_euclid(1.0)), y: cell(y) }
}

/// A tile placed on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// The tile, its column wrapped into the world (so the antimeridian repeats the map).
    pub tile: TileId,
    /// Top-left corner on screen, relative to the viewport's top-left.
    pub left: f64,
    pub top: f64,
    /// Edge length on screen.
    pub size: f64,
}

/// At most this many tiles are placed for one view (a huge viewport at a coarse zoom).
pub const MAX_PLACED: usize = 1024;

/// The tiles of zoom `z` that cover a `width × height` viewport centred on world point `center`
/// at view zoom `zoom`, nearest the centre first (the order to fetch them in). Columns wrap
/// across the antimeridian; rows above or below the map are left out.
pub fn visible_tiles(center: (f64, f64), zoom: f64, width: f64, height: f64, z: u8) -> Vec<Placed> {
    let (w, h) = (width.max(0.0), height.max(0.0));
    if !(w.is_finite() && h.is_finite() && zoom.is_finite()) || w == 0.0 || h == 0.0 {
        return Vec::new();
    }
    let s = scale(zoom);
    let size = s / f64::from(tiles_across(z));
    if !(size.is_finite() && size > 0.5) {
        return Vec::new();
    }
    // world pixel at the viewport's top-left
    let (ox, oy) = (center.0 * s - w / 2.0, center.1 * s - h / 2.0);
    let n = i64::from(tiles_across(z));
    let x0 = (ox / size).floor() as i64;
    let x1 = ((ox + w) / size).ceil() as i64;
    let y0 = ((oy / size).floor() as i64).max(0);
    let y1 = (((oy + h) / size).ceil() as i64).min(n);
    let mut out = Vec::new();
    for ty in y0..y1 {
        for tx in x0..x1 {
            out.push(Placed {
                tile: TileId { z, x: tx.rem_euclid(n) as u32, y: ty as u32 },
                left: tx as f64 * size - ox,
                top: ty as f64 * size - oy,
                size,
            });
            if out.len() >= MAX_PLACED * 4 {
                break;
            }
        }
    }
    let (cx, cy) = (w / 2.0, h / 2.0);
    let dist = |p: &Placed| {
        let (dx, dy) = (p.left + p.size / 2.0 - cx, p.top + p.size / 2.0 - cy);
        dx * dx + dy * dy
    };
    out.sort_by(|a, b| dist(a).total_cmp(&dist(b)));
    out.truncate(MAX_PLACED);
    out
}

/// Ground metres per screen pixel at `lat` and view `zoom`.
pub fn metres_per_pixel(lat: f64, zoom: f64) -> f64 {
    let lat = if lat.is_finite() { lat.clamp(-MAX_LAT, MAX_LAT) } else { 0.0 };
    M_PER_PX_Z0 * lat.to_radians().cos() / zoom.clamp(0.0, f64::from(MAX_ZOOM)).exp2()
}

/// Centre and zoom that fit world `points` in a `width × height` viewport with `margin` pixels
/// on every side, no deeper than `max_zoom`. `None` without points.
pub fn fit(points: &[(f64, f64)], width: f64, height: f64, margin: f64, max_zoom: f64) -> Option<((f64, f64), f64)> {
    let mut it = points.iter().filter(|p| p.0.is_finite() && p.1.is_finite());
    let first = it.next()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
    for p in it {
        x0 = x0.min(p.0);
        y0 = y0.min(p.1);
        x1 = x1.max(p.0);
        y1 = y1.max(p.1);
    }
    let max_zoom = max_zoom.clamp(0.0, f64::from(MAX_ZOOM));
    let w = (width - 2.0 * margin).max(16.0);
    let h = (height - 2.0 * margin).max(16.0);
    let (dx, dy) = (x1 - x0, y1 - y0);
    // world units per pixel needed on each axis, as a zoom
    let zoom_for = |span: f64, px: f64| if span > 0.0 { (px / (span * TILE)).log2() } else { max_zoom };
    let zoom = zoom_for(dx, w).min(zoom_for(dy, h)).clamp(0.0, max_zoom);
    Some((((x0 + x1) / 2.0, (y0 + y1) / 2.0), zoom))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn lat_lon_round_trips(lat in -85.0f64..85.0, lon in -179.999f64..179.999) {
            let (x, y) = to_world(lat, lon);
            prop_assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
            let (lat2, lon2) = from_world(x, y);
            prop_assert!((lat - lat2).abs() < 1e-9, "{lat} -> {lat2}");
            prop_assert!((lon - lon2).abs() < 1e-9, "{lon} -> {lon2}");
        }

        #[test]
        fn round_trips_through_pixels(lat in -85.0f64..85.0, lon in -179.0f64..179.0, zoom in 0.0f64..19.0) {
            let (x, y) = to_world(lat, lon);
            let s = scale(zoom);
            let (px, py) = (x * s, y * s);
            let (lat2, lon2) = from_world(px / s, py / s);
            prop_assert!((lat - lat2).abs() < 1e-8 && (lon - lon2).abs() < 1e-8);
        }
    }

    #[test]
    fn known_tile() {
        // Zoom 10 tile of the Golden Gate Bridge (37.8199 N, 122.4783 W) is 163 / 395.
        let (x, y) = to_world(37.8199, -122.4783);
        assert_eq!(tile_at(x, y, 10), TileId { z: 10, x: 163, y: 395 });
    }

    #[test]
    fn poles_clamp_and_hostile_input_is_harmless() {
        assert!(to_world(90.0, 0.0).1.abs() < 1e-12);
        assert!((to_world(-90.0, 0.0).1 - 1.0).abs() < 1e-12);
        assert_eq!(to_world(f64::NAN, 1.0), (0.5, 0.5));
        assert_eq!(tile_at(0.5, 1.0, 3), TileId { z: 3, x: 4, y: 7 });
        assert_eq!(tile_at(f64::INFINITY, -5.0, 3).y, 0);
        assert!(visible_tiles((0.5, 0.5), f64::NAN, 100.0, 100.0, 2).is_empty());
        assert!(metres_per_pixel(f64::NAN, 3.0).is_finite());
    }

    #[test]
    fn tile_range_wraps_at_the_antimeridian() {
        // centre on the antimeridian at zoom 2: columns 3 and 0 sit side by side
        let placed = visible_tiles((0.0, 0.5), 2.0, 512.0, 256.0, 2);
        let cols: Vec<u32> = placed.iter().map(|p| p.tile.x).collect();
        assert!(cols.contains(&3) && cols.contains(&0), "{cols:?}");
        let left_of_centre = placed.iter().find(|p| p.tile.x == 3).unwrap();
        assert!(left_of_centre.left < 256.0);
        assert!(placed.iter().all(|p| p.tile.x < 4));
    }

    #[test]
    fn tile_range_stops_at_the_poles() {
        // a tall viewport at zoom 1 sees above and below the map: only rows 0 and 1 exist
        let placed = visible_tiles((0.5, 0.5), 1.0, 256.0, 4000.0, 1);
        assert!(placed.iter().all(|p| p.tile.y < 2));
        assert_eq!(placed.len(), 4);
        // nearest the centre first
        let first = placed[0];
        assert!(first.top.abs() < 2000.0);
    }

    #[test]
    fn huge_viewports_are_bounded() {
        assert!(visible_tiles((0.5, 0.5), 18.0, 1e9, 1e9, 18).len() <= MAX_PLACED);
    }

    #[test]
    fn ancestor_picks_the_parent_and_quadrant() {
        let t = TileId { z: 19, x: 0b1011, y: 0b0110 };
        let (p, (u, v, s)) = t.ancestor(3);
        assert_eq!(p, TileId { z: 16, x: 1, y: 0 });
        assert_eq!((u, v, s), (3.0 / 8.0, 6.0 / 8.0, 1.0 / 8.0));
        assert_eq!(TileId { z: 1, x: 1, y: 1 }.ancestor(5).0, TileId { z: 0, x: 0, y: 0 });
    }

    #[test]
    fn fit_contains_every_point() {
        let pts = [to_world(40.0, -75.0), to_world(40.01, -74.98), to_world(39.995, -75.01)];
        let (c, z) = fit(&pts, 800.0, 600.0, 40.0, 19.0).unwrap();
        let s = scale(z);
        for p in pts {
            assert!(((p.0 - c.0) * s).abs() <= 400.0 - 39.0);
            assert!(((p.1 - c.1) * s).abs() <= 300.0 - 39.0);
        }
        // one point: the deepest zoom allowed
        assert_eq!(fit(&pts[..1], 800.0, 600.0, 40.0, 17.0).unwrap().1, 17.0);
        assert!(fit(&[], 1.0, 1.0, 0.0, 1.0).is_none());
    }

    #[test]
    fn scale_bar_resolution() {
        // equator, zoom 0: ~156 km per pixel
        assert!((metres_per_pixel(0.0, 0.0) - 156_543.03).abs() < 0.1);
        assert!((metres_per_pixel(60.0, 1.0) - 156_543.03 / 4.0).abs() < 0.1);
    }
}
