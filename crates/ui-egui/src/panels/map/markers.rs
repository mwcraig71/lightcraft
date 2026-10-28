//! Markers on the Map view: points the user placed, with documents and photos attached (see
//! `lightcraft_engine::cmd::markers` and docs/map.md → Markers and attachments).
//!
//! - Each marker draws as a teal diamond with a count badge, above the photo pins and never
//!   merged into their clusters; zoomed far out (where sites show as one named dot) it is drawn
//!   small, without its badge.
//! - Right-click on the map: **Add Marker Here**, **Attach Files Here…** (on a marker: attach to
//!   it). Files dropped on the map attach to the marker whose card is open, else at the point
//!   under the pointer (a new marker there; the map's centre when the pointer isn't on the map).
//! - A click on a marker opens its card: title and note, the attachments (Open, Show in Folder,
//!   Remove), Attach Files…, Photos here (narrows the filmstrip), Delete Marker.
//!
//! The UI-side commands are `map.openMarker`, `map.attachFiles` (file dialog; big documents are
//! copied on a worker under an activity row), `map.openAttachment` and `map.revealAttachment`.

use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};

use egui::{Align2, Color32, Pos2, Rect, RichText, Stroke, StrokeKind, pos2, vec2};
use lightcraft_engine::cmd::markers::Staged;
use serde_json::{Value, json};

use super::widget::Viewport;
use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::pick::{PickKind, PickRequest, Picked};
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// Marker colour (teal: apart from the red photo pins, the yellow cones and the blue accuracy).
pub const MARKER: Color32 = Color32::from_rgb(0, 150, 136);
/// Half the diamond's diagonal (points), full size and far out.
const MARKER_R: f32 = 10.0;
const MARKER_SMALL_R: f32 = 6.0;
/// Below this zoom (a whole city on screen, where photos are one named dot per site) markers draw
/// small, without their count and title.
const SMALL_BELOW: f64 = 12.0;
/// Documents bigger than this in total are copied on a worker thread.
const BACKGROUND_BYTES: u64 = 16 * 1024 * 1024;
const CARD_W: f32 = 360.0;

/// What the Map view keeps about its markers between frames (not saved).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarkerUi {
    /// The marker whose card is open.
    pub open: Option<u64>,
    /// The card's title and note being edited, and the marker (and its `modified`) they are of.
    pub title: String,
    pub note: String,
    pub loaded: Option<(u64, String)>,
    /// Delete Marker asked to confirm.
    pub confirm_delete: bool,
    /// Where the context menu was opened (lat, lon), and the marker under it.
    pub menu_at: Option<(f64, f64)>,
    pub menu_marker: Option<u64>,
    /// The map as last drawn (drops are placed on it).
    pub viewport: Option<Viewport>,
    /// Copies running on workers.
    pub jobs: Jobs,
}

/// Copies running on a worker: the marker and the channel the results come back on.
#[derive(Clone, Default)]
pub struct Jobs(Arc<Mutex<Vec<(u64, Receiver<Vec<Staged>>)>>>);

impl PartialEq for Jobs {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for Jobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Jobs")
    }
}

/// A marker as drawn: its id, world point and how many things are attached.
struct Shown {
    id: u64,
    world: (f64, f64),
    count: usize,
    title: String,
}

fn shown(app: &LightcraftApp) -> Vec<Shown> {
    app.session
        .catalog
        .markers()
        .map(|m| Shown {
            id: m.id.0,
            world: lightcraft_tiles::mercator::to_world(m.lat, m.lon),
            count: m.attachments.len() + m.photos.len(),
            title: if m.title.is_empty() { tr_format!("Marker {id}", id = m.id.0) } else { m.title.clone() },
        })
        .collect()
}

fn radius(vp: &Viewport) -> f32 {
    if vp.zoom < SMALL_BELOW { MARKER_SMALL_R } else { MARKER_R }
}

