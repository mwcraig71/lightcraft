//! Moving photos, markers and attached files to another place on the Map view (right-click ▸
//! Move…, a marker card's **Move To…**). See docs/map.md → Moving photos, markers and files.
//!
//! `map.beginMove` puts the map in a placing mode: a banner across the top says what moves and
//! how to cancel, the pointer is a crosshair and a ghost of what moves follows it (a group as its
//! shape, translated). Drag still pans and the wheel zooms; a click without a drag places
//! (`map.place {lat, lon}`); Esc, a right-click or the banner's Cancel (`map.cancelMove`) go back
//! to the ordinary map at once. After a move the map stays where it is and a notice says what
//! moved, with Undo.
//!
//! The moves themselves are engine commands: `photo.setLocation` (translate the group, or with
//! **Stack at one point** put every photo on the point), `map.updateMarker {id, lat, lon}` (the
//! marker and its files; its photos keep their own places) and `map.moveAttachment` (one file to
//! the marker clicked, or to a new marker at the point).

use egui::{Align2, Color32, Pos2, Rect, RichText, Stroke, UiBuilder, pos2, vec2};
use serde_json::{Value, json};

use super::widget::{self, Viewport};
use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// Most ghost pins drawn for a group (the move itself takes them all).
const MAX_GHOSTS: usize = 2000;
/// How long the "Moved 3 photos · Undo" notice stays (seconds).
const NOTICE_SECONDS: f64 = 8.0;
/// Most photos one move takes (as `photo.setLocation`).
const MAX_PHOTOS: usize = lightcraft_engine::cmd::relocate::MAX_PHOTOS;

/// What is being moved.
#[derive(Clone, Debug, PartialEq)]
pub enum Moving {
    Photos(Vec<u64>),
    Marker(u64),
    Attachment { marker: u64, attachment: u64, name: String },
}

/// The placing mode: what moves, and the ghost that follows the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct Placing {
    pub what: Moving,
    /// The world points of what moves now (photos with a location; the marker).
    pub ghost: Vec<(f64, f64)>,
    /// Their centroid: it goes where the click is (translate).
    pub anchor: Option<(f64, f64)>,
    /// The frame the map last drew it in (leaving the Map view ends the mode).
    pub frame: Option<u64>,
}

/// The notice after a move: its text, until when, and the undo step it can take back.
#[derive(Clone, Debug, PartialEq)]
pub struct Done {
    pub text: String,
    pub until: f64,
    /// The undo step's label and the undo stack's length right after the move: Undo is offered
    /// while that step is still the last one.
    pub label: String,
    pub undo_len: usize,
}

/// The Map view's moving state (not saved).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoveUi {
    pub placing: Option<Placing>,
    /// **Stack at one point**: a group goes onto the clicked point instead of keeping its shape.
    pub stack: bool,
    pub done: Option<Done>,
}

/// Is the map placing something?
pub fn active(app: &LightcraftApp) -> bool {
    app.ui.map.moving.placing.is_some()
}

/// Leave the placing mode (Esc's Back does this first). Returns whether it was on.
pub fn cancel(app: &mut LightcraftApp) -> bool {
    app.ui.map.moving.placing.take().is_some()
}

/// The world points of these photos that have a usable location.
fn located(app: &LightcraftApp, ids: &[u64]) -> Vec<(f64, f64)> {
    ids.iter()
        .filter_map(|id| app.session.catalog.photo(lightcraft_catalog::PhotoId(*id)))
        .filter_map(|p| p.meta.gps.filter(|g| super::pins::usable(*g)))
        .map(|(lat, lon)| lightcraft_tiles::mercator::to_world(lat, lon))
        .collect()
}

/// The centroid of world points (x taken relative to the first, so a group across the 180th
/// meridian stays together, as `photo.setLocation` does).
fn centroid(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let first = *points.first()?;
    let n = points.len() as f64;
    let (sx, sy) = points.iter().fold((0.0, 0.0), |(a, b), w| {
        let dx = w.0 - first.0;
        (a + dx - dx.round(), b + w.1 - first.1)
    });
    Some((first.0 + sx / n, first.1 + sy / n))
}

// ---------------------------------------------------------------- commands

