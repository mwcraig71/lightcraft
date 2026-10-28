//! Drone orthomosaics on the map: their tiles drawn above the base map and below the pins (at
//! their opacity, also with tiles off: they are offline imagery), a thin outline of each one's
//! footprint when zoomed out, the Layers panel (show, opacity, fit, remove, add), and the
//! `map.addOrthomosaic` / `map.fitOrthomosaic` / `map.layers` commands. The orthomosaics, their
//! tile builds and the `orthomosaic.*` commands are the engine's (`lightcraft_engine::orthomosaic`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use egui::{Color32, Pos2, Rect, Stroke, StrokeKind, UiBuilder, pos2, vec2};
use lightcraft_engine::orthomosaic::{Status, View};
use lightcraft_tiles::{TileId, mercator};
use serde_json::{Value, json};

use super::widget::Viewport;
use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::pick::{PickRequest, Picked};
use crate::state::ViewMode;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// Textures kept for orthomosaic tiles.
const TEXTURES: usize = 320;
/// Tiles read from disk per frame (the rest next frame: the UI stays responsive).
const LOADS_PER_FRAME: usize = 12;
/// How far up the pyramid a loaded tile stands in for one not read yet.
const FALLBACK_LEVELS: u8 = 8;
/// Below its coarsest tiles by more than this, an orthomosaic is only its outline.
const OUTLINE_ONLY_BELOW: u8 = 3;
const OUTLINE: Color32 = Color32::from_rgb(0, 200, 255);

type Key = (u64, u64, TileId);

#[derive(Default)]
struct Cache {
    textures: HashMap<Key, (egui::TextureHandle, u64)>,
    /// Tiles found absent, and the engine's generation then (looked for again when it changes).
    absent: HashMap<Key, u64>,
    tick: u64,
}

type Shared = Arc<Mutex<Cache>>;

fn cache(ctx: &egui::Context) -> Shared {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Shared>(egui::Id::unique("lc-map-ortho-textures")).clone())
}

/// The orthomosaics, as the engine has them.
fn views(app: &mut LightcraftApp) -> (Vec<View>, u64) {
    let views = app.session.orthomosaic_views();
    (views, app.session.orthomosaics.generation())
}

/// Draw every visible orthomosaic in `vp` (the Map view: with outlines and automation ids).
pub fn draw(app: &mut LightcraftApp, ui: &egui::Ui, vp: &Viewport) {
    draw_as(app, ui, vp, "mapOverlay");
}

/// The same for the Info panel's mini-map.
pub fn draw_mini(app: &mut LightcraftApp, ui: &egui::Ui, vp: &Viewport) {
    draw_as(app, ui, vp, "info:miniMapOverlay");
}

