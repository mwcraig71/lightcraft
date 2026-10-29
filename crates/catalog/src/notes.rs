//! A photo's inspection log: dated entries an inspector writes about what the photo shows (a
//! bridge's deck, a bearing, a pier), each with a kind (inspection, repair, preservation), the
//! structural element, an optional condition rating and free text.
//!
//! The log is a library-level record keyed by photo, kept in [`crate::Catalog`] and changed only
//! through [`crate::Op::SetNotes`] (the photo's whole list at once, so the inverse is the list as
//! it was and every add, edit or delete is one undo step). Everything is checked
//! ([`notes_problem`]): the log comes from the user, scripts and agents, so lengths, dates and ids
//! are hostile until proven otherwise.
//!
//! Catalog format version 11 (`Catalog.notes`, `Op::SetNotes`).

use serde::{Deserialize, Serialize};

/// Most entries one photo's log holds.
pub const MAX_NOTES: usize = 500;
/// Longest entry text (characters).
pub const MAX_TEXT: usize = 5000;
/// Longest element name, "Other: …" included (characters).
pub const MAX_ELEMENT: usize = 80;

/// The common structural elements of a bridge (and culverts), in the order the log offers them.
/// "Other" carries free text: `Other: Lighting pole`.
pub const ELEMENTS: &[&str] = &[
    "Deck",
    "Wearing surface",
    "Joint",
    "Railing/Barrier",
    "Girder/Beam",
    "Floor beam",
    "Stringer",
    "Truss",
    "Cable/Suspender",
    "Bearing",
    "Pier",
    "Pier cap",
    "Abutment",
    "Wingwall",
    "Footing/Foundation",
    "Culvert",
    "Drainage",
    "Approach",
    "Other",
];

/// What an entry records.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteKind {
    /// What was found.
    #[default]
    Inspection,
    /// What was repaired.
    Repair,
    /// Preventive work (cleaning, sealing, painting).
    Preservation,
}

impl NoteKind {
    pub const ALL: [NoteKind; 3] = [NoteKind::Inspection, NoteKind::Repair, NoteKind::Preservation];

    /// Command id.
    pub fn id(self) -> &'static str {
        match self {
            NoteKind::Inspection => "inspection",
            NoteKind::Repair => "repair",
            NoteKind::Preservation => "preservation",
        }
    }

    /// English label (translated where shown).
    pub fn label(self) -> &'static str {
        match self {
            NoteKind::Inspection => "Inspection",
            NoteKind::Repair => "Repair",
            NoteKind::Preservation => "Preservation",
        }
    }

    /// From its id or label, any case.
    pub fn parse(s: &str) -> Option<NoteKind> {
        let s = s.trim();
        NoteKind::ALL.into_iter().find(|k| k.id().eq_ignore_ascii_case(s))
    }
}

/// A condition rating, best to worst.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Condition {
    Good,
    Fair,
    Poor,
    Critical,
}

impl Condition {
    pub const ALL: [Condition; 4] = [Condition::Good, Condition::Fair, Condition::Poor, Condition::Critical];

    pub fn id(self) -> &'static str {
        match self {
            Condition::Good => "good",
            Condition::Fair => "fair",
            Condition::Poor => "poor",
            Condition::Critical => "critical",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Condition::Good => "Good",
            Condition::Fair => "Fair",
            Condition::Poor => "Poor",
            Condition::Critical => "Critical",
        }
    }

    pub fn parse(s: &str) -> Option<Condition> {
        let s = s.trim();
        Condition::ALL.into_iter().find(|c| c.id().eq_ignore_ascii_case(s))
    }
}

/// One dated entry of a photo's log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    /// Unique within its photo's log.
    pub id: u64,
    pub kind: NoteKind,
    /// One of [`ELEMENTS`], or `Other: <free text>` ([`normalize_element`]).
    pub element: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,
    /// Plain text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// The day it is about (`YYYY-MM-DD`; the inspection's date, not when it was typed).
    pub date: String,
    /// When it was written and last changed (ISO 8601).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub modified: String,
}

