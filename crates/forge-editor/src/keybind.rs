//! The keybindings editor's model (Ch.21 §21.21 "Keybindings", DoD M2-44): a searchable
//! list of every action with its chords, rebind by pressing the chord, conflicts named
//! with both actions (the `KeyMap` refuses them, `test_keybinding_conflicts`), reset per
//! binding or all. The keymap is user config; nothing here touches the project.

use crate::keymap::{Binding, KeyContext, KeyMap};
use forge_ui::fuzzy::rank;

/// One action as the shell's registry describes it (the editor lists every action,
/// bound or not).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionSummary {
    pub id: String,
    pub title: String,
    pub category: String,
}

/// One row of the keybindings editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRow {
    pub action: String,
    pub title: String,
    pub category: String,
    /// `(chord as a person reads it, context)` for each binding, outermost context first.
    pub chords: Vec<(String, KeyContext)>,
    /// Differs from the defaults (the row offers "Reset").
    pub modified: bool,
}

impl KeyRow {
    /// The chords as one line ("Ctrl+Z · Ctrl+Shift+Z").
    pub fn chord_text(&self) -> String {
        if self.chords.is_empty() {
            return "\u{2014}".into();
        }
        self.chords
            .iter()
            .map(|(c, ctx)| match ctx {
                KeyContext::Global | KeyContext::Window => c.clone(),
                other => format!("{c} ({})", other.label()),
            })
            .collect::<Vec<_>>()
            .join(" \u{b7} ")
    }
}

fn bindings_of<'a>(km: &'a KeyMap, action: &str) -> Vec<&'a Binding> {
    let mut v: Vec<&Binding> = km
        .bindings()
        .iter()
        .filter(|b| b.action == action)
        .collect();
    v.sort_by_key(|b| (b.context.clone(), b.chord.to_string()));
    v
}

/// Every action's row, in registry order, filtered by `query` (fuzzy over category,
/// title and chord as the UI locale shows them, and id; best match first). An empty query
/// lists everything.
pub fn rows(actions: &[ActionSummary], km: &KeyMap, base: &KeyMap, query: &str) -> Vec<KeyRow> {
    let all: Vec<KeyRow> = actions
        .iter()
        .map(|a| {
            let now = bindings_of(km, &a.id);
            let then = bindings_of(base, &a.id);
            KeyRow {
                action: a.id.clone(),
                title: a.title.clone(),
                category: a.category.clone(),
                chords: now
                    .iter()
                    .map(|b| (b.chord.label(), b.context.clone()))
                    .collect(),
                modified: now != then,
            }
        })
        .collect();
    let q = query.trim();
    if q.is_empty() {
        return all;
    }
    let texts: Vec<String> = all
        .iter()
        .map(|r| {
            format!(
                "{}: {} {} {}",
                forge_ui::l10n::tr_str(&r.category),
                forge_ui::l10n::tr_str(&r.title),
                r.action,
                r.chord_text()
            )
        })
        .collect();
    rank(q, &texts, String::as_str)
        .into_iter()
        .map(|(i, _)| all[i].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{Chord, default_keymap};

    fn fixture() -> (Vec<ActionSummary>, KeyMap) {
        let f = default_keymap().unwrap_or_else(|e| panic!("{e}"));
        let km = KeyMap::from_layers(&f, &[]).0;
        let actions = crate::actions::builtin_actions()
            .into_iter()
            .map(|(id, a)| ActionSummary {
                id: id.to_string(),
                title: a.title,
                category: a.category,
            })
            .collect();
        (actions, km)
    }

    #[test]
    fn every_action_is_listed_and_search_finds_by_title_or_chord() {
        let (actions, km) = fixture();
        let all = rows(&actions, &km, &km, "");
        assert_eq!(all.len(), actions.len());
        let undo = all
            .iter()
            .find(|r| r.action == "forge.edit.undo")
            .unwrap_or_else(|| panic!("undo row"));
        assert_eq!(undo.chord_text(), "Ctrl+Z");
        assert!(!undo.modified);
        let hits = rows(&actions, &km, &km, "palette");
        assert_eq!(
            hits.first().map(|r| r.action.as_str()),
            Some("forge.palette.open")
        );
        let by_chord = rows(&actions, &km, &km, "Ctrl+Shift+P");
        assert_eq!(
            by_chord.first().map(|r| r.action.as_str()),
            Some("forge.palette.open")
        );
    }

    #[test]
    fn a_rebound_row_is_marked_modified() {
        let (actions, base) = fixture();
        let mut km = base.clone();
        km.rebind(
            "forge.edit.undo",
            KeyContext::Window,
            Chord::parse("Alt+Z").unwrap_or_else(|e| panic!("{e}")),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let r = rows(&actions, &km, &base, "undo");
        let undo = r
            .iter()
            .find(|r| r.action == "forge.edit.undo")
            .unwrap_or_else(|| panic!("row"));
        assert!(undo.modified);
        assert_eq!(undo.chord_text(), "Alt+Z");
    }
}
