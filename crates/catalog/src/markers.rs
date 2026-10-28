//! Map markers: points the user placed on the Map view (a spot on a bridge, a pier, a joint),
//! each with a title, a note, documents attached to it (an inspection PDF, a spreadsheet, notes)
//! and the photos attached at that spot.
//!
//! A marker is a library-level record, kept in [`crate::Catalog`] by id and changed only through
//! [`crate::Op::SetMarker`] (the whole marker at once, so the inverse is the marker as it was).
//! The attached files themselves live in the library folder (`Attachments/<marker id>/`); the
//! catalog keeps only where (a path relative to the library folder), never their bytes. The
//! engine copies them in and decides when they may go (see `lightcraft_engine::cmd::markers`).
//!
//! Catalog format version 10 (`Catalog.markers`, `Op::SetMarker`).

use serde::{Deserialize, Serialize};

use crate::PhotoId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MarkerId(pub u64);

/// Most attachments one marker holds.
pub const MAX_ATTACHMENTS: usize = 2000;
/// Longest title and note (characters).
pub const MAX_TITLE: usize = 200;
pub const MAX_NOTE: usize = 20_000;

/// What kind of document an attachment is, from its file name's extension.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentKind {
    Pdf,
    Spreadsheet,
    Markdown,
    Text,
    /// Anything else (and kinds a newer build knows).
    #[default]
    #[serde(other)]
    Other,
}

impl AttachmentKind {
    /// The kind of a file named `name` (by its extension, any case).
    pub fn of(name: &str) -> AttachmentKind {
        let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        match ext.as_str() {
            "pdf" => AttachmentKind::Pdf,
            "xlsx" | "xls" | "xlsm" | "xlsb" | "ods" | "csv" | "tsv" | "numbers" => AttachmentKind::Spreadsheet,
            "md" | "markdown" => AttachmentKind::Markdown,
            "txt" | "text" | "log" | "rtf" => AttachmentKind::Text,
            _ => AttachmentKind::Other,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            AttachmentKind::Pdf => "pdf",
            AttachmentKind::Spreadsheet => "spreadsheet",
            AttachmentKind::Markdown => "markdown",
            AttachmentKind::Text => "text",
            AttachmentKind::Other => "other",
        }
    }
}

/// A file attached to a marker: a copy in the library folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// Unique within its marker.
    pub id: u64,
    /// The file's name as attached (the copy may carry a `-1`, `-2`… to keep names apart).
    pub file_name: String,
    /// Where the copy is, relative to the library folder, with `/` separators
    /// (`Attachments/3/inspection.pdf`).
    pub path: String,
    /// Bytes.
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub kind: AttachmentKind,
    /// When it was attached (ISO 8601).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub added: String,
}

/// A point on the map with what was attached to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    pub id: MarkerId,
    pub lat: f64,
    pub lon: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// Plain text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// ISO 8601.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub modified: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    /// Photos attached at this spot (they are ordinary library photos with this point as their
    /// location), in the order they were attached.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub photos: Vec<PhotoId>,
}

impl Marker {
    pub fn new(id: MarkerId, lat: f64, lon: f64, created: &str) -> Marker {
        Marker {
            id,
            lat,
            lon,
            title: String::new(),
            note: String::new(),
            created: created.to_string(),
            modified: created.to_string(),
            attachments: Vec::new(),
            photos: Vec::new(),
        }
    }

    /// The id the next attachment gets.
    pub fn next_attachment_id(&self) -> u64 {
        self.attachments.iter().map(|a| a.id).max().map_or(1, |m| m.saturating_add(1))
    }

    pub fn attachment(&self, id: u64) -> Option<&Attachment> {
        self.attachments.iter().find(|a| a.id == id)
    }

    /// What is wrong with it as a record, if anything: a position that is not on the earth, a
    /// title or note past its length, too many attachments, an attachment path that could
    /// leave the library folder.
    pub fn problem(&self) -> Option<String> {
        if !(self.lat.is_finite() && self.lon.is_finite() && self.lat.abs() <= 90.0 && self.lon.abs() <= 180.0) {
            return Some(format!("marker {}: latitude and longitude must be numbers within ±90 and ±180", self.id.0));
        }
        if self.title.chars().count() > MAX_TITLE {
            return Some(format!("marker {}: the title is longer than {MAX_TITLE} characters", self.id.0));
        }
        if self.note.chars().count() > MAX_NOTE {
            return Some(format!("marker {}: the note is longer than {MAX_NOTE} characters", self.id.0));
        }
        if self.attachments.len() > MAX_ATTACHMENTS {
            return Some(format!("marker {}: more than {MAX_ATTACHMENTS} attachments", self.id.0));
        }
        let mut ids = std::collections::HashSet::new();
        for a in &self.attachments {
            if !ids.insert(a.id) {
                return Some(format!("marker {}: attachment id {} is used twice", self.id.0, a.id));
            }
            if !safe_relative(&a.path) {
                return Some(format!("marker {}: attachment path `{}` is not a path inside the library", self.id.0, a.path));
            }
        }
        None
    }
}

/// Whether `path` is a relative path that stays inside the folder it is relative to: `/`
/// separated, no empty, `.` or `..` part, no drive or root, no backslash or colon.
pub fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':', '\0'])
        && !path.starts_with('/')
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_extension() {
        assert_eq!(AttachmentKind::of("Inspection 2026.PDF"), AttachmentKind::Pdf);
        assert_eq!(AttachmentKind::of("deck.xlsx"), AttachmentKind::Spreadsheet);
        assert_eq!(AttachmentKind::of("notes.md"), AttachmentKind::Markdown);
        assert_eq!(AttachmentKind::of("readme.txt"), AttachmentKind::Text);
        assert_eq!(AttachmentKind::of("model.ifc"), AttachmentKind::Other);
        assert_eq!(AttachmentKind::of("no-extension"), AttachmentKind::Other);
        // a kind a newer build writes reads as Other
        let k: AttachmentKind = serde_json::from_str("\"pointcloud\"").unwrap();
        assert_eq!(k, AttachmentKind::Other);
    }

    #[test]
    fn relative_paths_stay_inside() {
        assert!(safe_relative("Attachments/3/a.pdf"));
        for bad in ["", "/etc/passwd", "../x", "Attachments/../../x", "C:/x", "a\\b", "a//b", "./a"] {
            assert!(!safe_relative(bad), "{bad}");
        }
    }

    #[test]
    fn a_hostile_marker_is_refused() {
        let mut m = Marker::new(MarkerId(1), 40.0, -75.0, "2026-01-01T00:00:00");
        assert_eq!(m.problem(), None);
        m.lat = f64::NAN;
        assert!(m.problem().is_some());
        m.lat = 40.0;
        m.title = "x".repeat(MAX_TITLE + 1);
        assert!(m.problem().is_some());
        m.title.clear();
        m.attachments.push(Attachment {
            id: 1,
            file_name: "a".into(),
            path: "../a".into(),
            size: 1,
            kind: AttachmentKind::Other,
            added: String::new(),
        });
        assert!(m.problem().is_some());
    }
}