/// The marker under `p`, nearest first.
fn hit(vp: &Viewport, markers: &[Shown], p: Pos2) -> Option<u64> {
    let r = radius(vp) + 4.0;
    markers.iter().map(|m| (m.id, vp.screen_of(m.world).distance(p))).filter(|(_, d)| *d <= r).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(id, _)| id)
}

/// The diamond with its badge.
pub(crate) fn draw_marker(painter: &egui::Painter, at: Pos2, r: f32, count: usize, open: bool, accent: Color32) {
    let r = if open { r + 2.0 } else { r };
    let diamond = |r: f32| vec![at + vec2(0.0, -r), at + vec2(r, 0.0), at + vec2(0.0, r), at + vec2(-r, 0.0)];
    let shadow: Vec<Pos2> = diamond(r + 1.5).into_iter().map(|p| p + vec2(0.0, 1.0)).collect();
    painter.add(egui::Shape::convex_polygon(shadow, Color32::from_black_alpha(90), Stroke::NONE));
    painter.add(egui::Shape::convex_polygon(
        diamond(r),
        if open { accent } else { MARKER },
        Stroke::new(if open { 2.5 } else { 1.8 }, Color32::WHITE),
    ));
    // a sheet of paper inside: what a marker holds
    if r >= MARKER_R {
        let sheet = Rect::from_center_size(at, vec2(r * 0.55, r * 0.7));
        painter.rect_filled(sheet, 0.5, Color32::WHITE);
        for i in 1..=2 {
            let y = sheet.top() + sheet.height() * i as f32 / 3.0;
            painter.line_segment([pos2(sheet.left() + 1.0, y), pos2(sheet.right() - 1.0, y)], Stroke::new(1.0, MARKER));
        }
        if count > 0 {
            let c = at + vec2(r * 0.8, -r * 0.8);
            let text = if count > 99 { "99+".to_string() } else { count.to_string() };
            let w = 7.0 + 3.5 * text.len() as f32;
            let badge = Rect::from_center_size(c, vec2(w.max(14.0), 14.0));
            painter.rect(badge, 7.0, Color32::from_rgb(40, 44, 52), Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
            painter.text(c, Align2::CENTER_CENTER, text, egui::FontId::proportional(9.5), Color32::WHITE);
        }
    }
}

/// Draw the markers and handle a click on one (opens its card). Returns whether the pointer is
/// on a marker, so the pins under it don't take the click or show their hover card.
pub fn layer(app: &mut LightcraftApp, ui: &mut egui::Ui, vp: &Viewport, resp: &egui::Response) -> bool {
    let t = Tokens::get(ui.ctx());
    let markers = shown(app);
    let painter = ui.painter_at(vp.rect);
    let r = radius(vp);
    let open = app.ui.map.markers.open;
    for m in &markers {
        let at = vp.screen_of(m.world);
        if !vp.rect.expand(r * 2.0).contains(at) {
            continue;
        }
        draw_marker(&painter, at, r, m.count, open == Some(m.id), t.accent);
        if r >= MARKER_R {
            // its title on a dark label below-right, as a site's name is
            let g = crate::widgets::one_line(&painter, &m.title, t.semibold(11.0), Color32::WHITE, 180.0);
            let label = Rect::from_min_size(at + vec2(r + 4.0, 2.0), g.size() + vec2(10.0, 4.0));
            painter.rect_filled(label, 3.0, Color32::from_black_alpha(170));
            painter.galley(label.min + vec2(5.0, 2.0), g, Color32::WHITE);
        }
        crate::widgets::register_geometry(ui.ctx(), format!("mapMarker:{}", m.id), Rect::from_center_size(at, vec2(2.0 * r, 2.0 * r)));
    }
    app.ui.map.markers.viewport = Some(*vp);
    let pointer = resp.hover_pos();
    let under = pointer.and_then(|p| hit(vp, &markers, p));
    if resp.clicked() {
        match under {
            Some(id) => {
                let _ = app.run("map.openMarker", json!({"id": id}));
            }
            // a click elsewhere on the map closes the card
            None if app.ui.map.markers.open.is_some() => app.ui.map.markers.open = None,
            None => {}
        }
    }
    if let Some(id) = under {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if let Some(m) = app.session.catalog.marker(lightcraft_catalog::MarkerId(id))
            && let Some(p) = pointer
        {
            let title = if m.title.is_empty() { tr_format!("Marker {id}", id = id) } else { m.title.clone() };
            let what = tr_format!("{a} attachments · {p} photos", a = m.attachments.len(), p = m.photos.len());
            hover_label(ui, p, &title, &what);
        }
    }
    if resp.secondary_clicked()
        && let Some(p) = pointer
    {
        let (x, y) = vp.world_at(p);
        app.ui.map.markers.menu_at = Some(lightcraft_tiles::mercator::from_world(x, y));
        app.ui.map.markers.menu_marker = under;
    }
    context_menu(app, resp);
    under.is_some()
}

fn hover_label(ui: &egui::Ui, at: Pos2, title: &str, what: &str) {
    let t = Tokens::get(ui.ctx());
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("lc-marker-hover")));
    let a = painter.layout_no_wrap(title.to_string(), t.semibold(11.5), t.text);
    let b = painter.layout_no_wrap(what.to_string(), t.font(11.0), t.text_dim);
    let size = vec2(a.size().x.max(b.size().x) + 16.0, a.size().y + b.size().y + 12.0);
    let card = Rect::from_min_size(at + vec2(16.0, 12.0), size);
    painter.rect(card, 5.0, t.chrome, Stroke::new(1.0, t.divider), StrokeKind::Inside);
    let below = card.top() + 6.0 + a.size().y;
    painter.galley(card.min + vec2(8.0, 6.0), a, t.text);
    painter.galley(pos2(card.left() + 8.0, below), b, t.text_dim);
    crate::widgets::register_geometry(ui.ctx(), "mapMarker:hover", card);
}

