//! A photo shown large in the Map view's place, beside its inspection log (`map.showPhoto`).
//!
//! A double-click on a photo in the Map view's filmstrip, or on a photo's pin, shows the photo
//! where the map was, with its log on the right (below on a narrow window). The map isn't drawn
//! meanwhile, so it is exactly as it was when the photo goes: Esc, **Back to Map**, a double-click
//! on the photo, or the same double-click on its filmstrip thumbnail again. Left / Right and clicks
//! in the filmstrip show the other photos of the strip (as narrowed to a spot, when it is).
//!
//! The photo is rendered off the UI thread like the second window's view ([`Slot::MapPhoto`]):
//! the loupe's job sized to the area, with the loupe's own render or the thumbnail standing in
//! until it is done.

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::i18n::tr;
use crate::render::Slot;
use crate::state::ViewMode;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// Narrower than this, the log goes below the photo.
const SIDE_BY_SIDE: f32 = 900.0;
/// The log's width beside the photo.
const LOG_W: f32 = 380.0;
/// A double-click this soon after the photo shown followed the click that began it shows that
/// photo rather than going back to the map (the first click of a double-click on another
/// thumbnail already brought it up).
const JUST_SWITCHED: f64 = 0.8;

/// Is the Map view showing a photo in the map's place?
pub fn photo_mode(app: &LightcraftApp) -> bool {
    app.ui.view == ViewMode::Map && app.ui.map.photo.is_some()
}

/// The photos of the Map view's filmstrip: the grid's list, narrowed to a spot's when it is.
fn strip(app: &mut LightcraftApp) -> Vec<PhotoId> {
    let mut ids = app.session.visible_cloned();
    if !app.ui.map.focus.is_empty() {
        let focus: std::collections::HashSet<u64> = app.ui.map.focus.iter().copied().collect();
        ids.retain(|id| focus.contains(&id.0));
    }
    ids
}

/// `map.showPhoto {id | null}`.
pub fn run(app: &mut LightcraftApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if id != "map.showPhoto" {
        return None;
    }
    Some(match p.get("id") {
        None | Some(Value::Null) => {
            close_photo(app);
            Ok(json!({"photo": null}))
        }
        Some(v) => match v.as_u64() {
            Some(n) => show_photo(app, PhotoId(n)),
            None => Err(format!("map.showPhoto: `id` must be a photo id (or null for the map), not {v}")),
        },
    })
}

/// Show photo `id` in the map's place (and select it, so the filmstrip and Info follow).
fn show_photo(app: &mut LightcraftApp, id: PhotoId) -> Result<Value, String> {
    if app.session.catalog.photo(id).is_none() {
        return Err(format!("map.showPhoto: no such photo {}", id.0));
    }
    if app.session.active() != Some(id) {
        app.run("library.select", json!({"ids": [id.0], "mode": "replace"}))?;
    }
    app.ui.view = ViewMode::Map;
    app.ui.map.photo = Some(id.0);
    // the map isn't drawn meanwhile: nothing may be dropped on where it was
    app.ui.map.markers.viewport = None;
    Ok(json!({"photo": id.0}))
}

/// Back to the map, as it was: its place, the narrowed filmstrip and an open marker card are
/// untouched; the photo stepped to meanwhile doesn't pull the map to its pin.
pub fn close_photo(app: &mut LightcraftApp) {
    if app.ui.map.photo.take().is_none() {
        return;
    }
    app.ui.map.followed = app.session.active().map(|a| a.0);
    app.ui.map.notes.switched = None;
    app.ui.map.notes.close_map_editor();
    app.renderer.release(Slot::MapPhoto);
}

/// A double-click on photo `id` in the Map view's filmstrip: show it in the map's place, or go
/// back to the map when it is the photo shown.
pub fn film_double_click(app: &mut LightcraftApp, id: PhotoId, now: f64) {
    let just = app.ui.map.notes.switched.is_some_and(|(p, at)| p == id.0 && now - at < JUST_SWITCHED);
    if app.ui.map.photo == Some(id.0) && !just {
        close_photo(app);
    } else if let Err(e) = show_photo(app, id) {
        log::warn!("{e}");
    }
}

/// Left / Right in photo mode: the previous / next photo of the filmstrip. Returns whether the key
/// was taken (it was in photo mode).
pub fn step_photo(app: &mut LightcraftApp, step: i64) -> bool {
    if !photo_mode(app) {
        return false;
    }
    let ids = strip(app);
    let Some(shown) = app.ui.map.photo else { return false };
    let next = ids
        .iter()
        .position(|p| p.0 == shown)
        .and_then(|i| i64::try_from(i).ok())
        .and_then(|i| i.checked_add(step))
        .and_then(|i| usize::try_from(i).ok())
        .and_then(|i| ids.get(i).copied());
    if let Some(id) = next
        && let Err(e) = show_photo(app, id)
    {
        log::warn!("{e}");
    }
    true
}