impl Note {
    /// What is wrong with it as a record, if anything.
    pub fn problem(&self) -> Option<String> {
        match normalize_element(&self.element) {
            Ok(e) if e == self.element => {}
            Ok(_) => return Some(format!("entry {}: element `{}` isn't written the way the log writes it", self.id, self.element)),
            Err(why) => return Some(format!("entry {}: {why}", self.id)),
        }
        if self.text.chars().count() > MAX_TEXT {
            return Some(format!("entry {}: the text is longer than {MAX_TEXT} characters", self.id));
        }
        if let Err(why) = check_date(&self.date) {
            return Some(format!("entry {}: {why}", self.id));
        }
        for t in [&self.created, &self.modified] {
            if t.chars().count() > 40 || t.chars().any(char::is_control) {
                return Some(format!("entry {}: a timestamp isn't a date and time", self.id));
            }
        }
        None
    }
}

/// The element as the log stores it: a known one ([`ELEMENTS`], any case) by its own spelling;
/// `Other: <text>` (or any other text, which becomes that) with the text trimmed. Empty, too long
/// or with control characters is an error.
pub fn normalize_element(s: &str) -> Result<String, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("give the element (Deck, Bearing, Pier…, or Other: what it is)".into());
    }
    if s.chars().any(char::is_control) {
        return Err("the element can't have line breaks or control characters".into());
    }
    if let Some(known) = ELEMENTS.iter().find(|e| e.eq_ignore_ascii_case(s)) {
        return Ok((*known).to_string());
    }
    // "Other: x", "other:x", or free text
    let rest = match s.get(..6) {
        Some(head) if head.eq_ignore_ascii_case("other:") => s.get(6..).unwrap_or("").trim(),
        _ => s,
    };
    let out = if rest.is_empty() { "Other".to_string() } else { format!("Other: {rest}") };
    if out.chars().count() > MAX_ELEMENT {
        return Err(format!("the element is longer than {MAX_ELEMENT} characters"));
    }
    Ok(out)
}

/// Whether `d` is a calendar date written `YYYY-MM-DD`, between the years 1800 and 9999.
pub fn check_date(d: &str) -> Result<(), String> {
    let ok = d.len() == 10
        && chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok_and(|n| {
            use chrono::Datelike;
            (1800..=9999).contains(&n.year())
        });
    if ok { Ok(()) } else { Err(format!("`{}` isn't a date written YYYY-MM-DD", d.chars().take(40).collect::<String>())) }
}

/// What is wrong with a photo's whole log, if anything: too many entries, an id used twice, a bad
/// entry.
pub fn notes_problem(notes: &[Note]) -> Option<String> {
    if notes.len() > MAX_NOTES {
        return Some(format!("a photo's log holds at most {MAX_NOTES} entries"));
    }
    let mut ids = std::collections::HashSet::new();
    for n in notes {
        if !ids.insert(n.id) {
            return Some(format!("entry id {} is used twice", n.id));
        }
        if let Some(why) = n.problem() {
            return Some(why);
        }
    }
    None
}

/// The id the next entry of `notes` gets.
pub fn next_id(notes: &[Note]) -> u64 {
    notes.iter().map(|n| n.id).max().map_or(1, |m| m.saturating_add(1))
}

/// The entries newest first: by date, then when they were written, then id.
pub fn newest_first(notes: &[Note]) -> Vec<&Note> {
    let mut v: Vec<&Note> = notes.iter().collect();
    v.sort_by(|a, b| (b.date.as_str(), b.created.as_str(), b.id).cmp(&(a.date.as_str(), a.created.as_str(), a.id)));
    v
}

/// The photo's condition as its log has it now: for each element the condition of its newest
/// rated entry (a repair rated Good after an inspection rated Poor counts as Good), and the worst
/// of those. `None` when no entry is rated.
pub fn worst_condition(notes: &[Note]) -> Option<Condition> {
    let mut latest: std::collections::HashMap<&str, Condition> = std::collections::HashMap::new();
    // oldest first, so a newer entry of an element replaces an older one
    for n in newest_first(notes).into_iter().rev() {
        if let Some(c) = n.condition {
            latest.insert(n.element.as_str(), c);
        }
    }
    latest.into_values().max()
}