/// The map's right-click menu.
fn context_menu(app: &mut LightcraftApp, resp: &egui::Response) {
    crate::widgets::context_menu(resp, |ui| {
        let Some((lat, lon)) = app.ui.map.markers.menu_at else {
            ui.close();
            return;
        };
        let on = app.ui.map.markers.menu_marker;
        if on.is_none() {
            let b = ui.button(tr("Add Marker Here"));
            crate::widgets::register_response("mapMenu:addMarker", b.rect, &b);
            if b.clicked() {
                match app.run("map.addMarker", json!({"lat": lat, "lon": lon})) {
                    Ok(r) => {
                        let _ = app.run("map.openMarker", json!({"id": r["id"]}));
                    }
                    Err(e) => toast(app, e),
                }
                ui.close();
            }
        }
        let label = if on.is_some() { "Attach Files to This Marker…" } else { "Attach Files Here…" };
        let b = ui.button(tr(label));
        crate::widgets::register_response("mapMenu:attach", b.rect, &b);
        if b.clicked() {
            let params = match on {
                Some(id) => json!({"marker": id}),
                None => json!({"lat": lat, "lon": lon}),
            };
            if let Err(e) = app.run("map.attachFiles", params) {
                toast(app, e);
            }
            ui.close();
        }
        if let Some(id) = on {
            let b = ui.button(tr("Open Marker"));
            crate::widgets::register_response("mapMenu:open", b.rect, &b);
            if b.clicked() {
                let _ = app.run("map.openMarker", json!({"id": id}));
                ui.close();
            }
        }
    });
}

fn toast(app: &mut LightcraftApp, text: impl Into<String>) {
    let ctx = app.tasks.repaint.clone().unwrap_or_default();
    app.toast(&ctx, text);
}

// ---------------------------------------------------------------- the card

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "pdf" => "PDF",
        "spreadsheet" => "XLS",
        "markdown" => "MD",
        "text" => "TXT",
        _ => "FILE",
    }
}

