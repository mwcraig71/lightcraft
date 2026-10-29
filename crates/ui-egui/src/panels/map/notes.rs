//! A photo's inspection log on screen: the list (newest first) with its editor, beside the photo
//! in the Map view's photo mode and, compact, in the Info panel; the badge a pin with a log gets;
//! the hover card's latest entry. The log itself is the catalog's (`photo.addNote`,
//! `photo.updateNote`, `photo.removeNote`; each change is one undo step).

use egui::{Align2, Color32, Pos2, Rect, RichText, Stroke, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_catalog::notes::{self, Condition, ELEMENTS, Note, NoteKind};
use serde_json::json;

use super::pins::{Cluster, Pin};
use super::widget::{PIN_R, Viewport};
use crate::LightcraftApp;
use crate::i18n::{tr, tr_format};
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

/// Where a log is shown (widget ids are unique on screen: each place has its own).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Beside the photo in the Map view.
    Map,
    /// The Info panel.
    Info,
}

impl Place {
    fn key(self) -> &'static str {
        match self {
            Place::Map => "map",
            Place::Info => "info",
        }
    }
}

/// An entry being written or edited.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub place: Place,
    pub photo: u64,
    /// The entry edited (`None`: a new one).
    pub note: Option<u64>,
    pub kind: NoteKind,
    /// One of [`ELEMENTS`]; "Other" takes [`Draft::other`].
    pub element: String,
    pub other: String,
    pub condition: Option<Condition>,
    pub date: String,
    pub text: String,
}

impl Draft {
    /// The element as the commands take it.
    fn element_param(&self) -> String {
        if self.element == "Other" && !self.other.trim().is_empty() { format!("Other: {}", self.other.trim()) } else { self.element.clone() }
    }

    fn of(place: Place, photo: u64, n: &Note) -> Draft {
        let (element, other) = match n.element.strip_prefix("Other: ") {
            Some(rest) => ("Other".to_string(), rest.to_string()),
            None => (n.element.clone(), String::new()),
        };
        Draft { place, photo, note: Some(n.id), kind: n.kind, element, other, condition: n.condition, date: n.date.clone(), text: n.text.clone() }
    }
}

/// What the log's views keep between frames (not saved).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NotesUi {
    pub draft: Option<Draft>,
    /// Delete asked to confirm: (place, photo, entry).
    pub confirm_delete: Option<(Place, u64, u64)>,
    /// When the photo shown in the Map view last followed a click in the filmstrip: (photo, time).
    pub switched: Option<(u64, f64)>,
    /// The Info panel shows every entry, not only the latest few.
    pub info_all: bool,
}

impl NotesUi {
    /// Drop what the Map view's log had under way (it is going away).
    pub fn close_map_editor(&mut self) {
        if self.draft.as_ref().is_some_and(|d| d.place == Place::Map) {
            self.draft = None;
        }
        if self.confirm_delete.is_some_and(|c| c.0 == Place::Map) {
            self.confirm_delete = None;
        }
    }
}

/// Entries the Info panel lists before "All N entries".
const INFO_LATEST: usize = 3;

fn toast(app: &mut LightcraftApp, text: impl Into<String>) {
    let ctx = app.tasks.repaint.clone().unwrap_or_default();
    app.toast(&ctx, text);
}

/// A condition's colour: Critical red, Poor orange, Fair yellow, Good green.
pub fn condition_color(c: Condition) -> Color32 {
    match c {
        Condition::Critical => Color32::from_rgb(220, 38, 38),
        Condition::Poor => Color32::from_rgb(245, 124, 0),
        Condition::Fair => Color32::from_rgb(250, 204, 21),
        Condition::Good => Color32::from_rgb(56, 166, 92),
    }
}

/// Text on a condition chip (dark on the light yellow and orange).
fn condition_text(c: Condition) -> Color32 {
    match c {
        Condition::Fair | Condition::Poor => Color32::from_rgb(30, 30, 30),
        Condition::Critical | Condition::Good => Color32::WHITE,
    }
}

fn kind_color(k: NoteKind) -> Color32 {
    match k {
        NoteKind::Inspection => Color32::from_rgb(70, 110, 160),
        NoteKind::Repair => Color32::from_rgb(120, 90, 160),
        NoteKind::Preservation => Color32::from_rgb(40, 130, 130),
    }
}

