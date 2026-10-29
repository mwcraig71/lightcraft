//! `photo.setLocation`: move photos to another place on the map (the Map view's right-click ▸
//! Move…). Typing a position in Edit Info (`photo.setMeta {gps}`) puts every photo on one point
//! and forgets the altitude; this moves a group as a group:
//!
//! - **translate** (default): every photo moves by the same distance and direction, the one that
//!   takes the group's centroid to `lat`, `lon`, so the photos keep their arrangement (worked out
//!   in Web Mercator world coordinates, which keep shapes at the scale of a site). Photos without
//!   a location are left out (`skipped`).
//! - **stack**: every photo goes to `lat`, `lon` (also those without a location: a way to give
//!   them one).
//!
//! The new place is the user's: the positioning error the file recorded (`gps_accuracy`) goes;
//! the altitude and the heading stay. The catalog changes (`Op::SetMeta`) and, with XMP sidecars
//! on, the sidecar; the original's EXIF is never changed. One undo step.

use std::collections::HashSet;

use serde_json::{Value, json};

use lightcraft_catalog::{Op, PhotoId};

use super::{CommandSpec, always, bad, cmd};
use crate::{Result, Session};

/// Most photos one call moves.
pub const MAX_PHOTOS: usize = 100_000;
/// Web Mercator's latitude limit (the map's square world).
pub const MAX_LAT: f64 = 85.051_128_779_806_59;

/// (lat, lon) → Web Mercator world coordinates (0..1, y down).
pub fn to_world(lat: f64, lon: f64) -> (f64, f64) {
    let lat = lat.clamp(-MAX_LAT, MAX_LAT).to_radians();
    let x = (lon + 180.0) / 360.0;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0;
    (x, y)
}

/// World coordinates → (lat, lon); x wraps round the world, y stops at the map's edge.
pub fn from_world(x: f64, y: f64) -> (f64, f64) {
    let x = if x.is_finite() { x.rem_euclid(1.0) } else { 0.5 };
    let y = if y.is_finite() { y.clamp(0.0, 1.0) } else { 0.5 };
    let lon = x * 360.0 - 180.0;
    let n = std::f64::consts::PI * (1.0 - 2.0 * y);
    let lat = n.sinh().atan().to_degrees();
    (lat.clamp(-MAX_LAT, MAX_LAT), lon)
}

/// Degrees rounded to 1e-7 (about a centimetre), as positions from a track log are.
fn round7(v: f64) -> f64 {
    (v * 1e7).round() / 1e7
}

/// A usable position: finite, in range, not the 0, 0 some cameras write for "no fix".
fn usable((lat, lon): (f64, f64)) -> bool {
    lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0 && !(lat == 0.0 && lon == 0.0)
}

/// Where each photo goes. `located`: the photos' current positions (`None`: no location).
/// Returns (id, new position) for the photos placed and how many were left out.
pub fn plan(located: &[(PhotoId, Option<(f64, f64)>)], lat: f64, lon: f64, stack: bool) -> (Vec<(PhotoId, (f64, f64))>, usize) {
    let target = (round7(lat).clamp(-MAX_LAT, MAX_LAT), round7(lon));
    if stack {
        return (located.iter().map(|(id, _)| (*id, target)).collect(), 0);
    }
    let with: Vec<(PhotoId, (f64, f64))> = located.iter().filter_map(|(id, g)| g.filter(|g| usable(*g)).map(|g| (*id, to_world(g.0, g.1)))).collect();
    let skipped = located.len() - with.len();
    let Some(first) = with.first().map(|(_, w)| *w) else { return (Vec::new(), skipped) };
    // the centroid, with x taken relative to the first photo so a group across the 180th meridian
    // stays together
    let rel = |w: (f64, f64)| {
        let dx = w.0 - first.0;
        (dx - dx.round(), w.1 - first.1)
    };
    let n = with.len() as f64;
    let (sx, sy) = with.iter().map(|(_, w)| rel(*w)).fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    let centroid = (first.0 + sx / n, first.1 + sy / n);
    let to = to_world(target.0, target.1);
    let (dx, dy) = (to.0 - centroid.0, to.1 - centroid.1);
    let placed = with
        .into_iter()
        .map(|(id, w)| {
            let (r, s) = rel(w);
            let (la, lo) = from_world(first.0 + r + dx, first.1 + s + dy);
            (id, (round7(la).clamp(-MAX_LAT, MAX_LAT), round7(lo)))
        })
        .collect();
    (placed, skipped)
}

fn ids(s: &Session, p: &Value, c: &str) -> Result<Vec<PhotoId>> {
    let list = match p.get("ids") {
        Some(Value::Array(a)) => a,
        Some(_) => return Err(bad(c, "`ids` must be a list of photo ids")),
        None => return Err(bad(c, "missing `ids` (the photos to move)")),
    };
    if list.is_empty() {
        return Err(bad(c, "`ids` is empty: no photos to move"));
    }
    if list.len() > MAX_PHOTOS {
        return Err(bad(c, format!("at most {MAX_PHOTOS} photos at a time")));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for v in list {
        let id = v.as_u64().map(PhotoId).ok_or_else(|| bad(c, format!("`ids` must be photo ids (whole numbers), not {v}")))?;
        if !s.catalog.photo(id).is_some_and(|ph| !ph.deleted) {
            return Err(bad(c, format!("no photo {}", id.0)));
        }
        if seen.insert(id) {
            out.push(id);
        }
    }
    Ok(out)
}

fn point(p: &Value, c: &str) -> Result<(f64, f64)> {
    let num = |k: &str| -> Result<f64> {
        match p.get(k) {
            None | Some(Value::Null) => Err(bad(c, format!("missing `{k}`"))),
            Some(v) => v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| bad(c, format!("`{k}` must be a number of degrees"))),
        }
    };
    let (lat, lon) = (num("lat")?, num("lon")?);
    if lat.abs() > 90.0 || lon.abs() > 180.0 {
        return Err(bad(c, "latitude must be within ±90 and longitude within ±180"));
    }
    Ok((lat, lon))
}

