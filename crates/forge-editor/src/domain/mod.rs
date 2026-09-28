//! The domain editors' models (Ch.21 §21.21 rows `forge.editors_2d`, `forge.audio_mixer`,
//! `forge.input_map`; WP-U13, ADR 0026): the 2D pipeline's tile sets, tile maps, sprite
//! sheets and `Skeleton2D` rigs ([`scene2d`], Ch.35), the audio mixer's buses and sends
//! ([`audio`], Ch.20) and the game's input actions ([`input`], Ch.28).
//!
//! * **Project state is settings, edited by commands (I7).** Each domain lives in its own
//!   project-settings namespace (`d2.`, `audio.`, `input.`), one value per field
//!   (`audio.bus.music.volume_db = -6.0`). Every edit is a `SetSetting` command — several
//!   fields at once are one transaction, one undo entry — so an automation session or a script
//!   can do everything the panels do, and the change log, undo history and semantic history (Ch.33)
//!   see it like any other edit. No new command variant is needed (Ch.7 is frozen).
//! * **Parsing is total.** A model is read back from the mirror; a value of the wrong type
//!   or a malformed one (an automation session wrote it) is skipped and reported in the model's
//!   `problems`, never a panic.
//! * **Backends (D-4).** What only the real subsystem can answer — autotile solving,
//!   slicing and posing against the 2D pipeline (`forge-2d`, M4-11), metering and
//!   spatialisation from the audio engine (`forge-audio`), device controls from the input
//!   layer (`forge-play`, M5-8) — goes through the trait the plan names ([`Scene2d`],
//!   [`AudioBuses`], [`InputActions`]). The labelled in-memory implementations make the
//!   editors complete and testable now; the gate lists each real backend as `UNBUILT`.
//!
//! [`Scene2d`]: scene2d::Scene2d
//! [`AudioBuses`]: audio::AudioBuses
//! [`InputActions`]: input::InputActions

pub mod audio;
pub mod input;
pub mod scene2d;

use std::collections::BTreeMap;

use forge_cmd::{EditorCommand, Value};

use crate::mirror::ProjectMirror;

/// What a panel shows about the backend it talks to (D-4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendInfo {
    /// Short name (`"in-memory mixer"`).
    pub name: String,
    /// True for the labelled stand-in; the panel says what is not built yet.
    pub in_memory: bool,
    /// One sentence for the panel's footer.
    pub note: String,
}

/// Write one setting.
pub fn set(key: impl Into<String>, value: Value) -> EditorCommand {
    EditorCommand::SetSetting {
        key: key.into(),
        value: Some(value),
    }
}

/// Clear one setting.
pub fn clear(key: impl Into<String>) -> EditorCommand {
    EditorCommand::SetSetting {
        key: key.into(),
        value: None,
    }
}

/// Clear every setting under `prefix.` (removing an object and all its fields).
pub fn clear_under(m: &ProjectMirror, prefix: &str) -> Vec<EditorCommand> {
    let p = format!("{prefix}.");
    m.settings_under(&p).map(|(k, _)| clear(k)).collect()
}

/// The objects under `prefix.`: `id → (field path → value)`. A key `prefix.id.a.b` is field
/// `a.b` of object `id`; a key `prefix.id` with no field is ignored.
pub fn objects<'a>(
    m: &'a ProjectMirror,
    prefix: &str,
) -> BTreeMap<&'a str, BTreeMap<&'a str, &'a Value>> {
    let p = format!("{prefix}.");
    let mut out: BTreeMap<&str, BTreeMap<&str, &Value>> = BTreeMap::new();
    for (k, v) in m.settings_under(&p) {
        let rest = &k[p.len()..];
        if let Some((id, field)) = rest.split_once('.') {
            out.entry(id).or_default().insert(field, v);
        }
    }
    out
}

