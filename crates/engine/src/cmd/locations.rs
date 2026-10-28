//! Export photo locations (`photo.exportLocations`) as KML 2.2 (Google Earth) or GeoJSON
//! (RFC 7946, for GIS), for inspection reports: one point per photo with a usable position, in
//! capture order, and optionally a track line through them.
//!
//! Sources: OGC KML 2.2 (07-147r2) and RFC 7946 (prose); own design.

use serde_json::{Map, Value, json};

use lightcraft_catalog::{Marker, Photo, PhotoId, Source};

use super::{CommandSpec, always, bad, bool_or, cmd, str_param};
use crate::{EngineError, Result, Session};

const C: &str = "photo.exportLocations";

/// The two file formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Kml,
    GeoJson,
}

impl Format {
    fn parse(s: &str) -> Option<Format> {
        match s.trim().to_ascii_lowercase().as_str() {
            "kml" => Some(Format::Kml),
            "geojson" | "json" => Some(Format::GeoJson),
            _ => None,
        }
    }

    /// From a file name's extension (`.kml`, `.geojson`, `.json`).
    fn of_path(path: &str) -> Option<Format> {
        std::path::Path::new(path).extension().and_then(|e| e.to_str()).and_then(Format::parse)
    }

    fn name(self) -> &'static str {
        match self {
            Format::Kml => "kml",
            Format::GeoJson => "geojson",
        }
    }
}

/// A usable latitude/longitude: finite, in range, and not the 0, 0 some cameras write for "no
/// fix" (the Map view's rule).
pub fn usable((lat, lon): (f64, f64)) -> bool {
    lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0 && !(lat == 0.0 && lon == 0.0)
}

/// One photo's place in the export.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub id: PhotoId,
    /// The photo's title, else its file name.
    pub name: String,
    pub file_name: String,
    /// The original's path (none for demo photos).
    pub path: Option<String>,
    /// ISO 8601 capture time, when the photo has one that reads.
    pub captured: Option<String>,
    /// The sort key: the capture time, else the import time (as the Map view orders its pins).
    sort_key: String,
    pub lat: f64,
    pub lon: f64,
    /// Metres above sea level.
    pub altitude: Option<f64>,
    /// Degrees clockwise from north, and whether from magnetic north.
    pub heading: Option<f64>,
    pub magnetic: bool,
    /// How far off the position may be (metres; `GPSHPositioningError`).
    pub accuracy: Option<f64>,
}

/// Rounded to `places` decimals (headings are f32 in the catalog: 87.3 not 87.30000305).
fn round(v: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (v * k).round() / k
}

impl Point {
    /// The photo's point, or `None` when it has no usable location.
    pub fn of(p: &Photo) -> Option<Point> {
        let (lat, lon) = p.meta.gps.filter(|g| usable(*g))?;
        let title = p.meta.title.trim();
        let captured = p.captured.as_deref().and_then(lightcraft_meta::DateTime::parse_iso).map(|d| d.to_iso());
        Some(Point {
            id: p.id,
            name: if title.is_empty() { p.file_name.clone() } else { title.to_string() },
            file_name: p.file_name.clone(),
            path: match &p.source {
                Source::File { path } => Some(path.clone()),
                Source::Demo { .. } => None,
            },
            captured,
            sort_key: p.date().to_string(),
            lat,
            lon,
            altitude: p.meta.gps_altitude.filter(|a| a.is_finite()).map(|a| round(a, 3)),
            heading: p.meta.gps_direction.map(f64::from).filter(|d| d.is_finite()).map(|d| round(d, 2)),
            magnetic: p.meta.gps_direction_magnetic && p.meta.gps_direction.is_some(),
            accuracy: p.meta.gps_accuracy.filter(|m| m.is_finite() && *m > 0.0).map(|m| round(m, 2)),
        })
    }

