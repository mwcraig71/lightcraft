//! Reading an orthomosaic a window at a time: the full-resolution image's strips or tiles are
//! decoded on demand (LZW, Deflate, JPEG or none; classic TIFF or BigTIFF; 8 or 16 bits) into a
//! small cache, never the whole raster. Pixels come out as RGBA with no-data (GDAL's tag or an
//! alpha band) as transparent.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

use crate::Error;
use crate::georef::{self, GeoKeys, Georef};

/// The largest decoded strip or tile accepted (a whole-image strip of a huge file is refused).
const MAX_CHUNK_BYTES: usize = 64 << 20;
/// The largest single tag value (tile offset tables of a 100 000 px BigTIFF fit).
const MAX_TAG_BYTES: usize = 32 << 20;
/// IFDs looked at (the image, its overviews and masks).
const MAX_IFDS: usize = 64;
/// Images wider or taller than this are refused (about 100 km at 1 cm).
pub const MAX_SIDE: u32 = 10_000_000;

/// How the samples map to RGBA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bands {
    Gray,
    GrayAlpha,
    Rgb,
    Rgba,
    YCbCr,
}

impl Bands {
    fn samples(self) -> usize {
        match self {
            Bands::Gray => 1,
            Bands::GrayAlpha => 2,
            Bands::Rgb | Bands::YCbCr => 3,
            Bands::Rgba => 4,
        }
    }
}

/// What the file is, read from its directories (no pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub georef: Georef,
    /// "RGBA, 8-bit, tiled 512×512, LZW".
    pub layout: String,
    /// Reduced-resolution images (internal overviews) in the file.
    pub overviews: usize,
    pub nodata: Option<f64>,
    pub alpha: bool,
    pub file_size: u64,
}

/// An open orthomosaic: its full-resolution image, and decoded chunks cached by recent use.
pub struct Raster {
    dec: Decoder<BufReader<File>>,
    pub header: Header,
    bands: Bands,
    bits: u8,
    /// Chunk size (a strip is the image's width × rows per strip).
    chunk_w: u32,
    chunk_h: u32,
    tiled: bool,
    across: u32,
    cache: HashMap<u32, (Arc<Chunk>, u64)>,
    cache_bytes: usize,
    cache_limit: usize,
    tick: u64,
    /// Chunks that failed to decode (drawn transparent).
    pub unreadable: u64,
}

