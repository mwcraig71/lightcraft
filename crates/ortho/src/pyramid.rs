//! The orthomosaic as Web Mercator map tiles: the zooms it covers, building its tiles from the
//! source a window at a time, and reading them back.
//!
//! The deepest zoom ("native") is the one whose map pixel is closest to the orthomosaic's ground
//! pixel. Its tiles are made by inverse mapping: each tile pixel's centre goes Web Mercator →
//! latitude / longitude → the source CRS → a source pixel, sampled bilinearly (the exact mapping
//! at a 17 × 17 grid per tile, interpolated in between: the projections are smooth, the error is
//! far below a pixel). Each coarser zoom is made from the four tiles under it (a 2 × 2 box
//! filter), down to [`MIN_ZOOM`]. Tiles wholly outside the orthomosaic aren't written; opaque
//! tiles are JPEG, tiles with transparent parts (the edge, no-data) PNG.
//!
//! Layout: `<dir>/<z>/<x>/<y>.jpg` or `.png`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use lightcraft_tiles::mercator::{self, TILE};
use lightcraft_tiles::{TileId, TileImage};
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::georef::{self, Georef};
use crate::raster::{Header, Raster};

/// The coarsest zoom tiles are made for (about 40 m a pixel); farther out the map draws the outline.
pub const MIN_ZOOM: u8 = 12;
/// More tiles than this are refused (a native zoom of 22 over about 15 km × 15 km).
pub const MAX_TILES: u64 = 4_000_000;
const T: usize = 256;
/// Decoded source chunks each worker keeps.
const WORKER_CACHE: usize = 96 << 20;
/// Exact projections per tile side (the rest interpolated).
const GRID: usize = 16;

/// What an orthomosaic is and where it goes on the map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub georef: Georef,
    /// The CRS's name ("WGS 84 / UTM zone 10N (EPSG:32610)").
    pub crs: String,
    pub epsg: u16,
    /// Ground size of a pixel, metres (at the centre).
    pub resolution_m: f64,
    /// Corners (top-left, top-right, bottom-right, bottom-left), latitude / longitude.
    pub corners: [(f64, f64); 4],
    /// The footprint's bounds in world coordinates (`[x0, y0, x1, y1]`, 0..1, y down).
    pub world_bounds: [f64; 4],
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub layout: String,
    pub overviews: usize,
    pub nodata: Option<f64>,
    pub file_size: u64,
}

impl Info {
    pub fn from_header(h: &Header) -> Result<Info, Error> {
        let g = &h.georef;
        let (w, ht) = (f64::from(h.width), f64::from(h.height));
        let at = |c: f64, r: f64| g.lat_lon(c, r).ok_or_else(|| Error::Invalid("the orthomosaic's corners aren't on the Earth".into()));
        let corners = [at(0.0, 0.0)?, at(w, 0.0)?, at(w, ht)?, at(0.0, ht)?];
        // the outline, a few points along each edge (a UTM grid is slightly curved on the map)
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for k in 0..=8 {
            let t = f64::from(k) / 8.0;
            for (c, r) in [(t * w, 0.0), (w, t * ht), (t * w, ht), (0.0, t * ht)] {
                let (lat, lon) = at(c, r)?;
                let (x, y) = mercator::to_world(lat, lon);
                b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
            }
        }
        // ground size of a pixel at the centre
        let (cx, cy) = (w / 2.0, ht / 2.0);
        let centre = at(cx, cy)?;
        let res = (georef::distance_m(centre, at(cx + 1.0, cy)?) + georef::distance_m(centre, at(cx, cy + 1.0)?)) / 2.0;
        if !(res.is_finite() && res > 1e-4) {
            return Err(Error::Invalid(format!("the orthomosaic's pixel size ({res} m) makes no sense")));
        }
        let max_zoom = native_zoom(res, centre.0);
        let info = Info {
            width: h.width,
            height: h.height,
            georef: *g,
            crs: g.crs.name(),
            epsg: g.crs.epsg(),
            resolution_m: res,
            corners,
            world_bounds: b,
            min_zoom: MIN_ZOOM.min(max_zoom),
            max_zoom,
            layout: h.layout.clone(),
            overviews: h.overviews,
            nodata: h.nodata,
            file_size: h.file_size,
        };
        let tiles = info.tile_count();
        if tiles > MAX_TILES {
            return Err(Error::TooLarge(format!(
                "the orthomosaic would need {tiles} map tiles (it covers {:.1} km² at {:.1} cm a pixel); the most is {MAX_TILES}",
                info.area_km2(),
                res * 100.0
            )));
        }
        Ok(info)
    }