    /// The point's properties, in order, for both formats (GeoJSON `properties`, KML
    /// `ExtendedData`). Add a new field here and both files carry it.
    pub fn properties(&self) -> Vec<(&'static str, Value)> {
        let mut v = vec![("kind", json!("photo")), ("name", json!(self.name)), ("id", json!(self.id.0)), ("fileName", json!(self.file_name))];
        if let Some(c) = &self.captured {
            v.push(("captured", json!(c)));
        }
        v.push(("latitude", json!(self.lat)));
        v.push(("longitude", json!(self.lon)));
        if let Some(a) = self.altitude {
            v.push(("altitude", json!(a)));
        }
        if let Some(h) = self.heading {
            v.push(("heading", json!(h)));
            v.push(("headingRef", json!(if self.magnetic { "magnetic" } else { "true" })));
        }
        if let Some(m) = self.accuracy {
            v.push(("accuracy", json!(m)));
        }
        if let Some(p) = &self.path {
            v.push(("path", json!(p)));
        }
        v
    }
}

/// The points of `ids` in capture order, and how many of them have no usable location.
pub fn points(s: &Session, ids: &[PhotoId]) -> (Vec<Point>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0;
    for id in ids {
        match s.catalog.photo(*id).and_then(|p| Point::of(p)) {
            Some(p) => out.push(p),
            None => skipped += 1,
        }
    }
    out.sort_by(|a, b| a.sort_key.cmp(&b.sort_key).then_with(|| a.file_name.cmp(&b.file_name)).then(a.id.0.cmp(&b.id.0)));
    (out, skipped)
}

/// Text for XML element content and attribute values: the five special characters escaped,
/// characters XML 1.0 forbids (control characters other than tab and line breaks) left out.
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// A map marker in the export (`kind: marker`): its point, title, note and what is attached.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkerPoint {
    pub id: u64,
    /// The marker's title, else "Marker <id>".
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub note: String,
    /// The attached files' names.
    pub attachments: Vec<String>,
    /// The photos attached at the marker.
    pub photos: Vec<u64>,
}

impl MarkerPoint {
    pub fn of(m: &Marker) -> Option<MarkerPoint> {
        if !usable((m.lat, m.lon)) {
            return None;
        }
        let title = m.title.trim();
        Some(MarkerPoint {
            id: m.id.0,
            name: if title.is_empty() { format!("Marker {}", m.id.0) } else { title.to_string() },
            lat: m.lat,
            lon: m.lon,
            note: m.note.clone(),
            attachments: m.attachments.iter().map(|a| a.file_name.clone()).collect(),
            photos: m.photos.iter().map(|p| p.0).collect(),
        })
    }

    /// Its properties, in order, for both formats (lists are written `; `-joined in KML).
    pub fn properties(&self) -> Vec<(&'static str, Value)> {
        let mut v = vec![("kind", json!("marker")), ("name", json!(self.name)), ("id", json!(self.id))];
        if !self.note.is_empty() {
            v.push(("note", json!(self.note)));
        }
        v.push(("latitude", json!(self.lat)));
        v.push(("longitude", json!(self.lon)));
        if !self.attachments.is_empty() {
            v.push(("attachments", json!(self.attachments)));
        }
        if !self.photos.is_empty() {
            v.push(("photos", json!(self.photos)));
        }
        v
    }
}

/// The library's markers with a usable position, by id.
pub fn marker_points(s: &Session) -> Vec<MarkerPoint> {
    s.catalog.markers().filter_map(MarkerPoint::of).collect()
}

/// A property's text in KML `ExtendedData`: numbers as the coordinates print them (118, not
/// 118.0), lists joined with `; `.
fn kml_text(value: Value) -> String {
    match value {
        Value::String(t) => t,
        Value::Number(n) if n.is_f64() => n.as_f64().map_or_else(|| n.to_string(), |f| f.to_string()),
        Value::Array(a) => a.into_iter().map(kml_text).collect::<Vec<_>>().join("; "),
        v => v.to_string(),
    }
}

/// The track has altitudes only when every point has one (a mix would drop some to sea level).
fn track_has_altitude(pts: &[Point]) -> bool {
    pts.iter().all(|p| p.altitude.is_some())
}