fn kind_color(kind: &str) -> Color32 {
    match kind {
        "pdf" => Color32::from_rgb(200, 60, 50),
        "spreadsheet" => Color32::from_rgb(30, 130, 70),
        "markdown" => Color32::from_rgb(90, 90, 160),
        "text" => Color32::from_rgb(110, 110, 110),
        _ => Color32::from_rgb(120, 100, 70),
    }
}

/// "12 KB", "3.4 MB".
pub fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", b / 1024.0 / 1024.0)
    } else {
        format!("{:.1} GB", b / 1024.0 / 1024.0 / 1024.0)
    }
}

/// The open marker's card, at the map's right below the controls.
pub fn card(app: &mut LightcraftApp, ui: &mut egui::Ui, map: Rect) {
    let Some(id) = app.ui.map.markers.open else { return };
    // (read straight from the catalog: a command each frame would also save the library each frame)
    let Some(m) = app.session.catalog.marker(lightcraft_catalog::MarkerId(id)).map(|m| lightcraft_engine::cmd::markers::marker_json(&app.session, m))
    else {
        // deleted (or undone away): the card goes
        app.ui.map.markers.open = None;
        return;
    };
    let modified = m["modified"].as_str().unwrap_or_default().to_string();
    let st = &mut app.ui.map.markers;
    let editing = crate::text_field::was_editing(ui.ctx(), "markerTitle") || crate::text_field::was_editing(ui.ctx(), "markerNote");
    if st.loaded.as_ref() != Some(&(id, modified.clone())) && !editing {
        st.title = m["title"].as_str().unwrap_or_default().to_string();
        st.note = m["note"].as_str().unwrap_or_default().to_string();
        st.loaded = Some((id, modified));
    }
    let t = Tokens::get(ui.ctx());
    let top = map.top() + 46.0;
    let max_h = (map.bottom() - top - 100.0).max(160.0);
    let pos = pos2((map.right() - CARD_W - 12.0).max(map.left() + 8.0), top);
    let ctx = ui.ctx().clone();
    let area = egui::Area::new(egui::Id::unique("lc-marker-card")).order(egui::Order::Foreground).fixed_pos(pos).show(&ctx, |ui| {
        egui::Frame::new().fill(t.chrome).stroke(Stroke::new(1.0, t.divider)).corner_radius(6.0).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
            ui.set_width(CARD_W - 24.0);
            ui.set_max_height(max_h);
            card_contents(app, ui, id, &m);
        })
    });
    register(ui, "markerCard", area.response.rect);
    crate::widgets::screen_reader_name(&ctx, area.response.id, tr("Marker"));
}

