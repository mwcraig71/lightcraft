//! The Map view: a status line ("312 photos · 287 with location"), the map, the filmstrip.

use egui::{Align2, Color32, Rect, RichText, Stroke, StrokeKind, UiBuilder, pos2, vec2};
use serde_json::json;

use super::pins::{self, Cluster, Pins};
use super::widget::{self, CLUSTER_RADIUS, Viewport};
use super::{MapTiles, PHOTO_ZOOM};
use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

const STATUS_H: f32 = 30.0;
/// The frame that last drew the map while some of its tiles were still loading.
const LOADING: &str = "lc-map-tiles-loading";

/// Were tiles still loading in the last frame drawn (headless `ui.settle` waits for them)?
pub fn tiles_loading(ctx: &egui::Context) -> bool {
    let at = ctx.data(|d| d.get_temp::<u64>(egui::Id::unique(LOADING)));
    at.is_some_and(|f| f.saturating_add(1) >= ctx.cumulative_frame_nr())
}
/// Some tiles drawn this frame are still loading: look again soon (and `ui.settle` waits).
pub(super) fn note_loading(ctx: &egui::Context) {
    ctx.request_repaint_after(std::time::Duration::from_millis(120));
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(LOADING), frame));
}

/// Settings ▸ Map ▸ Tile cache, applied to the provider when it changes.
pub(super) fn apply_cache_limit(app: &LightcraftApp, ctx: &egui::Context) {
    let Some(p) = app.services.map_tiles.as_deref() else { return };
    let mb = app.ui.settings.map_cache_mb.max(10);
    let key = egui::Id::unique("lc-map-cache-limit");
    if ctx.data(|d| d.get_temp::<u32>(key)) != Some(mb) {
        p.set_cache_limit(u64::from(mb) * 1_000_000);
        ctx.data_mut(|d| d.insert_temp(key, mb));
    }
}

/// Pins kept clear of the map's edge when fitting.
const FIT_MARGIN: f64 = 48.0;

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let full = ui.max_rect();
    let film_h = if app.ui.filmstrip { t.film_h } else { 0.0 };
    let status = Rect::from_min_max(full.min, pos2(full.right(), full.top() + STATUS_H));
    let map_rect = Rect::from_min_max(pos2(full.left(), status.bottom()), pos2(full.right(), full.bottom() - film_h));
    if app.ui.filmstrip {
        // a click on a cluster narrows the strip to its photos (in the grid's order)
        let mut ids = app.session.visible_cloned();
        if !app.ui.map.focus.is_empty() {
            let focus: std::collections::HashSet<u64> = app.ui.map.focus.iter().copied().collect();
            ids.retain(|id| focus.contains(&id.0));
        }
        crate::panels::detail::filmstrip_of(app, ui, Rect::from_min_max(pos2(full.left(), map_rect.bottom()), full.max), ids);
    }
    let pins = pins::collect(app);
    status_line(app, ui, status, &pins);
    if map_rect.height() < 40.0 || map_rect.width() < 40.0 {
        return;
    }
    register(ui, "map:view", map_rect);
    map(app, ui, map_rect, &pins);
}

fn status_line(app: &mut LightcraftApp, ui: &mut egui::Ui, r: Rect, pins: &Pins) {
    let t = Tokens::get(ui.ctx());
    ui.painter().rect_filled(r, 0.0, t.chrome);
    ui.painter().line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, t.divider));
    let mut child = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(12.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let ui = &mut child;
    let text = tr_format!("{total} photos · {located} with location", total = pins.total, located = pins.pins.len());
    let label = ui.label(RichText::new(text).font(t.font(12.0)).color(t.text_label));
    register(ui, "map:status", label.rect);
    if !pins.without.is_empty() {
        ui.add_space(8.0);
        let without = tr_format!("{n} without location", n = pins.without.len());
        let link = ui.link(RichText::new(without).font(t.font(12.0)).color(t.accent)).on_hover_text(tr(
            "Select the photos that have no location. Camera photos without GPS can get one from a phone's GPX track: Photo → Auto-Tag from Tracklog…",
        ));
        register(ui, "map:without", link.rect);
        if link.clicked() {
            let ids: Vec<u64> = pins.without.iter().map(|i| i.0).collect();
            app.ui.map.focus.clear();
            let _ = app.run("library.select", json!({"ids": ids, "mode": "replace"}));
        }
    }
    // the filmstrip narrowed to one spot's photos, and the way back to all of them
    if !app.ui.map.focus.is_empty() {
        ui.add_space(16.0);
        let n = app.ui.map.focus.len();
        let here = ui.label(RichText::new(tr_format!("Filmstrip: {n} photos here", n = n)).font(t.semibold(12.0)).color(t.text));
        register(ui, "map:focus", here.rect);
        ui.add_space(6.0);
        let all =
            ui.link(RichText::new(tr("Show All")).font(t.font(12.0)).color(t.accent)).on_hover_text(tr("Show every photo in the filmstrip again"));
        register(ui, "map:focusClear", all.rect);
        if all.clicked() {
            let _ = app.run("map.focus", json!({"ids": []}));
        }
    }
}

