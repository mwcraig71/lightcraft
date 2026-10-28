//! `orthomosaic.*`: drone orthomosaics (GeoTIFFs) drawn on the Map view. See
//! [`crate::orthomosaic`] and docs/map.md → Orthomosaics.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::orthomosaic::{Status, View};
use crate::{Result, Session};

/// An orthomosaic as the commands report it.
pub fn describe(v: &View) -> Value {
    let o = &v.overlay;
    let i = &o.info;
    let lats = i.corners.iter().map(|c| c.0);
    let lons = i.corners.iter().map(|c| c.1);
    let (north, south) = (lats.clone().fold(f64::NEG_INFINITY, f64::max), lats.fold(f64::INFINITY, f64::min));
    let (east, west) = (lons.clone().fold(f64::NEG_INFINITY, f64::max), lons.fold(f64::INFINITY, f64::min));
    let round = |v: f64, k: f64| (v * k).round() / k;
    json!({
        "id": o.id,
        "name": o.name,
        "path": o.path.to_string_lossy(),
        "visible": o.visible,
        "opacity": o.opacity,
        "crs": i.crs,
        "epsg": i.epsg,
        "width": i.width,
        "height": i.height,
        "resolutionCm": round(i.resolution_m * 100.0, 100.0),
        "bounds": {"north": north, "south": south, "east": east, "west": west},
        "corners": i.corners.iter().map(|c| [c.0, c.1]).collect::<Vec<_>>(),
        "minZoom": i.min_zoom,
        "maxZoom": i.max_zoom,
        "layout": i.layout,
        "overviews": i.overviews,
        "fileSize": o.size,
        "missing": v.missing,
        "status": v.status,
        "progress": match v.status {
            Status::Building { done, total } if total > 0 => round(done as f64 / total as f64, 1000.0),
            Status::Building { .. } => 0.0,
            _ => 1.0,
        },
    })
}

fn id_param(p: &Value, c: &str) -> Result<u64> {
    p.get("id").and_then(Value::as_u64).ok_or_else(|| bad(c, "`id` must be an orthomosaic id (a whole number)"))
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "orthomosaic.add";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(C, "missing `path` (a GeoTIFF)"))?;
    let v = s.add_orthomosaic(std::path::Path::new(path), str_param(p, "name")).map_err(|e| bad(C, e))?;
    Ok(describe(&v))
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    // look at the files again: one may have gone or changed since
    let views = s.orthomosaic_views();
    Ok(json!({"orthomosaics": views.iter().map(describe).collect::<Vec<_>>()}))
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "orthomosaic.set";
    let id = id_param(p, C)?;
    let visible = match p.get("visible") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(v) => return Err(bad(C, format!("`visible` must be true or false, not {v}"))),
    };
    let opacity = match p.get("opacity") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().ok_or_else(|| bad(C, format!("`opacity` must be a number from 0 to 1, not {v}")))?),
    };
    let v = s.set_orthomosaic(id, visible, opacity, str_param(p, "name")).map_err(|e| bad(C, e))?;
    Ok(describe(&v))
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "orthomosaic.remove";
    let id = id_param(p, C)?;
    s.remove_orthomosaic(id).map_err(|e| bad(C, e))?;
    Ok(json!({"removed": id}))
}