fn card_contents(app: &mut LightcraftApp, ui: &mut egui::Ui, id: u64, m: &Value) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
        draw_marker(ui.painter(), r.center(), 8.0, 0, false, t.accent);
        ui.label(RichText::new(tr("Marker")).font(t.semibold(13.0)).color(t.text));
        let (lat, lon) = (m["lat"].as_f64().unwrap_or(0.0), m["lon"].as_f64().unwrap_or(0.0));
        ui.label(RichText::new(format!("{lat:.5}, {lon:.5}")).font(t.font(11.0)).color(t.text_dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if text_button(ui, "markerCardClose", "Close", false).on_hover_text(tr("Close the marker card")).clicked() {
                app.ui.map.markers.open = None;
            }
        });
    });
    // title and note: saved when the edit ends (Return, or leaving the field)
    let st = &mut app.ui.map.markers;
    let title = crate::text_field::TextField::singleline("markerTitle", &mut st.title)
        .hint(tr("Title"))
        .label(tr("Marker title"))
        .width(f32::INFINITY)
        .show(ui);
    let note = crate::text_field::TextField::multiline("markerNote", &mut st.note)
        .hint(tr("Note"))
        .label(tr("Marker note"))
        .rows(3)
        .width(f32::INFINITY)
        .show(ui);
    if title.committed() || note.committed() {
        let (title, note) = (st.title.clone(), st.note.clone());
        let changed = title != m["title"].as_str().unwrap_or_default() || note != m["note"].as_str().unwrap_or_default();
        if changed && let Err(e) = app.run("map.updateMarker", json!({"id": id, "title": title, "note": note})) {
            toast(app, e);
        }
    }
    // the attachments
    let empty = Vec::new();
    let attachments = m["attachments"].as_array().unwrap_or(&empty);
    ui.add_space(2.0);
    ui.label(RichText::new(tr_format!("Attachments ({n})", n = attachments.len())).font(t.semibold(12.0)).color(t.text_label));
    if attachments.is_empty() {
        ui.label(RichText::new(tr("Drop files on the map, or use Attach Files…")).font(t.font(11.0)).color(t.text_dim));
    }
    egui::ScrollArea::vertical().id_salt("markerAttachments").max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
        for a in attachments {
            attachment_row(app, ui, id, a);
        }
    });
    let photos: Vec<u64> = m["photos"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        if text_button(ui, "markerAttach", "Attach Files…", false).on_hover_text(tr("Copy files into the library and attach them here")).clicked()
            && let Err(e) = app.run("map.attachFiles", json!({"marker": id}))
        {
            toast(app, e);
        }
        if !photos.is_empty() {
            let label = tr_format!("Photos here ({n})", n = photos.len());
            if text_button(ui, "markerPhotos", &label, false).on_hover_text(tr("Show only these photos in the filmstrip")).clicked()
                && let Err(e) = app.run("map.focus", json!({"ids": photos}))
            {
                toast(app, e);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if text_button(ui, "markerDelete", "Delete Marker", false).clicked() {
                if attachments.is_empty() && photos.is_empty() {
                    delete(app, id);
                } else {
                    app.ui.map.markers.confirm_delete = true;
                }
            }
        });
    });
    if app.ui.map.markers.confirm_delete {
        let text = tr_format!(
            "Delete this marker? Its {a} attachments and {p} photos stay in the library folder; Undo brings the marker back.",
            a = attachments.len(),
            p = photos.len()
        );
        ui.label(RichText::new(text).font(t.font(11.0)).color(t.caution));
        ui.horizontal(|ui| {
            if text_button(ui, "markerDeleteConfirm", "Delete", false).clicked() {
                delete(app, id);
            }
            if text_button(ui, "markerDeleteCancel", "Cancel", false).clicked() {
                app.ui.map.markers.confirm_delete = false;
            }
        });
    }
}

fn delete(app: &mut LightcraftApp, id: u64) {
    match app.run("map.removeMarker", json!({"id": id})) {
        Ok(_) => {
            app.ui.map.markers.open = None;
            app.ui.map.markers.confirm_delete = false;
            toast(app, tr("Marker deleted (Edit → Undo brings it back)"));
        }
        Err(e) => toast(app, e),
    }
}

