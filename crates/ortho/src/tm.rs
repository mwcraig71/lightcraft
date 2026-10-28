//! Transverse Mercator on an ellipsoid, forward and inverse, and UTM on top of it.
//!
//! Method: Krüger's series in the third flattening `n`, to `n³`, as given in the "Universal
//! Transverse Mercator coordinate system" article (Wikipedia, section "Simplified formulae"), which
//! follows L. Krüger, *Konforme Abbildung des Erdellipsoids in der Ebene* (1912) and C. F. F.
//! Karney, *Transverse Mercator with an accuracy of a few nanometers*, J. Geodesy 85 (2011).
//! Within a UTM zone (±3° from the central meridian) and well beyond, the `n³` series is good to
//! about a millimetre. Own implementation from the published formulas; the tests check it against
//! known meridian lengths, J. P. Snyder's worked example (USGS Professional Paper 1395, 1987) and
//! Snyder's independent series.

/// An ellipsoid: semi-major axis (metres) and flattening.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ellipsoid {
    pub a: f64,
    pub f: f64,
}

/// WGS 84.
pub const WGS84: Ellipsoid = Ellipsoid { a: 6_378_137.0, f: 1.0 / 298.257_223_563 };
/// GRS 1980 (NAD83).
pub const GRS80: Ellipsoid = Ellipsoid { a: 6_378_137.0, f: 1.0 / 298.257_222_101 };

/// A transverse Mercator projection: ellipsoid, central meridian, scale on it, false easting and northing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransverseMercator {
    pub ellipsoid: Ellipsoid,
    /// Central meridian, degrees.
    pub lon0: f64,
    pub k0: f64,
    pub false_easting: f64,
    pub false_northing: f64,
}

/// The series' coefficients for one ellipsoid.
struct Series {
    /// Rectifying radius times `k0`.
    ka: f64,
    alpha: [f64; 3],
    beta: [f64; 3],
    delta: [f64; 3],
    /// First eccentricity, `2√n / (1 + n)`.
    e: f64,
}

impl Series {
    fn new(el: Ellipsoid, k0: f64) -> Series {
        let n = el.f / (2.0 - el.f);
        let (n2, n3) = (n * n, n * n * n);
        let a = el.a / (1.0 + n) * (1.0 + n2 / 4.0 + n2 * n2 / 64.0);
        Series {
            ka: k0 * a,
            alpha: [n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0, 13.0 * n2 / 48.0 - 3.0 * n3 / 5.0, 61.0 * n3 / 240.0],
            beta: [n / 2.0 - 2.0 * n2 / 3.0 + 37.0 * n3 / 96.0, n2 / 48.0 + n3 / 15.0, 17.0 * n3 / 480.0],
            delta: [2.0 * n - 2.0 * n2 / 3.0 - 2.0 * n3, 7.0 * n2 / 3.0 - 8.0 * n3 / 5.0, 56.0 * n3 / 15.0],
            e: 2.0 * n.sqrt() / (1.0 + n),
        }
    }
}

impl TransverseMercator {
    /// Easting and northing (metres) of a latitude / longitude in degrees. `None` for non-finite
    /// input or a point 90° or more from the central meridian (where the projection runs off to infinity).
    pub fn forward(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        if !(lat.is_finite() && lon.is_finite()) || lat.abs() > 90.0 {
            return None;
        }
        let s = Series::new(self.ellipsoid, self.k0);
        let phi = lat.to_radians();
        let dl = ((lon - self.lon0 + 180.0).rem_euclid(360.0) - 180.0).to_radians();
        if dl.abs() >= std::f64::consts::FRAC_PI_2 - 1e-9 {
            return None;
        }
        let sp = phi.sin();
        let t = (sp.atanh() - s.e * (s.e * sp).atanh()).sinh();
        let xi1 = t.atan2(dl.cos());
        let eta1 = (dl.sin() / (1.0 + t * t).sqrt()).atanh();
        let (mut xi, mut eta) = (xi1, eta1);
        for (j, a) in s.alpha.iter().enumerate() {
            let k = 2.0 * (j as f64 + 1.0);
            xi += a * (k * xi1).sin() * (k * eta1).cosh();
            eta += a * (k * xi1).cos() * (k * eta1).sinh();
        }
        let (e, n) = (self.false_easting + s.ka * eta, self.false_northing + s.ka * xi);
        (e.is_finite() && n.is_finite()).then_some((e, n))
    }