    /// Area of the footprint's bounds, km².
    pub fn area_km2(&self) -> f64 {
        let [x0, y0, x1, y1] = self.world_bounds;
        let (lat, _) = mercator::from_world((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let m = 40_075_016.686 * lat.to_radians().cos();
        (x1 - x0) * m * (y1 - y0) * m / 1e6
    }

    /// Tiles at zoom `z` overlapping the footprint's bounds, as column and row ranges (inclusive).
    pub fn tile_range(&self, z: u8) -> (u32, u32, u32, u32) {
        let [x0, y0, x1, y1] = self.world_bounds;
        let a = mercator::tile_at(x0, y0, z);
        let b = mercator::tile_at(x1, y1, z);
        (a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
    }

    /// Tiles at every zoom from [`Info::min_zoom`] to [`Info::max_zoom`] (an upper bound: those wholly outside are skipped).
    pub fn tile_count(&self) -> u64 {
        (self.min_zoom..=self.max_zoom)
            .map(|z| {
                let (x0, y0, x1, y1) = self.tile_range(z);
                (u64::from(x1 - x0) + 1).saturating_mul(u64::from(y1 - y0) + 1)
            })
            .fold(0u64, u64::saturating_add)
    }
}

/// The zoom whose map pixel at latitude `lat` is closest to `res` metres, at most 22.
pub fn native_zoom(res: f64, lat: f64) -> u8 {
    let z0 = mercator::metres_per_pixel(lat, 0.0);
    let z = (z0 / res).log2().round();
    if z.is_finite() { z.clamp(0.0, f64::from(mercator::MAX_ZOOM)) as u8 } else { 0 }
}

/// Read an orthomosaic's header and work out where it goes.
pub fn inspect(path: &Path) -> Result<Info, Error> {
    let r = Raster::open(path, 1 << 20)?;
    Info::from_header(&r.header)
}

/// What a build did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Built {
    /// Tiles written.
    pub tiles: u64,
    /// Bytes written.
    pub bytes: u64,
    /// Source strips or tiles that couldn't be decoded (drawn transparent).
    pub unreadable: u64,
}

/// The file of a tile, if it has been made.
pub fn tile_file(dir: &Path, t: TileId) -> Option<PathBuf> {
    let base = dir.join(t.z.to_string()).join(t.x.to_string());
    ["jpg", "png"].into_iter().map(|ext| base.join(format!("{}.{ext}", t.y))).find(|p| p.is_file())
}

/// A tile's image; `Ok(None)` when it wasn't made (outside the orthomosaic, or not yet).
pub fn read_tile(dir: &Path, t: TileId) -> Result<Option<TileImage>, Error> {
    let Some(path) = tile_file(dir, t) else { return Ok(None) };
    let bytes = std::fs::read(&path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    let jpeg = path.extension().is_some_and(|e| e == "jpg");
    let img = if jpeg { decode_jpeg(&bytes)? } else { decode_png(&bytes)? };
    Ok(Some(img))
}

fn decode_jpeg(bytes: &[u8]) -> Result<TileImage, Error> {
    use zune_core::bytestream::ZCursor;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA).set_max_width(4096).set_max_height(4096);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), opts);
    let rgba = d.decode().map_err(|e| Error::Invalid(format!("tile: {e:?}")))?;
    let (w, h) = d.dimensions().ok_or_else(|| Error::Invalid("tile: no size".into()))?;
    let img = TileImage { width: w, height: h, rgba };
    if img.rgba.len() != img.width * img.height * 4 {
        return Err(Error::Invalid("tile: unexpected size".into()));
    }
    Ok(img)
}