fn map(app: &mut LightcraftApp, ui: &mut egui::Ui, rect: Rect, pins: &Pins) {
    let mut vp = Viewport::from_state(rect, &app.ui.map);
    // fit on opening, after Fit All, and when the folder or filter changes the pins
    if app.ui.map.fit_pending || app.ui.map.fitted != Some(pins.key) {
        if app.ui.map.fitted != Some(pins.key) {
            // another folder or filter: the narrowed filmstrip belonged to the old one
            app.ui.map.focus.clear();
        }
        fit(&mut vp, pins.pins.iter().map(|p| p.world));
        app.ui.map.fit_pending = false;
        app.ui.map.fitted = Some(pins.key);
        app.ui.map.followed = app.session.active().map(|a| a.0);
    }
    // a photo chosen elsewhere (the filmstrip) brings its pin on screen
    let active = app.session.active();
    if active.map(|a| a.0) != app.ui.map.followed {
        app.ui.map.followed = active.map(|a| a.0);
        if let Some(pin) = active.and_then(|a| pins.pins.iter().find(|p| p.id == a))
            && !vp.contains(pin.world, 24.0)
        {
            vp.center = pin.world;
        }
    }
    let resp = widget::interact(ui, &mut vp);

    let tiles = app.ui.settings.map_tiles;
    let provider = app.services.map_tiles.clone();
    widget::draw_background(ui, &vp);
    let mut attribution = None;
    apply_cache_limit(app, ui.ctx());
    if let (Some(source), Some(provider)) = (tiles.source(&app.ui.settings.map_custom), provider.as_deref()) {
        if widget::draw_tiles(ui, &vp, provider, &source) {
            note_loading(ui.ctx());
        }
        attribution = Some(source.attribution.to_string());
    }

    // an album is the user's name for the job; otherwise the folder its photos share
    let title = crate::i18n::source_title(&app.session);
    let album = matches!(app.session.source, lightcraft_engine::LibrarySource::Album(_));
    let name = |m: &[usize]| if album { title.clone() } else { pins::site_name(&pins.pins, m, &title) };
    let clusters = pins::layout(&pins.pins, vp.scale(), vp.zoom, CLUSTER_RADIUS, &name);
    let selected: std::collections::HashSet<u64> = app.session.selection.ids.iter().map(|i| i.0).chain(app.session.active().map(|a| a.0)).collect();
    widget::draw_pins(ui, &vp, &pins.pins, &clusters, app.ui.map.track, &|id| selected.contains(&id));
    for c in &clusters {
        let at = vp.screen_of(c.world);
        if let Some(name) = &c.site {
            crate::widgets::register_geometry(ui.ctx(), format!("mapSite:{name}"), Rect::from_center_size(at, vec2(20.0, 20.0)));
        } else if c.members.len() == 1
            && let Some(p) = c.members.first().and_then(|i| pins.pins.get(*i))
        {
            crate::widgets::register_geometry(ui.ctx(), format!("mapPin:{}", p.id.0), Rect::from_center_size(at, vec2(12.0, 12.0)));
        } else {
            crate::widgets::register_geometry(ui.ctx(), format!("mapCluster:{}", c.members.len()), Rect::from_center_size(at, vec2(26.0, 26.0)));
        }
    }

    let pointer = resp.hover_pos();
    let under = pointer.and_then(|p| widget::hit(&vp, &clusters, p));
    if resp.clicked()
        && let Some(i) = under
        && let Some(c) = clusters.get(i)
    {
        click(app, ui, &mut vp, pins, c);
    }
    if resp.double_clicked()
        && let Some(c) = under.and_then(|i| clusters.get(i))
        && c.site.is_none()
        && let [only] = c.members.as_slice()
        && let Some(p) = pins.pins.get(*only)
    {
        let _ = app.run("library.select", json!({"ids": [p.id.0]}));
        let _ = app.run("view.detail", json!({}));
    }
    if under.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    widget::draw_scale_bar(ui, &vp);
    if let Some(a) = &attribution {
        widget::draw_attribution(ui, &vp, a);
    }
    controls(app, ui, &mut vp);
    if tiles == MapTiles::Custom
        && let Err(e) = app.ui.settings.map_custom.source()
    {
        custom_not_set_up(ui, rect, &e);
    }
    if tiles == MapTiles::Ask {
        ask(app, ui, rect);
    } else if pins.pins.is_empty() {
        empty(app, ui, rect);
    }
    if let Some(c) = under.and_then(|i| clusters.get(i))
        && let Some(pos) = pointer
    {
        hover_card(app, ui, pins, c, pos);
    }
    vp.store(&mut app.ui.map);
}

