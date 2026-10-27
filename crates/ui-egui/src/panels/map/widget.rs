//! The slippy map: drag to pan, wheel or pinch to zoom about the pointer (+ / − buttons too),
//! tiles from the provider drawn under the pins (a loaded parent tile stands in while its
//! children load), a scale bar bottom-left and the tile source's attribution bottom-right
//! whenever its tiles are drawn.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_tiles::{TileId, TileProvider, TileSource, TileState, mercator};

use super::pins::{Cluster, Pin};
use super::{MAX_ZOOM, MIN_ZOOM, MapState};
use crate::theme::Tokens;

/// Textures kept for tiles (each 256 KB of video memory).
const TEXTURES: usize = 384;
/// Pins closer than this (points) draw as one cluster.
pub const CLUSTER_RADIUS: f64 = 24.0;
/// Pin radius (points).
pub const PIN_R: f32 = 6.0;
const CLUSTER_R: f32 = 13.0;
/// How far up the pyramid a loaded tile may stand in for one still loading.
const FALLBACK_LEVELS: u8 = 6;

/// Where the map shows: centre, zoom and the screen rect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub rect: Rect,
    pub center: (f64, f64),
    pub zoom: f64,
}

impl Viewport {
    pub fn from_state(rect: Rect, s: &MapState) -> Viewport {
        Viewport { rect, center: (s.center[0], s.center[1]), zoom: s.zoom }
    }

    pub fn scale(&self) -> f64 {
        mercator::scale(self.zoom)
    }

    /// Screen position of a world point (the copy of the world nearest the centre).
    pub fn screen_of(&self, w: (f64, f64)) -> Pos2 {
        let s = self.scale();
        let mut dx = w.0 - self.center.0;
        dx -= dx.round();
        let c = self.rect.center();
        pos2(c.x + (dx * s) as f32, c.y + ((w.1 - self.center.1) * s) as f32)
    }

    pub fn world_at(&self, p: Pos2) -> (f64, f64) {
        let s = self.scale();
        let c = self.rect.center();
        (self.center.0 + f64::from(p.x - c.x) / s, self.center.1 + f64::from(p.y - c.y) / s)
    }

    /// Zoom by `dz` keeping the world point under `about` fixed.
    pub fn zoom_about(&mut self, dz: f64, about: Pos2) {
        let before = self.world_at(about);
        self.zoom = (self.zoom + dz).clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.world_at(about);
        self.center = (self.center.0 + before.0 - after.0, self.center.1 + before.1 - after.1);
        self.normalise();
    }

    pub fn pan(&mut self, delta: egui::Vec2) {
        let s = self.scale();
        self.center = (self.center.0 - f64::from(delta.x) / s, self.center.1 - f64::from(delta.y) / s);
        self.normalise();
    }

    fn normalise(&mut self) {
        let x = if self.center.0.is_finite() { self.center.0.rem_euclid(1.0) } else { 0.5 };
        let y = if self.center.1.is_finite() { self.center.1.clamp(0.0, 1.0) } else { 0.5 };
        self.center = (x, y);
        if !self.zoom.is_finite() {
            self.zoom = MIN_ZOOM;
        }
    }

    pub fn store(&self, s: &mut MapState) {
        s.center = [self.center.0, self.center.1];
        s.zoom = self.zoom;
    }

    pub fn contains(&self, w: (f64, f64), margin: f32) -> bool {
        self.rect.shrink(margin).contains(self.screen_of(w))
    }
}

/// Uploaded tile textures, least recently used dropped first.
#[derive(Default)]
struct Textures {
    map: HashMap<(&'static str, TileId), (egui::TextureHandle, u64)>,
    tick: u64,
}

impl Textures {
    fn get(&mut self, key: (&'static str, TileId)) -> Option<egui::TextureId> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(&key).map(|(t, used)| {
            *used = tick;
            t.id()
        })
    }