fn decode_png(bytes: &[u8]) -> Result<TileImage, Error> {
    let mut d = png::Decoder::new(std::io::Cursor::new(bytes));
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = d.read_info().map_err(|e| Error::Invalid(format!("tile: {e}")))?;
    let (w, h) = (r.info().width as usize, r.info().height as usize);
    if w > 4096 || h > 4096 {
        return Err(Error::Invalid("tile: too large".into()));
    }
    let mut buf = vec![0; r.output_buffer_size().ok_or_else(|| Error::Invalid("tile: too large".into()))?];
    let info = r.next_frame(&mut buf).map_err(|e| Error::Invalid(format!("tile: {e}")))?;
    let data = buf.get(..info.buffer_size()).ok_or_else(|| Error::Invalid("tile: short".into()))?;
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect(),
        png::ColorType::GrayscaleAlpha => data.as_chunks::<2>().0.iter().flat_map(|&[g, a]| [g, g, g, a]).collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
        png::ColorType::Indexed => return Err(Error::Invalid("tile: palette".into())),
    };
    if rgba.len() != w * h * 4 {
        return Err(Error::Invalid("tile: unexpected size".into()));
    }
    Ok(TileImage { width: w, height: h, rgba })
}

/// Premultiplied RGBA, `T × T`, as floats.
type Tile = Vec<[f32; 4]>;

/// Render one native-zoom tile from the source.
fn render(raster: &mut Raster, g: &Georef, t: TileId) -> Tile {
    let n = f64::from(mercator::tiles_across(t.z));
    let side = TILE * n;
    let step = T / GRID;
    // the exact source position at grid points (tile-pixel edges 0, 16, …, 256)
    let mut grid = vec![(f64::NAN, f64::NAN); (GRID + 1) * (GRID + 1)];
    for gy in 0..=GRID {
        for gx in 0..=GRID {
            let wx = (f64::from(t.x) * TILE + (gx * step) as f64) / side;
            let wy = (f64::from(t.y) * TILE + (gy * step) as f64) / side;
            let (lat, lon) = mercator::from_world(wx, wy);
            if let Some(p) = g.pixel(lat, lon)
                && let Some(cell) = grid.get_mut(gy * (GRID + 1) + gx)
            {
                *cell = p;
            }
        }
    }
    let mut out = vec![[0f32; 4]; T * T];
    for py in 0..T {
        let v = (py as f64 + 0.5) / step as f64;
        let gy = (v.floor() as usize).min(GRID - 1);
        let fy = v - gy as f64;
        for px in 0..T {
            let u = (px as f64 + 0.5) / step as f64;
            let gx = (u.floor() as usize).min(GRID - 1);
            let fx = u - gx as f64;
            let at = |x: usize, y: usize| grid.get(y * (GRID + 1) + x).copied().unwrap_or((f64::NAN, f64::NAN));
            let (p00, p10, p01, p11) = (at(gx, gy), at(gx + 1, gy), at(gx, gy + 1), at(gx + 1, gy + 1));
            let lerp = |a: f64, b: f64, c: f64, d: f64| (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy;
            let col = lerp(p00.0, p10.0, p01.0, p11.0);
            let row = lerp(p00.1, p10.1, p01.1, p11.1);
            if let Some(o) = out.get_mut(py * T + px) {
                *o = raster.sample(col, row);
            }
        }
    }
    out
}

/// A tile from the four under it (missing ones transparent).
fn downsample(children: [Option<Tile>; 4]) -> Tile {
    let mut out = vec![[0f32; 4]; T * T];
    let h = T / 2;
    for (i, child) in children.iter().enumerate() {
        let Some(c) = child else { continue };
        let (ox, oy) = ((i % 2) * h, (i / 2) * h);
        for y in 0..h {
            for x in 0..h {
                let mut s = [0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    if let Some(p) = c.get((2 * y + dy) * T + 2 * x + dx) {
                        for k in 0..4 {
                            s[k] += p[k] / 4.0;
                        }
                    }
                }
                if let Some(o) = out.get_mut((oy + y) * T + ox + x) {
                    *o = s;
                }
            }
        }
    }
    out
}

fn from_image(img: &TileImage) -> Option<Tile> {
    if img.width != T || img.height != T || img.rgba.len() != T * T * 4 {
        return None;
    }
    Some(
        img.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&[r, g, b, a8]| {
                let a = f32::from(a8) / 255.0;
                [f32::from(r) * a, f32::from(g) * a, f32::from(b) * a, f32::from(a8)]
            })
            .collect(),
    )
}