fn attachment_row(app: &mut LightcraftApp, ui: &mut egui::Ui, marker: u64, a: &Value) {
    let t = Tokens::get(ui.ctx());
    let aid = a["id"].as_u64().unwrap_or(0);
    let kind = a["kind"].as_str().unwrap_or("other");
    let name = a["fileName"].as_str().unwrap_or_default();
    let missing = a["exists"] == false;
    let row = ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(34.0, 18.0), egui::Sense::hover());
        ui.painter().rect_filled(r, 3.0, kind_color(kind));
        ui.painter().text(r.center(), Align2::CENTER_CENTER, kind_label(kind), t.semibold(9.5), Color32::WHITE);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let galley = crate::widgets::one_line(ui.painter(), name, t.font(11.5), t.text, 150.0);
            let (nr, _) = ui.allocate_exact_size(vec2(150.0, galley.size().y), egui::Sense::hover());
            ui.painter().galley(nr.min, galley, t.text);
            let size = size_text(a["size"].as_u64().unwrap_or(0));
            let sub = if missing { tr_format!("{size} · missing", size = size) } else { size };
            ui.label(RichText::new(sub).font(t.font(10.5)).color(if missing { t.caution } else { t.text_dim }));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            if text_button(ui, &format!("markerRemove-{aid}"), "Remove", false)
                .on_hover_text(tr("Remove it from the marker (the copy stays in the library)"))
                .clicked()
                && let Err(e) = app.run("map.detach", json!({"marker": marker, "attachment": aid}))
            {
                toast(app, e);
            }
            if text_button(ui, &format!("markerReveal-{aid}"), "Show", false).on_hover_text(tr(crate::menus::reveal_label())).clicked()
                && let Err(e) = app.run("map.revealAttachment", json!({"marker": marker, "attachment": aid}))
            {
                toast(app, e);
            }
            if text_button(ui, &format!("markerOpen-{aid}"), "Open", false).on_hover_text(tr("Open the library's copy in its default app")).clicked()
                && let Err(e) = app.run("map.openAttachment", json!({"marker": marker, "attachment": aid}))
            {
                toast(app, e);
            }
        });
    });
    register(ui, format!("markerAttachment:{aid}"), row.response.rect);
    // the whole name, when the row cuts it short
    let _ = row.response.clone().on_hover_text(name);
    crate::widgets::screen_reader_name(ui.ctx(), row.response.id, name);
}

// ---------------------------------------------------------------- commands

/// The UI-side marker commands; `None` for any other id.
pub fn run(app: &mut LightcraftApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "map.openMarker" => open_marker(app, p),
        "map.attachFiles" => attach_files(app, p),
        "map.openAttachment" => attachment_file(app, p, false),
        "map.revealAttachment" => attachment_file(app, p, true),
        _ => return None,
    })
}

/// `map.openMarker {id}`: open its card (`null`: close it); on the Map view.
fn open_marker(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let st = &mut app.ui.map.markers;
    match p.get("id") {
        None | Some(Value::Null) => {
            st.open = None;
            Ok(json!({"open": null}))
        }
        Some(v) => {
            let id = v.as_u64().ok_or_else(|| format!("map.openMarker: `id` must be a marker id, not {v}"))?;
            if app.session.catalog.marker(lightcraft_catalog::MarkerId(id)).is_none() {
                return Err(format!("map.openMarker: no marker {id}"));
            }
            if st.open != Some(id) {
                st.confirm_delete = false;
                st.loaded = None;
            }
            st.open = Some(id);
            app.ui.view = crate::state::ViewMode::Map;
            Ok(json!({"open": id}))
        }
    }
}

