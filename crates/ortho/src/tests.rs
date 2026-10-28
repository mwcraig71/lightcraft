//! End-to-end: synthetic GeoTIFFs written with [`crate::testdata`] → [`inspect`] → [`build`] →
//! tiles with the expected colours at known places.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use lightcraft_tiles::{TileId, mercator};

use crate::testdata::{Spec, write_geotiff};
use crate::{Crs, Error, build, guarded, inspect, read_tile};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("lc-ortho-{name}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn path(&self, p: &str) -> PathBuf {
        self.0.join(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const RES: f64 = 0.03;

/// The Golden Gate deck's mid-span in UTM 10 N, and a `size`-pixel square at 3 cm centred on it.
fn golden_gate(size: u32) -> Spec {
    let (e, n) = Crs::from_epsg(32610).unwrap().from_lat_lon(37.8199, -122.4783).unwrap();
    let half = f64::from(size) * RES / 2.0;
    Spec::utm(size, size, 32610, RES, (e - half, n + half))
}

/// Four quadrants: red, green, blue, white.
fn quadrants(size: u32) -> impl Fn(u32, u32) -> [u8; 4] {
    move |x, y| match (x < size / 2, y < size / 2) {
        (true, true) => [220, 30, 30, 255],
        (false, true) => [30, 200, 40, 255],
        (true, false) => [30, 40, 210, 255],
        (false, false) => [250, 250, 250, 255],
    }
}

/// The tile pixel at `lat, lon` and zoom `z`.
fn tile_pixel(dir: &Path, lat: f64, lon: f64, z: u8) -> Option<[u8; 4]> {
    let (wx, wy) = mercator::to_world(lat, lon);
    let t = mercator::tile_at(wx, wy, z);
    let s = mercator::scale(f64::from(z));
    let (px, py) = ((wx * s - f64::from(t.x) * 256.0) as usize, (wy * s - f64::from(t.y) * 256.0) as usize);
    let img = read_tile(dir, t).unwrap()?;
    let at = (py * img.width + px) * 4;
    Some(img.rgba[at..at + 4].try_into().unwrap())
}

fn close(a: [u8; 4], b: [u8; 4], tol: i32) -> bool {
    a.iter().zip(b).all(|(x, y)| (i32::from(*x) - i32::from(y)).abs() <= tol)
}

fn build_all(src: &Path, out: &Path) -> (crate::Info, crate::Built) {
    let info = inspect(src).unwrap();
    let cancel = AtomicBool::new(false);
    let last = AtomicU64::new(0);
    let built = build(src, &info, out, 4, &cancel, &|done, total| {
        assert!(done <= total, "{done} of {total}");
        last.store(done, Ordering::Relaxed);
    })
    .unwrap();
    assert_eq!(last.load(Ordering::Relaxed), info.tile_count(), "progress reaches the total");
    (info, built)
}

#[test]
fn a_tiled_utm_orthomosaic_becomes_tiles_with_its_colours_in_place() {
    let dir = Scratch::new("tiled");
    let src = dir.path("deck.tif");
    let size = 2048;
    let spec = golden_gate(size);
    write_geotiff(&src, &spec, quadrants(size)).unwrap();
    let (info, built) = build_all(&src, &dir.path("tiles"));
    assert_eq!((info.width, info.height), (2048, 2048));
    assert_eq!(info.epsg, 32610);
    assert!((info.resolution_m - 0.03).abs() < 0.0005, "{}", info.resolution_m);
    assert_eq!((info.min_zoom, info.max_zoom), (12, 22));
    assert!(info.crs.contains("UTM zone 10N"), "{}", info.crs);
    assert!(built.tiles > 80, "{built:?}");
    // the quadrants' centres, 15 m from the middle, at the deepest zoom and two zooms out
    let g = &info.georef;
    let tiles = dir.path("tiles");
    for ((c, r), want) in [
        ((512.0, 512.0), [220, 30, 30, 255]),
        ((1536.0, 512.0), [30, 200, 40, 255]),
        ((512.0, 1536.0), [30, 40, 210, 255]),
        ((1536.0, 1536.0), [250, 250, 250, 255]),
    ] {
        let (lat, lon) = g.lat_lon(c, r).unwrap();
        for z in [22, 20, 17] {
            let got = tile_pixel(&tiles, lat, lon, z).unwrap_or_else(|| panic!("no tile at z{z} for {lat} {lon}"));
            assert!(close(got, want, 6), "z{z} at pixel {c},{r}: {got:?} want {want:?}");
        }
    }
    // a quadrant boundary is where the georeferencing puts it: 3 cm either side of the centre line
    let (lat, lon_left) = g.lat_lon(1024.0 - 3.0, 512.0).unwrap();
    let (_, lon_right) = g.lat_lon(1024.0 + 3.0, 512.0).unwrap();
    assert!(close(tile_pixel(&tiles, lat, lon_left, 22).unwrap(), [220, 30, 30, 255], 8));
    assert!(close(tile_pixel(&tiles, lat, lon_right, 22).unwrap(), [30, 200, 40, 255], 8));
    // outside the orthomosaic: no tile, or a transparent pixel
    let (lat, lon) = g.lat_lon(-200.0, -200.0).unwrap();
    assert!(tile_pixel(&tiles, lat, lon, 22).is_none_or(|p| p[3] == 0));
    // the coarsest zoom has the one tile, partly transparent (a PNG)
    let (wx, wy) = mercator::to_world(37.8199, -122.4783);
    let t = mercator::tile_at(wx, wy, 12);
    let file = crate::tile_file(&tiles, t).unwrap();
    assert_eq!(file.extension().unwrap(), "png");
    // a tile inside is opaque, a JPEG
    let (lat, lon) = g.lat_lon(512.0, 512.0).unwrap();
    let (wx, wy) = mercator::to_world(lat, lon);
    assert_eq!(crate::tile_file(&tiles, mercator::tile_at(wx, wy, 22)).unwrap().extension().unwrap(), "jpg");
}

#[test]
fn nodata_is_transparent() {
    let dir = Scratch::new("nodata");
    let src = dir.path("nodata.tif");
    let size = 512;
    let mut spec = golden_gate(size);
    spec.bands = 3;
    spec.nodata = Some("0".into());
    // the left half no-data (black), the right half grey
    write_geotiff(&src, &spec, |x, _| if x < 256 { [0, 0, 0, 255] } else { [128, 128, 128, 255] }).unwrap();
    let (info, _) = build_all(&src, &dir.path("tiles"));
    assert_eq!(info.nodata, Some(0.0));
    let g = &info.georef;
    let (lat, lon) = g.lat_lon(100.0, 256.0).unwrap();
    assert_eq!(tile_pixel(&dir.path("tiles"), lat, lon, 22).map_or(0, |p| p[3]), 0, "no-data is transparent");
    let (lat, lon) = g.lat_lon(400.0, 256.0).unwrap();
    assert!(close(tile_pixel(&dir.path("tiles"), lat, lon, 22).unwrap(), [128, 128, 128, 255], 3));
}

#[test]
fn an_alpha_band_is_transparent_too() {
    let dir = Scratch::new("alpha");
    let src = dir.path("alpha.tif");
    let size = 512;
    write_geotiff(&src, &golden_gate(size), |_, y| if y < 256 { [10, 20, 30, 0] } else { [200, 100, 50, 255] }).unwrap();
    let (info, _) = build_all(&src, &dir.path("tiles"));
    let (lat, lon) = info.georef.lat_lon(256.0, 100.0).unwrap();
    assert_eq!(tile_pixel(&dir.path("tiles"), lat, lon, 22).map_or(0, |p| p[3]), 0);
    let (lat, lon) = info.georef.lat_lon(256.0, 400.0).unwrap();
    assert!(close(tile_pixel(&dir.path("tiles"), lat, lon, 22).unwrap(), [200, 100, 50, 255], 3));
}

#[test]
fn strips_bigtiff_overviews_and_lat_lon_files_read_too() {
    let dir = Scratch::new("variants");
    let size = 600;
    // BigTIFF in strips with an overview, NAD83 UTM
    let src = dir.path("strips.tif");
    let mut spec = golden_gate(size);
    spec.epsg = 26910;
    spec.tile = None;
    spec.rows_per_strip = 7;
    spec.bigtiff = true;
    spec.overview = true;
    write_geotiff(&src, &spec, quadrants(size)).unwrap();
    let info = inspect(&src).unwrap();
    assert_eq!(info.overviews, 1);
    assert_eq!(info.width, 600, "the full-resolution image, not the overview");
    assert!(info.layout.contains("strips") && info.crs.contains("NAD83"), "{} {}", info.layout, info.crs);
    let (info, _) = build_all(&src, &dir.path("strip-tiles"));
    let (lat, lon) = info.georef.lat_lon(450.0, 450.0).unwrap();
    assert!(close(tile_pixel(&dir.path("strip-tiles"), lat, lon, 22).unwrap(), [250, 250, 250, 255], 6));

    // WGS 84 latitude / longitude, ~3 cm in degrees
    let geo = dir.path("geo.tif");
    let deg = 0.03 / 111_320.0;
    let spec = Spec {
        geographic: true,
        epsg: 4326,
        pixel_scale: (deg / 37.82f64.to_radians().cos(), deg),
        origin: (-122.479, 37.8202),
        ..golden_gate(size)
    };
    write_geotiff(&geo, &spec, quadrants(size)).unwrap();
    let (info, _) = build_all(&geo, &dir.path("geo-tiles"));
    assert!((info.resolution_m - 0.03).abs() < 0.001, "{}", info.resolution_m);
    assert!((info.corners[0].0 - 37.8202).abs() < 1e-9 && (info.corners[0].1 + 122.479).abs() < 1e-9);
    let (lat, lon) = info.georef.lat_lon(150.0, 450.0).unwrap();
    assert!(close(tile_pixel(&dir.path("geo-tiles"), lat, lon, 22).unwrap(), [30, 40, 210, 255], 6));

    // Web Mercator with a rotated affine transform (30°)
    let wm = dir.path("rotated.tif");
    let (x, y) = Crs::WebMercator.from_lat_lon(37.8199, -122.4783).unwrap();
    let k = 0.03 / 37.82f64.to_radians().cos();
    let (s, c) = (30f64.to_radians().sin() * k, 30f64.to_radians().cos() * k);
    let spec = Spec { epsg: 3857, transform: Some([c, -s, 0.0, x, -s, -c, 0.0, y, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]), ..golden_gate(size) };
    write_geotiff(&wm, &spec, quadrants(size)).unwrap();
    let (info, _) = build_all(&wm, &dir.path("wm-tiles"));
    for ((cc, rr), want) in [((150.0, 150.0), [220, 30, 30, 255]), ((450.0, 150.0), [30, 200, 40, 255]), ((450.0, 450.0), [250, 250, 250, 255])] {
        let (lat, lon) = info.georef.lat_lon(cc, rr).unwrap();
        let got = tile_pixel(&dir.path("wm-tiles"), lat, lon, 22).unwrap();
        assert!(close(got, want, 6), "{cc},{rr}: {got:?}");
    }
}

#[test]
fn unsupported_and_plain_tiffs_say_why() {
    let dir = Scratch::new("unsupported");
    let src = dir.path("stateplane.tif");
    let spec = Spec { epsg: 2227, ..golden_gate(64) };
    write_geotiff(&src, &spec, quadrants(64)).unwrap();
    match inspect(&src) {
        Err(Error::Unsupported(m)) => assert!(m.contains("EPSG:2227") && m.contains("UTM"), "{m}"),
        other => panic!("{other:?}"),
    }
    // a TIFF without GeoTIFF tags
    let plain = dir.path("plain.tif");
    let data = lightcraft_tiff_free_plain_tiff();
    std::fs::write(&plain, data).unwrap();
    match inspect(&plain) {
        Err(Error::NotGeo(m)) => assert!(m.contains("not a GeoTIFF"), "{m}"),
        other => panic!("{other:?}"),
    }
}

/// A 1 × 1 grey TIFF with no GeoTIFF tags, by hand.
fn lightcraft_tiff_free_plain_tiff() -> Vec<u8> {
    let mut v = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
    let entries: [(u16, u16, u32, u32); 8] =
        [(256, 4, 1, 1), (257, 4, 1, 1), (258, 3, 1, 8), (259, 3, 1, 1), (262, 3, 1, 1), (273, 4, 1, 110), (277, 3, 1, 1), (279, 4, 1, 1)];
    v.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, typ, count, value) in entries {
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(&typ.to_le_bytes());
        v.extend_from_slice(&count.to_le_bytes());
        v.extend_from_slice(&value.to_le_bytes());
    }
    v.extend_from_slice(&0u32.to_le_bytes());
    v.resize(110, 0);
    v.push(128);
    v
}

#[test]
fn hostile_files_are_errors_not_panics() {
    let dir = Scratch::new("hostile");
    let src = dir.path("ok.tif");
    write_geotiff(&src, &golden_gate(512), quadrants(512)).unwrap();
    let whole = std::fs::read(&src).unwrap();
    // cut short at many places (the directory is at the end: most cuts lose it)
    for cut in [0, 3, 8, 100, whole.len() / 3, whole.len() / 2, whole.len() - 200, whole.len() - 1] {
        let p = dir.path(&format!("cut-{cut}.tif"));
        std::fs::write(&p, &whole[..cut]).unwrap();
        let r = guarded("inspect", || inspect(&p));
        assert!(r.is_err() || cut > whole.len() / 2, "cut at {cut}: {r:?}");
        if let Ok(info) = r {
            let cancel = AtomicBool::new(false);
            let _ = guarded("build", || build(&p, &info, &dir.path(&format!("cut-{cut}")), 2, &cancel, &|_, _| {}));
        }
    }
    // garbage, an empty file, a missing file
    std::fs::write(dir.path("garbage.tif"), (0..5000u32).map(|i| (i * 7919 % 251) as u8).collect::<Vec<_>>()).unwrap();
    assert!(inspect(&dir.path("garbage.tif")).is_err());
    std::fs::write(dir.path("empty.tif"), b"").unwrap();
    assert!(inspect(&dir.path("empty.tif")).is_err());
    assert!(matches!(inspect(&dir.path("missing.tif")), Err(Error::Io(_))));

    // every strip and tile past the end of the file: the build says nothing could be read
    let bad = dir.path("offsets.tif");
    write_geotiff(&bad, &Spec { corrupt_offsets: true, ..golden_gate(512) }, quadrants(512)).unwrap();
    let info = inspect(&bad).unwrap();
    let cancel = AtomicBool::new(false);
    let r = guarded("build", || build(&bad, &info, &dir.path("offsets"), 2, &cancel, &|_, _| {}));
    assert!(matches!(&r, Err(Error::Invalid(m)) if m.contains("could be read")), "{r:?}");

    // absurd sizes: a header claiming 4 000 000 000 px a side
    let mut huge = lightcraft_tiff_free_plain_tiff();
    huge[8 + 2 + 8..8 + 2 + 12].copy_from_slice(&4_000_000_000u32.to_le_bytes());
    huge[8 + 2 + 12 + 8..8 + 2 + 12 + 12].copy_from_slice(&4_000_000_000u32.to_le_bytes());
    std::fs::write(dir.path("huge.tif"), &huge).unwrap();
    assert!(guarded("inspect", || inspect(&dir.path("huge.tif"))).is_err());

    // a 2 000 000 px square at 3 cm (60 km): refused before any tile is made
    let src_info = inspect(&src).unwrap();
    let header = crate::raster::Header {
        width: 2_000_000,
        height: 2_000_000,
        georef: src_info.georef,
        layout: String::new(),
        overviews: 0,
        nodata: None,
        alpha: true,
        file_size: 0,
    };
    assert!(matches!(crate::Info::from_header(&header), Err(Error::TooLarge(_))));
}

#[test]
fn a_cancelled_build_stops_with_cancelled() {
    let dir = Scratch::new("cancel");
    let src = dir.path("deck.tif");
    write_geotiff(&src, &golden_gate(1024), quadrants(1024)).unwrap();
    let info = inspect(&src).unwrap();
    let cancel = AtomicBool::new(false);
    let r = build(&src, &info, &dir.path("tiles"), 2, &cancel, &|done, _| {
        if done >= 3 {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert_eq!(r, Err(Error::Cancelled));
}

#[test]
fn reading_a_missing_or_damaged_tile() {
    let dir = Scratch::new("tiles");
    assert_eq!(read_tile(&dir.0, TileId { z: 3, x: 1, y: 1 }), Ok(None));
    std::fs::create_dir_all(dir.path("3/1")).unwrap();
    std::fs::write(dir.path("3/1/1.png"), b"not a png").unwrap();
    assert!(read_tile(&dir.0, TileId { z: 3, x: 1, y: 1 }).is_err());
    std::fs::write(dir.path("3/1/2.jpg"), b"\xff\xd8 nope").unwrap();
    assert!(read_tile(&dir.0, TileId { z: 3, x: 1, y: 2 }).is_err());
}