    /// Latitude and longitude in degrees of an easting / northing. `None` for non-finite input.
    pub fn inverse(&self, easting: f64, northing: f64) -> Option<(f64, f64)> {
        if !(easting.is_finite() && northing.is_finite()) {
            return None;
        }
        let s = Series::new(self.ellipsoid, self.k0);
        let xi = (northing - self.false_northing) / s.ka;
        let eta = (easting - self.false_easting) / s.ka;
        let (mut xi1, mut eta1) = (xi, eta);
        for (j, b) in s.beta.iter().enumerate() {
            let k = 2.0 * (j as f64 + 1.0);
            xi1 -= b * (k * xi).sin() * (k * eta).cosh();
            eta1 -= b * (k * xi).cos() * (k * eta).sinh();
        }
        let chi = (xi1.sin() / eta1.cosh()).clamp(-1.0, 1.0).asin();
        let mut phi = chi;
        for (j, d) in s.delta.iter().enumerate() {
            phi += d * (2.0 * (j as f64 + 1.0) * chi).sin();
        }
        let lon = self.lon0 + eta1.sinh().atan2(xi1.cos()).to_degrees();
        let lon = (lon + 180.0).rem_euclid(360.0) - 180.0;
        let lat = phi.to_degrees();
        (lat.is_finite() && lon.is_finite()).then_some((lat, lon))
    }
}

