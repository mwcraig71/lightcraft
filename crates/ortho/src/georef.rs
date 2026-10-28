//! GeoTIFF georeferencing: where each pixel is on the ground.
//!
//! - `ModelPixelScaleTag` (33550) with one `ModelTiepointTag` (33922), or `ModelTransformationTag`
//!   (34264), an affine 4×4 matrix (rotation and skew included), map pixel (column, row) to model
//!   coordinates.
//! - `GeoKeyDirectoryTag` (34735) says what the model coordinates are: `GTModelTypeGeoKey` (1024),
//!   `GTRasterTypeGeoKey` (1025, pixel-is-area or pixel-is-point), `GeographicTypeGeoKey` (2048),
//!   `ProjectedCSTypeGeoKey` (3072), and for a user-defined projected CRS `ProjectionGeoKey` (3074)
//!   with a UTM code.
//!
//! Source: the GeoTIFF format specification 1.0 (Ritter & Ruth, 2000) and OGC GeoTIFF 1.1
//! (19-008r4), prose; own implementation.

use serde::{Deserialize, Serialize};

use crate::crs::{self, Crs};

pub const MODEL_PIXEL_SCALE: u16 = 33550;
pub const MODEL_TIEPOINT: u16 = 33922;
pub const MODEL_TRANSFORMATION: u16 = 34264;
pub const GEO_KEY_DIRECTORY: u16 = 34735;
pub const GEO_DOUBLE_PARAMS: u16 = 34736;
pub const GEO_ASCII_PARAMS: u16 = 34737;
/// GDAL's no-data value, as text.
pub const GDAL_NODATA: u16 = 42113;

const GT_MODEL_TYPE: u16 = 1024;
const GT_RASTER_TYPE: u16 = 1025;
const GEOGRAPHIC_TYPE: u16 = 2048;
const PROJECTED_CS_TYPE: u16 = 3072;
const PROJECTION: u16 = 3074;
const USER_DEFINED: u16 = 32767;
/// Keys read at most (a real file has a dozen or two).
const MAX_KEYS: usize = 512;

/// The GeoKeys that matter here.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeoKeys {
    pub model_type: Option<u16>,
    pub raster_type: Option<u16>,
    pub geographic: Option<u16>,
    pub projected: Option<u16>,
    pub projection: Option<u16>,
}

impl GeoKeys {
    /// Read the key directory (`GeoKeyDirectoryTag`'s SHORTs). Keys stored in the double or ASCII
    /// parameter tags aren't codes and are skipped.
    pub fn parse(dir: &[u16]) -> Result<GeoKeys, String> {
        let [version, _, _, count, ..] = *dir else { return Err("the GeoTIFF key directory is too short".into()) };
        if version != 1 {
            return Err(format!("unknown GeoTIFF key directory version {version}"));
        }
        let mut keys = GeoKeys::default();
        for i in 0..usize::from(count).min(MAX_KEYS) {
            let Some(&[id, location, _count, value]) = dir.get(4 + i * 4..8 + i * 4).and_then(|s| <&[u16; 4]>::try_from(s).ok()) else {
                break;
            };
            // location 0: the value is the code itself
            if location != 0 {
                continue;
            }
            match id {
                GT_MODEL_TYPE => keys.model_type = Some(value),
                GT_RASTER_TYPE => keys.raster_type = Some(value),
                GEOGRAPHIC_TYPE => keys.geographic = Some(value),
                PROJECTED_CS_TYPE => keys.projected = Some(value),
                PROJECTION => keys.projection = Some(value),
                _ => {}
            }
        }
        Ok(keys)
    }

    /// The CRS the keys name, or an error naming the code that isn't supported.
    pub fn crs(&self) -> Result<Crs, String> {
        match self.model_type {
            // projected
            Some(1) => match self.projected {
                Some(USER_DEFINED) | None => match self.projection {
                    // EPSG "UTM zone N" projection codes 16001–16060 (north), 16101–16160 (south)
                    Some(p @ 16001..=16060) => Crs::from_epsg(32600 + (p - 16000)),
                    Some(p @ 16101..=16160) => Crs::from_epsg(32700 + (p - 16100)),
                    Some(p) if p != USER_DEFINED => Err(crs::unsupported(&format!("projection {p}"))),
                    _ => Err(crs::unsupported("a user-defined projection")),
                },
                Some(code) => Crs::from_epsg(code),
            },
            // geographic
            Some(2) => match self.geographic {
                Some(USER_DEFINED) => Err(crs::unsupported("a user-defined geographic system")),
                Some(code) => Crs::from_epsg(code),
                None => Crs::from_epsg(4326),
            },
            Some(3) => Err(crs::unsupported("geocentric")),
            // no model type: what the codes say
            _ => match (self.projected, self.geographic) {
                (Some(code), _) if code != USER_DEFINED => Crs::from_epsg(code),
                (_, Some(code)) if code != USER_DEFINED => Crs::from_epsg(code),
                _ => Err("the GeoTIFF names no coordinate system (no ProjectedCSType or GeographicType key)".into()),
            },
        }
    }

