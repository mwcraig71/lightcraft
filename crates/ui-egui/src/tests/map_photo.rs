//! The Map view's photo mode and the inspection log, headless: a double-click in the map's
//! filmstrip (or on a pin) shows the photo in the map's place beside its log, never Develop; Esc,
//! Back to Map, a double-click on the photo and the thumbnail's double-click again bring the map
//! back exactly as it was; ←/→ step; the log's entries are added, edited and deleted through its
//! widgets; a pin with a log gets a badge coloured by its worst condition.

use std::time::Duration;

use lightcraft_catalog::{Op, PhotoId};
use serde_json::{Value, json};

use crate::headless::Headless;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

/// The demo with three photos along a bridge, tiles off, the Map view open.
fn app(size: [f32; 2]) -> (Headless, Vec<PhotoId>) {
    let services = Services { png: None, ..Default::default() };
    let mut h = Headless::new(LightcraftApp::new(lightcraft_engine::Session::with_demo(), services), size, 1.0);
    h.settle(T);
    let ids: Vec<PhotoId> = h.app.session.visible_cloned();
    for (i, id) in ids.iter().enumerate().take(3) {
        let mut meta = h.app.session.catalog.photo(*id).unwrap().meta.clone();
        meta.gps = Some((40.0, -75.0 + i as f64 * 0.0003));
        h.app.session.commit("setup", Op::SetMeta { id: *id, meta: Box::new(meta) }).unwrap();
    }
    run(&mut h, "map.setTileSource", json!({"source": "off"}));
    run(&mut h, "view.map", json!({}));
    h.settle(T);
    h.step();
    (h, ids)
}

fn run(h: &mut Headless, command: &str, params: Value) -> Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn widget(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r)
}