/// Draw the photo mode in `area` (where the map goes). Returns `false` when no photo is shown, so
/// the map is drawn instead.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, area: Rect) -> bool {
    let Some(shown) = app.ui.map.photo else {
        if app.renderer.textures.contains_key(&Slot::MapPhoto) {
            app.renderer.release(Slot::MapPhoto);
        }
        return false;
    };
    if app.session.catalog.photo(PhotoId(shown)).is_none() {
        // removed (or undone away): back to the map
        close_photo(app);
        return false;
    }
    // a click in the filmstrip shows that photo
    let now = ui.input(|i| i.time);
    if let Some(active) = app.session.active()
        && active.0 != shown
        && strip(app).contains(&active)
    {
        app.ui.map.photo = Some(active.0);
        app.ui.map.notes.switched = Some((active.0, now));
    }
    app.ui.map.markers.viewport = None;
    let Some(id) = app.ui.map.photo.map(PhotoId) else { return false };
    let t = Tokens::get(ui.ctx());
    ui.painter().rect_filled(area, 0.0, t.canvas);
    let (photo_r, log_r) = if area.width() >= SIDE_BY_SIDE {
        let split = area.right() - LOG_W;
        (Rect::from_min_max(area.min, pos2(split, area.bottom())), Rect::from_min_max(pos2(split, area.top()), area.max))
    } else {
        let split = area.top() + (area.height() * 0.55).max(120.0).min(area.height());
        (Rect::from_min_max(area.min, pos2(area.right(), split)), Rect::from_min_max(pos2(area.left(), split), area.max))
    };
    register(ui, "map:photo", photo_r);
    picture(app, ui, photo_r, id);
    // the way back, top-left over the photo
    let back = Rect::from_min_size(photo_r.min + vec2(12.0, 12.0), vec2(140.0, 26.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(back).layout(egui::Layout::left_to_right(egui::Align::Center)));
    if text_button(&mut child, "mapPhotoBack", "Back to Map", false)
        .on_hover_text(tr("Show the map again (Esc, or double-click the photo)"))
        .clicked()
    {
        close_photo(app);
        return true;
    }
    super::notes::panel(app, ui, log_r, id);
    true
}

/// The photo fitted in `r`, its file name and capture time under it; a double-click on it goes
/// back to the map.
fn picture(app: &mut LightcraftApp, ui: &mut egui::Ui, r: Rect, id: PhotoId) {
    let t = Tokens::get(ui.ctx());
    let caption_h = 28.0;
    let img_area = Rect::from_min_max(r.min + vec2(12.0, 48.0), pos2(r.right() - 12.0, r.bottom() - caption_h)).intersect(r);
    let resp = ui.interact(r, egui::Id::unique("lc-map-photo"), Sense::click());
    let name = app.session.catalog.photo(id).map(|p| p.file_name.clone()).unwrap_or_default();
    crate::widgets::announce(&resp, &name);
    if img_area.width() > 8.0 && img_area.height() > 8.0 {
        let ppp = ui.ctx().pixels_per_point();
        let side = crate::panels::detail::texture_side(ui.ctx()).clamp(64, 4096);
        let (w, h) = (((img_area.width() * ppp) as usize).clamp(64, side), ((img_area.height() * ppp) as usize).clamp(64, side));
        if let Some(job) = app.session.loupe_job(id, w, h, true).map(|j| app.renderer.job_for(Slot::MapPhoto, j))
            && app.renderer.textures.get(&Slot::MapPhoto).is_none_or(|x| x.key != job.key)
            && !app.renderer.is_pending(Slot::MapPhoto)
        {
            app.renderer.request(Slot::MapPhoto, job, 90);
        }
        crate::panels::grid::request_thumb(app, id, 512, 20);
        let mine = |s: Slot| app.renderer.textures.get(&s).filter(|x| x.photo == id);
        match mine(Slot::MapPhoto).or_else(|| mine(Slot::Main)).or_else(|| app.renderer.thumb(id)) {
            Some(tex) => {
                let fit = crate::panels::detail::fit_texture_rect(img_area, tex.size);
                ui.painter().image(tex.tex.id(), fit, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                register(ui, "map:photoImage", fit);
            }
            None => {
                ui.painter().rect_filled(img_area, 0.0, t.skeleton);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }
    let when = app.session.catalog.photo(id).and_then(|p| p.captured.clone()).map(|c| crate::i18n::display_time(&c)).unwrap_or_default();
    let line = if when.is_empty() { name } else { format!("{name}  ·  {when}") };
    ui.painter().text(pos2(r.center().x, r.bottom() - caption_h / 2.0), Align2::CENTER_CENTER, line, t.font(12.0), t.text_label);
    if resp.double_clicked() {
        close_photo(app);
    }
}