/// Fit `points` on screen: as deep as needed for pins close together to separate, but a single
/// spot (one photo, or photos taken from one place) at [`PHOTO_ZOOM`].
fn fit(vp: &mut Viewport, points: impl Iterator<Item = (f64, f64)>) {
    let pts: Vec<(f64, f64)> = points.collect();
    let one_spot = pts.windows(2).all(|w| w.first() == w.last());
    let max = if one_spot { PHOTO_ZOOM } else { super::MAX_ZOOM };
    if let Some((center, zoom)) = lightcraft_tiles::mercator::fit(&pts, f64::from(vp.rect.width()), f64::from(vp.rect.height()), FIT_MARGIN, max) {
        vp.center = center;
        vp.zoom = zoom.clamp(super::MIN_ZOOM, super::MAX_ZOOM);
    }
}

/// A pin selects its photo (Shift: a range, Cmd/Ctrl: toggle, as in the grid). A cluster zooms
/// in to its photos, selects them and narrows the filmstrip to them (`map.focus`); one that can't
/// come apart (photos taken at one spot) zooms one step. A site zooms in to its photos (and
/// narrows the filmstrip when other sites are in the list). A pin outside the narrowed set shows
/// every photo again.
fn click(app: &mut LightcraftApp, ui: &egui::Ui, vp: &mut Viewport, pins: &Pins, c: &Cluster) {
    let ids: Vec<u64> = c.members.iter().filter_map(|i| pins.pins.get(*i)).map(|p| p.id.0).collect();
    if c.site.is_some() {
        fit(vp, c.members.iter().filter_map(|i| pins.pins.get(*i)).map(|p| p.world));
        vp.zoom = vp.zoom.max(super::pins::SITE_MAX_ZOOM);
        if ids.len() < pins.pins.len() {
            let _ = app.run("map.focus", json!({"ids": ids}));
        }
        return;
    }
    if let [only] = ids.as_slice() {
        if !app.ui.map.focus.is_empty() && !app.ui.map.focus.contains(only) {
            app.ui.map.focus.clear();
        }
        let m = ui.input(|i| i.modifiers);
        let mode = if m.shift {
            "range"
        } else if m.command {
            "toggle"
        } else {
            "replace"
        };
        let _ = app.run("library.select", json!({"ids": [only], "mode": mode}));
        app.ui.map.followed = Some(*only);
        return;
    }
    let before = vp.zoom;
    fit(vp, c.members.iter().filter_map(|i| pins.pins.get(*i)).map(|p| p.world));
    // at the deepest zoom (or no deeper than now) the cluster stays: one step closer
    if vp.zoom <= before + 0.01 {
        vp.zoom = (before + 1.0).min(super::MAX_ZOOM);
    }
    let _ = app.run("map.focus", json!({"ids": ids}));
}