fn draw_as(app: &mut LightcraftApp, ui: &egui::Ui, vp: &Viewport, prefix: &str) {
    let (views, generation) = views(app);
    if views.is_empty() {
        return;
    }
    let ctx = ui.ctx();
    let shared = cache(ctx);
    let mut cache = shared.lock().unwrap_or_else(PoisonError::into_inner);
    let painter = ui.painter_at(vp.rect);
    let mut budget = LOADS_PER_FRAME;
    let mut waiting = false;
    for v in views.iter().filter(|v| v.overlay.visible) {
        let info = &v.overlay.info;
        let [x0, y0, x1, y1] = info.world_bounds;
        let a = vp.screen_of((x0, y0));
        let b = vp.screen_of((x1, y1));
        let bounds = Rect::from_two_pos(a, b);
        if !bounds.intersects(vp.rect) {
            continue;
        }
        let z = vp.zoom.round().clamp(0.0, f64::from(mercator::MAX_ZOOM)) as u8;
        let tz = z.clamp(info.min_zoom, info.max_zoom);
        let mut drawn = 0usize;
        if z + OUTLINE_ONLY_BELOW >= info.min_zoom {
            let tint = Color32::WHITE.gamma_multiply(v.overlay.opacity.clamp(0.0, 1.0));
            let (rx0, ry0, rx1, ry1) = info.tile_range(tz);
            let stamp = v.overlay.modified_ms ^ v.overlay.size;
            for p in mercator::visible_tiles(vp.center, vp.zoom, f64::from(vp.rect.width()), f64::from(vp.rect.height()), tz) {
                let t = p.tile;
                if t.x < rx0 || t.x > rx1 || t.y < ry0 || t.y > ry1 {
                    continue;
                }
                let rect = Rect::from_min_size(vp.rect.min + vec2(p.left as f32, p.top as f32), vec2(p.size as f32, p.size as f32));
                let key = (v.overlay.id, stamp, t);
                let mut tex = cache.get(key);
                if tex.is_none() && cache.absent.get(&key) != Some(&generation) {
                    if budget > 0 {
                        budget -= 1;
                        match lightcraft_ortho::read_tile(&v.dir, t) {
                            Ok(Some(img)) => tex = cache.insert(ctx, key, &img),
                            Ok(None) => {
                                cache.absent.insert(key, generation);
                            }
                            Err(e) => {
                                log::warn!("orthomosaic {}: tile {t:?}: {e}", v.overlay.id);
                                cache.absent.insert(key, generation);
                            }
                        }
                    } else {
                        waiting = true;
                    }
                }
                if let Some(id) = tex {
                    painter.image(id, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
                    drawn += 1;
                    continue;
                }
                // a coarser tile already on hand stands in, scaled up
                for up in 1..=FALLBACK_LEVELS.min(t.z.saturating_sub(info.min_zoom)) {
                    let (parent, (u, v0, s)) = t.ancestor(up);
                    if let Some(id) = cache.get((v.overlay.id, stamp, parent)) {
                        let uv = Rect::from_min_max(pos2(u as f32, v0 as f32), pos2((u + s) as f32, (v0 + s) as f32));
                        painter.image(id, rect, uv, tint);
                        drawn += 1;
                        break;
                    }
                }
            }
        }
        // the footprint: always when zoomed out, and while its tiles are being made
        let building = matches!(v.status, Status::Building { .. });
        if z + 4 <= info.max_zoom || drawn == 0 || building {
            let pts: Vec<Pos2> = info.corners.iter().map(|c| vp.screen_of(mercator::to_world(c.0, c.1))).collect();
            let mut ring = pts.clone();
            if let Some(first) = pts.first() {
                ring.push(*first);
            }
            painter.add(egui::Shape::line(ring.clone(), Stroke::new(3.0, Color32::from_black_alpha(90))));
            painter.add(egui::Shape::line(ring, Stroke::new(1.5, OUTLINE)));
            if bounds.width() < 60.0 || building || v.missing {
                footprint_label(ui, &painter, bounds, v);
            }
        }
        let clipped = bounds.intersect(vp.rect);
        crate::widgets::register_geometry(ctx, format!("{prefix}:{}", v.overlay.id), clipped);
        if drawn > 0 {
            crate::widgets::register_geometry(ctx, format!("{prefix}Tiles:{}", v.overlay.id), clipped);
        }
        if building {
            waiting = true;
        }
    }
    if waiting {
        super::view::note_loading(ctx);
    }
}

/// The name (and what is happening to it) beside a small or unfinished footprint.
fn footprint_label(ui: &egui::Ui, painter: &egui::Painter, bounds: Rect, v: &View) {
    let t = Tokens::get(ui.ctx());
    let text = match &v.status {
        Status::Building { done, total } if *total > 0 => tr_format!("{name} · building {pct} %", name = v.overlay.name, pct = done * 100 / total),
        Status::Building { .. } => tr_format!("{name} · building", name = v.overlay.name),
        _ if v.missing => tr_format!("{name} · file missing", name = v.overlay.name),
        Status::Failed { .. } => tr_format!("{name} · no tiles", name = v.overlay.name),
        Status::Ready => v.overlay.name.clone(),
    };
    let galley = painter.layout_no_wrap(text, t.font(11.0), Color32::WHITE);
    let at = pos2(bounds.right() + 6.0, bounds.center().y - galley.size().y / 2.0 - 2.0);
    let r = Rect::from_min_size(at, galley.size() + vec2(8.0, 4.0));
    painter.rect_filled(r, 3.0, Color32::from_black_alpha(160));
    painter.galley(r.min + vec2(4.0, 2.0), galley, Color32::WHITE);
}

impl Cache {
    fn get(&mut self, key: Key) -> Option<egui::TextureId> {
        self.tick += 1;
        let tick = self.tick;
        self.textures.get_mut(&key).map(|(t, used)| {
            *used = tick;
            t.id()
        })
    }

    fn insert(&mut self, ctx: &egui::Context, key: Key, img: &lightcraft_tiles::TileImage) -> Option<egui::TextureId> {
        if img.rgba.len() != img.width.saturating_mul(img.height).saturating_mul(4) {
            return None;
        }
        let color = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.rgba);
        let name = format!("ortho-{}-{}-{}-{}", key.0, key.2.z, key.2.x, key.2.y);
        let tex = ctx.load_texture(name, color, egui::TextureOptions::LINEAR);
        let id = tex.id();
        self.tick += 1;
        self.textures.insert(key, (tex, self.tick));
        self.absent.remove(&key);
        while self.textures.len() > TEXTURES {
            let Some(oldest) = self.textures.iter().min_by_key(|(_, (_, u))| *u).map(|(k, _)| *k) else { break };
            self.textures.remove(&oldest);
        }
        if self.absent.len() > 20_000 {
            self.absent.clear();
        }
        Some(id)
    }
}