/// `map.beginMove`, `map.cancelMove`, `map.place`; `None` for any other id.
pub fn run(app: &mut LightcraftApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "map.beginMove" => begin(app, p),
        "map.cancelMove" => Ok(json!({"cancelled": cancel(app)})),
        "map.place" => place(app, p),
        _ => return None,
    })
}

/// `map.beginMove {photos: [ids]} | {marker} | {attachment: {marker, attachment}}`.
fn begin(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    const C: &str = "map.beginMove";
    let given = ["photos", "marker", "attachment"].iter().filter(|k| p.get(**k).is_some_and(|v| !v.is_null())).count();
    if given != 1 {
        return Err(format!("{C}: give one of `photos` (photo ids), `marker` (a marker id) or `attachment` ({{marker, attachment}})"));
    }
    let marker_of = |app: &LightcraftApp, v: &Value, what: &str| -> Result<lightcraft_catalog::Marker, String> {
        let id = v.as_u64().ok_or_else(|| format!("{C}: `{what}` must be a marker id, not {v}"))?;
        app.session.catalog.marker(lightcraft_catalog::MarkerId(id)).cloned().ok_or_else(|| format!("{C}: no marker {id}"))
    };
    let placing = if let Some(v) = p.get("photos").filter(|v| !v.is_null()) {
        let list = v.as_array().ok_or_else(|| format!("{C}: `photos` must be a list of photo ids, not {v}"))?;
        if list.is_empty() {
            return Err(format!("{C}: `photos` is empty: no photos to move"));
        }
        if list.len() > MAX_PHOTOS {
            return Err(format!("{C}: at most {MAX_PHOTOS} photos at a time"));
        }
        let mut ids: Vec<u64> = Vec::with_capacity(list.len());
        for v in list {
            let id = v.as_u64().ok_or_else(|| format!("{C}: `photos` must be photo ids, not {v}"))?;
            if !app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).is_some_and(|ph| !ph.deleted) {
                return Err(format!("{C}: no photo {id}"));
            }
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        let ghost = located(app, &ids);
        Placing { anchor: centroid(&ghost), ghost, what: Moving::Photos(ids), frame: None }
    } else if let Some(v) = p.get("marker").filter(|v| !v.is_null()) {
        let m = marker_of(app, v, "marker")?;
        let w = lightcraft_tiles::mercator::to_world(m.lat, m.lon);
        Placing { what: Moving::Marker(m.id.0), ghost: vec![w], anchor: Some(w), frame: None }
    } else {
        let a = p.get("attachment").unwrap_or(&Value::Null);
        let m = marker_of(app, a.get("marker").unwrap_or(&Value::Null), "attachment.marker")?;
        let aid = a.get("attachment").and_then(Value::as_u64).ok_or_else(|| format!("{C}: `attachment.attachment` must be an attachment id"))?;
        let att = m.attachment(aid).ok_or_else(|| format!("{C}: marker {} has no attachment {aid}", m.id.0))?;
        Placing {
            what: Moving::Attachment { marker: m.id.0, attachment: aid, name: att.file_name.clone() },
            ghost: Vec::new(),
            anchor: None,
            frame: None,
        }
    };
    let count = match &placing.what {
        Moving::Photos(ids) => ids.len(),
        _ => 1,
    };
    let located = placing.ghost.len();
    let st = &mut app.ui.map.moving;
    st.placing = Some(placing);
    st.done = None;
    app.ui.view = crate::state::ViewMode::Map;
    Ok(json!({"placing": true, "count": count, "located": located}))
}