/// A small coloured label.
fn chip(ui: &mut egui::Ui, text: &str, fill: Color32, color: Color32) {
    let t = Tokens::get(ui.ctx());
    let galley = ui.painter().layout_no_wrap(text.to_string(), t.semibold(10.0), color);
    let (r, _) = ui.allocate_exact_size(vec2(galley.size().x + 10.0, 16.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 3.0, fill);
    ui.painter().galley(pos2(r.left() + 5.0, r.center().y - galley.size().y / 2.0), galley, color);
}

/// "2026-10-28 · Inspection · Bearing · Poor: Bearing 3 squeals": an entry in one line.
pub fn entry_line(n: &Note) -> String {
    notes::summary(std::slice::from_ref(n))
}

/// The newest entry's line, for the map's hover card.
pub fn latest_line(app: &LightcraftApp, id: PhotoId) -> Option<String> {
    notes::newest_first(app.session.catalog.notes(id)).first().map(|n| entry_line(n))
}

// ---------------------------------------------------------------- the pins' badges

/// A dot at the top right of each pin whose photo has a log (a cluster: of any of its photos),
/// coloured by the worst current condition (white when no entry is rated). Clustering is the
/// map's own; this only draws over it.
pub fn draw_badges(app: &LightcraftApp, ui: &egui::Ui, vp: &Viewport, pins: &[Pin], clusters: &[Cluster]) {
    let cat = &app.session.catalog;
    if cat.all_notes().next().is_none() {
        return;
    }
    let painter = ui.painter_at(vp.rect);
    for c in clusters.iter().filter(|c| c.site.is_none()) {
        let mut any = false;
        let mut worst: Option<Condition> = None;
        for p in c.members.iter().filter_map(|i| pins.get(*i)) {
            let log = cat.notes(p.id);
            if !log.is_empty() {
                any = true;
                worst = worst.max(notes::worst_condition(log));
            }
        }
        if !any {
            continue;
        }
        let at = vp.screen_of(c.world);
        let offset = if c.members.len() == 1 { PIN_R + 1.0 } else { 12.0 };
        let dot = at + vec2(offset * 0.75, -offset * 0.75);
        draw_badge(&painter, dot, worst);
        let key = c.members.first().and_then(|i| pins.get(*i)).map_or(0, |p| p.id.0);
        crate::widgets::register_geometry(
            ui.ctx(),
            format!("mapLog:{key}:{}", worst.map_or("none", Condition::id)),
            Rect::from_center_size(dot, vec2(8.0, 8.0)),
        );
    }
}

fn draw_badge(painter: &egui::Painter, at: Pos2, worst: Option<Condition>) {
    painter.circle_filled(at, 5.0, Color32::from_black_alpha(200));
    painter.circle_filled(at, 3.6, worst.map_or(Color32::WHITE, condition_color));
}

// ---------------------------------------------------------------- the log beside the photo

/// The log of photo `id` in `r` (the Map view's photo mode).
pub fn panel(app: &mut LightcraftApp, ui: &mut egui::Ui, r: Rect, id: PhotoId) {
    let t = Tokens::get(ui.ctx());
    ui.painter().rect_filled(r, 0.0, t.chrome);
    ui.painter().line_segment([r.left_top(), r.left_bottom()], Stroke::new(1.0, t.divider));
    ui.painter().line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, t.divider));
    register(ui, "map:log", r);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(vec2(14.0, 12.0))).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut child;
    egui::ScrollArea::vertical().id_salt("mapLogScroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(ui.available_width());
        contents(app, ui, id, Place::Map, usize::MAX);
    });
}

/// The Info panel's log of the active photo: the latest few entries and Add Entry.
pub fn info_section(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    ui.add_space(10.0);
    let all = app.ui.map.notes.info_all;
    contents(app, ui, id, Place::Info, if all { usize::MAX } else { INFO_LATEST });
}

