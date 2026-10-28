//! The Info panel's mini-map: the active photo's place at [`PHOTO_ZOOM`], its pin, its view
//! cone and its accuracy circle, under the GPS field. Static (no pan or zoom); a click opens the Map view there. Tiles
//! only when the user has turned them on, as in the Map view; it never asks for them itself.

use egui::{Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_tiles::mercator;
use serde_json::json;

use super::widget::{self, Viewport};
use super::{PHOTO_ZOOM, pins, view};
use crate::LightcraftApp;
use crate::i18n::tr;
use crate::theme::Tokens;

/// The mini-map's height (points); it is as wide as the panel.
pub const MINI_MAP_H: f32 = 180.0;

/// The mini-map for photo `id`, `width` points wide, when it has a usable location (nothing otherwise).
pub fn mini_map(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId, width: f32) {
    let Some(p) = app.session.catalog.photo(id) else { return };
    let Some((lat, lon)) = p.meta.gps.filter(|g| pins::usable(*g)) else { return };
    let direction = p.meta.gps_direction.filter(|d| d.is_finite());
    let accuracy = p.meta.gps_accuracy;
    let world = mercator::to_world(lat, lon);
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(width.min(ui.available_width()).max(40.0), MINI_MAP_H), Sense::hover());
    let resp = ui
        .interact(rect, egui::Id::unique("lc-info-mini-map"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tr("Open in Map view"));
    crate::widgets::announce(&resp, tr("Map of the photo's location: open in Map view"));
    crate::widgets::register(ui, "info:miniMap", rect);

    let vp = Viewport { rect, center: world, zoom: PHOTO_ZOOM };
    widget::draw_background(ui, &vp);
    let mut attribution = None;
    view::apply_cache_limit(app, ui.ctx());
    if let (Some(source), Some(provider)) = (app.ui.settings.map_tiles.source(&app.ui.settings.map_custom), app.services.map_tiles.as_deref()) {
        if widget::draw_tiles(ui, &vp, provider, &source) {
            view::note_loading(ui.ctx());
        }
        attribution = Some(source.attribution.to_string());
    }
    super::ortho::draw_mini(app, ui, &vp);
    let painter = ui.painter_at(rect);
    let at = vp.screen_of(world);
    if let Some(r) = accuracy.and_then(|e| widget::accuracy_radius(e, lat, vp.zoom)) {
        widget::draw_accuracy(&painter, at, r, widget::ACCURACY);
        crate::widgets::register_geometry(ui.ctx(), "info:miniMapAccuracy", Rect::from_center_size(at, vec2(2.0 * r, 2.0 * r)));
    }
    if let Some(d) = direction {
        widget::draw_cone(&painter, at, d, widget::CONE);
    }
    widget::draw_pin(&painter, at, false, t.accent);
    // the attribution bottom-right, the scale bar above it (a narrow panel has no room beside it)
    let mut bar = vp;
    if let Some(a) = &attribution {
        let r = widget::draw_attribution_as(ui, &vp, a, 9.5, "info:miniMapAttribution");
        bar.rect = Rect::from_min_max(rect.min, pos2(rect.right(), r.top()));
    }
    widget::draw_scale_bar(ui, &bar);
    let edge = if resp.hovered() { t.accent } else { t.divider };
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, edge), StrokeKind::Inside);
    if resp.clicked()
        && let Err(e) = app.run("view.map", json!({"photo": id.0}))
    {
        app.toast(ui.ctx(), e);
    }
}
