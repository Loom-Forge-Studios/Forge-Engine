//! Semantic history (Ch.33 §33.4): a revision reads as what its commands did, per issuer —
//! "automation:sess-4 placed 37 “Tree”" — not as "scene.ron changed, +412 −9".
//!
//! A gesture's many frames on one property count once (distinct entity and path), so a
//! 600-frame drag reads "edited 1 property", as the undo history shows it.
//!
//! [`Summary`] is the same reading kept **incrementally**: a command is added when it is
//! applied and removed when its transaction is undone (and added back on redo), each in
//! O(log n); its lines cost O(issuers), never a walk of the commands. A team sandbox keeps
//! one for its unpublished deltas, so the status and the server's sandbox row never re-read
//! the whole list (WP-U10, owner rule 2).

use std::collections::BTreeMap;

use forge_cmd::{CommandEnvelope, EditorCommand, EntityKey};

/// A multiset: how many times each key is held.
#[derive(Clone, Debug)]
struct Counts<K: Ord>(BTreeMap<K, usize>);

impl<K: Ord> Default for Counts<K> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}

impl<K: Ord> Counts<K> {
    /// Hold `k` once more; whether it is newly held.
    fn add(&mut self, k: K) -> bool {
        let n = self.0.entry(k).or_insert(0);
        *n += 1;
        *n == 1
    }
    /// Hold `k` once less; whether it is no longer held.
    fn remove(&mut self, k: &K) -> bool {
        let Some(n) = self.0.get_mut(k) else {
            return false;
        };
        *n -= 1;
        if *n == 0 {
            self.0.remove(k);
            true
        } else {
            false
        }
    }
    /// Distinct keys held.
    fn len(&self) -> usize {
        self.0.len()
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
struct PerIssuer {
    /// Commands of this issuer held (the issuer has a line while any are).
    held: usize,
    spawned: Counts<String>,
    spawned_total: usize,
    deleted: usize,
    renamed: Counts<EntityKey>,
    moved: Counts<EntityKey>,
    props: Counts<(EntityKey, String)>,
    /// Entities with at least one distinct edited property (counted per property).
    prop_entities: Counts<EntityKey>,
    settings: Counts<String>,
    /// Setting namespaces (`editor` of `editor.grid_size`), counted per distinct setting.
    spaces: Counts<String>,
    invoked: Counts<String>,
}

impl PerIssuer {
    fn apply(&mut self, cmd: &EditorCommand, add: bool) {
        match cmd {
            EditorCommand::Spawn { name, .. } => {
                if add {
                    self.spawned.add(name.clone());
                    self.spawned_total += 1;
                } else {
                    self.spawned.remove(name);
                    self.spawned_total = self.spawned_total.saturating_sub(1);
                }
            }
            EditorCommand::Despawn { .. } => {
                if add {
                    self.deleted += 1;
                } else {
                    self.deleted = self.deleted.saturating_sub(1);
                }
            }
            EditorCommand::Rename { entity, .. } => {
                if add {
                    self.renamed.add(*entity);
                } else {
                    self.renamed.remove(entity);
                }
            }
            EditorCommand::Reparent { entity, .. } => {
                if add {
                    self.moved.add(*entity);
                } else {
                    self.moved.remove(entity);
                }
            }
            EditorCommand::SetProperty { entity, path, .. }
            | EditorCommand::RemoveProperty { entity, path } => {
                let k = (*entity, path.clone());
                if add {
                    if self.props.add(k) {
                        self.prop_entities.add(*entity);
                    }
                } else if self.props.remove(&k) {
                    self.prop_entities.remove(entity);
                }
            }
            EditorCommand::SetSetting { key, .. } => {
                let space = key.split('.').next().unwrap_or(key.as_str()).to_string();
                if add {
                    if self.settings.add(key.clone()) {
                        self.spaces.add(space);
                    }
                } else if self.settings.remove(key) {
                    self.spaces.remove(&space);
                }
            }
            EditorCommand::Invoke { target, .. } => {
                if add {
                    self.invoked.add(target.clone());
                } else {
                    self.invoked.remove(target);
                }
            }
        }
    }

    fn line(&self, who: &str) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.spawned_total > 0 {
            match self.spawned.0.keys().next() {
                Some(first) if self.spawned.len() == 1 => parts.push(format!(
                    "placed {} \u{201c}{first}\u{201d}",
                    self.spawned_total
                )),
                _ => parts.push(format!(
                    "placed {}",
                    plural(self.spawned_total, "entity", "entities")
                )),
            }
        }
        if self.deleted > 0 {
            parts.push(format!(
                "deleted {}",
                plural(self.deleted, "entity", "entities")
            ));
        }
        if !self.renamed.is_empty() {
            parts.push(format!(
                "renamed {}",
                plural(self.renamed.len(), "entity", "entities")
            ));
        }
        if !self.moved.is_empty() {
            parts.push(format!(
                "moved {} in the hierarchy",
                plural(self.moved.len(), "entity", "entities")
            ));
        }
        if !self.props.is_empty() {
            parts.push(format!(
                "edited {} on {}",
                plural(self.props.len(), "property", "properties"),
                plural(self.prop_entities.len(), "entity", "entities")
            ));
        }
        if !self.settings.is_empty() {
            let shown: Vec<&str> = self.spaces.0.keys().take(3).map(String::as_str).collect();
            let more = if self.spaces.len() > 3 {
                ", \u{2026}"
            } else {
                ""
            };
            parts.push(format!(
                "changed {} ({}{more})",
                plural(self.settings.len(), "setting", "settings"),
                shown.join(", ")
            ));
        }
        for (t, n) in &self.invoked.0 {
            parts.push(invoke_phrase(t, *n));
        }
        format!("{who}: {}", parts.join(", "))
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// What a known `Invoke` target did, in words.
fn invoke_phrase(target: &str, n: usize) -> String {
    match target {
        "forge.project.promote" => "changed the workspace preset".into(),
        "forge.project.link_remote" => "linked a remote".into(),
        "forge.project.load" => "loaded the project".into(),
        other if n == 1 => format!("ran {other}"),
        other => format!("ran {other} \u{d7}{n}"),
    }
}

/// The per-issuer reading of a changing set of commands, kept incrementally (see the module
/// docs). Issuers are listed in the order they first contributed.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    order: Vec<String>,
    by: BTreeMap<String, PerIssuer>,
    /// Bumped whenever a line may have changed.
    generation: u64,
}

impl Summary {
    /// Count `e` in.
    pub fn add(&mut self, e: &CommandEnvelope) {
        let who = e.issuer.tag();
        let p = match self.by.get_mut(&who) {
            Some(p) => p,
            None => {
                self.order.push(who.clone());
                self.by.entry(who).or_default()
            }
        };
        p.held += 1;
        p.apply(&e.cmd, true);
        self.generation += 1;
    }

    /// Count `e` out again (it was added before: its transaction was undone or cancelled, or
    /// a later frame of its gesture replaced it).
    pub fn remove(&mut self, e: &CommandEnvelope) {
        let who = e.issuer.tag();
        let Some(p) = self.by.get_mut(&who) else {
            return;
        };
        p.apply(&e.cmd, false);
        p.held = p.held.saturating_sub(1);
        if p.held == 0 {
            self.by.remove(&who);
            self.order.retain(|w| *w != who);
        }
        self.generation += 1;
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        self.order.clear();
        self.by.clear();
        self.generation += 1;
    }

    /// Bumped by every [`Summary::add`], [`Summary::remove`] and [`Summary::clear`].
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// One line per issuer: `"<issuer>: <what it did>"`.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.order
            .iter()
            .filter_map(|who| self.by.get(who).map(|p| p.line(who)))
            .collect()
    }
}

/// One line per issuer (first-seen order): `"<issuer>: <what it did>"`.
#[must_use]
pub fn summarize(envelopes: &[CommandEnvelope]) -> Vec<String> {
    let mut s = Summary::default();
    for e in envelopes {
        s.add(e);
    }
    s.lines()
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_cmd::{CommandId, Issuer, TxnId, Value};

    fn env(issuer: Issuer, cmd: EditorCommand) -> CommandEnvelope {
        CommandEnvelope {
            id: CommandId(0),
            txn: TxnId(0),
            issuer,
            cmd,
        }
    }

    #[test]
    fn history_reads_as_what_each_issuer_did() {
        let auto = Issuer::Automation {
            session: "sess-4".into(),
            tool: "apply".into(),
        };
        let ada = Issuer::Human { user: "ada".into() };
        let mut log = Vec::new();
        for _ in 0..37 {
            log.push(env(
                auto.clone(),
                EditorCommand::Spawn {
                    name: "Tree".into(),
                    parent: None,
                },
            ));
        }
        // A 600-frame drag of one property is one property.
        for i in 0..600 {
            log.push(env(
                ada.clone(),
                EditorCommand::SetProperty {
                    entity: EntityKey(3),
                    path: "transform.yaw".into(),
                    value: Value::Float(f64::from(i)),
                },
            ));
        }
        log.push(env(
            ada.clone(),
            EditorCommand::SetSetting {
                key: "editor.grid_size".into(),
                value: Some(Value::Float(2.0)),
            },
        ));
        log.push(env(
            ada,
            EditorCommand::Invoke {
                target: "forge.project.promote".into(),
                args: "{}".into(),
            },
        ));
        let s = summarize(&log);
        assert_eq!(
            s,
            vec![
                "automation:sess-4: placed 37 \u{201c}Tree\u{201d}".to_string(),
                "human:ada: edited 1 property on 1 entity, changed 1 setting (editor), changed the workspace preset".to_string(),
            ]
        );
        assert!(summarize(&[]).is_empty());
    }

    /// The incremental summary reads exactly as a fresh one over what it still holds, after
    /// any mix of adds and removes (an undo, a redo, a folded gesture frame).
    #[test]
    fn removing_reads_as_never_having_added() {
        let ada = Issuer::Human { user: "ada".into() };
        let bob = Issuer::Human { user: "bob".into() };
        let mut log = Vec::new();
        for (i, who) in [&ada, &bob, &ada, &ada, &bob].into_iter().enumerate() {
            let k = u64::try_from(i).unwrap_or(0);
            log.push(env(
                who.clone(),
                EditorCommand::Spawn {
                    name: if i % 2 == 0 { "Rock" } else { "Tree" }.into(),
                    parent: None,
                },
            ));
            log.push(env(
                who.clone(),
                EditorCommand::SetProperty {
                    entity: EntityKey(k % 2),
                    path: format!("p{}", i % 3),
                    value: Value::Float(1.0),
                },
            ));
            log.push(env(
                who.clone(),
                EditorCommand::SetSetting {
                    key: format!("s{}.k{i}", i % 4),
                    value: None,
                },
            ));
            log.push(env(
                who.clone(),
                EditorCommand::Invoke {
                    target: "forge.project.link_remote".into(),
                    args: "{}".into(),
                },
            ));
            log.push(env(
                who.clone(),
                EditorCommand::Despawn {
                    entity: EntityKey(k),
                },
            ));
        }
        let mut s = Summary::default();
        for e in &log {
            s.add(e);
        }
        assert_eq!(s.lines(), summarize(&log));
        // Take out every third command, then every one of bob's.
        let mut kept = Vec::new();
        for (i, e) in log.iter().enumerate() {
            if i % 3 == 0 {
                s.remove(e);
            } else {
                kept.push(e.clone());
            }
        }
        assert_eq!(s.lines(), summarize(&kept));
        let (bobs, rest): (Vec<_>, Vec<_>) = kept.into_iter().partition(|e| e.issuer == bob);
        for e in &bobs {
            s.remove(e);
        }
        assert_eq!(s.lines(), summarize(&rest));
        // Put bob's back (a redo): he is listed after ada, as a fresh reading would.
        let mut all = rest.clone();
        for e in &bobs {
            s.add(e);
            all.push(e.clone());
        }
        assert_eq!(s.lines(), summarize(&all));
        s.clear();
        assert!(s.lines().is_empty());
    }
}