/// `map.place {lat, lon, marker?}`: put what is being placed there. An attachment goes to
/// `marker` when given (a click on a marker), else to a new marker at the point.
fn place(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    const C: &str = "map.place";
    let num = |k: &str| p.get(k).and_then(Value::as_f64).filter(|x| x.is_finite()).ok_or_else(|| format!("{C}: `{k}` must be a number of degrees"));
    let (lat, lon) = (num("lat")?, num("lon")?);
    if lat.abs() > 90.0 || lon.abs() > 180.0 {
        return Err(format!("{C}: latitude must be within ±90 and longitude within ±180"));
    }
    let lat = lat.clamp(-lightcraft_engine::cmd::relocate::MAX_LAT, lightcraft_engine::cmd::relocate::MAX_LAT);
    let onto = match p.get("marker") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().ok_or_else(|| format!("{C}: `marker` must be a marker id, not {v}"))?),
    };
    let Some(placing) = app.ui.map.moving.placing.clone() else {
        return Err(format!("{C}: nothing is being moved (map.beginMove first)"));
    };
    let stack = app.ui.map.moving.stack;
    let (result, text) = match &placing.what {
        Moving::Photos(ids) => {
            // photos without a location can only be stacked on the point
            let mode = if stack || placing.ghost.is_empty() { "stack" } else { "translate" };
            let r = app.run("photo.setLocation", json!({"ids": ids, "lat": lat, "lon": lon, "mode": mode}));
            let text = r.as_ref().ok().map(|r| {
                let n = r["moved"].as_u64().unwrap_or(0);
                let skipped = r["skipped"].as_u64().unwrap_or(0);
                let moved = if n == 1 { tr("Moved 1 photo").to_string() } else { tr_format!("Moved {n} photos", n = n) };
                if skipped > 0 { tr_format!("{moved} ({n} without a location left as they were)", moved = moved, n = skipped) } else { moved }
            });
            (r, text)
        }
        Moving::Marker(id) => {
            let r = app.run("map.updateMarker", json!({"id": id, "lat": lat, "lon": lon}));
            (r, Some(tr("Moved the marker").to_string()))
        }
        Moving::Attachment { marker, attachment, name } => {
            let to = match onto.filter(|m| m != marker) {
                Some(m) => json!(m),
                None => json!({"lat": lat, "lon": lon}),
            };
            let r = app.run("map.moveAttachment", json!({"marker": marker, "attachment": attachment, "to": to}));
            let text = r.as_ref().ok().map(|r| {
                if r["created"] == true {
                    tr_format!("Moved {name} to a new marker", name = name)
                } else {
                    tr_format!("Moved {name} to marker {m}", name = name, m = r["marker"].as_u64().unwrap_or(0))
                }
            });
            (r, text)
        }
    };
    app.ui.map.moving.placing = None;
    let r = result?;
    // the map stays where it is: these pins count as fitted already
    let key = super::pins::collect(app).key;
    app.ui.map.fitted = Some(key);
    let label = app.session.undo.last().map(|e| e.label.clone()).unwrap_or_default();
    let now = app.tick_now();
    app.ui.map.moving.done = Some(Done { text: text.unwrap_or_default(), until: now + NOTICE_SECONDS, label, undo_len: app.session.undo.len() });
    Ok(r)
}

/// Take the last move back (the notice's Undo).
fn undo(app: &mut LightcraftApp) {
    let Some(done) = app.ui.map.moving.done.take() else { return };
    if !undoable(app, &done) {
        return;
    }
    let ctx = app.tasks.repaint.clone().unwrap_or_default();
    match app.run("edit.undo", json!({})) {
        Ok(_) => {
            let key = super::pins::collect(app).key;
            app.ui.map.fitted = Some(key);
            app.toast(&ctx, tr("Move undone"));
        }
        Err(e) => app.toast_error(&ctx, e),
    }
}

/// Whether the move is still the last undo step.
fn undoable(app: &LightcraftApp, done: &Done) -> bool {
    !done.label.is_empty() && app.session.undo.len() == done.undo_len && app.session.undo.last().is_some_and(|e| e.label == done.label)
}

// ---------------------------------------------------------------- the map