/// One decoded chunk: `w × h` straight-alpha RGBA.
pub struct Chunk {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

fn tiff_err(what: &str, e: tiff::TiffError) -> Error {
    Error::Invalid(format!("{what}: {e}"))
}

fn tag_f64s(dec: &mut Decoder<BufReader<File>>, tag: u16) -> Result<Option<Vec<f64>>, Error> {
    match dec.find_tag(Tag::from_u16_exhaustive(tag)) {
        Ok(Some(v)) => v.into_f64_vec().map(Some).map_err(|e| tiff_err("a georeferencing tag", e)),
        Ok(None) => Ok(None),
        Err(e) => Err(tiff_err("a georeferencing tag", e)),
    }
}

fn tag_u32(dec: &mut Decoder<BufReader<File>>, tag: Tag) -> Option<u32> {
    dec.find_tag_unsigned::<u32>(tag).ok().flatten()
}

impl Raster {
    /// Open `path` and read its directories and georeferencing; `cache_limit` bounds the decoded chunks kept (bytes).
    pub fn open(path: &Path, cache_limit: usize) -> Result<Raster, Error> {
        let file = File::open(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
        let file_size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let mut limits = Limits::default();
        limits.decoding_buffer_size = MAX_CHUNK_BYTES;
        limits.ifd_value_size = MAX_TAG_BYTES;
        limits.intermediate_buffer_size = MAX_CHUNK_BYTES;
        let mut dec = Decoder::new(BufReader::with_capacity(1 << 16, file)).map_err(|e| tiff_err("not a readable TIFF", e))?.with_limits(limits);
        // the full-resolution image: the first that is neither an overview nor a mask
        let mut main = None;
        let mut overviews = 0;
        for i in 0..MAX_IFDS {
            let subfile = tag_u32(&mut dec, Tag::NewSubfileType).unwrap_or(0);
            if subfile & 0b101 == 0 {
                main.get_or_insert(i);
            } else if subfile & 0b100 == 0 {
                overviews += 1;
            }
            if !dec.more_images() {
                break;
            }
            if dec.next_image().is_err() {
                break;
            }
        }
        let main = main.ok_or_else(|| Error::Invalid("the TIFF has no full-resolution image".into()))?;
        dec.seek_to_image(main).map_err(|e| tiff_err("the TIFF's image", e))?;
        let (width, height) = dec.dimensions().map_err(|e| tiff_err("the image size", e))?;
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(Error::Invalid(format!("the image is {width} × {height} pixels, which can't be an orthomosaic")));
        }

        // where it is
        let keys = match dec.find_tag(Tag::from_u16_exhaustive(georef::GEO_KEY_DIRECTORY)) {
            Ok(Some(v)) => GeoKeys::parse(&v.into_u16_vec().map_err(|e| tiff_err("the GeoTIFF key directory", e))?).map_err(Error::Unsupported)?,
            Ok(None) => {
                return Err(Error::NotGeo(
                    "the file has no GeoTIFF keys: it is a plain TIFF, not a GeoTIFF (export the orthomosaic as a GeoTIFF)".into(),
                ));
            }
            Err(e) => return Err(tiff_err("the GeoTIFF key directory", e)),
        };
        let scale = tag_f64s(&mut dec, georef::MODEL_PIXEL_SCALE)?;
        let ties = tag_f64s(&mut dec, georef::MODEL_TIEPOINT)?;
        let transform = tag_f64s(&mut dec, georef::MODEL_TRANSFORMATION)?;
        keys.crs().map_err(Error::Unsupported)?;
        let georef = Georef::new(&keys, scale.as_deref(), ties.as_deref(), transform.as_deref())
            .map_err(|e| if e.contains("not a GeoTIFF") { Error::NotGeo(e) } else { Error::Invalid(e) })?;
        let nodata = match dec.find_tag(Tag::from_u16_exhaustive(georef::GDAL_NODATA)) {
            Ok(Some(v)) => v.into_string().ok().and_then(|s| s.trim_matches(|c: char| c == '\0' || c.is_whitespace()).parse::<f64>().ok()),
            _ => None,
        };

        // how the pixels are stored
        if tag_u32(&mut dec, Tag::PlanarConfiguration) == Some(2) {
            return Err(Error::Unsupported(
                "the orthomosaic stores each band separately (planar configuration), which isn't supported: export it pixel-interleaved".into(),
            ));
        }
        let format = tag_u32(&mut dec, Tag::SampleFormat).unwrap_or(1);
        if format != 1 {
            return Err(Error::Unsupported(format!(
                "the orthomosaic's samples are {} (an elevation model or index map?): only 8- or 16-bit unsigned colour images are shown",
                match format {
                    2 => "signed integers",
                    3 => "floating point",
                    _ => "of an unknown format",
                }
            )));
        }
        let color = dec.colortype().map_err(|e| Error::Unsupported(format!("the orthomosaic's colour layout isn't supported ({e})")))?;
        let (bands, bits) = match color {
            ColorType::Gray(b) => (Bands::Gray, b),
            ColorType::GrayA(b) => (Bands::GrayAlpha, b),
            ColorType::RGB(b) => (Bands::Rgb, b),
            ColorType::RGBA(b) => (Bands::Rgba, b),
            ColorType::YCbCr(b) => (Bands::YCbCr, b),
            ColorType::Multiband { bit_depth, num_samples: 2 } => (Bands::GrayAlpha, bit_depth),
            ColorType::Multiband { bit_depth, num_samples } if num_samples >= 3 => (Bands::Rgb, bit_depth),
            other => return Err(Error::Unsupported(format!("the orthomosaic's colour layout ({other:?}) isn't supported: export RGB or RGBA"))),
        };
        if bits != 8 && bits != 16 {
            return Err(Error::Unsupported(format!("{bits}-bit samples aren't supported: export 8 or 16 bits per band")));
        }
        // a multiband image keeps its extra bands in each pixel
        let stored_samples = match color {
            ColorType::Multiband { num_samples, .. } => usize::from(num_samples),
            _ => bands.samples(),
        };
        let (tiled, chunk_w, chunk_h) = match (tag_u32(&mut dec, Tag::TileWidth), tag_u32(&mut dec, Tag::TileLength)) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (true, w, h),
            (Some(_), _) | (_, Some(_)) => return Err(Error::Invalid("the tile size is missing or zero".into())),
            _ => (false, width, tag_u32(&mut dec, Tag::RowsPerStrip).unwrap_or(height).clamp(1, height)),
        };
        let chunk_bytes = u64::from(chunk_w) * u64::from(chunk_h) * stored_samples as u64 * u64::from(bits / 8);
        if chunk_bytes > MAX_CHUNK_BYTES as u64 {
            return Err(Error::Unsupported(format!(
                "the orthomosaic is stored in {} of {} MB each, too large to read a piece at a time: export it tiled (GDAL: -co TILED=YES)",
                if tiled { "tiles" } else { "strips" },
                chunk_bytes >> 20
            )));
        }
        let compression = tag_u32(&mut dec, Tag::Compression).unwrap_or(1);
        let alpha = matches!(bands, Bands::Rgba | Bands::GrayAlpha);
        let layout = format!(
            "{}, {bits}-bit, {} {chunk_w}×{chunk_h}, {}",
            match bands {
                Bands::Gray => "grey",
                Bands::GrayAlpha => "grey + alpha",
                Bands::Rgb => "RGB",
                Bands::Rgba => "RGBA",
                Bands::YCbCr => "YCbCr",
            },
            if tiled { "tiles" } else { "strips" },
            match compression {
                1 => "uncompressed".to_string(),
                5 => "LZW".into(),
                7 => "JPEG".into(),
                8 | 32946 => "Deflate".into(),
                50000 => "Zstandard".into(),
                n => format!("compression {n}"),
            }
        );
        let header = Header { width, height, georef, layout, overviews, nodata, alpha, file_size };
        Ok(Raster {
            dec,
            header,
            bands,
            bits,
            chunk_w,
            chunk_h,
            tiled,
            across: if tiled { width.div_ceil(chunk_w) } else { 1 },
            cache: HashMap::new(),
            cache_bytes: 0,
            cache_limit: cache_limit.max(1 << 20),
            tick: 0,
            unreadable: 0,
        })
    }