/// `photo.setLocation`: see the module docs.
fn set_location(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.setLocation";
    let ids = ids(s, p, C)?;
    let (lat, lon) = point(p, C)?;
    let stack = match p.get("mode").filter(|v| !v.is_null()) {
        None => false,
        Some(Value::String(m)) if m == "translate" => false,
        Some(Value::String(m)) if m == "stack" => true,
        Some(v) => return Err(bad(C, format!("`mode` must be \"translate\" or \"stack\", not {v}"))),
    };
    let located: Vec<(PhotoId, Option<(f64, f64)>)> = ids.iter().map(|id| (*id, s.catalog.photo(*id).and_then(|ph| ph.meta.gps))).collect();
    let (placed, skipped) = plan(&located, lat, lon, stack);
    if placed.is_empty() {
        return Err(bad(C, "none of these photos has a location to move: use mode \"stack\" to give them this one"));
    }
    let mut ops = Vec::new();
    let mut photos = Vec::new();
    for (id, g) in &placed {
        let Some(ph) = s.catalog.photo(*id) else { continue };
        photos.push(json!({"id": id.0, "lat": g.0, "lon": g.1}));
        if ph.meta.gps == Some(*g) && ph.meta.gps_accuracy.is_none() {
            continue;
        }
        let mut meta = ph.meta.clone();
        meta.gps = Some(*g);
        // placed by hand: the fix the file recorded, and how far off it was, are no longer it
        meta.gps_accuracy = None;
        ops.push(Op::SetMeta { id: *id, meta: Box::new(meta) });
    }
    let moved = ops.len();
    if moved > 0 {
        let label = if moved == 1 { "Move Photo".to_string() } else { format!("Move {moved} Photos") };
        s.commit(&label, Op::Batch { ops })?;
    }
    Ok(
        json!({"moved": moved, "skipped": skipped, "unchanged": placed.len() - moved, "mode": if stack { "stack" } else { "translate" }, "photos": photos}),
    )
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "photo.setLocation",
        "Move Photos on the Map",
        [],
        None,
        "{ids, lat, lon, mode?: translate|stack} → {moved, skipped, unchanged, mode, photos: [{id, lat, lon}]} — translate (default): the group keeps its arrangement, its centroid goes to lat, lon (photos without a location are skipped); stack: every photo to lat, lon (also gives photos without a location one). Clears the recorded positioning error, keeps altitude and heading; catalog and XMP sidecar, never the original; one undo step",
        always,
        set_location
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_coordinates_round_trip() {
        for (lat, lon) in [(37.8199, -122.4783), (-33.86, 151.2), (0.0, 0.0), (85.0, 179.9)] {
            let (x, y) = to_world(lat, lon);
            let (la, lo) = from_world(x, y);
            assert!((la - lat).abs() < 1e-9 && (lo - lon).abs() < 1e-9, "{lat} {lon} → {la} {lo}");
        }
        assert_eq!(from_world(f64::NAN, f64::INFINITY).1, 0.0);
        assert!(from_world(0.5, -3.0).0 <= MAX_LAT);
    }

    #[test]
    fn a_group_keeps_its_shape_and_its_centroid_lands_on_the_point() {
        let a = (PhotoId(1), Some((40.0, -75.0)));
        let b = (PhotoId(2), Some((40.0, -74.999)));
        let none = (PhotoId(3), None);
        let (placed, skipped) = plan(&[a, b, none], 41.0, -76.0, false);
        assert_eq!(skipped, 1);
        let (p1, p2) = (placed[0].1, placed[1].1);
        // still side by side, the same longitude apart, centred on the point
        assert!((p1.0 - p2.0).abs() < 1e-6 && ((p2.1 - p1.1) - 0.001).abs() < 1e-6, "{p1:?} {p2:?}");
        assert!(((p1.1 + p2.1) / 2.0 + 76.0).abs() < 1e-6 && (p1.0 - 41.0).abs() < 1e-5);
        // stack: all on the point, the one without a location too
        let (placed, skipped) = plan(&[a, b, none], 41.0, -76.0, true);
        assert_eq!((placed.len(), skipped), (3, 0));
        assert!(placed.iter().all(|(_, g)| *g == (41.0, -76.0)));
        // the latitude stays on the map
        let (placed, _) = plan(&[a], 89.9, 0.0, true);
        assert!(placed[0].1.0 <= MAX_LAT);
    }

    #[test]
    fn a_group_across_the_date_line_stays_together() {
        let (placed, _) = plan(&[(PhotoId(1), Some((10.0, 179.9995))), (PhotoId(2), Some((10.0, -179.9995)))], 10.0, 0.0, false);
        let (p1, p2) = (placed[0].1, placed[1].1);
        assert!((p1.1 + 0.0005).abs() < 1e-6 && (p2.1 - 0.0005).abs() < 1e-6, "{p1:?} {p2:?}");
    }
}