/// While placing, each frame: a click places (on a marker: an attachment goes to it), Esc or a
/// right-click cancels, the pointer is a crosshair with the ghost under it.
pub fn frame(app: &mut LightcraftApp, ui: &mut egui::Ui, vp: &Viewport, resp: &egui::Response) {
    let marker_at = |app: &LightcraftApp, p: Pos2| super::markers::marker_at(app, vp, p);
    let frame = ui.ctx().cumulative_frame_nr();
    let Some(placing) = app.ui.map.moving.placing.as_mut() else { return };
    // the map wasn't drawn last frame (another view was up): the move was left behind
    if placing.frame.is_some_and(|f| frame > f.saturating_add(1)) {
        app.ui.map.moving.placing = None;
        return;
    }
    placing.frame = Some(frame);
    if resp.secondary_clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        crate::widgets::take_escape(ui.ctx());
        cancel(app);
        return;
    }
    let pointer = resp.hover_pos();
    if resp.clicked()
        && let Some(at) = resp.interact_pointer_pos().or(pointer)
    {
        let (x, y) = vp.world_at(at);
        let (lat, lon) = lightcraft_tiles::mercator::from_world(x, y);
        let mut params = json!({"lat": lat, "lon": lon});
        if let Some(m) = marker_at(app, at) {
            params["marker"] = json!(m);
        }
        let ctx = ui.ctx().clone();
        if let Err(e) = app.run("map.place", params) {
            app.toast_error(&ctx, e);
        }
        return;
    }
    if let Some(at) = pointer {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        let target = marker_at(app, at);
        ghost(app, ui, vp, at, target);
    }
}

/// What moves, drawn under the pointer: the group's shape (or one stacked dot), the marker, or
/// the file on its way to a marker.
fn ghost(app: &LightcraftApp, ui: &egui::Ui, vp: &Viewport, at: Pos2, onto: Option<u64>) {
    let Some(placing) = app.ui.map.moving.placing.as_ref() else { return };
    let t = Tokens::get(ui.ctx());
    let mut painter = ui.painter_at(vp.rect);
    // from where it is now to where it would go
    if let Some(anchor) = placing.anchor {
        let from = vp.screen_of(anchor);
        painter.line_segment([from, at], Stroke::new(1.5, Color32::from_white_alpha(170)));
        painter.circle_stroke(from, 4.0, Stroke::new(1.5, Color32::from_white_alpha(200)));
    }
    painter.set_opacity(0.75);
    match &placing.what {
        Moving::Photos(ids) => {
            let stacked = app.ui.map.moving.stack || placing.anchor.is_none();
            match placing.anchor {
                Some(anchor) if !stacked => {
                    let shift = at - vp.screen_of(anchor);
                    for w in placing.ghost.iter().take(MAX_GHOSTS) {
                        widget::draw_pin(&painter, vp.screen_of(*w) + shift, true, t.accent);
                    }
                }
                _ if ids.len() == 1 => widget::draw_pin(&painter, at, true, t.accent),
                _ => {
                    painter.circle(at, 13.0, t.accent, Stroke::new(2.0, Color32::WHITE));
                    painter.text(at, Align2::CENTER_CENTER, ids.len().to_string(), t.semibold(11.5), Color32::WHITE);
                }
            }
        }
        Moving::Marker(_) => super::markers::draw_marker(&painter, at, 10.0, 0, true, t.accent),
        Moving::Attachment { name, .. } => {
            super::markers::draw_marker(&painter, at, 10.0, 0, onto.is_some(), t.accent);
            let label = match onto {
                Some(m) => tr_format!("{name} → marker {m}", name = name, m = m),
                None => tr_format!("{name} → a new marker here", name = name),
            };
            let g = crate::widgets::one_line(&painter, &label, t.font(11.0), Color32::WHITE, 260.0);
            let r = Rect::from_min_size(at + vec2(14.0, 8.0), g.size() + vec2(10.0, 4.0));
            painter.rect_filled(r, 3.0, Color32::from_black_alpha(190));
            painter.galley(r.min + vec2(5.0, 2.0), g, Color32::WHITE);
        }
    }
    // the exact spot
    painter.set_opacity(1.0);
    let s = Stroke::new(1.0, Color32::WHITE);
    painter.line_segment([at - vec2(9.0, 0.0), at + vec2(9.0, 0.0)], s);
    painter.line_segment([at - vec2(0.0, 9.0), at + vec2(0.0, 9.0)], s);
    crate::widgets::register_geometry(ui.ctx(), "map:moveGhost", Rect::from_center_size(at, vec2(20.0, 20.0)));
}