    /// The chunk holding pixel `(x, y)` and the pixel's position in it.
    fn locate(&self, x: u32, y: u32) -> (u32, u32, u32) {
        let (cx, cy) = (x / self.chunk_w, y / self.chunk_h);
        (cy.saturating_mul(self.across).saturating_add(cx), x - cx * self.chunk_w, y - cy * self.chunk_h)
    }

    fn chunk(&mut self, index: u32) -> Option<Arc<Chunk>> {
        self.tick += 1;
        let tick = self.tick;
        if let Some((c, used)) = self.cache.get_mut(&index) {
            *used = tick;
            return Some(c.clone());
        }
        let chunk = match self.decode(index) {
            Ok(c) => Arc::new(c),
            Err(_) => {
                self.unreadable += 1;
                // an empty chunk stands in, so a bad chunk is decoded once, not per pixel
                Arc::new(Chunk { w: 0, h: 0, rgba: Vec::new() })
            }
        };
        self.cache_bytes += chunk.rgba.len() + 64;
        self.cache.insert(index, (chunk.clone(), tick));
        while self.cache_bytes > self.cache_limit && self.cache.len() > 1 {
            let Some(oldest) = self.cache.iter().filter(|(k, _)| **k != index).min_by_key(|(_, (_, u))| *u).map(|(k, _)| *k) else { break };
            if let Some((c, _)) = self.cache.remove(&oldest) {
                self.cache_bytes = self.cache_bytes.saturating_sub(c.rgba.len() + 64);
            }
        }
        Some(chunk)
    }

