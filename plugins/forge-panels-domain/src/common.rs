//! Shared pieces of the domain panels: stable row keys, list rebuilds that keep the
//! selection, namespace watches and refusal notices.

use std::collections::HashMap;

use forge_editor::mirror::{ProjectMirror, Watch};
use forge_editor::session::SessionState;
use forge_ui::widgets::{RowItem, VirtualTree};
use forge_ui::{Ui, WidgetId};

/// A stable 64-bit row key for an id path (FNV-1a): the same object keeps its key across
/// rebuilds, so the selection survives an edit.
pub fn key_of(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h ^= u64::from(b'/');
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        for b in p.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// One row of a list: `(parent key, key, item)`.
pub type Row = (Option<u64>, u64, RowItem);

/// What a list's rows stand for, by key.
pub struct Rows<T> {
    pub map: HashMap<u64, T>,
    /// A digest of the last rows shown (unchanged rows are not rebuilt).
    digest: u64,
}

impl<T> Default for Rows<T> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            digest: 0,
        }
    }
}

impl<T: Clone> Rows<T> {
    pub fn get(&self, k: u64) -> Option<T> {
        self.map.get(&k).cloned()
    }
}

fn digest(rows: &[Row]) -> u64 {
    let mut parts = Vec::with_capacity(rows.len() * 3);
    let texts: Vec<String> = rows
        .iter()
        .map(|(p, k, it)| format!("{p:?}:{k}:{}:{}", it.label, it.muted))
        .collect();
    for t in &texts {
        parts.push(t.as_str());
    }
    key_of(&parts)
}

/// Show `rows` in `list` (parents before children), keeping the selection and opening
/// every parent. Nothing happens when the rows are unchanged.
pub fn show_rows<T>(
    ui: &mut Ui,
    list: WidgetId,
    rows: Vec<Row>,
    map: HashMap<u64, T>,
    into: &mut Rows<T>,
) {
    into.map = map;
    let d = digest(&rows);
    if d == into.digest {
        return;
    }
    into.digest = d;
    VirtualTree::edit(ui, list, |t| {
        let selected = t.selected();
        t.clear();
        let mut parents = Vec::new();
        for (p, k, item) in rows {
            if let Some(p) = p {
                parents.push(p);
            }
            t.push(p, k, item);
        }
        for p in parents {
            if t.contains(p) && !t.is_expanded(p) {
                t.set_expanded(p, true);
            }
        }
        let keep: Vec<u64> = selected.into_iter().filter(|k| t.contains(*k)).collect();
        t.select(&keep);
    });
}

/// The list's selected (or focused) row key.
pub fn selected(ui: &mut Ui, list: WidgetId) -> Option<u64> {
    VirtualTree::edit(ui, list, |t| {
        t.selected().first().copied().or_else(|| t.active())
    })
    .flatten()
}

/// Select `key` in the list (after the rows that contain it were shown).
pub fn select(ui: &mut Ui, list: WidgetId, key: u64) {
    VirtualTree::edit(ui, list, |t| {
        if t.contains(key) {
            t.select(&[key]);
        }
    });
}

/// Start (or keep) watching a settings namespace; its current revision.
pub fn watch(m: &ProjectMirror, prefix: &str) -> u64 {
    m.watch(Watch::SettingPrefix(prefix.to_string()))
}

/// A refused edit: a warning toast naming why (never a silent no-op).
pub fn refuse(session: &mut SessionState, what: &str, why: &str) {
    session.refuse(what, why);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_stable_and_path_sensitive() {
        assert_eq!(key_of(&["a", "b"]), key_of(&["a", "b"]));
        assert_ne!(key_of(&["a", "b"]), key_of(&["ab"]));
        assert_ne!(key_of(&["a/b"]), key_of(&["a", "c"]));
    }
}