/// Fit All, Track, the tile source and + / −, top-right.
fn controls(app: &mut LightcraftApp, ui: &mut egui::Ui, vp: &mut Viewport) {
    let t = Tokens::get(ui.ctx());
    let w = 600.0f32.min(vp.rect.width() - 20.0).max(0.0);
    let bar = Rect::from_min_size(pos2(vp.rect.right() - 10.0 - w, vp.rect.top() + 10.0), vec2(w, 28.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Center)));
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 4.0;
    // the custom source has a button once it is set up (Settings ▸ Map ▸ Custom), under its name
    let custom = app.ui.settings.map_custom.source().is_ok().then(|| app.ui.settings.map_custom.label());
    for tiles in MapTiles::ALL.iter().rev() {
        let on = app.ui.settings.map_tiles == *tiles || (*tiles == MapTiles::Off && app.ui.settings.map_tiles == MapTiles::Ask);
        let (label, hover) = match tiles {
            MapTiles::Custom => match &custom {
                Some(name) => (name.as_str(), tr("Your own tile service (Settings → Map)")),
                None if on => (tiles.label(), tr("The custom tile source isn't set up (Settings → Map)")),
                None => continue,
            },
            MapTiles::Street => (tiles.label(), tr("OpenStreetMap street map")),
            MapTiles::Imagery => (tiles.label(), tr("USGS aerial imagery (United States)")),
            MapTiles::Esri => (tiles.label(), tr("Esri World Imagery (worldwide aerial imagery)")),
            MapTiles::Ask | MapTiles::Off => (tiles.label(), tr("No map tiles: no network requests")),
        };
        if text_button(ui, &format!("mapTiles-{}", tiles.id()), label, on).on_hover_text(hover).clicked() {
            let _ = app.run("map.setTileSource", json!({"source": tiles.id()}));
        }
    }
    ui.add_space(8.0);
    if text_button(ui, "mapTrack", "Track", app.ui.map.track).on_hover_text(tr("Line through the photos in capture order")).clicked() {
        let _ = app.run("map.toggleTrack", json!({}));
    }
    if text_button(ui, "mapFitAll", "Fit All", false).clicked() {
        let _ = app.run("map.fitAll", json!({}));
    }
    // + / − bottom-right, above the attribution
    let zoom = Rect::from_min_size(pos2(vp.rect.right() - 40.0, vp.rect.bottom() - 92.0), vec2(30.0, 60.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(zoom).layout(egui::Layout::top_down(egui::Align::Center)));
    child.spacing_mut().item_spacing.y = 4.0;
    let center = vp.rect.center();
    if text_button(&mut child, "mapZoomIn", "+", false).clicked() {
        vp.zoom_about(1.0, center);
    }
    if text_button(&mut child, "mapZoomOut", "−", false).clicked() {
        vp.zoom_about(-1.0, center);
    }
    let _ = t;
}