    fn insert(&mut self, ctx: &egui::Context, key: (&'static str, TileId), img: &lightcraft_tiles::TileImage) -> Option<egui::TextureId> {
        if img.rgba.len() != img.width.saturating_mul(img.height).saturating_mul(4) {
            return None;
        }
        let color = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.rgba);
        let name = format!("map-{}-{}-{}-{}", key.0, key.1.z, key.1.x, key.1.y);
        let tex = ctx.load_texture(name, color, egui::TextureOptions::LINEAR);
        let id = tex.id();
        self.tick += 1;
        self.map.insert(key, (tex, self.tick));
        while self.map.len() > TEXTURES {
            let Some(oldest) = self.map.iter().min_by_key(|(_, (_, u))| *u).map(|(k, _)| *k) else { break };
            self.map.remove(&oldest);
        }
        Some(id)
    }
}

type SharedTextures = Arc<Mutex<Textures>>;

/// Draw the tiles of `source` covering `vp`. Returns whether any are still loading.
pub fn draw_tiles(ui: &egui::Ui, vp: &Viewport, provider: &dyn TileProvider, source: &'static TileSource) -> bool {
    let ctx = ui.ctx();
    let textures: SharedTextures = ctx.data_mut(|d| d.get_temp_mut_or_default::<SharedTextures>(egui::Id::unique("lc-map-textures")).clone());
    let mut textures = textures.lock().unwrap_or_else(PoisonError::into_inner);
    let painter = ui.painter_at(vp.rect);
    let z = source.tile_zoom(vp.zoom);
    let placed = mercator::visible_tiles(vp.center, vp.zoom, f64::from(vp.rect.width()), f64::from(vp.rect.height()), z);
    let mut loading = false;
    for p in placed {
        let (native, uv) = source.native(p.tile);
        let rect = Rect::from_min_size(vp.rect.min + vec2(p.left as f32, p.top as f32), vec2(p.size as f32, p.size as f32));
        let key = (source.id, native);
        let tex = match textures.get(key) {
            Some(t) => Some(t),
            None => match provider.get(source, native) {
                TileState::Ready(img) => textures.insert(ctx, key, &img),
                TileState::Pending => {
                    loading = true;
                    None
                }
                TileState::Missing => None,
            },
        };
        if let Some(t) = tex {
            painter.image(t, rect, uv_rect(uv), Color32::WHITE);
            continue;
        }
        // a coarser tile already on hand stands in, scaled up
        for up in 1..=FALLBACK_LEVELS.min(native.z) {
            let (parent, sub) = native.ancestor(up);
            if let Some(t) = textures.get((source.id, parent)) {
                let (u, v, s) = uv;
                let nested = (sub.0 + u * sub.2, sub.1 + v * sub.2, s * sub.2);
                painter.image(t, rect, uv_rect(nested), Color32::WHITE);
                break;
            }
        }
    }
    provider.end_frame();
    loading
}

fn uv_rect((u, v, s): (f64, f64, f64)) -> Rect {
    Rect::from_min_max(pos2(u as f32, v as f32), pos2((u + s) as f32, (v + s) as f32))
}

/// The plain background: land-coloured with faint lines at tile edges, so panning shows movement
/// without tiles.
pub fn draw_background(ui: &egui::Ui, vp: &Viewport) {
    let t = Tokens::get(ui.ctx());
    let painter = ui.painter_at(vp.rect);
    painter.rect_filled(vp.rect, 0.0, t.inset);
    let z = vp.zoom.floor().clamp(0.0, f64::from(mercator::MAX_ZOOM)) as u8;
    let line = Stroke::new(1.0, t.divider.gamma_multiply(0.6));
    for p in mercator::visible_tiles(vp.center, vp.zoom, f64::from(vp.rect.width()), f64::from(vp.rect.height()), z) {
        let r = Rect::from_min_size(vp.rect.min + vec2(p.left as f32, p.top as f32), vec2(p.size as f32, p.size as f32));
        painter.rect_stroke(r, 0.0, line, StrokeKind::Inside);
    }
    // above and below the map's square world
    let top = vp.screen_of((vp.center.0, 0.0)).y;
    let bottom = vp.screen_of((vp.center.0, 1.0)).y;
    if top > vp.rect.top() {
        painter.rect_filled(Rect::from_min_max(vp.rect.min, pos2(vp.rect.right(), top)), 0.0, t.grid_bg);
    }
    if bottom < vp.rect.bottom() {
        painter.rect_filled(Rect::from_min_max(pos2(vp.rect.left(), bottom), vp.rect.max), 0.0, t.grid_bg);
    }
}