/// UTM zone `zone` (1–60), north or south of the equator, on `ellipsoid`.
pub fn utm(zone: u8, north: bool, ellipsoid: Ellipsoid) -> TransverseMercator {
    let zone = zone.clamp(1, 60);
    TransverseMercator {
        ellipsoid,
        lon0: f64::from(zone) * 6.0 - 183.0,
        k0: 0.9996,
        false_easting: 500_000.0,
        false_northing: if north { 0.0 } else { 10_000_000.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Snyder's series for the ellipsoidal transverse Mercator (USGS PP 1395, eqs. 8-9 to 8-15,
    /// 3-21): independent of Krüger's, and good to a few millimetres within a few degrees of the
    /// central meridian. The oracle the Krüger implementation is checked against.
    fn snyder_forward(tm: &TransverseMercator, lat: f64, lon: f64) -> (f64, f64) {
        let a = tm.ellipsoid.a;
        let f = tm.ellipsoid.f;
        let e2 = f * (2.0 - f);
        let ep2 = e2 / (1.0 - e2);
        let phi = lat.to_radians();
        let m = |phi: f64| {
            let (e4, e6) = (e2 * e2, e2 * e2 * e2);
            a * ((1.0 - e2 / 4.0 - 3.0 * e4 / 64.0 - 5.0 * e6 / 256.0) * phi
                - (3.0 * e2 / 8.0 + 3.0 * e4 / 32.0 + 45.0 * e6 / 1024.0) * (2.0 * phi).sin()
                + (15.0 * e4 / 256.0 + 45.0 * e6 / 1024.0) * (4.0 * phi).sin()
                - (35.0 * e6 / 3072.0) * (6.0 * phi).sin())
        };
        let n = a / (1.0 - e2 * phi.sin().powi(2)).sqrt();
        let t = phi.tan().powi(2);
        let c = ep2 * phi.cos().powi(2);
        let aa = (lon - tm.lon0).to_radians() * phi.cos();
        let x = tm.k0 * n * (aa + (1.0 - t + c) * aa.powi(3) / 6.0 + (5.0 - 18.0 * t + t * t + 72.0 * c - 58.0 * ep2) * aa.powi(5) / 120.0);
        let y = tm.k0
            * (m(phi)
                + n * phi.tan()
                    * (aa * aa / 2.0
                        + (5.0 - t + 9.0 * c + 4.0 * c * c) * aa.powi(4) / 24.0
                        + (61.0 - 58.0 * t + t * t + 600.0 * c - 330.0 * ep2) * aa.powi(6) / 720.0));
        (x + tm.false_easting, y + tm.false_northing)
    }

    #[test]
    fn central_meridian_northing_is_the_scaled_meridian_arc() {
        // WGS 84 meridian arc from the equator to 45°: 4 984 944.378 m; to the pole (the quarter
        // meridian): 10 001 965.729 m.
        let tm = utm(10, true, WGS84);
        let (e, n) = tm.forward(45.0, -123.0).unwrap();
        assert!((e - 500_000.0).abs() < 1e-6, "{e}");
        assert!((n - 0.9996 * 4_984_944.378).abs() < 0.01, "{n}");
        let (_, n) = tm.forward(89.999_999_9, -123.0).unwrap();
        assert!((n - 0.9996 * 10_001_965.729).abs() < 0.05, "{n}");
        // the equator on the central meridian is the origin
        let (e, n) = tm.forward(0.0, -123.0).unwrap();
        assert!((e - 500_000.0).abs() < 1e-6 && n.abs() < 1e-6);
    }

    #[test]
    fn snyders_worked_example() {
        // Snyder, Map Projections: A Working Manual, transverse Mercator (ellipsoid) example:
        // Clarke 1866, k0 = 0.9996, central meridian 75° W; 40°30' N, 73°30' W
        // → x = 127 106.5 m, y = 4 484 124.4 m.
        let clarke = Ellipsoid { a: 6_378_206.4, f: 1.0 - (1.0 - 0.006_768_66f64).sqrt() };
        let tm = TransverseMercator { ellipsoid: clarke, lon0: -75.0, k0: 0.9996, false_easting: 0.0, false_northing: 0.0 };
        let (x, y) = tm.forward(40.5, -73.5).unwrap();
        assert!((x - 127_106.5).abs() < 0.1, "{x}");
        assert!((y - 4_484_124.4).abs() < 0.1, "{y}");
        let (lat, lon) = tm.inverse(127_106.5, 4_484_124.4).unwrap();
        assert!((lat - 40.5).abs() < 1e-6 && (lon + 73.5).abs() < 1e-6, "{lat} {lon}");
    }

    #[test]
    fn golden_gate_in_utm_10n() {
        // the bridge deck at mid-span, in UTM zone 10 N; Krüger and Snyder agree to a centimetre
        let tm = utm(10, true, WGS84);
        let (e, n) = tm.forward(37.8199, -122.4783).unwrap();
        let (se, sn) = snyder_forward(&tm, 37.8199, -122.4783);
        assert!((e - se).abs() < 0.01 && (n - sn).abs() < 0.01, "{e} {n} vs {se} {sn}");
        assert!((545_000.0..547_000.0).contains(&e) && (4_185_000.0..4_187_000.0).contains(&n), "{e} {n}");
        let (lat, lon) = tm.inverse(e, n).unwrap();
        // 1e-8° is about a millimetre
        assert!((lat - 37.8199).abs() < 1e-8 && (lon + 122.4783).abs() < 1e-8);
    }

    #[test]
    fn southern_zones_use_the_false_northing() {
        let tm = utm(56, false, WGS84); // Sydney
        let (e, n) = tm.forward(-33.8568, 151.2153).unwrap();
        assert!(n > 6_000_000.0 && n < 10_000_000.0 && (300_000.0..700_000.0).contains(&e), "{e} {n}");
        let (lat, lon) = tm.inverse(e, n).unwrap();
        assert!((lat + 33.8568).abs() < 1e-8 && (lon - 151.2153).abs() < 1e-8);
    }

    #[test]
    fn hostile_input_gives_none() {
        let tm = utm(10, true, WGS84);
        assert_eq!(tm.forward(f64::NAN, 0.0), None);
        assert_eq!(tm.forward(10.0, -123.0 + 90.0), None, "90° from the central meridian");
        assert_eq!(tm.inverse(f64::INFINITY, 0.0), None);
        assert!(tm.inverse(1e12, 1e12).is_none_or(|(a, b)| a.is_finite() && b.is_finite()));
        assert_eq!(utm(0, true, WGS84).lon0, -177.0, "zone clamped");
    }

    proptest! {
        /// Krüger agrees with Snyder's independent series to under 5 cm across a whole zone, and
        /// the inverse returns the point to a millimetre.
        #[test]
        fn agrees_with_snyder_and_round_trips(lat in -80.0f64..84.0, dlon in -3.0f64..3.0, zone in 1u8..=60) {
            let tm = utm(zone, lat >= 0.0, WGS84);
            let lon = tm.lon0 + dlon;
            let (e, n) = tm.forward(lat, lon).unwrap();
            let (se, sn) = snyder_forward(&tm, lat, lon);
            prop_assert!((e - se).abs() < 0.05 && (n - sn).abs() < 0.05, "{lat} {lon}: {e} {n} vs {se} {sn}");
            let (lat2, lon2) = tm.inverse(e, n).unwrap();
            let lon2 = if lon2 - lon > 180.0 { lon2 - 360.0 } else if lon - lon2 > 180.0 { lon2 + 360.0 } else { lon2 };
            prop_assert!((lat2 - lat).abs() < 1e-8 && (lon2 - lon).abs() < 1e-8, "{lat} {lon} -> {lat2} {lon2}");
        }
    }
}