fn click_widget(h: &mut Headless, id: &str, count: u32) {
    let r = h.request("ui.clickWidget", json!({"id": id, "count": count}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

fn key(h: &mut Headless, k: &str) {
    let r = h.request("ui.key", json!({"key": k}), T);
    assert_eq!(r["ok"], true, "{k}: {r}");
    h.step();
    h.step();
}

/// Half a second of frames, so egui doesn't take the next clicks for more of the last burst.
fn pause(h: &mut Headless) {
    for _ in 0..40 {
        h.step();
    }
}

/// What "the map as it was" is: its place, the narrowed filmstrip, the open marker card.
fn map_state(h: &Headless) -> ([f64; 2], f64, Vec<u64>, Option<u64>) {
    let m = &h.app.ui.map;
    (m.center, m.zoom, m.focus.clone(), m.markers.open)
}

#[test]
fn a_double_click_in_the_map_filmstrip_shows_the_photo_and_every_way_back_keeps_the_map() {
    let (mut h, ids) = app([1400.0, 900.0]);
    // a narrowed filmstrip, an open marker card and a place of the user's own
    run(&mut h, "map.focus", json!({"ids": [ids[0].0, ids[1].0, ids[2].0]}));
    let marker = run(&mut h, "map.addMarker", json!({"lat": 40.0001, "lon": -75.0002, "title": "Pier 2"}))["id"].as_u64().unwrap();
    run(&mut h, "map.openMarker", json!({"id": marker}));
    h.step();
    h.step();
    let before = map_state(&h);
    assert_eq!(before.3, Some(marker));
    let film = format!("film:{}", ids[1].0);
    click_widget(&mut h, &film, 2);
    assert_eq!(h.app.ui.view, ViewMode::Map, "the Map view stays (not Develop)");
    assert_eq!(h.app.ui.map.photo, Some(ids[1].0));
    assert_eq!(h.app.session.active(), Some(ids[1]));
    assert!(widget(&h, "map:photo").is_some() && widget(&h, "map:log").is_some(), "the photo and its log in the map's place");
    assert!(widget(&h, "map:view").is_some());
    assert!(!h.app.widgets.iter().any(|(w, _)| w.starts_with("mapPin:")), "the map isn't drawn meanwhile");
    assert!(widget(&h, "button:mapPhotoBack").is_some());
    // Esc
    key(&mut h, "escape");
    assert_eq!(h.app.ui.map.photo, None);
    assert_eq!(h.app.ui.view, ViewMode::Map);
    assert_eq!(map_state(&h), before, "the map as it was");
    assert!(h.app.widgets.iter().any(|(w, _)| w.starts_with("mapPin:")));
    // Back to Map
    pause(&mut h);
    click_widget(&mut h, &film, 2);
    assert_eq!(h.app.ui.map.photo, Some(ids[1].0));
    click_widget(&mut h, "button:mapPhotoBack", 1);
    assert_eq!(h.app.ui.map.photo, None);
    assert_eq!(map_state(&h), before);
    // a double-click on the photo
    pause(&mut h);
    run(&mut h, "map.showPhoto", json!({"id": ids[0].0}));
    h.step();
    h.step();
    assert_eq!(h.app.ui.map.photo, Some(ids[0].0));
    click_widget(&mut h, "map:photoImage", 2);
    assert_eq!(h.app.ui.map.photo, None);
    assert_eq!(map_state(&h), before);
    // the same double-click on the thumbnail again
    pause(&mut h);
    click_widget(&mut h, &film, 2);
    assert_eq!(h.app.ui.map.photo, Some(ids[1].0));
    pause(&mut h);
    click_widget(&mut h, &film, 2);
    assert_eq!(h.app.ui.map.photo, None, "the thumbnail's double-click again: the map");
    assert_eq!(map_state(&h), before);
    // outside the Map view a filmstrip double-click does what it did
    run(&mut h, "view.detail", json!({}));
    h.settle(T);
    pause(&mut h);
    click_widget(&mut h, &format!("film:{}", ids[2].0), 2);
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    assert_eq!(h.app.ui.map.photo, None);
}

#[test]
fn a_double_click_on_a_pin_shows_its_photo() {
    let (mut h, ids) = app([1400.0, 900.0]);
    let pin = widget(&h, &format!("mapPin:{}", ids[2].0)).expect("pin");
    let r = h.request("ui.click", json!({"x": pin.center().x, "y": pin.center().y, "count": 2}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!((h.app.ui.view, h.app.ui.map.photo), (ViewMode::Map, Some(ids[2].0)));
}

#[test]
fn arrows_step_through_the_filmstrip_in_photo_mode_and_leave_the_map_alone() {
    let (mut h, ids) = app([1400.0, 900.0]);
    run(&mut h, "map.focus", json!({"ids": [ids[0].0, ids[1].0, ids[2].0]}));
    let before = map_state(&h);
    run(&mut h, "map.showPhoto", json!({"id": ids[0].0}));
    h.step();
    h.step();
    key(&mut h, "right");
    assert_eq!(h.app.ui.map.photo, Some(ids[1].0));
    assert_eq!(h.app.session.active(), Some(ids[1]));
    key(&mut h, "right");
    key(&mut h, "right");
    assert_eq!(h.app.ui.map.photo, Some(ids[2].0), "the strip's last photo: it stays");
    key(&mut h, "left");
    assert_eq!(h.app.ui.map.photo, Some(ids[1].0));
    // a click in the filmstrip shows that photo, still in photo mode
    click_widget(&mut h, &format!("film:{}", ids[0].0), 1);
    assert_eq!(h.app.ui.map.photo, Some(ids[0].0));
    assert_eq!(h.app.ui.view, ViewMode::Map);
    assert_eq!(map_state(&h), before);
    key(&mut h, "escape");
    assert_eq!(map_state(&h), before, "stepping didn't pull the map to another pin");
    // the photo shown survives a ui.set round trip
    run(&mut h, "map.showPhoto", json!({"id": ids[2].0}));
    h.step();
    let map = serde_json::to_value(&h.app.ui.map).unwrap();
    assert_eq!(map["photo"], ids[2].0, "{map}");
    h.app.ui.map.photo = None;
    let r = h.request("ui.set", json!({"map": map}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert_eq!(h.app.ui.map.photo, Some(ids[2].0), "restored by ui.set");
    // hostile params
    for p in [json!({"id": "x"}), json!({"id": 9_999_999}), json!({"id": -3})] {
        let r = h.request("engine.execute", json!({"command": "map.showPhoto", "params": p}), T);
        assert_ne!(r["ok"], true, "{p}: {r}");
    }
    run(&mut h, "map.showPhoto", json!({"id": null}));
    assert_eq!(h.app.ui.map.photo, None);
}

#[test]
fn log_entries_are_added_edited_and_deleted_with_the_logs_widgets() {
    let (mut h, ids) = app([1400.0, 900.0]);
    let id = ids[0];
    run(&mut h, "map.showPhoto", json!({"id": id.0}));
    h.step();
    h.step();
    assert!(h.painted_text().iter().any(|t| t.contains("No entries yet")), "{:?}", h.painted_text());
    click_widget(&mut h, "button:noteAdd-map", 1);
    assert!(widget(&h, "noteEditor-map").is_some(), "the editor opened");
    for w in ["combo:noteKind-map", "combo:noteElement-map", "combo:noteCondition-map", "noteDate-map", "noteText-map", "button:noteSave-map"] {
        assert!(widget(&h, w).is_some(), "{w}");
    }
    // pick from the dropdowns (the draft holds them), type the text into its field
    if let Some(d) = h.app.ui.map.notes.draft.as_mut() {
        d.element = "Bearing".into();
        d.condition = Some(lightcraft_catalog::Condition::Poor);
    }
    click_widget(&mut h, "noteText-map", 1);
    let r = h.request("ui.text", json!({"text": "Bearing 3 squeals under load"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    click_widget(&mut h, "button:noteSave-map", 1);
    let log = h.app.session.catalog.notes(id).to_vec();
    assert_eq!(log.len(), 1, "{:?}", h.app.ui.map.notes.draft);
    assert_eq!((log[0].element.as_str(), log[0].text.as_str()), ("Bearing", "Bearing 3 squeals under load"));
    assert_eq!(log[0].condition, Some(lightcraft_catalog::Condition::Poor));
    assert!(h.app.ui.map.notes.draft.is_none());
    let n = log[0].id;
    assert!(widget(&h, &format!("note:map:{n}")).is_some());
    let text = h.painted_text().join("\n");
    assert!(text.contains("Inspection Log (1)") && text.contains("Bearing 3 squeals under load") && text.contains("Poor"), "{text}");
    // edit
    click_widget(&mut h, &format!("button:noteEdit-map-{n}"), 1);
    assert_eq!(h.app.ui.map.notes.draft.as_ref().and_then(|d| d.note), Some(n));
    if let Some(d) = h.app.ui.map.notes.draft.as_mut() {
        d.condition = Some(lightcraft_catalog::Condition::Critical);
        d.kind = lightcraft_catalog::NoteKind::Repair;
    }
    click_widget(&mut h, "button:noteSave-map", 1);
    let log = h.app.session.catalog.notes(id).to_vec();
    assert_eq!((log[0].kind, log[0].condition), (lightcraft_catalog::NoteKind::Repair, Some(lightcraft_catalog::Condition::Critical)));
    // delete asks first
    click_widget(&mut h, &format!("button:noteDelete-map-{n}"), 1);
    assert_eq!(h.app.session.catalog.notes(id).len(), 1);
    click_widget(&mut h, &format!("button:noteDeleteCancel-map-{n}"), 1);
    assert_eq!(h.app.session.catalog.notes(id).len(), 1);
    click_widget(&mut h, &format!("button:noteDelete-map-{n}"), 1);
    click_widget(&mut h, &format!("button:noteDeleteConfirm-map-{n}"), 1);
    assert!(h.app.session.catalog.notes(id).is_empty());
    // each change was one undo step
    run(&mut h, "edit.undo", json!({}));
    assert_eq!(h.app.session.catalog.notes(id).len(), 1);
    run(&mut h, "edit.undo", json!({}));
    assert_eq!(h.app.session.catalog.notes(id)[0].condition, Some(lightcraft_catalog::Condition::Poor));
    // a bad date is said, and the editor stays open with what was typed
    click_widget(&mut h, "button:noteAdd-map", 1);
    if let Some(d) = h.app.ui.map.notes.draft.as_mut() {
        d.date = "2026-02-31".into();
        d.text = "Kept".into();
    }
    click_widget(&mut h, "button:noteSave-map", 1);
    assert_eq!(h.app.session.catalog.notes(id).len(), 1);
    assert_eq!(h.app.ui.map.notes.draft.as_ref().map(|d| d.text.as_str()), Some("Kept"));
    click_widget(&mut h, "button:noteCancel-map", 1);
    assert!(h.app.ui.map.notes.draft.is_none());
}

#[test]
fn narrow_windows_put_the_log_below_the_photo() {
    let (mut h, ids) = app([800.0, 900.0]);
    run(&mut h, "map.showPhoto", json!({"id": ids[0].0}));
    h.step();
    h.step();
    let (photo, log) = (widget(&h, "map:photo").unwrap(), widget(&h, "map:log").unwrap());
    assert!(log.top() >= photo.bottom() - 0.5, "{photo:?} {log:?}");
}

#[test]
fn a_pin_with_a_log_gets_a_badge_of_its_worst_condition_and_the_hover_card_its_latest_entry() {
    let (mut h, ids) = app([1400.0, 900.0]);
    run(&mut h, "photo.addNote", json!({"id": ids[0].0, "element": "Deck", "condition": "fair", "date": "2026-01-01"}));
    run(
        &mut h,
        "photo.addNote",
        json!({"id": ids[0].0, "element": "Cable/Suspender", "condition": "critical", "text": "Broken wire", "date": "2026-02-01"}),
    );
    run(&mut h, "photo.addNote", json!({"id": ids[1].0, "element": "Joint", "text": "Debris", "date": "2026-02-01"}));
    h.step();
    h.step();
    assert!(
        widget(&h, &format!("mapLog:{}:critical", ids[0].0)).is_some(),
        "{:?}",
        h.app.widgets.iter().map(|w| &w.0).filter(|w| w.starts_with("mapLog")).collect::<Vec<_>>()
    );
    assert!(widget(&h, &format!("mapLog:{}:none", ids[1].0)).is_some(), "a log with no rating: a plain badge");
    assert!(!h.app.widgets.iter().any(|(w, _)| w.starts_with(&format!("mapLog:{}:", ids[2].0))), "no log, no badge");
    // the hover card
    let pin = widget(&h, &format!("mapPin:{}", ids[0].0)).unwrap();
    let r = h.request("ui.move", json!({"x": pin.center().x, "y": pin.center().y}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(widget(&h, "map:hover").is_some());
    assert!(h.painted_text().iter().any(|t| t.contains("Critical: Broken wire")), "{:?}", h.painted_text());
    // closer out, a cluster with a log gets one too (clustering itself is unchanged)
    h.app.ui.map.zoom = 15.0;
    h.step();
    h.step();
    assert!(widget(&h, "mapCluster:3").is_some());
    assert!(h.app.widgets.iter().any(|(w, _)| w.ends_with(":critical") && w.starts_with("mapLog:")));
}

#[test]
fn the_info_panel_shows_the_active_photos_log_and_adds_to_it() {
    let services = Services { png: None, ..Default::default() };
    let mut h = Headless::new(LightcraftApp::new(lightcraft_engine::Session::with_demo(), services), [1400.0, 2000.0], 1.0);
    h.settle(T);
    let id = h.app.session.visible_cloned()[0];
    for (i, text) in ["one", "two", "three", "four"].iter().enumerate() {
        run(&mut h, "photo.addNote", json!({"id": id.0, "element": "Pier", "text": text, "date": format!("2026-01-0{}", i + 1)}));
    }
    run(&mut h, "library.select", json!({"ids": [id.0]}));
    h.app.ui.right = crate::state::RightPanel::Info;
    h.app.ui.view = ViewMode::PhotoGrid;
    h.step();
    h.step();
    assert!(widget(&h, "noteHeader-info").is_some(), "the log in Info");
    let text = h.painted_text().join("\n");
    assert!(text.contains("Inspection Log (4)") && text.contains("four") && !text.contains("\none\n"), "the latest three: {text}");
    assert!(widget(&h, "noteAll-info").is_some());
    click_widget(&mut h, "button:noteAdd-info", 1);
    assert!(widget(&h, "noteEditor-info").is_some());
    if let Some(d) = h.app.ui.map.notes.draft.as_mut() {
        d.text = "five".into();
    }
    click_widget(&mut h, "button:noteSave-info", 1);
    assert_eq!(h.app.session.catalog.notes(id).len(), 5);
}