/// A scale bar bottom-left: the longest round distance (1, 2, 5 × 10ⁿ m) under 120 points.
pub fn draw_scale_bar(ui: &egui::Ui, vp: &Viewport) {
    let t = Tokens::get(ui.ctx());
    let (lat, _) = mercator::from_world(vp.center.0, vp.center.1);
    let m_per_pt = mercator::metres_per_pixel(lat, vp.zoom);
    let Some((metres, width)) = scale_bar(m_per_pt, 120.0) else { return };
    let painter = ui.painter_at(vp.rect);
    let x0 = vp.rect.left() + 12.0;
    let y = vp.rect.bottom() - 14.0;
    let label = if metres >= 1000.0 { format!("{} km", metres / 1000.0) } else { format!("{metres} m") };
    let bg = Rect::from_min_max(pos2(x0 - 6.0, y - 20.0), pos2(x0 + width + 6.0, y + 6.0));
    painter.rect_filled(bg, 3.0, Color32::from_black_alpha(140));
    let stroke = Stroke::new(2.0, Color32::WHITE);
    painter.line_segment([pos2(x0, y), pos2(x0 + width, y)], stroke);
    painter.line_segment([pos2(x0, y - 5.0), pos2(x0, y)], stroke);
    painter.line_segment([pos2(x0 + width, y - 5.0), pos2(x0 + width, y)], stroke);
    painter.text(pos2(x0 + width / 2.0, y - 7.0), Align2::CENTER_BOTTOM, label, t.font(11.0), Color32::WHITE);
}

/// The distance shown and its length in points, for `m_per_pt` metres per point and at most `max_w` points.
pub fn scale_bar(m_per_pt: f64, max_w: f32) -> Option<(f64, f32)> {
    if !(m_per_pt.is_finite() && m_per_pt > 0.0) {
        return None;
    }
    let max_m = m_per_pt * f64::from(max_w);
    let pow = 10f64.powf(max_m.log10().floor());
    let metres = [5.0, 2.0, 1.0].into_iter().map(|k| k * pow).find(|m| *m <= max_m)?;
    Some((metres, (metres / m_per_pt) as f32))
}

/// The attribution bottom-right (always visible while the source's tiles are drawn).
pub fn draw_attribution(ui: &egui::Ui, vp: &Viewport, text: &str) {
    let t = Tokens::get(ui.ctx());
    let painter = ui.painter_at(vp.rect);
    let galley = painter.layout_no_wrap(text.to_string(), t.font(10.5), Color32::from_gray(30));
    let r = Rect::from_min_size(vp.rect.max - galley.size() - vec2(10.0, 4.0), galley.size() + vec2(8.0, 2.0));
    painter.rect_filled(r, 2.0, Color32::from_white_alpha(200));
    painter.galley(r.min + vec2(4.0, 1.0), galley, Color32::from_gray(30));
    crate::widgets::register_geometry(ui.ctx(), "map:attribution", r);
}

/// Pin colours: ordinary, selected.
const PIN: Color32 = Color32::from_rgb(226, 74, 51);
const RING: Color32 = Color32::WHITE;

