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
    /// Which way the camera pointed (degrees from north), and whether from magnetic north.
    pub direction: Option<f32>,
    pub magnetic: bool,
    /// Metres above sea level.
    pub altitude: Option<f64>,
    /// How far off the position may be, in metres (`GPSHPositioningError`).
    pub accuracy: Option<f64>,
    /// The folder holding the file (none for a demo photo); a site is named after its photos' folder.
    pub folder: Option<String>,
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
pub(crate) fn usable((lat, lon): (f64, f64)) -> bool {
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
            Some((lat, lon)) => pins.push(Pin {
                id: *id,
                world: mercator::to_world(lat, lon),
                captured: p.date().to_string(),
                file_name: p.file_name.clone(),
                direction: p.meta.gps_direction.filter(|d| d.is_finite()),
                magnetic: p.meta.gps_direction_magnetic,
                altitude: p.meta.gps_altitude.filter(|a| a.is_finite()),
                accuracy: p.meta.gps_accuracy.and_then(lightcraft_meta::Gps::accuracy_metres),
                folder: match &p.source {
                    lightcraft_catalog::Source::File { path } => parent(path),
                    lightcraft_catalog::Source::Demo { .. } => None,
                },
            }),
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

/// The folder part of a file path (either separator), if any.
fn parent(path: &str) -> Option<String> {
    let cut = path.rfind(['/', '\\'])?;
    path.get(..cut).filter(|f| !f.is_empty()).map(str::to_string)
}

/// Pins drawn as one: the indices of its pins (in capture order) and their mean world position.
/// A cluster of one pin is drawn as the pin. A site (`site` is its name) is a whole group of
/// photos taken close together, one dot with the name, when the map is zoomed far out.
#[derive(Clone, Debug, PartialEq)]
pub struct Cluster {
    pub members: Vec<usize>,
    pub world: (f64, f64),
    pub site: Option<String>,
}

/// Points between pins of one site (about: worked out on a grid of half-gap cells). Twice the
/// widest site, so a drone shot off to the side stays with its bridge instead of making a second,
/// one-photo site beside it.
pub const SITE_GAP: f64 = 2.0 * SITE_EXTENT;
/// A site no wider than this (points) on screen draws as one dot with its name.
pub const SITE_EXTENT: f64 = 64.0;
/// Sites only draw as a dot below this zoom; closer in, one lone photo is a pin again.
pub const SITE_MAX_ZOOM: f64 = 15.0;

/// What the map draws at this zoom (`scale` world pixels per world unit): far out, each site (photos
/// taken close together, such as along one bridge) is one dot named by `name(members)`; closer
/// in, pins within `radius` points are clusters with a count (one per pier or span); closer still,
/// one pin per photo.
pub fn layout(pins: &[Pin], scale: f64, zoom: f64, radius: f64, name: &dyn Fn(&[usize]) -> String) -> Vec<Cluster> {
    let mut out = Vec::new();
    let mut loose = Vec::new();
    for members in sites(pins, scale, SITE_GAP) {
        let pts = members.iter().filter_map(|i| pins.get(*i)).map(|p| (p.world.0 * scale, p.world.1 * scale));
        let (x0, y0, x1, y1) = pts.fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |(a, b, c, d), (x, y)| (a.min(x), b.min(y), c.max(x), d.max(y)));
        if zoom < SITE_MAX_ZOOM && (x1 - x0).hypot(y1 - y0) <= SITE_EXTENT {
            let world = ((x0 + x1) / 2.0 / scale, (y0 + y1) / 2.0 / scale);
            out.push(Cluster { site: Some(name(&members)), world, members });
        } else {
            loose.extend(members);
        }
    }
    loose.sort_unstable();
    out.extend(cluster_of(pins, &loose, scale, radius));
    out
}

/// Pins connected through neighbours less than about `gap` pixels apart, each set in capture
/// order. Pins share a cell of `gap / 2` pixels or sit in touching cells, so a few thousand photos
/// at one bridge stay cheap.
pub fn sites(pins: &[Pin], scale: f64, gap: f64) -> Vec<Vec<usize>> {
    let cell = if gap.is_finite() && gap > 2.0 { gap / 2.0 } else { 24.0 };
    let mut cells: HashMap<(i64, i64), usize> = HashMap::new();
    let mut of_pin = Vec::with_capacity(pins.len());
    for p in pins {
        let key = ((p.world.0 * scale / cell).floor() as i64, (p.world.1 * scale / cell).floor() as i64);
        let n = cells.len();
        of_pin.push(*cells.entry(key).or_insert(n));
    }
    // union touching cells
    let mut root: Vec<usize> = (0..cells.len()).collect();
    fn find(root: &mut [usize], mut i: usize) -> usize {
        while let Some(&r) = root.get(i) {
            if r == i {
                break;
            }
            let up = root.get(r).copied().unwrap_or(r);
            if let Some(slot) = root.get_mut(i) {
                *slot = up;
            }
            i = r;
        }
        i
    }
    for (&(x, y), &c) in &cells {
        for (dx, dy) in [(1, -1), (1, 0), (1, 1), (0, 1)] {
            if let Some(&n) = cells.get(&(x + dx, y + dy)) {
                let (a, b) = (find(&mut root, c), find(&mut root, n));
                if let Some(slot) = root.get_mut(a.max(b)) {
                    *slot = a.min(b);
                }
            }
        }
    }
    let mut index: HashMap<usize, usize> = HashMap::new();
    let mut out: Vec<Vec<usize>> = Vec::new();
    for (i, c) in of_pin.into_iter().enumerate() {
        let r = find(&mut root, c);
        let k = *index.entry(r).or_insert_with(|| {
            out.push(Vec::new());
            out.len() - 1
        });
        if let Some(set) = out.get_mut(k) {
            set.push(i);
        }
    }
    out
}