fn contents(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId, place: Place, max: usize) {
    let t = Tokens::get(ui.ctx());
    let k = place.key();
    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
    let log: Vec<Note> = notes::newest_first(app.session.catalog.notes(id)).into_iter().cloned().collect();
    let drafting = |app: &LightcraftApp| app.ui.map.notes.draft.as_ref().filter(|d| d.place == place && d.photo == id.0).map(|d| d.note);
    ui.horizontal(|ui| {
        let title = if log.is_empty() { tr("Inspection Log").to_string() } else { tr_format!("Inspection Log ({n})", n = log.len()) };
        let size = if place == Place::Map { 13.0 } else { 12.0 };
        let head = ui.label(RichText::new(title).font(t.semibold(size)).color(t.text));
        register(ui, format!("noteHeader-{k}"), head.rect);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let adding = drafting(app) == Some(None);
            if text_button(ui, &format!("noteAdd-{k}"), "+ Add Entry", adding)
                .on_hover_text(tr("Write a dated entry about what this photo shows: an inspection finding, a repair or preventive work"))
                .clicked()
                && !adding
            {
                start_new(app, id, place, &log);
            }
        });
    });
    if drafting(app) == Some(None) {
        editor(app, ui, place);
    }
    if log.is_empty() && drafting(app).is_none() {
        ui.label(
            RichText::new(tr("No entries yet. Add one for what this photo shows: a defect found, a repair, preventive work."))
                .font(t.font(11.5))
                .color(t.text_dim),
        );
    }
    for n in log.iter().take(max) {
        if drafting(app) == Some(Some(n.id)) {
            editor(app, ui, place);
        } else {
            entry(app, ui, id, place, n);
        }
    }
    if log.len() > max {
        let more = tr_format!("All {n} entries", n = log.len());
        let link = ui.link(RichText::new(more).font(t.font(11.5)).color(t.accent));
        register(ui, format!("noteAll-{k}"), link.rect);
        if link.clicked() {
            app.ui.map.notes.info_all = true;
        }
    } else if place == Place::Info && app.ui.map.notes.info_all && log.len() > INFO_LATEST {
        let link = ui.link(RichText::new(tr("Show Fewer")).font(t.font(11.5)).color(t.accent));
        register(ui, format!("noteFewer-{k}"), link.rect);
        if link.clicked() {
            app.ui.map.notes.info_all = false;
        }
    }
}

fn start_new(app: &mut LightcraftApp, id: PhotoId, place: Place, log: &[Note]) {
    let date = lightcraft_engine::cmd::notes::today(&app.session, "").unwrap_or_default();
    // the element of the newest entry: photos of one spot are usually logged together
    let last = log.first().map(|n| Draft::of(place, id.0, n));
    let (element, other) = last.map_or_else(|| ("Deck".to_string(), String::new()), |d| (d.element, d.other));
    app.ui.map.notes.draft =
        Some(Draft { place, photo: id.0, note: None, kind: NoteKind::Inspection, element, other, condition: None, date, text: String::new() });
    app.ui.map.notes.confirm_delete = None;
}

/// One entry: date, kind, element and condition, the text under them, Edit and Delete.
fn entry(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId, place: Place, n: &Note) {
    let t = Tokens::get(ui.ctx());
    let k = place.key();
    let frame = egui::Frame::new().fill(t.canvas).corner_radius(5.0).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(5.0, 4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&n.date).font(t.semibold(11.5)).color(t.text));
            chip(ui, tr(n.kind.label()), kind_color(n.kind), Color32::WHITE);
            ui.label(RichText::new(element_label(&n.element)).font(t.font(11.5)).color(t.text));
            if let Some(c) = n.condition {
                chip(ui, tr(c.label()), condition_color(c), condition_text(c));
            }
        });
        if !n.text.is_empty() {
            ui.add(egui::Label::new(RichText::new(&n.text).font(t.font(11.5)).color(t.text_label)).wrap());
        }
        let confirming = app.ui.map.notes.confirm_delete == Some((place, id.0, n.id));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if confirming {
                ui.label(RichText::new(tr("Delete this entry? Undo brings it back.")).font(t.font(11.0)).color(t.caution));
                if text_button(ui, &format!("noteDeleteConfirm-{k}-{}", n.id), "Delete", false).clicked() {
                    app.ui.map.notes.confirm_delete = None;
                    if let Err(e) = app.run("photo.removeNote", json!({"id": id.0, "note": n.id})) {
                        toast(app, e);
                    }
                }
                if text_button(ui, &format!("noteDeleteCancel-{k}-{}", n.id), "Cancel", false).clicked() {
                    app.ui.map.notes.confirm_delete = None;
                }
            } else {
                if text_button(ui, &format!("noteEdit-{k}-{}", n.id), "Edit", false).on_hover_text(tr("Change this entry")).clicked() {
                    app.ui.map.notes.draft = Some(Draft::of(place, id.0, n));
                    app.ui.map.notes.confirm_delete = None;
                }
                if text_button(ui, &format!("noteDelete-{k}-{}", n.id), "Delete", false)
                    .on_hover_text(tr("Delete this entry (Undo brings it back)"))
                    .clicked()
                {
                    app.ui.map.notes.confirm_delete = Some((place, id.0, n.id));
                }
            }
        });
    });
    register(ui, format!("note:{k}:{}", n.id), frame.response.rect);
    crate::widgets::screen_reader_name(ui.ctx(), frame.response.id, &entry_line(n));
}

/// An element as shown: "Other: Lighting pole" reads "Lighting pole".
fn element_label(e: &str) -> String {
    match e.strip_prefix("Other: ") {
        Some(rest) => rest.to_string(),
        None => tr(e).to_string(),
    }
}