// ------------------------------------------------------------------ Layers panel

fn layers_id() -> egui::Id {
    egui::Id::unique("lc-map-layers-open")
}

fn layers_open(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(layers_id())).unwrap_or(false)
}

fn set_layers_open(ctx: &egui::Context, open: bool) {
    ctx.data_mut(|d| d.insert_temp(layers_id(), open));
}

/// The Layers button top-left on the map, and its panel when open.
pub fn layers(app: &mut LightcraftApp, ui: &mut egui::Ui, vp: &mut Viewport) {
    let ctx = ui.ctx().clone();
    let button = Rect::from_min_size(pos2(vp.rect.left() + 10.0, vp.rect.top() + 10.0), vec2(90.0, 28.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(button).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let open = layers_open(&ctx);
    let n = app.session.orthomosaics().overlays().len();
    let label = if n > 0 { format!("{} ({n})", tr("Layers")) } else { tr("Layers").to_string() };
    if text_button(&mut child, "mapLayers", &label, open).on_hover_text(tr("Drone orthomosaics drawn on the map")).clicked() {
        set_layers_open(&ctx, !open);
    }
    if !layers_open(&ctx) {
        return;
    }
    let (views, _) = views(app);
    let t = Tokens::get(&ctx);
    let row_h = 52.0;
    let h = 64.0 + row_h * views.len().max(1) as f32;
    let card = Rect::from_min_size(
        pos2(vp.rect.left() + 10.0, button.bottom() + 6.0),
        vec2(360.0f32.min(vp.rect.width() - 20.0), h.min(vp.rect.height() - 60.0)),
    );
    ui.painter().rect(card, 6.0, t.chrome, Stroke::new(1.0, t.divider), StrokeKind::Inside);
    register(ui, "map:layers", card);
    // clicks on the card don't pan the map or hit pins under it
    let _ = ui.interact(card, egui::Id::unique("lc-map-layers-card"), egui::Sense::click_and_drag());
    let mut child = ui.new_child(UiBuilder::new().max_rect(card.shrink(10.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut child;
    ui.label(egui::RichText::new(tr("Orthomosaics")).font(t.semibold(12.5)).color(t.text));
    ui.add_space(4.0);
    if views.is_empty() {
        ui.label(
            egui::RichText::new(tr("A drone orthomosaic (GeoTIFF) drawn over the map, offline, at full detail."))
                .font(t.font(11.0))
                .color(t.text_label),
        );
    }
    for v in &views {
        let id = v.overlay.id;
        ui.horizontal(|ui| {
            let mut visible = v.overlay.visible;
            let r = ui.checkbox(&mut visible, egui::RichText::new(&v.overlay.name).font(t.semibold(11.5)).color(t.text));
            register(ui, format!("mapOrthoVisible:{id}"), r.rect);
            if r.changed() {
                run_or_toast(app, ui.ctx(), "orthomosaic.set", json!({"id": id, "visible": visible}));
            }
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let detail = match &v.status {
                Status::Building { done, total } if *total > 0 => tr_format!("building {pct} %", pct = done * 100 / total),
                Status::Building { .. } => tr("building").to_string(),
                _ if v.missing => tr("file missing").to_string(),
                Status::Failed { error } => error.clone(),
                Status::Ready => tr_format!("{cm} cm a pixel", cm = format!("{:.1}", v.overlay.info.resolution_m * 100.0)),
            };
            let shown: String = detail.chars().take(40).collect();
            let l = ui.label(egui::RichText::new(shown).font(t.font(10.5)).color(t.text_label)).on_hover_text(format!(
                "{}\n{}",
                v.overlay.path.display(),
                detail
            ));
            register(ui, format!("mapOrthoStatus:{id}"), l.rect);
            for pct in [25u32, 50, 75, 100] {
                let on = ((v.overlay.opacity * 100.0).round() as u32) == pct;
                if text_button(ui, &format!("mapOrthoOpacity-{id}-{pct}"), &format!("{pct}%"), on).clicked() {
                    run_or_toast(app, ui.ctx(), "orthomosaic.set", json!({"id": id, "opacity": f64::from(pct) / 100.0}));
                }
            }
            if text_button(ui, &format!("mapOrthoFit-{id}"), "Fit", false).on_hover_text(tr("Zoom to this orthomosaic")).clicked() {
                let _ = app.run("map.fitOrthomosaic", json!({"id": id}));
            }
            if matches!(v.status, Status::Failed { .. })
                && !v.missing
                && text_button(ui, &format!("mapOrthoRebuild-{id}"), "Rebuild", false).clicked()
            {
                run_or_toast(app, ui.ctx(), "orthomosaic.rebuild", json!({"id": id}));
            }
            if text_button(ui, &format!("mapOrthoRemove-{id}"), "Remove", false)
                .on_hover_text(tr("Take it off the map and delete its tiles (the GeoTIFF stays)"))
                .clicked()
            {
                run_or_toast(app, ui.ctx(), "orthomosaic.remove", json!({"id": id}));
            }
        });
        ui.add_space(4.0);
    }
    ui.add_space(4.0);
    if text_button(ui, "mapAddOrthomosaic", "Add Orthomosaic…", false).clicked() {
        let _ = app.run("map.addOrthomosaic", json!({}));
    }
}

fn run_or_toast(app: &mut LightcraftApp, ctx: &egui::Context, command: &str, params: Value) {
    if let Err(e) = app.run(command, params) {
        app.toast(ctx, e);
    }
}

// ------------------------------------------------------------------ commands

/// The last map rect drawn (fitting an orthomosaic needs the map's size).
pub(super) fn note_map_rect(ctx: &egui::Context, rect: Rect) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("lc-map-ortho-rect"), rect));
}

/// `map.addOrthomosaic`, `map.fitOrthomosaic`, `map.layers`; `None` for any other id.
pub(super) fn run(app: &mut LightcraftApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "map.addOrthomosaic" => add(app, id, p),
        "map.fitOrthomosaic" => match p.get("id").and_then(Value::as_u64) {
            Some(o) => fit(app, o).and_then(|v| match p.get("zoom").and_then(Value::as_f64) {
                // a closer look at its middle
                Some(z) if z.is_finite() => {
                    app.ui.map.zoom = z.clamp(super::MIN_ZOOM, super::MAX_ZOOM);
                    Ok(json!({"id": o, "zoom": app.ui.map.zoom}))
                }
                Some(z) => Err(format!("map.fitOrthomosaic: `zoom` must be a number, not {z}")),
                None => Ok(v),
            }),
            None => Err("map.fitOrthomosaic: `id` must be an orthomosaic id".into()),
        },
        "map.layers" => {
            let ctx = app.tasks.repaint.clone().unwrap_or_default();
            let open = match p.get("open") {
                None | Some(Value::Null) => !layers_open(&ctx),
                Some(Value::Bool(b)) => *b,
                Some(v) => return Some(Err(format!("map.layers: `open` must be true or false, not {v}"))),
            };
            set_layers_open(&ctx, open);
            Ok(json!({"open": open}))
        }
        _ => return None,
    })
}