    pub fn pixel_is_point(&self) -> bool {
        self.raster_type == Some(2)
    }
}

/// Pixel → model affine map: `x = a·col + b·row + c`, `y = d·col + e·row + f` (column and row
/// are pixel edges: (0, 0) is the top-left corner of the top-left pixel).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub fn apply(&self, col: f64, row: f64) -> (f64, f64) {
        (self.a * col + self.b * row + self.c, self.d * col + self.e * row + self.f)
    }

    /// The inverse map (model → pixel); `None` when the matrix is singular.
    pub fn inverse(&self) -> Option<Affine> {
        let det = self.a * self.e - self.b * self.d;
        if !(det.is_finite() && det.abs() > 1e-300) {
            return None;
        }
        let (a, b, d, e) = (self.e / det, -self.b / det, -self.d / det, self.a / det);
        let inv = Affine { a, b, c: -(a * self.c + b * self.f), d, e, f: -(d * self.c + e * self.f) };
        [inv.a, inv.b, inv.c, inv.d, inv.e, inv.f].iter().all(|v| v.is_finite()).then_some(inv)
    }

    /// Whether the pixel grid is rotated or skewed against the model axes.
    pub fn rotated(&self) -> bool {
        self.b != 0.0 || self.d != 0.0
    }
}

/// Where the raster is: its CRS and pixel → model map.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Georef {
    pub crs: Crs,
    pub to_model: Affine,
    pub to_pixel: Affine,
}

impl Georef {
    /// From the georeferencing tags' values (any may be missing).
    pub fn new(keys: &GeoKeys, pixel_scale: Option<&[f64]>, tiepoints: Option<&[f64]>, transform: Option<&[f64]>) -> Result<Georef, String> {
        let crs = keys.crs()?;
        let mut m = if let Some(t) = transform {
            let [a, b, _, c, d, e, _, f, ..] = *t else { return Err("the ModelTransformation tag has fewer than 16 values".into()) };
            Affine { a, b, c, d, e, f }
        } else {
            let Some(&[i, j, _, x, y, _]) = tiepoints.and_then(|t| t.get(..6)) else {
                return Err("the file has no georeferencing (no ModelTiepoint or ModelTransformation tag): it is a plain TIFF, not a GeoTIFF".into());
            };
            if tiepoints.is_some_and(|t| t.len() > 6) && pixel_scale.is_none() {
                return Err("the orthomosaic is placed by ground control points only (several tiepoints, no pixel scale), which isn't supported: export it with a pixel scale or an affine transform".into());
            }
            let Some(&[sx, sy, ..]) = pixel_scale.and_then(|s| s.get(..2)) else {
                return Err("the GeoTIFF has a tiepoint but no ModelPixelScale tag".into());
            };
            Affine { a: sx, b: 0.0, c: x - i * sx, d: 0.0, e: -sy, f: y + j * sy }
        };
        if ![m.a, m.b, m.c, m.d, m.e, m.f].iter().all(|v| v.is_finite()) {
            return Err("the georeferencing has non-finite numbers".into());
        }
        // pixel-is-point: the model coordinates are those of pixel centres
        if keys.pixel_is_point() {
            let (cx, cy) = (m.a * 0.5 + m.b * 0.5, m.d * 0.5 + m.e * 0.5);
            m.c -= cx;
            m.f -= cy;
        }
        let to_pixel = m.inverse().ok_or("the georeferencing is degenerate (zero pixel size)")?;
        Ok(Georef { crs, to_model: m, to_pixel })
    }

    /// Latitude / longitude of a pixel position (edges, as [`Affine`]).
    pub fn lat_lon(&self, col: f64, row: f64) -> Option<(f64, f64)> {
        let (x, y) = self.to_model.apply(col, row);
        self.crs.to_lat_lon(x, y)
    }

    /// Pixel position of a latitude / longitude.
    pub fn pixel(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        let (x, y) = self.crs.from_lat_lon(lat, lon)?;
        Some(self.to_pixel.apply(x, y))
    }
}