/// The first time: ask before loading any tiles.
fn ask(app: &mut LightcraftApp, ui: &mut egui::Ui, map: Rect) {
    let t = Tokens::get(ui.ctx());
    let card = Rect::from_center_size(pos2(map.center().x, map.bottom() - 110.0), vec2(520.0f32.min(map.width() - 20.0), 132.0));
    let p = ui.painter();
    p.rect(card, 6.0, t.chrome, Stroke::new(1.0, t.divider), StrokeKind::Inside);
    register(ui, "map:ask", card);
    let mut child = ui.new_child(UiBuilder::new().max_rect(card.shrink(14.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut child;
    ui.label(RichText::new(tr("Show a map under the pins?")).font(t.semibold(13.0)).color(t.text));
    ui.add_space(4.0);
    ui.label(
        RichText::new(tr(
            "Map tiles are downloaded from OpenStreetMap (street map), the USGS (aerial imagery, United States) or Esri (aerial imagery, worldwide, sharper). They see which area you look at, never your photos. Change this any time in Settings → Map.",
        ))
        .font(t.font(11.5))
        .color(t.text_label),
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        for (id, label, tiles) in [
            ("street", "Street Map", MapTiles::Street),
            ("imagery", "Imagery (US)", MapTiles::Imagery),
            ("esri", "Imagery (Esri)", MapTiles::Esri),
            ("off", "Not Now", MapTiles::Off),
        ] {
            if text_button(ui, &format!("mapAsk-{id}"), label, false).clicked() {
                app.ui.settings.map_tiles = tiles;
            }
        }
    });
}

/// Custom tiles are chosen but their settings don't make a tile source: say what to fix.
fn custom_not_set_up(ui: &mut egui::Ui, map: Rect, why: &str) {
    let t = Tokens::get(ui.ctx());
    let card = Rect::from_center_size(pos2(map.center().x, map.top() + 70.0), vec2(460.0f32.min(map.width() - 20.0), 64.0));
    ui.painter().rect(card, 6.0, t.chrome, Stroke::new(1.0, t.divider), StrokeKind::Inside);
    register(ui, "map:customError", card);
    let mut child = ui.new_child(UiBuilder::new().max_rect(card.shrink(12.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut child;
    ui.label(RichText::new(tr("The custom tile source isn't set up (Settings → Map)")).font(t.semibold(12.0)).color(t.text));
    ui.label(RichText::new(tr(why)).font(t.font(11.0)).color(t.text_label));
}

/// No photo here has a location: say where locations come from.
fn empty(app: &mut LightcraftApp, ui: &mut egui::Ui, map: Rect) {
    let t = Tokens::get(ui.ctx());
    let card = Rect::from_center_size(map.center(), vec2(460.0f32.min(map.width() - 20.0), 120.0));
    ui.painter().rect(card, 6.0, t.chrome, Stroke::new(1.0, t.divider), StrokeKind::Inside);
    register(ui, "map:empty", card);
    let mut child = ui.new_child(UiBuilder::new().max_rect(card.shrink(14.0)).layout(egui::Layout::top_down(egui::Align::Center)));
    let ui = &mut child;
    ui.label(RichText::new(tr("No photos here have a location")).font(t.semibold(13.0)).color(t.text));
    ui.add_space(4.0);
    ui.label(
        RichText::new(tr(
            "Phones and drones record where each photo was taken. For a camera without GPS, record a track on your phone while you shoot, then match it to the photos.",
        ))
        .font(t.font(11.5))
        .color(t.text_label),
    );
    ui.add_space(6.0);
    if text_button(ui, "mapAutoTag", "Auto-Tag from Tracklog…", false).clicked() {
        let _ = app.run("photo.autoTagTracklog", json!({}));
    }
}

/// The photo under the pointer: thumbnail, file name, capture time (a cluster: how many).
fn hover_card(app: &mut LightcraftApp, ui: &egui::Ui, pins: &Pins, c: &Cluster, at: egui::Pos2) {
    let t = Tokens::get(ui.ctx());
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("lc-map-hover")));
    let Some(first) = c.members.first().and_then(|i| pins.pins.get(*i)) else { return };
    let img_h = 132.0;
    let single = c.members.len() == 1 && c.site.is_none();
    let heading = single.then(|| facts(first)).flatten();
    // wide enough for the longest line (heading, altitude and accuracy), never narrower than 200
    let widest = heading.as_ref().map_or(0.0, |h| painter.layout_no_wrap(h.clone(), t.font(11.0), t.text_dim).size().x);
    let card_w = (widest + 16.0).clamp(200.0, 320.0);
    let size = vec2(card_w, img_h + if heading.is_some() { 62.0 } else { 46.0 });
    let screen = ui.ctx().content_rect();
    let mut min = at + vec2(16.0, 16.0);
    if min.x + size.x > screen.right() {
        min.x = at.x - 16.0 - size.x;
    }
    if min.y + size.y > screen.bottom() {
        min.y = at.y - 16.0 - size.y;
    }
    let card = Rect::from_min_size(min, size);
    painter.rect(card, 5.0, t.chrome, Stroke::new(1.0, t.divider), StrokeKind::Inside);
    let img = Rect::from_min_size(card.min + vec2(6.0, 6.0), vec2(card_w - 12.0, img_h - 6.0));
    painter.rect_filled(img, 2.0, t.cell);
    crate::panels::grid::request_thumb(app, first.id, 256, 10);
    if let Some(tex) = app.renderer.thumb(first.id) {
        let fit = crate::panels::detail::fit_texture_rect(img, tex.size);
        painter.image(tex.tex.id(), fit, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    let count = widget::photo_count(c.members.len());
    let name = match &c.site {
        Some(site) => site.clone(),
        None if c.members.len() > 1 => count.clone(),
        None => first.file_name.clone(),
    };
    painter.text(pos2(card.left() + 8.0, img.bottom() + 8.0), Align2::LEFT_TOP, name, t.semibold(11.5), t.text);
    let when = match c.members.last().and_then(|i| pins.pins.get(*i)) {
        // a site: how many and over which days
        Some(last) if c.site.is_some() => {
            let (a, b) = (first.captured.get(..10).unwrap_or(""), last.captured.get(..10).unwrap_or(""));
            if a == b { format!("{count} · {a}") } else { format!("{count} · {a} – {b}") }
        }
        _ => first.captured.replacen('T', " ", 1),
    };
    painter.text(pos2(card.left() + 8.0, img.bottom() + 25.0), Align2::LEFT_TOP, when, t.font(11.0), t.text_dim);
    if let Some(line) = heading {
        painter.text(pos2(card.left() + 8.0, img.bottom() + 41.0), Align2::LEFT_TOP, line, t.font(11.0), t.text_dim);
    }
    crate::widgets::register_geometry(ui.ctx(), "map:hover", card);
}

/// "Heading 355° (magnetic) · 67 m altitude · ±6 m": the hover card's line about where the camera
/// pointed from and how sure the position is.
fn facts(p: &pins::Pin) -> Option<String> {
    let heading = p.direction.map(|d| {
        let d = d.round() as i32 % 360;
        if p.magnetic { tr_format!("Heading {d}° (magnetic)", d = d) } else { tr_format!("Heading {d}°", d = d) }
    });
    let altitude = p.altitude.map(|a| tr_format!("{m} m altitude", m = a.round() as i64));
    let accuracy = p.accuracy.map(|e| tr_format!("±{m} m", m = e.round().max(1.0) as i64));
    let parts: Vec<String> = [heading, altitude, accuracy].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}
