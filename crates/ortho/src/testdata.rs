//! A minimal GeoTIFF writer for tests and demos (feature `testdata`): uncompressed 8-bit RGB or
//! RGBA in tiles or strips, classic TIFF or BigTIFF, with the GeoTIFF tags, written a chunk at a
//! time from a pixel function (so a large synthetic orthomosaic needs no memory). Media is never
//! committed: tests build their files with this.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

/// What to write.
#[derive(Clone, Debug)]
pub struct Spec {
    pub width: u32,
    pub height: u32,
    /// Tile size; `None`: strips of `rows_per_strip`.
    pub tile: Option<(u32, u32)>,
    pub rows_per_strip: u32,
    /// 3 (RGB) or 4 (RGBA, unassociated alpha).
    pub bands: u8,
    pub bigtiff: bool,
    /// EPSG code of the CRS (projected unless `geographic`).
    pub epsg: u16,
    pub geographic: bool,
    /// Pixel size in model units (x, y).
    pub pixel_scale: (f64, f64),
    /// Model coordinates of the top-left corner of the top-left pixel.
    pub origin: (f64, f64),
    /// An affine `ModelTransformation` instead of scale + tiepoint.
    pub transform: Option<[f64; 16]>,
    /// `GDAL_NODATA`.
    pub nodata: Option<String>,
    /// Also write a half-size reduced-resolution image (an internal overview).
    pub overview: bool,
    /// Point every strip or tile 1 GB past where it is (hostile-file tests).
    pub corrupt_offsets: bool,
}

impl Spec {
    /// RGBA tiles of 256 in UTM `epsg`, `res` metres a pixel, top-left corner at `origin`.
    pub fn utm(width: u32, height: u32, epsg: u16, res: f64, origin: (f64, f64)) -> Spec {
        Spec {
            width,
            height,
            tile: Some((256, 256)),
            rows_per_strip: 16,
            bands: 4,
            bigtiff: false,
            epsg,
            geographic: false,
            pixel_scale: (res, res),
            origin,
            transform: None,
            nodata: None,
            overview: false,
            corrupt_offsets: false,
        }
    }
}

enum V {
    Short(Vec<u16>),
    Long(Vec<u64>),
    Double(Vec<f64>),
    Ascii(String),
}

struct W {
    big: bool,
}

impl W {
    fn type_and_bytes(&self, v: &V) -> (u16, u64, Vec<u8>) {
        match v {
            V::Short(s) => (3, s.len() as u64, s.iter().flat_map(|x| x.to_le_bytes()).collect()),
            V::Long(s) if self.big => (16, s.len() as u64, s.iter().flat_map(|x| x.to_le_bytes()).collect()),
            V::Long(s) => (4, s.len() as u64, s.iter().flat_map(|x| (*x as u32).to_le_bytes()).collect()),
            V::Double(s) => (12, s.len() as u64, s.iter().flat_map(|x| x.to_le_bytes()).collect()),
            V::Ascii(t) => {
                let mut b = t.as_bytes().to_vec();
                b.push(0);
                (2, b.len() as u64, b)
            }
        }
    }

    /// Write an IFD at the end of `f`; returns its offset and where its next-IFD pointer is.
    fn ifd(&self, f: &mut (impl Write + Seek), mut entries: Vec<(u16, V)>) -> std::io::Result<(u64, u64)> {
        entries.sort_by_key(|e| e.0);
        let mut at = f.seek(SeekFrom::End(0))?;
        if at % 2 == 1 {
            f.write_all(&[0])?;
            at += 1;
        }
        let (count_size, entry_size, off_size) = if self.big { (8u64, 20u64, 8usize) } else { (2, 12, 4) };
        let table = count_size + entry_size * entries.len() as u64 + off_size as u64;
        let mut extra: Vec<u8> = Vec::new();
        let mut out: Vec<u8> = Vec::new();
        if self.big {
            out.extend_from_slice(&(entries.len() as u64).to_le_bytes());
        } else {
            out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        }
        for (tag, v) in &entries {
            let (typ, count, bytes) = self.type_and_bytes(v);
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&typ.to_le_bytes());
            if self.big {
                out.extend_from_slice(&count.to_le_bytes());
            } else {
                out.extend_from_slice(&(count as u32).to_le_bytes());
            }
            if bytes.len() <= off_size {
                let mut b = bytes.clone();
                b.resize(off_size, 0);
                out.extend_from_slice(&b);
            } else {
                let off = at + table + extra.len() as u64;
                if self.big {
                    out.extend_from_slice(&off.to_le_bytes());
                } else {
                    out.extend_from_slice(&(off as u32).to_le_bytes());
                }
                extra.extend_from_slice(&bytes);
                if extra.len() % 2 == 1 {
                    extra.push(0);
                }
            }
        }
        out.extend_from_slice(&vec![0u8; off_size]); // next IFD, patched by the caller
        out.extend_from_slice(&extra);
        f.write_all(&out)?;
        Ok((at, at + table - off_size as u64))
    }
}