/// `map.addOrthomosaic {path?, name?}`: a file dialog without `path`; then `orthomosaic.add`, the
/// Map view, and the map fitted to it.
fn add(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    let path = match p.get("path").and_then(Value::as_str) {
        Some(x) => x.to_string(),
        None => {
            let req = PickRequest::file(tr("Add Orthomosaic"), tr("GeoTIFF orthomosaic"), &["tif", "tiff"]);
            match crate::pick::ask(app, id, p, "path", req, |_| None) {
                Picked::Now(v) => match v.into_iter().next() {
                    Some(x) => x,
                    None => return Ok(Value::Null),
                },
                Picked::Later => return Ok(Value::Null),
                Picked::Unavailable => return Err("no file dialog on this platform".into()),
            }
        }
    };
    let mut params = json!({"path": path});
    if let Some(n) = p.get("name") {
        params["name"] = n.clone();
    }
    let r = app.session.execute("orthomosaic.add", &params).map_err(|e| e.to_string())?;
    if let Some(o) = r.get("id").and_then(Value::as_u64) {
        fit(app, o)?;
    }
    let ctx = app.tasks.repaint.clone().unwrap_or_default();
    app.toast(&ctx, tr_format!("Added {name}: its map tiles are being made", name = r["name"].as_str().unwrap_or("")));
    Ok(r)
}