/// Great-circle distance in metres (spherical, mean radius; good to 0.5 % for pixel sizes).
pub fn distance_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, la2) = (a.0.to_radians(), b.0.to_radians());
    let dla = la2 - la1;
    let dlo = (b.1 - a.1).to_radians();
    let h = (dla / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * h.sqrt().min(1.0).asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(keys: &[[u16; 4]]) -> Vec<u16> {
        let mut v = vec![1, 1, 0, keys.len() as u16];
        for k in keys {
            v.extend_from_slice(k);
        }
        v
    }

    #[test]
    fn keys_name_the_supported_systems() {
        let projected = |code| GeoKeys::parse(&dir(&[[1024, 0, 1, 1], [1025, 0, 1, 1], [3072, 0, 1, code]])).unwrap().crs();
        assert_eq!(projected(32610), Ok(Crs::Utm { epsg: 32610, zone: 10, north: true, nad83: false }));
        assert_eq!(projected(32733), Ok(Crs::Utm { epsg: 32733, zone: 33, north: false, nad83: false }));
        assert_eq!(projected(26918), Ok(Crs::Utm { epsg: 26918, zone: 18, north: true, nad83: true }));
        assert_eq!(projected(3857), Ok(Crs::WebMercator));
        let geo = GeoKeys::parse(&dir(&[[1024, 0, 1, 2], [2048, 0, 1, 4326]])).unwrap();
        assert_eq!(geo.crs(), Ok(Crs::Geographic { epsg: 4326 }));
        // State Plane California III (US feet): a clear error naming the code
        let e = projected(2227).unwrap_err();
        assert!(e.contains("EPSG:2227") && e.contains("WGS 84"), "{e}");
        // user-defined CRS with a UTM projection code
        let user = GeoKeys::parse(&dir(&[[1024, 0, 1, 1], [3072, 0, 1, 32767], [3074, 0, 1, 16010]])).unwrap();
        assert_eq!(user.crs(), Ok(Crs::Utm { epsg: 32610, zone: 10, north: true, nad83: false }));
        let custom = GeoKeys::parse(&dir(&[[1024, 0, 1, 1], [3072, 0, 1, 32767]])).unwrap();
        assert!(custom.crs().unwrap_err().contains("user-defined"));
        // pixel is point
        assert!(GeoKeys::parse(&dir(&[[1025, 0, 1, 2]])).unwrap().pixel_is_point());
    }

    #[test]
    fn hostile_key_directories_are_errors_or_partial() {
        assert!(GeoKeys::parse(&[]).is_err());
        assert!(GeoKeys::parse(&[2, 1, 0, 0]).is_err());
        // claims 60 000 keys, has one
        let mut d = dir(&[[3072, 0, 1, 32610]]);
        d[3] = 60_000;
        assert_eq!(GeoKeys::parse(&d).unwrap().projected, Some(32610));
        // keys stored elsewhere are skipped
        assert_eq!(GeoKeys::parse(&dir(&[[3072, 34736, 1, 0]])).unwrap().projected, None);
    }

    #[test]
    fn scale_and_tiepoint_map_pixels_and_back() {
        let keys = GeoKeys::parse(&dir(&[[1024, 0, 1, 1], [3072, 0, 1, 32610]])).unwrap();
        let g = Georef::new(&keys, Some(&[0.03, 0.03, 0.0]), Some(&[0.0, 0.0, 0.0, 545_000.0, 4_186_000.0, 0.0]), None).unwrap();
        assert_eq!(g.to_model.apply(100.0, 200.0), (545_003.0, 4_185_994.0));
        let (c, r) = g.to_pixel.apply(545_003.0, 4_185_994.0);
        assert!((c - 100.0).abs() < 1e-6 && (r - 200.0).abs() < 1e-6);
        let ll = g.lat_lon(0.0, 0.0).unwrap();
        let back = g.pixel(ll.0, ll.1).unwrap();
        // a hundredth of a pixel (0.3 mm): the series forward and back
        assert!(back.0.abs() < 0.01 && back.1.abs() < 0.01, "{back:?}");
        // pixel-is-point moves the grid half a pixel up-left
        let point = GeoKeys { raster_type: Some(2), ..keys.clone() };
        let g = Georef::new(&point, Some(&[0.03, 0.03, 0.0]), Some(&[0.0, 0.0, 0.0, 545_000.0, 4_186_000.0, 0.0]), None).unwrap();
        let (x, y) = g.to_model.apply(0.5, 0.5);
        assert!((x - 545_000.0).abs() < 1e-9 && (y - 4_186_000.0).abs() < 1e-9);
    }

    #[test]
    fn rotated_transforms_work_and_degenerate_ones_are_errors() {
        let keys = GeoKeys::parse(&dir(&[[1024, 0, 1, 1], [3072, 0, 1, 32610]])).unwrap();
        let (s, c) = (30f64.to_radians().sin() * 0.05, 30f64.to_radians().cos() * 0.05);
        let m = [c, -s, 0.0, 545_000.0, -s, -c, 0.0, 4_186_000.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let g = Georef::new(&keys, None, None, Some(&m)).unwrap();
        assert!(g.to_model.rotated());
        let (x, y) = g.to_model.apply(10.0, 20.0);
        let (col, row) = g.to_pixel.apply(x, y);
        assert!((col - 10.0).abs() < 1e-6 && (row - 20.0).abs() < 1e-6);
        assert!(Georef::new(&keys, Some(&[0.0, 0.0, 0.0]), Some(&[0.0; 6]), None).is_err(), "zero pixel size");
        assert!(Georef::new(&keys, None, None, None).unwrap_err().contains("not a GeoTIFF"));
        assert!(Georef::new(&keys, None, None, Some(&[1.0; 5])).is_err());
        assert!(Georef::new(&keys, Some(&[f64::NAN, 1.0]), Some(&[0.0; 6]), None).is_err());
        let gcps = [0.0; 12];
        assert!(Georef::new(&keys, None, Some(&gcps), None).unwrap_err().contains("ground control points"));
    }

    #[test]
    fn distances() {
        // one degree of latitude is about 111.2 km
        assert!((distance_m((0.0, 0.0), (1.0, 0.0)) - 111_195.0).abs() < 10.0);
    }
}