/// A site's name: the last name of the folder all its photos are in (`…/Bridge 1234/Span 2/a.jpg`
/// and `…/Bridge 1234/Pier 3/b.jpg` → "Bridge 1234"), else `fallback` (the album or source).
pub fn site_name(pins: &[Pin], members: &[usize], fallback: &str) -> String {
    let mut common: Option<Vec<&str>> = None;
    for p in members.iter().filter_map(|i| pins.get(*i)) {
        let Some(folder) = p.folder.as_deref() else { return fallback.to_string() };
        let parts: Vec<&str> = folder.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
        common = Some(match common {
            None => parts,
            Some(c) => c.iter().zip(&parts).take_while(|(a, b)| a == b).map(|(a, _)| *a).collect(),
        });
    }
    common.and_then(|c| c.last().map(|s| s.to_string())).filter(|s| !s.ends_with(':')).unwrap_or_else(|| fallback.to_string())
}

/// Group `pins` at world-pixel `scale` (pixels per world unit): in capture order, a pin joins the
/// first cluster whose first pin is within `radius` pixels, else starts one. Worked out in world
/// pixels, not on screen, so clusters don't change while panning; looked up in a grid of
/// `radius`-wide cells, so a few thousand pins stay cheap.
pub fn cluster(pins: &[Pin], scale: f64, radius: f64) -> Vec<Cluster> {
    let all: Vec<usize> = (0..pins.len()).collect();
    cluster_of(pins, &all, scale, radius)
}

/// [`cluster`] for the pins at `which` (indices in capture order).
fn cluster_of(pins: &[Pin], which: &[usize], scale: f64, radius: f64) -> Vec<Cluster> {
    let radius = if radius.is_finite() && radius > 1.0 { radius } else { 24.0 };
    let px = |p: &Pin| (p.world.0 * scale, p.world.1 * scale);
    let cell = |(x, y): (f64, f64)| ((x / radius).floor() as i64, (y / radius).floor() as i64);
    let mut seeds: Vec<(f64, f64)> = Vec::new();
    let mut members: Vec<Vec<usize>> = Vec::new();
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for &i in which {
        let Some(p) = pins.get(i) else { continue };
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
            Cluster { world: (sx / n, sy / n), members, site: None }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(id: u64, lat: f64, lon: f64) -> Pin {
        Pin {
            id: PhotoId(id),
            world: mercator::to_world(lat, lon),
            captured: format!("2024-05-01T10:00:{id:02}"),
            file_name: format!("{id}.jpg"),
            direction: None,
            magnetic: false,
            altitude: None,
            accuracy: None,
            folder: None,
        }
    }

    fn in_folder(p: Pin, folder: &str) -> Pin {
        Pin { folder: Some(folder.into()), ..p }
    }

    #[test]
    fn far_out_a_bridge_is_one_named_site_then_counts_then_pins() {
        // a 600 m bridge: photos every ~30 m from two span folders, and a second site 20 km away
        let mut pins: Vec<Pin> = (0u32..20)
            .map(|i| {
                let folder = if i < 10 { "/jobs/Bridge 1234/Span 1" } else { "/jobs/Bridge 1234/Span 2" };
                in_folder(pin(i.into(), 40.0, -75.0 + f64::from(i) * 0.00035), folder)
            })
            .collect();
        pins.push(in_folder(pin(50, 40.18, -75.0), "/jobs/Culvert 9"));
        let name = |m: &[usize]| site_name(&pins, m, "All Photos");
        let far = layout(&pins, mercator::scale(11.0), 11.0, 24.0, &name);
        let mut names: Vec<_> = far.iter().map(|c| (c.site.clone(), c.members.len())).collect();
        names.sort();
        assert_eq!(names, vec![(Some("Bridge 1234".into()), 20), (Some("Culvert 9".into()), 1)]);
        // closer: the bridge breaks into counted clusters, the lone culvert photo is a pin
        let mid = layout(&pins, mercator::scale(15.0), 15.0, 24.0, &name);
        assert!(mid.iter().all(|c| c.site.is_none()), "{mid:?}");
        assert!(mid.iter().any(|c| c.members.len() > 1));
        assert_eq!(mid.iter().map(|c| c.members.len()).sum::<usize>(), 21);
        // closest: one pin per photo
        let near = layout(&pins, mercator::scale(19.0), 19.0, 24.0, &name);
        assert_eq!(near.len(), 21);
    }

    #[test]
    fn a_site_without_a_shared_folder_takes_the_fallback_name() {
        let pins = [in_folder(pin(1, 40.0, -75.0), "C:\\a\\x"), in_folder(pin(2, 40.0, -75.0), "D:\\b"), pin(3, 40.0, -75.0)];
        assert_eq!(site_name(&pins, &[0], "All"), "x");
        assert_eq!(site_name(&pins, &[0, 1], "All"), "All");
        assert_eq!(site_name(&pins, &[0, 2], "All"), "All", "a demo photo has no folder");
        assert_eq!(parent("C:\\a\\x\\p.jpg").as_deref(), Some("C:\\a\\x"));
        assert_eq!(parent("p.jpg"), None);
    }

    #[test]
    fn photos_far_apart_are_separate_sites() {
        let pins = [pin(1, 40.0, -75.0), pin(2, 40.0, -74.9999), pin(3, 41.0, -75.0)];
        let s = sites(&pins, mercator::scale(12.0), SITE_GAP);
        assert_eq!(s, vec![vec![0, 1], vec![2]]);
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