/// KML 2.2: a Folder of photo Placemarks (TimeStamp, ExtendedData), a Folder of the map's
/// markers when there are any and, with `track`, a Placemark with the LineString through the
/// photos.
pub fn to_kml(pts: &[Point], markers: &[MarkerPoint], title: &str, track: bool) -> String {
    let mut k = String::new();
    k.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<kml xmlns=\"http://www.opengis.net/kml/2.2\">\n<Document>\n");
    k.push_str(&format!("  <name>{}</name>\n", xml_escape(title)));
    k.push_str("  <Style id=\"track\"><LineStyle><color>ff00a5ff</color><width>3</width></LineStyle></Style>\n");
    k.push_str("  <Folder>\n    <name>Photos</name>\n");
    for p in pts {
        k.push_str("    <Placemark>\n");
        k.push_str(&format!("      <name>{}</name>\n", xml_escape(&p.name)));
        if let Some(c) = &p.captured {
            k.push_str(&format!("      <TimeStamp><when>{}</when></TimeStamp>\n", xml_escape(c)));
        }
        k.push_str("      <ExtendedData>\n");
        for (name, value) in p.properties() {
            if name == "name" {
                continue;
            }
            k.push_str(&format!("        <Data name=\"{name}\"><value>{}</value></Data>\n", xml_escape(&kml_text(value))));
        }
        k.push_str("      </ExtendedData>\n");
        match p.altitude {
            Some(a) => {
                k.push_str(&format!("      <Point><altitudeMode>absolute</altitudeMode><coordinates>{},{},{a}</coordinates></Point>\n", p.lon, p.lat))
            }
            None => k.push_str(&format!("      <Point><coordinates>{},{}</coordinates></Point>\n", p.lon, p.lat)),
        }
        k.push_str("    </Placemark>\n");
    }
    k.push_str("  </Folder>\n");
    if !markers.is_empty() {
        k.push_str("  <Folder>\n    <name>Markers</name>\n");
        for m in markers {
            k.push_str("    <Placemark>\n");
            k.push_str(&format!("      <name>{}</name>\n", xml_escape(&m.name)));
            if !m.note.is_empty() {
                k.push_str(&format!("      <description>{}</description>\n", xml_escape(&m.note)));
            }
            k.push_str("      <ExtendedData>\n");
            for (name, value) in m.properties().into_iter().filter(|(n, _)| *n != "name") {
                k.push_str(&format!("        <Data name=\"{name}\"><value>{}</value></Data>\n", xml_escape(&kml_text(value))));
            }
            k.push_str("      </ExtendedData>\n");
            k.push_str(&format!("      <Point><coordinates>{},{}</coordinates></Point>\n", m.lon, m.lat));
            k.push_str("    </Placemark>\n");
        }
        k.push_str("  </Folder>\n");
    }
    if track && pts.len() >= 2 {
        let with_alt = track_has_altitude(pts);
        k.push_str("  <Placemark>\n    <name>Track</name>\n    <styleUrl>#track</styleUrl>\n");
        k.push_str("    <ExtendedData><Data name=\"kind\"><value>track</value></Data>");
        k.push_str(&format!("<Data name=\"photos\"><value>{}</value></Data></ExtendedData>\n", pts.len()));
        k.push_str("    <LineString>\n");
        k.push_str(if with_alt { "      <altitudeMode>absolute</altitudeMode>\n" } else { "      <tessellate>1</tessellate>\n" });
        k.push_str("      <coordinates>\n");
        for p in pts {
            match (with_alt, p.altitude) {
                (true, Some(a)) => k.push_str(&format!("        {},{},{a}\n", p.lon, p.lat)),
                _ => k.push_str(&format!("        {},{}\n", p.lon, p.lat)),
            }
        }
        k.push_str("      </coordinates>\n    </LineString>\n  </Placemark>\n");
    }
    k.push_str("</Document>\n</kml>\n");
    k
}

/// RFC 7946 position: longitude, latitude, then altitude when known.
fn position(p: &Point, with_alt: bool) -> Value {
    match (with_alt, p.altitude) {
        (true, Some(a)) => json!([p.lon, p.lat, a]),
        _ => json!([p.lon, p.lat]),
    }
}

