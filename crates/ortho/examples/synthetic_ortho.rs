//! Write a synthetic drone orthomosaic for demos and screenshots: a bridge deck (lanes, a median,
//! sidewalks, orange railings, expansion joints and numbered panels every 15 m) over water, 3 cm
//! a pixel, UTM zone 10 N, centred on the Golden Gate Bridge's mid-span, with a ragged
//! transparent edge as a drone survey has. Procedural: no media.
//!
//! `cargo run -p lightcraft-ortho --features testdata --example synthetic_ortho -- OUT.tif [SIZE]`

use lightcraft_ortho::Crs;
use lightcraft_ortho::testdata::{Spec, write_geotiff};

const RES: f64 = 0.03;

/// A 7-segment digit, `h` pixels high, at (x0, y0): is (x, y) lit?
fn digit(d: u32, x: f64, y: f64, h: f64) -> bool {
    let w = h * 0.55;
    let t = h * 0.13;
    if x < 0.0 || y < 0.0 || x > w || y > h {
        return false;
    }
    // segments a..g
    let seg = [
        y < t && x > t * 0.5 && x < w - t * 0.5,                         // a top
        x > w - t && y > t * 0.5 && y < h / 2.0,                         // b upper right
        x > w - t && y > h / 2.0 && y < h - t * 0.5,                     // c lower right
        y > h - t && x > t * 0.5 && x < w - t * 0.5,                     // d bottom
        x < t && y > h / 2.0 && y < h - t * 0.5,                         // e lower left
        x < t && y > t * 0.5 && y < h / 2.0,                             // f upper left
        (y - h / 2.0).abs() < t / 2.0 && x > t * 0.5 && x < w - t * 0.5, // g middle
    ];
    let on: [u8; 10] = [0b0111111, 0b0000110, 0b1011011, 0b1001111, 0b1100110, 0b1101101, 0b1111101, 0b0000111, 0b1111111, 0b1101111];
    let mask = on[(d % 10) as usize];
    seg.iter().enumerate().any(|(i, s)| *s && mask & (1 << i) != 0)
}

fn hash(x: u32, y: u32) -> u32 {
    let mut h = x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^ (h >> 16)
}

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "synthetic-ortho.tif".into());
    let size: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(4000);
    let (e, n) = Crs::from_epsg(32610)
        .and_then(|c| c.from_lat_lon(37.8199, -122.4783).ok_or_else(|| "no place".to_string()))
        .map_err(std::io::Error::other)?;
    let half = f64::from(size) * RES / 2.0;
    let spec = Spec { bigtiff: size > 8000, ..Spec::utm(size, size, 32610, RES, (e - half, n + half)) };
    let c = f64::from(size) / 2.0;
    let px = |m: f64| m / RES; // metres → pixels
    let started = std::time::Instant::now();
    write_geotiff(std::path::Path::new(&out), &spec, |x, y| {
        let (fx, fy) = (f64::from(x), f64::from(y));
        // a ragged survey edge: transparent outside a wobbly ellipse
        let (dx, dy) = ((fx - c) / c, (fy - c) / c);
        let wobble = 0.93 + 0.04 * (fy / 157.0).sin() + 0.03 * (fx / 211.0).cos();
        if dx * dx * 0.55 + dy * dy * 0.9 > wobble * wobble {
            return [0, 0, 0, 0];
        }
        let n = (hash(x, y) % 17) as i32 - 8;
        let shade =
            |rgb: [i32; 3], k: i32| [(rgb[0] + k).clamp(0, 255) as u8, (rgb[1] + k).clamp(0, 255) as u8, (rgb[2] + k).clamp(0, 255) as u8, 255];
        // across the deck, metres from its centre line; along it, metres from the image's top
        let across = (fx - c) * RES;
        let along = fy * RES;
        let deck = 13.5;
        if across.abs() > deck + 0.4 {
            // water with swell
            let wave = ((fx * 0.021 + fy * 0.013).sin() * 6.0 + (fx * 0.005 - fy * 0.017).cos() * 5.0) as i32;
            return shade([24, 66, 84], wave + n / 2);
        }
        if across.abs() > deck {
            // the railings: International Orange, posts every 1.2 m
            let post = (along % 1.2) < 0.12;
            return shade(if post { [150, 40, 22] } else { [192, 54, 30] }, n);
        }
        let panel = (along / 15.0).floor() as u32 + 1;
        let in_panel = along % 15.0;
        // expansion joints between panels
        if in_panel < 0.08 {
            return shade([38, 38, 40], n);
        }
        if across.abs() > deck - 3.0 {
            // sidewalks, with the panel number painted on the east one
            let walk = shade([168, 164, 156], n);
            if across > 0.0 {
                let h = px(1.6);
                let (ox, oy) = (c + px(deck - 2.6), px((panel as f64 - 1.0) * 15.0 + 1.2));
                let tens = digit(panel / 10, fx - ox, fy - oy, h);
                let ones = digit(panel % 10, fx - ox - h * 0.75, fy - oy, h);
                if tens || ones {
                    return [250, 210, 40, 255];
                }
            }
            return walk;
        }
        // the median barrier
        if across.abs() < 0.35 {
            return shade([214, 190, 60], n);
        }
        // lane lines: dashed white every 3.5 m
        let lane = (across.abs() - 0.35) % 3.6;
        if lane < 0.15 && (along % 9.0) < 3.0 && across.abs() > 1.0 {
            return shade([236, 236, 230], n / 2);
        }
        // asphalt, darker wheel tracks, a few patches
        let track = ((lane - 1.0).abs() < 0.35 || (lane - 2.6).abs() < 0.35) as i32 * -10;
        let patch = hash(x / 120, y / 120).is_multiple_of(23) as i32 * 18;
        shade([82, 84, 88], n + track + patch)
    })?;
    let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    println!("{out}: {size} × {size} px, {:.0} m, {} MB, {:.1} s", f64::from(size) * RES, bytes >> 20, started.elapsed().as_secs_f64());
    Ok(())
}
