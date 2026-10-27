//! Which photos get a pin, in what order, and how pins close together on screen merge.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use lightcraft_catalog::PhotoId;
use lightcraft_tiles::mercator;

use crate::LightcraftApp;

/// One photo with a location.
#[derive(Clone, Debug, PartialEq)]
pub struct Pin {
    pub id: PhotoId,
    /// Web Mercator world coordinates (0..1).
    pub world: (f64, f64),
    /// ISO 8601 capture time (the import time when there is none); the sort key.
    pub captured: String,
    pub file_name: String,
}

/// The pins for the grid's current list.
pub struct Pins {
    /// In capture order.
    pub pins: Vec<Pin>,
    /// Photos in the list.
    pub total: usize,
    /// Photos in the list without a location.
    pub without: Vec<PhotoId>,
    /// Changes when the set of pins or their places change (a new folder or filter fits again).
    pub key: u64,
}

/// A usable latitude/longitude (finite, in range, and not the 0, 0 some cameras write for "no fix").
fn usable((lat, lon): (f64, f64)) -> bool {
    lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0 && !(lat == 0.0 && lon == 0.0)
}

/// The pins of the photos the grid shows now (same list, same filter: no second filter here).
pub fn collect(app: &mut LightcraftApp) -> Pins {
    let (_, ids) = app.session.visible_shared();
    let mut pins = Vec::new();
    let mut without = Vec::new();
    for id in ids.iter() {
        let Some(p) = app.session.catalog.photo(*id) else { continue };
        match p.meta.gps.filter(|g| usable(*g)) {
            Some((lat, lon)) => {
                pins.push(Pin { id: *id, world: mercator::to_world(lat, lon), captured: p.date().to_string(), file_name: p.file_name.clone() })
            }
            None => without.push(*id),
        }
    }
    pins.sort_by(|a, b| a.captured.cmp(&b.captured).then_with(|| a.file_name.cmp(&b.file_name)).then(a.id.0.cmp(&b.id.0)));
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in &pins {
        p.id.0.hash(&mut h);
        p.world.0.to_bits().hash(&mut h);
        p.world.1.to_bits().hash(&mut h);
    }
    Pins { pins, total: ids.len(), without, key: h.finish() }
}

/// Pins drawn as one: the indices of its pins (in capture order) and their mean world position.
/// A cluster of one pin is drawn as the pin.
#[derive(Clone, Debug, PartialEq)]
pub struct Cluster {
    pub members: Vec<usize>,
    pub world: (f64, f64),
}

/// Group `pins` at world-pixel `scale` (pixels per world unit): in capture order, a pin joins the
/// first cluster whose first pin is within `radius` pixels, else starts one. Worked out in world
/// pixels, not on screen, so clusters don't change while panning; looked up in a grid of
/// `radius`-wide cells, so a few thousand pins stay cheap.
pub fn cluster(pins: &[Pin], scale: f64, radius: f64) -> Vec<Cluster> {
    let radius = if radius.is_finite() && radius > 1.0 { radius } else { 24.0 };
    let px = |p: &Pin| (p.world.0 * scale, p.world.1 * scale);
    let cell = |(x, y): (f64, f64)| ((x / radius).floor() as i64, (y / radius).floor() as i64);
    let mut seeds: Vec<(f64, f64)> = Vec::new();
    let mut members: Vec<Vec<usize>> = Vec::new();
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, p) in pins.iter().enumerate() {
        let at = px(p);
        let (cx, cy) = cell(at);
        let near = (cx - 1..=cx + 1)
            .flat_map(|x| (cy - 1..=cy + 1).map(move |y| (x, y)))
            .filter_map(|c| grid.get(&c))
            .flatten()
            .copied()
            .filter(|k| seeds.get(*k).is_some_and(|s| (s.0 - at.0).hypot(s.1 - at.1) <= radius))
            .min();
        match near.and_then(|k| members.get_mut(k)) {
            Some(m) => m.push(i),
            None => {
                grid.entry((cx, cy)).or_default().push(seeds.len());
                seeds.push(at);
                members.push(vec![i]);
            }
        }
    }
    members
        .into_iter()
        .map(|members| {
            let n = members.len().max(1) as f64;
            let (sx, sy) = members.iter().filter_map(|i| pins.get(*i)).fold((0.0, 0.0), |(x, y), p| (x + p.world.0, y + p.world.1));
            Cluster { world: (sx / n, sy / n), members }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(id: u64, lat: f64, lon: f64) -> Pin {
        Pin { id: PhotoId(id), world: mercator::to_world(lat, lon), captured: format!("2024-05-01T10:00:{id:02}"), file_name: format!("{id}.jpg") }
    }

    #[test]
    fn close_pins_cluster_when_zoomed_out_and_separate_when_zoomed_in() {
        // three photos at neighbouring piers, ~30 m apart, and one across the river (~2 km)
        let pins = [pin(1, 40.0, -75.0), pin(2, 40.0, -74.99965), pin(3, 40.0, -74.9993), pin(4, 40.02, -75.0)];
        let far = cluster(&pins, mercator::scale(12.0), 24.0);
        assert_eq!(far.len(), 2, "{far:?}");
        assert!(far.iter().any(|c| c.members == [0, 1, 2]));
        let near = cluster(&pins, mercator::scale(19.0), 24.0);
        assert_eq!(near.len(), 4);
    }

    #[test]
    fn evenly_spaced_pins_farther_apart_than_the_radius_stay_single() {
        // a pin every 30 points (the clusters of a fixed grid would pair some of them up)
        let s = mercator::scale(16.0);
        let pins: Vec<Pin> = (0u32..20).map(|i| Pin { world: (0.3 + f64::from(i) * 30.0 / s, 0.4), ..pin(i.into(), 0.0, 0.0) }).collect();
        assert!(cluster(&pins, s, 24.0).iter().all(|c| c.members.len() == 1));
        // and 10 points apart they merge
        let pins: Vec<Pin> = (0u32..20).map(|i| Pin { world: (0.3 + f64::from(i) * 10.0 / s, 0.4), ..pin(i.into(), 0.0, 0.0) }).collect();
        let c = cluster(&pins, s, 24.0);
        assert!(c.len() < 10 && c.iter().map(|c| c.members.len()).sum::<usize>() == 20);
    }

    #[test]
    fn unusable_coordinates_get_no_pin() {
        assert!(usable((40.0, -75.0)));
        assert!(!usable((0.0, 0.0)));
        assert!(!usable((f64::NAN, 1.0)));
        assert!(!usable((91.0, 1.0)));
        assert!(!usable((1.0, 181.0)));
    }
}