    fn decode(&mut self, index: u32) -> Result<Chunk, Error> {
        let (w, h) = if self.tiled {
            let (cx, cy) = (index % self.across, index / self.across);
            (
                self.chunk_w.min(self.header.width.saturating_sub(cx * self.chunk_w)),
                self.chunk_h.min(self.header.height.saturating_sub(cy.saturating_mul(self.chunk_h))),
            )
        } else {
            (self.header.width, self.chunk_h.min(self.header.height.saturating_sub(index.saturating_mul(self.chunk_h))))
        };
        if w == 0 || h == 0 {
            return Err(Error::Invalid("chunk outside the image".into()));
        }
        let data = self.dec.read_chunk(index).map_err(|e| tiff_err("a strip or tile", e))?;
        let px = w as usize * h as usize;
        let samples: Vec<u16> = match data {
            DecodingResult::U8(v) if self.bits == 8 => v.into_iter().map(u16::from).collect(),
            DecodingResult::U16(v) if self.bits == 16 => v,
            _ => return Err(Error::Invalid("unexpected sample type".into())),
        };
        // a padded edge tile decodes at full tile size
        let (row_w, n) = if samples.len() >= px.saturating_mul(self.bands.samples()) && samples.len().is_multiple_of(px.max(1)) {
            (w as usize, samples.len() / px.max(1))
        } else {
            let full = self.chunk_w as usize * h as usize;
            if full == 0 || !samples.len().is_multiple_of(full) {
                return Err(Error::Invalid("a chunk decoded to an unexpected size".into()));
            }
            (self.chunk_w as usize, samples.len() / full)
        };
        if n < self.bands.samples() {
            return Err(Error::Invalid("a chunk has fewer samples than bands".into()));
        }
        let nodata = self.header.nodata;
        let mut rgba = vec![0u8; px * 4];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let at = (y * row_w + x) * n;
                let Some(s) = samples.get(at..at + n) else { continue };
                let shift = if self.bits == 16 { 8 } else { 0 };
                let v = |i: usize| s.get(i).copied().unwrap_or(0) >> shift;
                let [r, g, b, a] = match self.bands {
                    Bands::Gray => [v(0), v(0), v(0), 255],
                    Bands::GrayAlpha => [v(0), v(0), v(0), v(1)],
                    Bands::Rgb => [v(0), v(1), v(2), 255],
                    Bands::Rgba => [v(0), v(1), v(2), v(3)],
                    Bands::YCbCr => ycbcr(v(0), v(1), v(2)),
                };
                // no-data: every colour band holds the value
                let a = match nodata {
                    Some(nd) if !matches!(self.bands, Bands::YCbCr) => {
                        let colour = s.get(..self.bands.samples().min(3)).unwrap_or(s);
                        if colour.iter().all(|c| f64::from(*c) == nd) { 0 } else { a }
                    }
                    _ => a,
                };
                if let Some(out) = rgba.get_mut((y * w as usize + x) * 4..(y * w as usize + x) * 4 + 4) {
                    out.copy_from_slice(&[r.min(255) as u8, g.min(255) as u8, b.min(255) as u8, a.min(255) as u8]);
                }
            }
        }
        Ok(Chunk { w, h, rgba })
    }

    /// The pixel at `(x, y)`, straight RGBA; transparent outside the image or in an unreadable chunk.
    pub fn pixel(&mut self, x: i64, y: i64) -> [u8; 4] {
        if x < 0 || y < 0 || x >= i64::from(self.header.width) || y >= i64::from(self.header.height) {
            return [0; 4];
        }
        let (index, cx, cy) = self.locate(x as u32, y as u32);
        let Some(c) = self.chunk(index) else { return [0; 4] };
        if cx >= c.w || cy >= c.h {
            return [0; 4];
        }
        let at = (cy as usize * c.w as usize + cx as usize) * 4;
        c.rgba.get(at..at + 4).and_then(|s| <[u8; 4]>::try_from(s).ok()).unwrap_or([0; 4])
    }

    /// Bilinear sample at pixel-edge coordinates (`(0.5, 0.5)` is the centre of the top-left
    /// pixel), premultiplied RGBA as `f32` in 0..255.
    pub fn sample(&mut self, col: f64, row: f64) -> [f32; 4] {
        if !(col.is_finite() && row.is_finite())
            || col < -1.0
            || row < -1.0
            || col > f64::from(self.header.width) + 1.0
            || row > f64::from(self.header.height) + 1.0
        {
            return [0.0; 4];
        }
        let (x, y) = (col - 0.5, row - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let mut out = [0f32; 4];
        for (dx, dy, w) in [(0, 0, (1.0 - fx) * (1.0 - fy)), (1, 0, fx * (1.0 - fy)), (0, 1, (1.0 - fx) * fy), (1, 1, fx * fy)] {
            if w <= 0.0 {
                continue;
            }
            let p = self.pixel(x0 + dx, y0 + dy);
            let a = f32::from(p[3]) / 255.0;
            out[0] += w * f32::from(p[0]) * a;
            out[1] += w * f32::from(p[1]) * a;
            out[2] += w * f32::from(p[2]) * a;
            out[3] += w * f32::from(p[3]);
        }
        out
    }
}

/// JFIF YCbCr → RGB (full range).
fn ycbcr(y: u16, cb: u16, cr: u16) -> [u16; 4] {
    let (y, cb, cr) = (f32::from(y), f32::from(cb) - 128.0, f32::from(cr) - 128.0);
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u16;
    [c(y + 1.402 * cr), c(y - 0.344_136 * cb - 0.714_136 * cr), c(y + 1.772 * cb), 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ycbcr_greys_stay_grey() {
        assert_eq!(ycbcr(128, 128, 128), [128, 128, 128, 255]);
        assert_eq!(ycbcr(255, 128, 128), [255, 255, 255, 255]);
        assert_eq!(ycbcr(0, 0, 255)[0], 178);
    }
}