/// `map.attachFiles {marker | lat, lon, paths?}`: without `paths`, a file dialog asks for them.
/// Photos are imported at once; other files over [`BACKGROUND_BYTES`] in all are copied on a
/// worker (an activity row) and attached when done. Opens the marker's card.
fn attach_files(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let paths: Vec<String> = match p.get("paths") {
        Some(Value::Array(a)) => {
            a.iter().map(|v| v.as_str().map(str::to_string).ok_or("map.attachFiles: `paths` must be file paths")).collect::<Result<_, _>>()?
        }
        Some(Value::Null) | None => {
            let req = PickRequest { kind: PickKind::Files, title: Some(tr("Attach Files").to_string()), filter: None, file_name: None };
            match crate::pick::ask(app, "map.attachFiles", p, "paths", req, |_| None) {
                Picked::Now(v) if !v.is_empty() => v,
                Picked::Now(_) | Picked::Later => return Ok(json!({"asked": true})),
                Picked::Unavailable => return Err("map.attachFiles: no file dialog here: give `paths`".into()),
            }
        }
        Some(v) => return Err(format!("map.attachFiles: `paths` must be a list of file paths, not {v}")),
    };
    if paths.is_empty() {
        return Ok(json!({"attached": []}));
    }
    let mut target = p.clone();
    if let Some(o) = target.as_object_mut() {
        o.remove("paths");
    }
    let (photos, docs): (Vec<String>, Vec<String>) =
        paths.into_iter().partition(|f| lightcraft_engine::import::is_supported(std::path::Path::new(f)));
    let big: u64 = docs.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
    let background = big > BACKGROUND_BYTES && cfg!(not(target_arch = "wasm32"));
    let now: Vec<String> = if background { photos } else { photos.into_iter().chain(docs.iter().cloned()).collect() };
    let mut marker = target.get("marker").and_then(Value::as_u64);
    let mut result = json!({"attached": [], "photos": [], "errors": []});
    if !now.is_empty() || marker.is_none() {
        let mut params = target.clone();
        params["paths"] = json!(now);
        if now.is_empty() {
            // a new marker for the files copied in the background
            let r = app.run("map.addMarker", json!({"lat": target["lat"], "lon": target["lon"]}))?;
            marker = r["id"].as_u64();
        } else {
            result = app.run("map.attach", params)?;
            marker = result["marker"].as_u64().or(marker);
        }
    }
    let Some(marker) = marker else { return Err("map.attachFiles: give `marker`, or `lat` and `lon`".into()) };
    if background {
        start_copy(app, marker, docs)?;
        result["background"] = json!(true);
    }
    result["marker"] = json!(marker);
    report(app, &result);
    let _ = open_marker(app, &json!({"id": marker}));
    Ok(result)
}

/// A toast for what an attach did.
fn report(app: &mut LightcraftApp, r: &Value) {
    let n = r["attached"].as_array().map_or(0, Vec::len) + r["photos"].as_array().map_or(0, Vec::len);
    let errors = r["errors"].as_array().cloned().unwrap_or_default();
    if let Some(first) = errors.first() {
        let path = first["path"].as_str().unwrap_or_default();
        let name = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let why = first["error"].as_str().unwrap_or_default();
        toast(app, tr_format!("Attached {n}; {e} not attached ({name}: {why})", n = n, e = errors.len(), name = name, why = why));
    } else if n > 0 {
        toast(app, tr_format!("Attached {n} file{}", if n == 1 { "" } else { "s" }, n = n));
    }
}

/// Copy `docs` into the marker's folder on a worker; [`poll`] attaches them when it is done.
#[cfg(not(target_arch = "wasm32"))]
fn start_copy(app: &mut LightcraftApp, marker: u64, docs: Vec<String>) -> Result<(), String> {
    let lib = lightcraft_engine::cmd::markers::library_dir(&app.session, "map.attachFiles").map_err(|e| e.to_string())?;
    let guard = app.session.activity.start("attach", "Copying files into the library", lightcraft_engine::activity::Cancel::Yes);
    let (tx, rx) = std::sync::mpsc::channel();
    let repaint = app.tasks.repaint.clone();
    std::thread::Builder::new()
        .name("lc-attach".into())
        .spawn(move || {
            let staged = lightcraft_engine::cmd::markers::stage(&lib, lightcraft_catalog::MarkerId(marker), &docs, &guard.handle());
            drop(guard);
            let _ = tx.send(staged);
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
        })
        .map_err(|e| format!("map.attachFiles: {e}"))?;
    app.ui.map.markers.jobs.0.lock().unwrap_or_else(PoisonError::into_inner).push((marker, rx));
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn start_copy(_: &mut LightcraftApp, _: u64, _: Vec<String>) -> Result<(), String> {
    Err("map.attachFiles: attached files are kept in a library folder on disk".into())
}

/// Each frame: attach what the workers finished copying.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    let done: Vec<(u64, Vec<Staged>)> = {
        let mut jobs = app.ui.map.markers.jobs.0.lock().unwrap_or_else(PoisonError::into_inner);
        if jobs.is_empty() {
            return;
        }
        let mut done = Vec::new();
        jobs.retain(|(marker, rx)| match rx.try_recv() {
            Ok(staged) => {
                done.push((*marker, staged));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        });
        if !jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        done
    };
    for (marker, staged) in done {
        let mut errors: Vec<Value> = Vec::new();
        let mut copies: Vec<Value> = Vec::new();
        for s in staged {
            match s.result {
                Ok(c) => copies.push(json!({"path": c.rel, "fileName": c.file_name})),
                Err(e) => errors.push(json!({"path": s.source, "error": e})),
            }
        }
        let mut r = if copies.is_empty() {
            json!({"attached": [], "photos": []})
        } else {
            match app.run("map.attach", json!({"marker": marker, "staged": copies})) {
                Ok(r) => r,
                Err(e) => {
                    app.toast_error(ctx, e);
                    continue;
                }
            }
        };
        if let Some(list) = r["errors"].as_array_mut() {
            list.extend(errors);
        } else {
            r["errors"] = json!(errors);
        }
        report(app, &r);
    }
}