/// The banner while placing ("Click the new location for 3 photos — Esc to cancel", Stack at one
/// point, Cancel), or the notice after a move ("Moved 3 photos", Undo), across the top of the map.
pub fn banner(app: &mut LightcraftApp, ui: &mut egui::Ui, map: Rect) {
    let now = app.tick_now();
    if app.ui.map.moving.done.as_ref().is_some_and(|d| now > d.until) {
        app.ui.map.moving.done = None;
    }
    let Some((text, group, done)) = banner_text(app) else { return };
    let t = Tokens::get(ui.ctx());
    let ctx = ui.ctx().clone();
    let width = (map.width() - 24.0).clamp(120.0, 640.0);
    let pos = pos2(map.center().x - width / 2.0, map.top() + 46.0);
    let area = egui::Area::new(egui::Id::unique("lc-map-move-banner")).order(egui::Order::Foreground).fixed_pos(pos).show(&ctx, |ui| {
        egui::Frame::new()
            .fill(if done { t.chrome } else { t.accent })
            .stroke(Stroke::new(1.0, t.divider))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(12, 6))
            .show(ui, |ui| {
                let (row, _) = ui.allocate_exact_size(vec2(width - 24.0, 24.0), egui::Sense::hover());
                // the buttons at the right, the text in what is left of the row
                let mut right = ui.new_child(UiBuilder::new().max_rect(row).layout(egui::Layout::right_to_left(egui::Align::Center)));
                right.spacing_mut().item_spacing.x = 6.0;
                {
                    let ui = &mut right;
                    if done {
                        let can = app.ui.map.moving.done.as_ref().is_some_and(|d| undoable(app, d));
                        if can && text_button(ui, "mapMoveUndo", "Undo", false).on_hover_text(tr("Put them back where they were")).clicked() {
                            undo(app);
                        }
                    } else {
                        if text_button(ui, "mapMoveCancel", "Cancel", false).on_hover_text(tr("Leave everything where it is (Esc)")).clicked() {
                            cancel(app);
                        }
                        if group {
                            let on = app.ui.map.moving.stack;
                            if text_button(ui, "mapMoveStack", "Stack at one point", on)
                                .on_hover_text(tr("Put every photo on the point clicked, instead of keeping their arrangement"))
                                .clicked()
                            {
                                app.ui.map.moving.stack = !on;
                            }
                        }
                    }
                }
                let used = right.min_rect();
                let left_end = if used.width() > 0.0 { used.left() - 10.0 } else { row.right() };
                let text_rect = Rect::from_min_max(row.min, pos2(left_end.max(row.left() + 20.0), row.bottom()));
                let mut left = ui.new_child(UiBuilder::new().max_rect(text_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
                let color = if done { t.text } else { Color32::WHITE };
                let label = left.add(egui::Label::new(RichText::new(&text).font(t.semibold(12.5)).color(color)).truncate());
                register(&left, "map:moveText", label.rect);
            });
    });
    register(ui, "map:moveBanner", area.response.rect);
    crate::widgets::screen_reader_name(&ctx, area.response.id, &text);
    if done {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// The banner's text, whether it offers Stack at one point (a group with locations), and whether
/// it is the notice after a move.
fn banner_text(app: &LightcraftApp) -> Option<(String, bool, bool)> {
    let st = &app.ui.map.moving;
    let Some(p) = &st.placing else {
        return st.done.as_ref().map(|d| (d.text.clone(), false, true));
    };
    Some(match &p.what {
        Moving::Photos(ids) => {
            let n = ids.len();
            let text = if p.ghost.is_empty() {
                tr_format!("Click a location for {what} — Esc to cancel", what = widget::photo_count(n))
            } else {
                tr_format!("Click the new location for {what} — Esc to cancel", what = widget::photo_count(n))
            };
            (text, p.ghost.len() > 1, false)
        }
        Moving::Marker(_) => (tr("Click the new location for the marker and its files — Esc to cancel").to_string(), false, false),
        Moving::Attachment { name, .. } => {
            (tr_format!("Click a marker, or a point for a new marker, for {name} — Esc to cancel", name = name), false, false)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_centroid_keeps_a_group_across_the_date_line_together() {
        let c = centroid(&[(0.999, 0.5), (0.001, 0.5)]).unwrap();
        assert!((c.0 - 1.0).abs() < 1e-9, "{c:?}");
        assert_eq!(centroid(&[]), None);
    }
}