/// Write a tile (JPEG when opaque, PNG otherwise); nothing for a wholly transparent one.
/// Returns the bytes written.
fn write(dir: &Path, t: TileId, tile: &Tile) -> Result<u64, Error> {
    let mut rgba = Vec::with_capacity(T * T * 4);
    let (mut any, mut opaque) = (false, true);
    for p in tile {
        let a = p[3].round().clamp(0.0, 255.0);
        let un = |c: f32| if a > 0.0 { (c * 255.0 / a).round().clamp(0.0, 255.0) as u8 } else { 0 };
        let a8 = a as u8;
        any |= a8 > 0;
        opaque &= a8 == 255;
        rgba.extend_from_slice(&[un(p[0]), un(p[1]), un(p[2]), a8]);
    }
    if !any {
        return Ok(0);
    }
    let folder = dir.join(t.z.to_string()).join(t.x.to_string());
    std::fs::create_dir_all(&folder).map_err(|e| Error::Io(format!("{}: {e}", folder.display())))?;
    let mut bytes = Vec::new();
    let ext = if opaque {
        let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|&[r, g, b, _]| [r, g, b]).collect();
        jpeg_encoder::Encoder::new(&mut bytes, 88)
            .encode(&rgb, T as u16, T as u16, jpeg_encoder::ColorType::Rgb)
            .map_err(|e| Error::Io(format!("tile: {e}")))?;
        "jpg"
    } else {
        let mut e = png::Encoder::new(&mut bytes, T as u32, T as u32);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.set_compression(png::Compression::Fast);
        let mut w = e.write_header().map_err(|e| Error::Io(format!("tile: {e}")))?;
        w.write_image_data(&rgba).map_err(|e| Error::Io(format!("tile: {e}")))?;
        w.finish().map_err(|e| Error::Io(format!("tile: {e}")))?;
        "png"
    };
    // the other format from an earlier build would shadow this one
    let other = folder.join(format!("{}.{}", t.y, if opaque { "png" } else { "jpg" }));
    let _ = std::fs::remove_file(other);
    let path = folder.join(format!("{}.{ext}", t.y));
    let tmp = folder.join(format!("{}.{ext}.part", t.y));
    std::fs::write(&tmp, &bytes).map_err(|e| Error::Io(format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    Ok(bytes.len() as u64)
}

/// Progress: tiles looked at so far, and in all.
pub type Progress<'a> = &'a (dyn Fn(u64, u64) + Sync);

/// Build every tile of the orthomosaic at `source` into `dir`, on `threads` worker threads.
/// Stops with [`Error::Cancelled`] soon after `cancel` is set (the tiles made so far stay).
pub fn build(source: &Path, info: &Info, dir: &Path, threads: usize, cancel: &AtomicBool, progress: Progress) -> Result<Built, Error> {
    let total = info.tile_count();
    let done = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);
    let tiles = AtomicU64::new(0);
    let unreadable = AtomicU64::new(0);
    let first_error: Mutex<Option<Error>> = Mutex::new(None);
    let threads = threads.clamp(1, 32);
    progress(0, total);
    std::fs::create_dir_all(dir).map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
    let fail = |e: Error| {
        let mut slot = first_error.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.get_or_insert(e);
        cancel.load(Ordering::Relaxed)
    };
    let stop = || cancel.load(Ordering::Relaxed) || first_error.lock().map(|e| e.is_some()).unwrap_or(true);

    // the native zoom, a block of up to 8 × 8 tiles per job so each worker reads one area of the source
    let z = info.max_zoom;
    let block = z.saturating_sub(3).max(info.min_zoom);
    let shift = z - block;
    let (bx0, by0, bx1, by1) = info.tile_range(block);
    let (nx0, ny0, nx1, ny1) = info.tile_range(z);
    let blocks: Vec<(u32, u32)> = (by0..=by1).flat_map(|y| (bx0..=bx1).map(move |x| (x, y))).collect();
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut raster = match Raster::open(source, WORKER_CACHE) {
                    Ok(r) => r,
                    Err(e) => {
                        fail(e);
                        return;
                    }
                };
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(bx, by)) = blocks.get(i) else { break };
                    for ty in (by << shift)..((by + 1) << shift) {
                        for tx in (bx << shift)..((bx + 1) << shift) {
                            if !(nx0..=nx1).contains(&tx) || !(ny0..=ny1).contains(&ty) {
                                continue;
                            }
                            if stop() {
                                unreadable.fetch_add(raster.unreadable, Ordering::Relaxed);
                                return;
                            }
                            let t = TileId { z, x: tx, y: ty };
                            let tile = render(&mut raster, &info.georef, t);
                            match write(dir, t, &tile) {
                                Ok(0) => {}
                                Ok(b) => {
                                    bytes.fetch_add(b, Ordering::Relaxed);
                                    tiles.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(e) => {
                                    fail(e);
                                }
                            }
                            progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                        }
                    }
                }
                unreadable.fetch_add(raster.unreadable, Ordering::Relaxed);
            });
        }
    });

    // coarser zooms from the four tiles under each
    for z in (info.min_zoom..info.max_zoom).rev() {
        if stop() {
            break;
        }
        let (x0, y0, x1, y1) = info.tile_range(z);
        let list: Vec<TileId> = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| TileId { z, x, y })).collect();
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        if stop() {
                            return;
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&t) = list.get(i) else { break };
                        let child = |dx: u32, dy: u32| {
                            let c = TileId { z: t.z + 1, x: t.x * 2 + dx, y: t.y * 2 + dy };
                            read_tile(dir, c).ok().flatten().and_then(|img| from_image(&img))
                        };
                        let kids = [child(0, 0), child(1, 0), child(0, 1), child(1, 1)];
                        if kids.iter().any(Option::is_some) {
                            match write(dir, t, &downsample(kids)) {
                                Ok(0) => {}
                                Ok(b) => {
                                    bytes.fetch_add(b, Ordering::Relaxed);
                                    tiles.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(e) => {
                                    fail(e);
                                }
                            }
                        }
                        progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                    }
                });
            }
        });
    }
    if let Some(e) = first_error.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner) {
        return Err(e);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let built = Built { tiles: tiles.into_inner(), bytes: bytes.into_inner(), unreadable: unreadable.into_inner() };
    if built.tiles == 0 && built.unreadable > 0 {
        return Err(Error::Invalid(format!("none of the orthomosaic's image data could be read ({} strips or tiles failed)", built.unreadable)));
    }
    progress(total, total);
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_zoom_matches_the_ground_resolution() {
        // 3 cm at the Golden Gate: zoom 22 (2.9 cm a map pixel)
        assert_eq!(native_zoom(0.03, 37.82), 22);
        // 10 cm: 20 (11.8 cm)
        assert_eq!(native_zoom(0.10, 37.82), 20);
        assert_eq!(native_zoom(0.001, 0.0), 22, "capped");
        assert_eq!(native_zoom(f64::NAN, 0.0), 0);
        assert_eq!(native_zoom(1e9, 0.0), 0);
    }

    #[test]
    fn downsampling_averages_premultiplied() {
        let red = vec![[255.0, 0.0, 0.0, 255.0]; T * T];
        let out = downsample([Some(red), None, None, None]);
        assert_eq!(out[0], [255.0, 0.0, 0.0, 255.0]);
        assert_eq!(out[T - 1], [0.0; 4], "the top-right child is missing");
    }
}