/// GeoJSON (RFC 7946): a FeatureCollection of Point features and, with `track`, a LineString
/// feature whose `kind` is `track` (the photos' `kind` is `photo`, the markers' `marker`).
pub fn to_geojson(pts: &[Point], markers: &[MarkerPoint], title: &str, track: bool) -> Value {
    let mut features: Vec<Value> = pts
        .iter()
        .map(|p| {
            let props: Map<String, Value> = p.properties().into_iter().map(|(k, v)| (k.to_string(), v)).collect();
            json!({"type": "Feature", "geometry": {"type": "Point", "coordinates": position(p, true)}, "properties": props})
        })
        .collect();
    features.extend(markers.iter().map(|m| {
        let props: Map<String, Value> = m.properties().into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        json!({"type": "Feature", "geometry": {"type": "Point", "coordinates": [m.lon, m.lat]}, "properties": props})
    }));
    if track && pts.len() >= 2 {
        let with_alt = track_has_altitude(pts);
        let line: Vec<Value> = pts.iter().map(|p| position(p, with_alt)).collect();
        features.push(json!({
            "type": "Feature",
            "geometry": {"type": "LineString", "coordinates": line},
            "properties": {"kind": "track", "name": "Track", "photos": pts.len()},
        }));
    }
    json!({"type": "FeatureCollection", "name": title, "features": features})
}

/// The photos a call means: `ids` / `id`, else a selection of several photos, else the photos
/// shown (as `cull.*` does: one selected photo is just the active one).
fn export_targets(s: &mut Session, p: &Value) -> Result<Vec<PhotoId>> {
    if super::names_photos(p) || s.selection.ids.len() > 1 { s.targets(p) } else { Ok(s.visible_cloned()) }
}

fn export_locations(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").map(str::trim).filter(|x| !x.is_empty());
    // an original or its sidecar is refused before anything else is said about the path
    if let Some(path) = path {
        s.check_write_target(path).map_err(|e| bad(C, e))?;
    }
    let format = match (str_param(p, "format"), path) {
        (Some(f), _) => Format::parse(f).ok_or_else(|| bad(C, format!("unknown format `{f}` (kml or geojson)")))?,
        (None, Some(path)) => Format::of_path(path).ok_or_else(|| bad(C, "give `format` (kml or geojson) or a .kml / .geojson path"))?,
        (None, None) => Format::GeoJson,
    };
    let track = bool_or(p, "track", true);
    let ids = export_targets(s, p)?;
    let (pts, skipped) = points(s, &ids);
    let markers = if bool_or(p, "markers", true) { marker_points(s) } else { Vec::new() };
    if pts.is_empty() && markers.is_empty() {
        return Err(bad(C, format!("none of the {} photo(s) has a location", ids.len())));
    }
    let title = str_param(p, "name").map(str::trim).filter(|t| !t.is_empty()).unwrap_or("Photo locations");
    let text = match format {
        Format::Kml => to_kml(&pts, &markers, title, track),
        Format::GeoJson => {
            let mut t = serde_json::to_string_pretty(&to_geojson(&pts, &markers, title, track)).map_err(|e| EngineError::Other(e.to_string()))?;
            t.push('\n');
            t
        }
    };
    let mut r =
        json!({"written": pts.len(), "markers": markers.len(), "skipped": skipped, "format": format.name(), "track": track && pts.len() >= 2});
    match path {
        Some(path) => {
            lightcraft_catalog::safe_file::write_atomic(std::path::Path::new(path), text.as_bytes())
                .map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
            r["path"] = json!(path);
        }
        None => r["text"] = json!(text),
    }
    Ok(r)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        query "photo.exportLocations",
        "Export Photo Locations",
        [],
        None,
        "{path?: .kml | .geojson file (none: the text is returned), format?: kml | geojson (default: from the path's extension), ids? (default: the selection when several photos are selected, else the photos shown), track?: bool (true: a line through the points in capture order, `kind: track`), markers?: bool (true: the Map view's markers too, `kind: marker`, with title, note, attachment file names and photo ids), name?: the document's name} → {written, markers, skipped: photos without a usable location, format, track, path | text} — one point per photo in capture order: name (title, else file name), capture time, lat/lon, altitude, heading (and whether magnetic), accuracy (metres), file path",
        always,
        export_locations
    )]
}