/// Draw the track, then the clusters and pins. `selected(id)` marks highlighted pins.
pub fn draw_pins(ui: &egui::Ui, vp: &Viewport, pins: &[Pin], clusters: &[Cluster], track: bool, selected: &dyn Fn(u64) -> bool) {
    let t = Tokens::get(ui.ctx());
    let painter = ui.painter_at(vp.rect);
    if track && pins.len() > 1 {
        let pts: Vec<Pos2> = pins.iter().map(|p| vp.screen_of(p.world)).collect();
        painter.add(egui::Shape::line(pts.clone(), Stroke::new(3.5, Color32::from_black_alpha(90))));
        painter.add(egui::Shape::line(pts, Stroke::new(1.8, Color32::from_rgb(255, 196, 64))));
    }
    // highlighted last, so they sit on top
    let mut order: Vec<&Cluster> = clusters.iter().collect();
    order.sort_by_key(|c| c.members.iter().any(|i| pins.get(*i).is_some_and(|p| selected(p.id.0))));
    for c in order {
        let pos = vp.screen_of(c.world);
        let hot = c.members.iter().any(|i| pins.get(*i).is_some_and(|p| selected(p.id.0)));
        if c.members.len() == 1 {
            let r = if hot { PIN_R + 2.0 } else { PIN_R };
            painter.circle_filled(pos + vec2(0.0, 1.0), r + 1.5, Color32::from_black_alpha(80));
            painter.circle(pos, r, if hot { t.accent } else { PIN }, Stroke::new(if hot { 2.5 } else { 1.5 }, RING));
        } else {
            painter.circle_filled(pos + vec2(0.0, 1.0), CLUSTER_R + 1.5, Color32::from_black_alpha(80));
            painter.circle(pos, CLUSTER_R, if hot { t.accent } else { Color32::from_rgb(40, 44, 52) }, Stroke::new(2.0, RING));
            painter.text(pos, Align2::CENTER_CENTER, c.members.len().to_string(), t.semibold(11.5), Color32::WHITE);
        }
    }
}

/// The cluster under `p`, if any (nearest first).
pub fn hit(vp: &Viewport, clusters: &[Cluster], p: Pos2) -> Option<usize> {
    clusters
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let r = if c.members.len() == 1 { PIN_R + 4.0 } else { CLUSTER_R + 2.0 };
            (i, vp.screen_of(c.world).distance(p), r)
        })
        .filter(|(_, d, r)| d <= r)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _, _)| i)
}

/// Pan, wheel / pinch zoom. Returns the map's response (clicks are the caller's).
pub fn interact(ui: &mut egui::Ui, vp: &mut Viewport) -> egui::Response {
    let resp = ui.interact(vp.rect, egui::Id::unique("lc-map"), Sense::click_and_drag());
    crate::widgets::announce(&resp, crate::i18n::tr("Map of the photos' locations"));
    if resp.dragged() {
        vp.pan(resp.drag_delta());
    }
    if resp.hovered() || resp.contains_pointer() {
        let (scroll, pinch, pointer) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
        let about = pointer.unwrap_or(vp.rect.center());
        let dz = f64::from(scroll) / 120.0 + f64::from(pinch).log2();
        if dz.is_finite() && dz != 0.0 {
            vp.zoom_about(dz, about);
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp() -> Viewport {
        Viewport { rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)), center: (0.3, 0.4), zoom: 12.0 }
    }

    #[test]
    fn screen_and_world_round_trip() {
        let v = vp();
        let w = (0.30001, 0.39999);
        let back = v.world_at(v.screen_of(w));
        assert!((back.0 - w.0).abs() < 1e-9 && (back.1 - w.1).abs() < 1e-9);
        assert_eq!(v.screen_of(v.center), v.rect.center());
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let mut v = vp();
        let at = pos2(100.0, 500.0);
        let w = v.world_at(at);
        v.zoom_about(1.5, at);
        assert_eq!(v.zoom, 13.5);
        let p = v.screen_of(w);
        assert!(p.distance(at) < 0.01, "{p:?}");
        v.zoom_about(100.0, at);
        assert_eq!(v.zoom, MAX_ZOOM);
        v.zoom_about(f64::NAN, at);
        assert!(v.center.0.is_finite() && v.zoom.is_finite());
    }

    #[test]
    fn panning_wraps_east_west_and_stops_at_the_poles() {
        let mut v = Viewport { zoom: 1.0, ..vp() };
        v.pan(vec2(-10_000.0, 1e7));
        assert!((0.0..1.0).contains(&v.center.0));
        assert_eq!(v.center.1, 0.0);
    }

    #[test]
    fn scale_bar_picks_round_distances() {
        assert_eq!(scale_bar(1.0, 120.0), Some((100.0, 100.0)));
        assert_eq!(scale_bar(0.3, 120.0).map(|s| s.0), Some(20.0));
        assert_eq!(scale_bar(40.0, 120.0).map(|s| s.0), Some(2000.0));
        assert_eq!(scale_bar(0.0, 120.0), None);
        assert_eq!(scale_bar(f64::NAN, 120.0), None);
    }
}