/// The open entry's fields: kind, element (and what "Other" is), condition, date, text; Save and
/// Cancel.
fn editor(app: &mut LightcraftApp, ui: &mut egui::Ui, place: Place) {
    let t = Tokens::get(ui.ctx());
    let k = place.key();
    let Some(mut d) = app.ui.map.notes.draft.clone() else { return };
    let mut save = false;
    let mut cancel = false;
    let frame =
        egui::Frame::new().fill(t.canvas).stroke(Stroke::new(1.0, t.accent)).corner_radius(5.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            let label = |ui: &mut egui::Ui, text: &str| {
                let (r, _) = ui.allocate_exact_size(vec2(70.0, 20.0), egui::Sense::hover());
                ui.painter().text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, tr(text), t.font(11.5), t.text_label);
            };
            ui.horizontal(|ui| {
                label(ui, "Kind");
                let c = crate::widgets::combo_box(format!("noteKind-{k}")).width(150.0).selected_text(tr(d.kind.label())).show_ui(ui, |ui| {
                    for kind in NoteKind::ALL {
                        ui.selectable_value(&mut d.kind, kind, tr(kind.label()));
                    }
                });
                crate::widgets::name_combo(&c.response, tr("Kind"));
                register(ui, format!("combo:noteKind-{k}"), c.response.rect);
            });
            ui.horizontal(|ui| {
                label(ui, "Element");
                let c = crate::widgets::combo_box(format!("noteElement-{k}")).width(150.0).height(320.0).selected_text(tr(&d.element)).show_ui(
                    ui,
                    |ui| {
                        for e in ELEMENTS {
                            ui.selectable_value(&mut d.element, (*e).to_string(), tr(e));
                        }
                    },
                );
                crate::widgets::name_combo(&c.response, tr("Element"));
                register(ui, format!("combo:noteElement-{k}"), c.response.rect);
            });
            if d.element == "Other" {
                ui.horizontal(|ui| {
                    label(ui, "");
                    crate::text_field::TextField::singleline(&format!("noteOther-{k}"), &mut d.other)
                        .hint(tr("What it is (e.g. Lighting pole)"))
                        .label(tr("Other element"))
                        .width(ui.available_width())
                        .show(ui);
                });
            }
            ui.horizontal(|ui| {
                label(ui, "Condition");
                let shown = d.condition.map_or_else(|| tr("Not rated").to_string(), |c| tr(c.label()).to_string());
                let c = crate::widgets::combo_box(format!("noteCondition-{k}")).width(150.0).selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.condition, None, tr("Not rated"));
                    for c in Condition::ALL {
                        ui.selectable_value(&mut d.condition, Some(c), tr(c.label()));
                    }
                });
                crate::widgets::name_combo(&c.response, tr("Condition"));
                register(ui, format!("combo:noteCondition-{k}"), c.response.rect);
            });
            ui.horizontal(|ui| {
                label(ui, "Date");
                crate::text_field::TextField::singleline(&format!("noteDate-{k}"), &mut d.date)
                    .hint("YYYY-MM-DD")
                    .label(tr("Date"))
                    .width(110.0)
                    .show(ui);
            });
            crate::text_field::TextField::multiline(&format!("noteText-{k}"), &mut d.text)
                .hint(tr("What you see, or what was done"))
                .label(tr("Entry text"))
                .rows(3)
                .width(f32::INFINITY)
                .show(ui);
            ui.horizontal(|ui| {
                let label = if d.note.is_some() { "Save" } else { "Add" };
                save = text_button(ui, &format!("noteSave-{k}"), label, false).clicked();
                cancel = text_button(ui, &format!("noteCancel-{k}"), "Cancel", false).clicked();
            });
        });
    register(ui, format!("noteEditor-{k}"), frame.response.rect);
    crate::widgets::screen_reader_name(ui.ctx(), frame.response.id, tr("Log entry"));
    // the fields write into the draft as they are edited
    if app.ui.map.notes.draft.as_ref().is_some_and(|x| x.place == d.place && x.photo == d.photo && x.note == d.note) {
        app.ui.map.notes.draft = Some(d.clone());
    }
    if cancel {
        app.ui.map.notes.draft = None;
    } else if save {
        save_draft(app, &d);
    }
}

fn save_draft(app: &mut LightcraftApp, d: &Draft) {
    let mut p = json!({
        "id": d.photo,
        "kind": d.kind.id(),
        "element": d.element_param(),
        "condition": d.condition.map(Condition::id),
        "text": d.text,
        "date": d.date.trim(),
    });
    let command = match d.note {
        Some(n) => {
            p["note"] = json!(n);
            "photo.updateNote"
        }
        None => "photo.addNote",
    };
    match app.run(command, p) {
        Ok(_) => app.ui.map.notes.draft = None,
        Err(e) => toast(app, e),
    }
}
