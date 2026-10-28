//! `photo.exportLocations`: photo locations as KML and GeoJSON.

use serde_json::{Value, json};

use lightcraft_catalog::{Op, PhotoId};

use crate::Session;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-locations-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Give photo `id` a position, altitude, heading, title and capture time (a photo with an
/// altitude also gets a ±6.5 m accuracy).
fn place(s: &mut Session, id: u64, gps: Option<(f64, f64)>, alt: Option<f64>, heading: Option<(f32, bool)>, title: &str, captured: &str) {
    let id = PhotoId(id);
    let mut meta = s.catalog.photo(id).unwrap().meta.clone();
    meta.gps = gps;
    meta.gps_altitude = alt;
    meta.gps_accuracy = alt.map(|_| 6.5);
    meta.gps_direction = heading.map(|h| h.0);
    meta.gps_direction_magnetic = heading.is_some_and(|h| h.1);
    meta.title = title.into();
    s.commit("test", Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
    s.commit("test", Op::SetCaptured { id, captured: Some(captured.into()) }).unwrap();
}

/// Four demo photos: two with full locations (out of capture order), one at 0, 0 and one with
/// no location at all.
fn session() -> (Session, Vec<u64>) {
    let mut s = Session::with_demo();
    let ids: Vec<u64> = s.visible_cloned().iter().take(4).map(|i| i.0).collect();
    place(&mut s, ids[0], Some((40.5, -75.25)), Some(120.5), Some((87.3, true)), "Pier 2 <north> & \"east\"", "2026-05-01T10:05:00");
    place(&mut s, ids[1], Some((40.4, -75.2)), Some(118.0), None, "", "2026-05-01T10:00:00");
    place(&mut s, ids[2], Some((0.0, 0.0)), None, None, "", "2026-05-01T09:00:00");
    place(&mut s, ids[3], None, None, None, "", "2026-05-01T09:30:00");
    (s, ids)
}

/// GeoJSON: a FeatureCollection of Points in capture order with [lon, lat, alt] coordinates, the
/// properties, a `track` LineString, and the photos without a usable location counted.
#[test]
fn geojson_has_points_in_capture_order_and_a_track() {
    let (mut s, ids) = session();
    let dir = temp_dir("geojson");
    let path = dir.join("sites.geojson");
    let r = s.execute("photo.exportLocations", &json!({"ids": ids, "path": path.to_string_lossy()})).unwrap();
    assert_eq!((r["written"].as_u64(), r["skipped"].as_u64(), r["format"].as_str()), (Some(2), Some(2), Some("geojson")), "{r}");
    let g: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(g["type"], "FeatureCollection");
    let f = g["features"].as_array().unwrap();
    assert_eq!(f.len(), 3);
    // the earlier photo first; longitude before latitude, then altitude
    assert_eq!(f[0]["geometry"], json!({"type": "Point", "coordinates": [-75.2, 40.4, 118.0]}));
    assert_eq!(f[1]["geometry"]["coordinates"], json!([-75.25, 40.5, 120.5]));
    let p = &f[1]["properties"];
    assert_eq!(p["kind"], "photo");
    assert_eq!(p["name"], "Pier 2 <north> & \"east\"");
    assert_eq!(p["captured"], "2026-05-01T10:05:00");
    assert_eq!(p["altitude"], 120.5);
    assert_eq!(p["heading"], 87.3);
    assert_eq!(p["headingRef"], "magnetic");
    assert_eq!(p["accuracy"], 6.5);
    assert_eq!(p["id"], ids[0]);
    // an untitled photo is named by its file
    let untitled = s.catalog.photo(PhotoId(ids[1])).unwrap().file_name.clone();
    assert_eq!(f[0]["properties"]["name"], json!(untitled));
    assert!(f[0]["properties"].get("heading").is_none());
    // the track, told apart by its kind
    assert_eq!(f[2]["properties"]["kind"], "track");
    assert_eq!(f[2]["geometry"], json!({"type": "LineString", "coordinates": [[-75.2, 40.4, 118.0], [-75.25, 40.5, 120.5]]}));
    // without the track
    let r = s.execute("photo.exportLocations", &json!({"ids": ids, "track": false})).unwrap();
    let g: Value = serde_json::from_str(r["text"].as_str().unwrap()).unwrap();
    assert_eq!(g["features"].as_array().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// KML 2.2: escaped names, `lon,lat,alt` coordinates, TimeStamp, ExtendedData and the track.
#[test]
fn kml_escapes_names_and_writes_lon_lat_alt() {
    let (mut s, ids) = session();
    let dir = temp_dir("kml");
    let path = dir.join("sites.kml");
    let r = s.execute("photo.exportLocations", &json!({"ids": ids, "path": path.to_string_lossy()})).unwrap();
    assert_eq!((r["written"].as_u64(), r["skipped"].as_u64(), r["format"].as_str()), (Some(2), Some(2), Some("kml")), "{r}");
    let k = std::fs::read_to_string(&path).unwrap();
    assert!(k.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<kml xmlns=\"http://www.opengis.net/kml/2.2\">"), "{k}");
    assert!(k.contains("<name>Pier 2 &lt;north&gt; &amp; &quot;east&quot;</name>"), "{k}");
    assert!(!k.contains("<north>"));
    assert!(k.contains("<coordinates>-75.25,40.5,120.5</coordinates>"), "{k}");
    assert!(k.contains("<TimeStamp><when>2026-05-01T10:05:00</when></TimeStamp>"), "{k}");
    assert!(k.contains("<Data name=\"heading\"><value>87.3</value></Data>"), "{k}");
    assert!(k.contains("<Data name=\"headingRef\"><value>magnetic</value></Data>"), "{k}");
    assert!(k.contains("<Data name=\"altitude\"><value>118</value></Data>"), "{k}");
    assert!(k.contains("<Data name=\"accuracy\"><value>6.5</value></Data>"), "{k}");
    // capture order: the 10:00 photo before the 10:05 one, in the folder and along the track
    let (a, b) = (k.find("-75.2,40.4,118").unwrap(), k.find("-75.25,40.5,120.5").unwrap());
    assert!(a < b);
    assert!(k.contains("<name>Track</name>") && k.contains("<LineString>"), "{k}");
    assert!(k.contains("<Data name=\"kind\"><value>track</value></Data>"));
    assert!(k.contains("        -75.2,40.4,118\n        -75.25,40.5,120.5\n"), "{k}");
    // balanced: every opened Placemark is closed
    assert_eq!(k.matches("<Placemark>").count(), 3);
    assert_eq!(k.matches("</Placemark>").count(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

/// XML-forbidden control characters are dropped, the rest escaped.
#[test]
fn xml_escape_drops_control_characters() {
    assert_eq!(crate::cmd::locations::xml_escape("a\u{1}b<'c'>\td"), "ab&lt;&apos;c&apos;&gt;\td");
}

/// The format follows `format`, else the extension; an unknown one, nothing to export, or a
/// folder that isn't there are errors, never a panic.
#[test]
fn bad_formats_paths_and_empty_sets_are_errors() {
    let (mut s, ids) = session();
    let dir = temp_dir("errors");
    let txt = dir.join("sites.txt");
    assert!(s.execute("photo.exportLocations", &json!({"ids": ids, "path": txt.to_string_lossy()})).is_err());
    let r = s.execute("photo.exportLocations", &json!({"ids": ids, "path": txt.to_string_lossy(), "format": "KML"})).unwrap();
    assert_eq!(r["format"], "kml");
    assert!(std::fs::read_to_string(&txt).unwrap().contains("<kml"));
    assert!(s.execute("photo.exportLocations", &json!({"ids": ids, "format": "gpx"})).is_err());
    let e = s.execute("photo.exportLocations", &json!({"ids": [ids[2], ids[3]]})).unwrap_err().to_string();
    assert!(e.contains("none of the 2"), "{e}");
    let missing = dir.join("no-such-folder").join("x.kml");
    let e = s.execute("photo.exportLocations", &json!({"ids": ids, "path": missing.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("x.kml"), "{e}");
    assert!(s.execute("photo.exportLocations", &json!({"ids": "1"})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// No `ids`: several selected photos, else the photos shown.
#[test]
fn defaults_to_the_selection_else_the_photos_shown() {
    let (mut s, ids) = session();
    s.execute("library.select", &json!({"ids": [ids[0], ids[2]]})).unwrap();
    let r = s.execute("photo.exportLocations", &json!({})).unwrap();
    assert_eq!((r["written"].as_u64(), r["skipped"].as_u64()), (Some(1), Some(1)), "{r}");
    s.execute("library.select", &json!({"ids": [ids[0]]})).unwrap();
    let shown = s.visible_cloned().len() as u64;
    let r = s.execute("photo.exportLocations", &json!({})).unwrap();
    assert_eq!(r["written"].as_u64().unwrap() + r["skipped"].as_u64().unwrap(), shown, "{r}");
}