fn rebuild(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "orthomosaic.rebuild";
    let id = id_param(p, C)?;
    let v = s.rebuild_orthomosaic(id).map_err(|e| bad(C, e))?;
    Ok(describe(&v))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "orthomosaic.add",
            "Add Orthomosaic",
            [],
            None,
            "{path: a GeoTIFF orthomosaic (WGS 84, Web Mercator or a WGS 84 / NAD83 UTM zone), name?} → {id, name, crs, epsg, resolutionCm, bounds: {north, south, east, west}, corners, minZoom, maxZoom, status, …} — referenced in place; its map tiles are built in the background (activity `orthomosaic`) into the library's Orthomosaics folder",
            always,
            add
        ),
        cmd!(
            query "orthomosaic.list",
            "List Orthomosaics",
            [],
            None,
            "{} → {orthomosaics: [{id, name, path, visible, opacity, crs, resolutionCm, bounds, status: {state: building|ready|failed, …}, progress, missing}]}",
            always,
            list
        ),
        cmd!("orthomosaic.set", "Orthomosaic Layer", [], None, "{id, visible?: bool, opacity?: 0..1, name?} → the orthomosaic", always, set),
        cmd!(
            "orthomosaic.remove",
            "Remove Orthomosaic",
            [],
            None,
            "{id} → {removed} — takes it off the map and deletes its tiles (never the GeoTIFF)",
            always,
            remove
        ),
        cmd!(
            "orthomosaic.rebuild",
            "Rebuild Orthomosaic Tiles",
            [],
            None,
            "{id} → the orthomosaic — makes its map tiles again (after a cancel or failure, or a changed file)",
            always,
            rebuild
        ),
    ]
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use lightcraft_ortho::testdata::{Spec, write_geotiff};
    use serde_json::json;

    use crate::Session;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lc-engine-ortho-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn deck(path: &std::path::Path, size: u32) {
        let (e, n) = lightcraft_ortho::Crs::from_epsg(32610).unwrap().from_lat_lon(37.8199, -122.4783).unwrap();
        let half = f64::from(size) * 0.03 / 2.0;
        write_geotiff(path, &Spec::utm(size, size, 32610, 0.03, (e - half, n + half)), |x, y| [(x % 256) as u8, (y % 256) as u8, 90, 255]).unwrap();
    }

    #[test]
    fn add_builds_tiles_in_the_background_and_the_list_survives_a_reload() {
        let dir = scratch("add");
        let tif = dir.join("deck.tif");
        deck(&tif, 768);
        let mut s = Session::new();
        s.orthomosaics.root_override = Some(dir.join("lib"));
        let r = s.execute("orthomosaic.add", &json!({"path": tif.to_string_lossy()})).unwrap();
        assert_eq!(r["id"], 1);
        assert_eq!(r["epsg"], 32610);
        assert_eq!(r["name"], "deck");
        assert!((r["resolutionCm"].as_f64().unwrap() - 3.0).abs() < 0.1, "{r}");
        let north = r["bounds"]["north"].as_f64().unwrap();
        assert!((37.8199..37.83).contains(&north), "{r}");
        assert_eq!(r["maxZoom"], 22);
        // the build shows in the activity stack while it runs
        let rows = s.activity.list();
        let row = rows.iter().find(|t| t.kind == "orthomosaic");
        assert!(row.is_some_and(|t| t.cancellable) || s.orthomosaics().status(1) == crate::orthomosaic::Status::Ready, "{rows:?}");
        assert!(s.wait_for_orthomosaics(Duration::from_secs(60)));
        assert_eq!(s.orthomosaics().status(1), crate::orthomosaic::Status::Ready);
        assert!(s.activity.list().iter().all(|t| t.kind != "orthomosaic"), "the row goes when the build ends");
        let tiles = s.orthomosaics().tile_dir(1).unwrap();
        assert!(tiles.join("22").is_dir() && tiles.join("12").is_dir());

        // hide it, halve it; a new session on the same folder reads it back as built
        s.execute("orthomosaic.set", &json!({"id": 1, "visible": false, "opacity": 0.5})).unwrap();
        assert!(s.execute("orthomosaic.set", &json!({"id": 1, "opacity": 2})).is_err());
        assert!(s.execute("orthomosaic.set", &json!({"id": 9})).is_err());
        let mut again = Session::new();
        again.orthomosaics.root_override = Some(dir.join("lib"));
        let l = again.execute("orthomosaic.list", &json!({})).unwrap();
        let o = &l["orthomosaics"][0];
        assert_eq!((o["visible"].clone(), o["opacity"].clone()), (json!(false), json!(0.5)), "{l}");
        assert_eq!(o["status"]["state"], "ready", "{l}");
        // the same file twice is refused
        assert!(again.execute("orthomosaic.add", &json!({"path": tif.to_string_lossy()})).is_err());

        // a missing file is reported, its tiles still there
        std::fs::rename(&tif, dir.join("moved.tif")).unwrap();
        let mut third = Session::new();
        third.orthomosaics.root_override = Some(dir.join("lib"));
        let l = third.execute("orthomosaic.list", &json!({})).unwrap();
        assert_eq!(l["orthomosaics"][0]["missing"], true, "{l}");
        assert_eq!(l["orthomosaics"][0]["status"]["state"], "ready");
        assert!(third.execute("orthomosaic.rebuild", &json!({"id": 1})).is_err());

        // remove deletes the tiles, never the GeoTIFF
        third.execute("orthomosaic.remove", &json!({"id": 1})).unwrap();
        assert!(!tiles.exists());
        assert!(dir.join("moved.tif").exists());
        assert_eq!(third.execute("orthomosaic.list", &json!({})).unwrap()["orthomosaics"], json!([]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_files_and_params_are_errors() {
        let dir = scratch("bad");
        let mut s = Session::new();
        s.orthomosaics.root_override = Some(dir.join("lib"));
        assert!(s.execute("orthomosaic.add", &json!({})).is_err());
        let e = s.execute("orthomosaic.add", &json!({"path": dir.join("none.tif").to_string_lossy()})).unwrap_err();
        assert!(e.to_string().contains("no such file"), "{e}");
        std::fs::write(dir.join("junk.tif"), b"II*\0garbage").unwrap();
        assert!(s.execute("orthomosaic.add", &json!({"path": dir.join("junk.tif").to_string_lossy()})).is_err());
        assert!(s.execute("orthomosaic.remove", &json!({"id": "x"})).is_err());
        assert_eq!(s.execute("orthomosaic.list", &json!({})).unwrap()["orthomosaics"], json!([]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_the_row_stops_the_build_and_rebuild_finishes_it() {
        let dir = scratch("cancel");
        let tif = dir.join("big.tif");
        deck(&tif, 3072);
        let mut s = Session::new();
        s.orthomosaics.root_override = Some(dir.join("lib"));
        s.execute("orthomosaic.add", &json!({"path": tif.to_string_lossy()})).unwrap();
        if let Some(row) = s.activity.list().iter().find(|t| t.kind == "orthomosaic") {
            s.activity.cancel(row.id).unwrap();
            assert!(s.wait_for_orthomosaics(Duration::from_secs(60)));
            match s.orthomosaics().status(1) {
                crate::orthomosaic::Status::Failed { error } => assert_eq!(error, "cancelled"),
                // it finished before the cancel landed
                crate::orthomosaic::Status::Ready => {}
                other => panic!("{other:?}"),
            }
        }
        s.execute("orthomosaic.rebuild", &json!({"id": 1})).unwrap();
        assert!(s.wait_for_orthomosaics(Duration::from_secs(120)));
        assert_eq!(s.orthomosaics().status(1), crate::orthomosaic::Status::Ready);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