/// The sub-objects of one object's fields under `sub.`: `sub.id.f → (id → (f → value))`.
pub fn sub_objects<'a>(
    fields: &BTreeMap<&'a str, &'a Value>,
    sub: &str,
) -> BTreeMap<&'a str, BTreeMap<&'a str, &'a Value>> {
    let p = format!("{sub}.");
    let mut out: BTreeMap<&str, BTreeMap<&str, &Value>> = BTreeMap::new();
    for (k, v) in fields {
        if let Some(rest) = k.strip_prefix(p.as_str())
            && let Some((id, field)) = rest.split_once('.')
        {
            out.entry(id).or_default().insert(field, *v);
        }
    }
    out
}

/// A text field (`default` when absent or of another type; a wrong type is a problem).
pub fn text(
    f: &BTreeMap<&str, &Value>,
    key: &str,
    default: &str,
    problems: &mut Vec<String>,
    at: &str,
) -> String {
    match f.get(key) {
        None => default.to_string(),
        Some(Value::Text(t)) => t.clone(),
        Some(v) => {
            problems.push(format!("{at}.{key}: expected text, found {}", v.kind()));
            default.to_string()
        }
    }
}

/// An integer field (see [`text`]).
pub fn int(
    f: &BTreeMap<&str, &Value>,
    key: &str,
    default: i64,
    problems: &mut Vec<String>,
    at: &str,
) -> i64 {
    match f.get(key) {
        None => default,
        Some(Value::Int(i)) => *i,
        Some(v) => {
            problems.push(format!(
                "{at}.{key}: expected an integer, found {}",
                v.kind()
            ));
            default
        }
    }
}

/// A number field: a float, or an integer read as one (see [`text`]).
pub fn float(
    f: &BTreeMap<&str, &Value>,
    key: &str,
    default: f64,
    problems: &mut Vec<String>,
    at: &str,
) -> f64 {
    match f.get(key) {
        None => default,
        Some(Value::Float(x)) => *x,
        Some(Value::Int(i)) => *i as f64,
        Some(v) => {
            problems.push(format!("{at}.{key}: expected a number, found {}", v.kind()));
            default
        }
    }
}

/// A boolean field (see [`text`]).
pub fn boolean(
    f: &BTreeMap<&str, &Value>,
    key: &str,
    default: bool,
    problems: &mut Vec<String>,
    at: &str,
) -> bool {
    match f.get(key) {
        None => default,
        Some(Value::Bool(b)) => *b,
        Some(v) => {
            problems.push(format!(
                "{at}.{key}: expected true/false, found {}",
                v.kind()
            ));
            default
        }
    }
}

/// An identifier (a settings-key segment) from a display name: lower-case ASCII letters,
/// digits and `_`; anything else becomes `_`; a leading digit gets an `x`; empty → `item`.
pub fn ident_from(name: &str) -> String {
    let mut s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    let s = s.trim_matches('_').to_string();
    let mut s = if s.is_empty() { "item".to_string() } else { s };
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, 'x');
    }
    s.truncate(48);
    s
}

/// `base`, or `base_2`, `base_3`… — the first one `taken` refuses.
pub fn unique_id(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    let mut n = 2u32;
    loop {
        let c = format!("{base}_{n}");
        if !taken(&c) {
            return c;
        }
        n += 1;
    }
}

/// Is `s` usable as one settings-key segment (an `ident`)?
pub fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(f) if f.is_ascii_alphabetic() || f == '_')
        && c.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idents_are_key_segments_and_unique() {
        assert_eq!(ident_from("Music Bus"), "music_bus");
        assert_eq!(ident_from("  3D sfx!!"), "x3d_sfx");
        assert_eq!(ident_from("äöü"), "item");
        for n in ["Music Bus", "3D", "", "a--b", "Jump/Fire"] {
            assert!(is_ident(&ident_from(n)), "{n:?}");
        }
        let taken = ["music", "music_2"];
        assert_eq!(unique_id("music", |s| taken.contains(&s)), "music_3");
        assert_eq!(unique_id("sfx", |s| taken.contains(&s)), "sfx");
    }
}
