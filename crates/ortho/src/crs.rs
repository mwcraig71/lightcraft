//! The coordinate reference systems an orthomosaic may be in, by EPSG code: WGS 84 latitude /
//! longitude, Web Mercator, and the WGS 84 and NAD83 UTM zones (what DJI Terra, Pix4D, Agisoft
//! and WebODM / ODM write by default).
//!
//! NAD83 is treated as WGS 84 (the two differ by one to two metres in North America); a
//! NAD83 orthomosaic over a WGS 84 base map can be off by that much.

use serde::{Deserialize, Serialize};

use crate::tm::{self, TransverseMercator};

/// A supported CRS.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Crs {
    /// EPSG:4326 (also NAD83 geographic, 4269): x is longitude, y latitude, in degrees.
    Geographic { epsg: u16 },
    /// EPSG:3857 Web Mercator, metres.
    WebMercator,
    /// A UTM zone, metres.
    Utm { epsg: u16, zone: u8, north: bool, nad83: bool },
}

/// Earth radius Web Mercator uses (the WGS 84 semi-major axis).
const R: f64 = 6_378_137.0;

impl Crs {
    /// The CRS for an EPSG code, or an error naming the code and what to export instead.
    pub fn from_epsg(code: u16) -> Result<Crs, String> {
        Ok(match code {
            4326 | 4269 => Crs::Geographic { epsg: code },
            3857 | 3785 => Crs::WebMercator,
            32601..=32660 => Crs::Utm { epsg: code, zone: (code - 32600) as u8, north: true, nad83: false },
            32701..=32760 => Crs::Utm { epsg: code, zone: (code - 32700) as u8, north: false, nad83: false },
            26901..=26923 => Crs::Utm { epsg: code, zone: (code - 26900) as u8, north: true, nad83: true },
            _ => return Err(unsupported(&format!("EPSG:{code}"))),
        })
    }

    pub fn epsg(&self) -> u16 {
        match *self {
            Crs::Geographic { epsg } | Crs::Utm { epsg, .. } => epsg,
            Crs::WebMercator => 3857,
        }
    }

    /// "WGS 84 / UTM zone 10N (EPSG:32610)".
    pub fn name(&self) -> String {
        match *self {
            Crs::Geographic { epsg: 4269 } => "NAD83 (EPSG:4269)".into(),
            Crs::Geographic { epsg } => format!("WGS 84 (EPSG:{epsg})"),
            Crs::WebMercator => "WGS 84 / Pseudo-Mercator (EPSG:3857)".into(),
            Crs::Utm { epsg, zone, north, nad83 } => {
                format!("{} / UTM zone {zone}{} (EPSG:{epsg})", if nad83 { "NAD83" } else { "WGS 84" }, if north { "N" } else { "S" })
            }
        }
    }

    fn tm(&self) -> Option<TransverseMercator> {
        match *self {
            Crs::Utm { zone, north, nad83, .. } => Some(tm::utm(zone, north, if nad83 { tm::GRS80 } else { tm::WGS84 })),
            _ => None,
        }
    }

    /// Latitude / longitude (degrees) of a point in this CRS.
    pub fn to_lat_lon(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        match self {
            Crs::Geographic { .. } => (y.abs() <= 90.0).then_some((y, x)),
            Crs::WebMercator => {
                let lat = (y / R).sinh().atan().to_degrees();
                let lon = (x / R).to_degrees();
                (lat.is_finite() && lon.is_finite()).then_some((lat, lon))
            }
            Crs::Utm { .. } => self.tm()?.inverse(x, y),
        }
    }

    /// A latitude / longitude (degrees) in this CRS.
    pub fn from_lat_lon(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        if !(lat.is_finite() && lon.is_finite()) {
            return None;
        }
        match self {
            Crs::Geographic { .. } => Some((lon, lat)),
            Crs::WebMercator => {
                let lat = lat.clamp(-89.9, 89.9).to_radians();
                Some((R * lon.to_radians(), R * (std::f64::consts::FRAC_PI_4 + lat / 2.0).tan().ln()))
            }
            Crs::Utm { .. } => self.tm()?.forward(lat, lon),
        }
    }
}

/// The error for a CRS LightCraft can't place.
pub fn unsupported(what: &str) -> String {
    format!(
        "the orthomosaic's coordinate system ({what}) isn't supported: export it in WGS 84 (EPSG:4326), Web Mercator (EPSG:3857) or a WGS 84 / NAD83 UTM zone (EPSG:326xx, 327xx, 269xx)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_map_to_zones() {
        assert_eq!(Crs::from_epsg(32610), Ok(Crs::Utm { epsg: 32610, zone: 10, north: true, nad83: false }));
        assert_eq!(Crs::from_epsg(32756), Ok(Crs::Utm { epsg: 32756, zone: 56, north: false, nad83: false }));
        assert_eq!(Crs::from_epsg(26910), Ok(Crs::Utm { epsg: 26910, zone: 10, north: true, nad83: true }));
        assert_eq!(Crs::from_epsg(4326), Ok(Crs::Geographic { epsg: 4326 }));
        assert_eq!(Crs::from_epsg(3857), Ok(Crs::WebMercator));
        let e = Crs::from_epsg(2227).unwrap_err();
        assert!(e.contains("EPSG:2227") && e.contains("UTM"), "{e}");
        assert!(Crs::from_epsg(32661).is_err() && Crs::from_epsg(26924).is_err());
        assert_eq!(Crs::from_epsg(32610).unwrap().name(), "WGS 84 / UTM zone 10N (EPSG:32610)");
    }

    #[test]
    fn web_mercator_round_trips_and_matches_the_tile_maths() {
        let c = Crs::WebMercator;
        let (x, y) = c.from_lat_lon(37.8199, -122.4783).unwrap();
        let (lat, lon) = c.to_lat_lon(x, y).unwrap();
        assert!((lat - 37.8199).abs() < 1e-10 && (lon + 122.4783).abs() < 1e-10);
        // the same place as the slippy-map world coordinates
        let (wx, wy) = lightcraft_tiles::mercator::to_world(37.8199, -122.4783);
        let half = std::f64::consts::PI * R;
        assert!(((x + half) / (2.0 * half) - wx).abs() < 1e-12);
        assert!(((half - y) / (2.0 * half) - wy).abs() < 1e-12);
    }

    #[test]
    fn utm_goes_both_ways() {
        let c = Crs::from_epsg(32610).unwrap();
        let (e, n) = c.from_lat_lon(37.8199, -122.4783).unwrap();
        let (lat, lon) = c.to_lat_lon(e, n).unwrap();
        assert!((lat - 37.8199).abs() < 1e-9 && (lon + 122.4783).abs() < 1e-9);
        assert_eq!(c.to_lat_lon(f64::NAN, 1.0), None);
        assert_eq!(Crs::Geographic { epsg: 4326 }.to_lat_lon(10.0, 95.0), None);
    }
}