/// Write `spec` to `path`, each pixel from `pixel(x, y)` (RGBA; alpha dropped for 3 bands).
pub fn write_geotiff(path: &Path, spec: &Spec, pixel: impl Fn(u32, u32) -> [u8; 4]) -> std::io::Result<()> {
    let mut f = BufWriter::new(File::create(path)?);
    let w = W { big: spec.bigtiff };
    if spec.bigtiff {
        f.write_all(b"II\x2b\x00\x08\x00\x00\x00")?;
        f.write_all(&0u64.to_le_bytes())?;
    } else {
        f.write_all(b"II\x2a\x00")?;
        f.write_all(&0u32.to_le_bytes())?;
    }
    let main = image(&mut f, spec, 1, &pixel)?;
    let (ifd0, next) = w.ifd(&mut f, main)?;
    patch(&mut f, &w, if spec.bigtiff { 8 } else { 4 }, ifd0)?;
    if spec.overview {
        let entries = image(&mut f, spec, 2, &pixel)?;
        let (ifd1, _) = w.ifd(&mut f, entries)?;
        patch(&mut f, &w, next, ifd1)?;
    }
    f.flush()?;
    Ok(())
}

fn patch(f: &mut (impl Write + Seek), w: &W, at: u64, value: u64) -> std::io::Result<()> {
    f.seek(SeekFrom::Start(at))?;
    if w.big { f.write_all(&value.to_le_bytes()) } else { f.write_all(&(value as u32).to_le_bytes()) }?;
    f.seek(SeekFrom::End(0))?;
    Ok(())
}

/// Write the pixel data of the image at `1/reduce` scale; returns its IFD entries.
fn image(f: &mut (impl Write + Seek), spec: &Spec, reduce: u32, pixel: &impl Fn(u32, u32) -> [u8; 4]) -> std::io::Result<Vec<(u16, V)>> {
    let (iw, ih) = (spec.width.div_ceil(reduce), spec.height.div_ceil(reduce));
    let bands = usize::from(spec.bands.clamp(3, 4));
    let mut offsets = Vec::new();
    let mut counts = Vec::new();
    let px = |x: u32, y: u32, buf: &mut Vec<u8>| {
        let p = if x < iw && y < ih { pixel((x * reduce).min(spec.width - 1), (y * reduce).min(spec.height - 1)) } else { [0; 4] };
        buf.extend_from_slice(&p[..bands]);
    };
    let (cw, ch) = spec.tile.unwrap_or((iw, spec.rows_per_strip.clamp(1, ih)));
    let (across, down) = if spec.tile.is_some() { (iw.div_ceil(cw), ih.div_ceil(ch)) } else { (1, ih.div_ceil(ch)) };
    for cy in 0..down {
        for cx in 0..across {
            let mut buf = Vec::with_capacity(cw as usize * ch as usize * bands);
            let rows = if spec.tile.is_some() { ch } else { ch.min(ih - cy * ch) };
            for y in 0..rows {
                for x in 0..cw {
                    px(cx * cw + x, cy * ch + y, &mut buf);
                }
            }
            let at = f.seek(SeekFrom::End(0))?;
            f.write_all(&buf)?;
            offsets.push(if spec.corrupt_offsets { at + (1 << 30) } else { at });
            counts.push(buf.len() as u64);
        }
    }
    let mut e: Vec<(u16, V)> = vec![
        (256, V::Long(vec![u64::from(iw)])),
        (257, V::Long(vec![u64::from(ih)])),
        (258, V::Short(vec![8; bands])),
        (259, V::Short(vec![1])),
        (262, V::Short(vec![2])),
        (277, V::Short(vec![bands as u16])),
        (284, V::Short(vec![1])),
        (339, V::Short(vec![1; bands])),
    ];
    if reduce > 1 {
        e.push((254, V::Long(vec![1])));
    }
    if bands == 4 {
        e.push((338, V::Short(vec![2])));
    }
    if spec.tile.is_some() {
        e.push((322, V::Long(vec![u64::from(cw)])));
        e.push((323, V::Long(vec![u64::from(ch)])));
        e.push((324, V::Long(offsets)));
        e.push((325, V::Long(counts)));
    } else {
        e.push((273, V::Long(offsets)));
        e.push((278, V::Long(vec![u64::from(ch)])));
        e.push((279, V::Long(counts)));
    }
    let r = f64::from(reduce);
    if let Some(t) = spec.transform {
        let mut t = t;
        t[0] *= r;
        t[1] *= r;
        t[4] *= r;
        t[5] *= r;
        e.push((34264, V::Double(t.to_vec())));
    } else {
        e.push((33550, V::Double(vec![spec.pixel_scale.0 * r, spec.pixel_scale.1 * r, 0.0])));
        e.push((33922, V::Double(vec![0.0, 0.0, 0.0, spec.origin.0, spec.origin.1, 0.0])));
    }
    let keys: Vec<u16> = if spec.geographic {
        vec![1, 1, 0, 3, 1024, 0, 1, 2, 1025, 0, 1, 1, 2048, 0, 1, spec.epsg]
    } else {
        vec![1, 1, 0, 3, 1024, 0, 1, 1, 1025, 0, 1, 1, 3072, 0, 1, spec.epsg]
    };
    e.push((34735, V::Short(keys)));
    if let Some(nd) = &spec.nodata {
        e.push((42113, V::Ascii(nd.clone())));
    }
    Ok(e)
}