/// `map.openAttachment` / `map.revealAttachment {marker, attachment}`: hand the library's copy to
/// the system's default app (documents and pictures only), or show it in its folder.
fn attachment_file(app: &mut LightcraftApp, p: &Value, reveal: bool) -> Result<Value, String> {
    let r = app.session.execute("map.attachmentPath", p).map_err(|e| e.to_string())?;
    let path = r["path"].as_str().unwrap_or_default().to_string();
    if r["exists"] != true {
        return Err(tr_format!("{name} is missing from the library folder", name = r["fileName"].as_str().unwrap_or_default()));
    }
    if reveal {
        let f = app.services.reveal.as_mut().ok_or("not available here")?;
        f(&path)?;
        return Ok(json!({"path": path, "revealed": true}));
    }
    if r["openable"] != true {
        return Err(r["reason"].as_str().unwrap_or("this file isn't opened").to_string());
    }
    let f = app.services.open_with.as_mut().ok_or("not available here")?;
    f(&path, "")?;
    Ok(json!({"path": path, "opened": true}))
}

/// Files dropped while the Map view is up attach to the open card's marker, else at the point
/// under the pointer (a new marker; the map's centre when the pointer isn't on it). `None` when
/// the drop was taken; the paths back when it is not the map's.
pub fn take_drop(app: &mut LightcraftApp, ctx: &egui::Context, paths: Vec<String>) -> Option<Vec<String>> {
    if app.ui.view != crate::state::ViewMode::Map || paths.is_empty() {
        return Some(paths);
    }
    let Some(vp) = app.ui.map.markers.viewport else { return Some(paths) };
    let params = match app.ui.map.markers.open {
        Some(id) => json!({"marker": id, "paths": paths}),
        None => {
            let at = ctx.input(|i| i.pointer.latest_pos()).filter(|p| vp.rect.contains(*p)).unwrap_or(vp.rect.center());
            let (x, y) = vp.world_at(at);
            let (lat, lon) = lightcraft_tiles::mercator::from_world(x, y);
            json!({"lat": lat, "lon": lon, "paths": paths})
        }
    };
    if let Err(e) = app.run("map.attachFiles", params) {
        app.toast_error(ctx, e);
    }
    None
}

/// What the drop card says while files are dragged over the Map view.
pub fn drop_hint(app: &LightcraftApp, n: usize) -> Option<String> {
    if app.ui.view != crate::state::ViewMode::Map || app.ui.map.markers.viewport.is_none() {
        return None;
    }
    Some(match app.ui.map.markers.open {
        Some(_) => tr_format!("Drop to attach {n} file{} to the open marker", if n == 1 { "" } else { "s" }, n = n),
        None => tr_format!("Drop to attach {n} file{} at this point on the map", if n == 1 { "" } else { "s" }, n = n),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_well() {
        assert_eq!(size_text(12), "12 B");
        assert_eq!(size_text(12 * 1024), "12 KB");
        assert_eq!(size_text(3_565_158), "3.4 MB");
    }
}