/// A plain-text summary of the log, newest first, one entry a line:
/// `2026-10-28 · Inspection · Bearing · Poor: Bearing 3 squeals` (line breaks in the text become
/// spaces). Empty for an empty log.
pub fn summary(notes: &[Note]) -> String {
    newest_first(notes)
        .into_iter()
        .map(|n| {
            let mut head = format!("{} · {} · {}", n.date, n.kind.label(), n.element);
            if let Some(c) = n.condition {
                head.push_str(" · ");
                head.push_str(c.label());
            }
            let text: String = n.text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() { head } else { format!("{head}: {text}") }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: u64, date: &str, element: &str, c: Option<Condition>) -> Note {
        Note {
            id,
            kind: NoteKind::Inspection,
            element: element.into(),
            condition: c,
            text: String::new(),
            date: date.into(),
            created: format!("{date}T10:00:00"),
            modified: String::new(),
        }
    }

    #[test]
    fn elements_are_normalized() {
        assert_eq!(normalize_element("bearing").unwrap(), "Bearing");
        assert_eq!(normalize_element(" Pier cap ").unwrap(), "Pier cap");
        assert_eq!(normalize_element("Other").unwrap(), "Other");
        assert_eq!(normalize_element("other:  Lighting pole").unwrap(), "Other: Lighting pole");
        assert_eq!(normalize_element("Lighting pole").unwrap(), "Other: Lighting pole");
        assert!(normalize_element("").is_err());
        assert!(normalize_element("a\nb").is_err());
        assert!(normalize_element(&"x".repeat(200)).is_err());
        // a multi-byte start never slices inside a character
        assert_eq!(normalize_element("橋脚").unwrap(), "Other: 橋脚");
    }

    #[test]
    fn dates_are_checked() {
        assert!(check_date("2026-10-28").is_ok());
        for bad in ["2026-02-30", "26-10-28", "2026-1-28", "", "1700-01-01", "2026-10-28T00:00", "２０２６-10-28"] {
            assert!(check_date(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn kinds_and_conditions_parse() {
        assert_eq!(NoteKind::parse("Repair"), Some(NoteKind::Repair));
        assert_eq!(NoteKind::parse("x"), None);
        assert_eq!(Condition::parse("CRITICAL"), Some(Condition::Critical));
        // an unknown value in a log line is refused, not guessed
        assert!(serde_json::from_str::<Condition>("\"awful\"").is_err());
    }

    #[test]
    fn a_hostile_log_is_refused() {
        let good = note(1, "2026-10-28", "Deck", None);
        assert_eq!(notes_problem(std::slice::from_ref(&good)), None);
        assert!(notes_problem(&[good.clone(), good.clone()]).is_some(), "an id twice");
        let mut n = good.clone();
        n.text = "x".repeat(MAX_TEXT + 1);
        assert!(notes_problem(&[n]).is_some());
        let mut n = good.clone();
        n.date = "yesterday".into();
        assert!(notes_problem(&[n]).is_some());
        let mut n = good.clone();
        n.element = "deck".into();
        assert!(notes_problem(&[n]).is_some(), "not stored normalized");
        let many: Vec<Note> = (1..=MAX_NOTES as u64 + 1).map(|i| note(i, "2026-10-28", "Deck", None)).collect();
        assert!(notes_problem(&many).is_some());
    }

    #[test]
    fn worst_condition_follows_each_elements_newest_rating() {
        let a = note(1, "2026-01-10", "Bearing", Some(Condition::Poor));
        let b = note(2, "2026-03-02", "Bearing", Some(Condition::Good));
        let c = note(3, "2026-02-01", "Deck", Some(Condition::Fair));
        let d = note(4, "2026-04-01", "Deck", None);
        assert_eq!(worst_condition(std::slice::from_ref(&a)), Some(Condition::Poor));
        assert_eq!(worst_condition(&[a.clone(), b.clone()]), Some(Condition::Good), "repaired");
        assert_eq!(worst_condition(&[a, b, c, d]), Some(Condition::Fair));
        assert_eq!(worst_condition(&[]), None);
    }

    #[test]
    fn summary_is_newest_first_one_line_each() {
        let mut a = note(1, "2026-01-10", "Bearing", Some(Condition::Poor));
        a.text = "Squeals\nunder load".into();
        let b = note(2, "2026-03-02", "Deck", None);
        assert_eq!(summary(&[a, b]), "2026-03-02 · Inspection · Deck\n2026-01-10 · Inspection · Bearing · Poor: Squeals under load");
        assert_eq!(next_id(&[]), 1);
    }
}