/// Open the Map view zoomed to an orthomosaic.
fn fit(app: &mut LightcraftApp, id: u64) -> Result<Value, String> {
    let Some(o) = app.session.orthomosaics().get(id).cloned() else {
        return Err(format!("map.fitOrthomosaic: no orthomosaic {id}"));
    };
    let ctx = app.tasks.repaint.clone().unwrap_or_default();
    let rect =
        ctx.data(|d| d.get_temp::<Rect>(egui::Id::unique("lc-map-ortho-rect"))).unwrap_or(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0)));
    let [x0, y0, x1, y1] = o.info.world_bounds;
    let Some((center, zoom)) = mercator::fit(&[(x0, y0), (x1, y1)], f64::from(rect.width()), f64::from(rect.height()), 24.0, super::MAX_ZOOM) else {
        return Err("map.fitOrthomosaic: the orthomosaic has no place".into());
    };
    app.ui.view = ViewMode::Map;
    let pins = super::pins::collect(app);
    let m = &mut app.ui.map;
    m.center = [center.0, center.1];
    m.zoom = zoom.clamp(super::MIN_ZOOM, super::MAX_ZOOM);
    // the pins count as fitted, so the view doesn't fit them on its first frame
    m.fit_pending = false;
    m.fitted = Some(pins.key);
    m.followed = app.session.active().map(|a| a.0);
    Ok(json!({"id": id, "zoom": app.ui.map.zoom}))
}
