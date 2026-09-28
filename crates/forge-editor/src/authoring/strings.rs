//! The game's string tables (Ch.28 localisation; Ch.21 §21.21 "Localisation", DoD M2-65):
//! tables of keys, a source locale and translations, with the pseudo-locale as a built-in
//! check. What the game looks up at run time is exactly `forge_ui::l10n` (the same
//! templates, fallback and pseudo-localisation the editor previews).
//!
//! Project settings (see [`crate::domain`]):
//!
//! | Key | Value |
//! |---|---|
//! | `loc.source` | the source locale's id (default `en`) |
//! | `loc.locale.<l>.tag` / `.name` | the BCP-47 tag (`fr-FR`) and its endonym (`Français`) |
//! | `loc.table.<t>.key.<k>.text.<l>` | the text of `<t>.<k>` in locale `<l>` |
//! | `loc.table.<t>.key.<k>.note` | context for translators |
//! | `loc.table.<t>.key.<k>.max` | the most characters the UI has room for (integer) |
//!
//! A string's runtime key is `<t>.<k>` (`menu.play`). Every edit is a `SetSetting` command
//! (I7); a CSV import is one transaction, one undo entry.
//!
//! **Problems** the editor lists ([`StringsDoc::issues`]): a translation missing in a
//! locale, a translation that drops or invents a `{placeholder}` (it would format wrong),
//! and text — including the pseudo-localised source, which is ~40 % longer — that exceeds
//! the key's `max` (it would not fit).

use std::collections::BTreeMap;

use forge_cmd::{EditorCommand, Value};
use forge_ui::l10n::{
    CsvTable, Localiser, PSEUDO_LOCALE, parse_csv, placeholder_mismatch, pseudo_localise, to_csv,
};

use crate::domain::{BackendInfo, ident_from, int, is_ident, objects, set, sub_objects, text};
use crate::mirror::ProjectMirror;

pub const TABLE: &str = "loc.table";
pub const LOCALE: &str = "loc.locale";
pub const SOURCE: &str = "loc.source";

/// One locale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Locale {
    /// The settings id (`fr_fr`).
    pub id: String,
    /// The BCP-47 tag (`fr-FR`), what the runtime is asked for.
    pub tag: String,
    pub name: String,
}

/// One string.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub table: String,
    pub key: String,
    /// Locale id → text.
    pub texts: BTreeMap<String, String>,
    pub note: String,
    /// Characters the UI has room for.
    pub max: Option<usize>,
}

impl Entry {
    /// `table.key`, the runtime key.
    pub fn full_key(&self) -> String {
        format!("{}.{}", self.table, self.key)
    }
}

/// A problem with one string, for the panel's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Issue {
    Missing {
        key: String,
        locale: String,
    },
    Placeholders {
        key: String,
        locale: String,
        missing: Vec<String>,
        extra: Vec<String>,
    },
    TooLong {
        key: String,
        locale: String,
        len: usize,
        max: usize,
    },
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Issue::Missing { key, locale } => write!(f, "{key}: no {locale} text"),
            Issue::Placeholders {
                key,
                locale,
                missing,
                extra,
            } => {
                write!(f, "{key} ({locale}): ")?;
                if !missing.is_empty() {
                    write!(f, "drops {{{}}}", missing.join("}, {"))?;
                }
                if !extra.is_empty() {
                    if !missing.is_empty() {
                        write!(f, "; ")?;
                    }
                    write!(f, "invents {{{}}}", extra.join("}, {"))?;
                }
                Ok(())
            }
            Issue::TooLong {
                key,
                locale,
                len,
                max,
            } => write!(f, "{key} ({locale}): {len} characters, room for {max}"),
        }
    }
}

/// Every table, read from the mirror.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StringsDoc {
    pub source: String,
    /// By id; the source locale is always present.
    pub locales: BTreeMap<String, Locale>,
    /// By `(table, key)`.
    pub entries: BTreeMap<(String, String), Entry>,
    pub problems: Vec<String>,
}

impl StringsDoc {
    pub fn read(m: &ProjectMirror) -> StringsDoc {
        let mut d = StringsDoc::default();
        let p = &mut d.problems;
        d.source = match m.setting(SOURCE) {
            Some(Value::Text(s)) if is_ident(s) => s.clone(),
            None => "en".into(),
            Some(v) => {
                p.push(format!(
                    "{SOURCE}: expected a locale id, found {v:?} (en used)"
                ));
                "en".into()
            }
        };
        for (id, f) in objects(m, LOCALE) {
            let at = format!("{LOCALE}.{id}");
            d.locales.insert(
                id.to_string(),
                Locale {
                    id: id.to_string(),
                    tag: text(&f, "tag", id, p, &at),
                    name: text(&f, "name", id, p, &at),
                },
            );
        }
        d.locales.entry(d.source.clone()).or_insert_with(|| Locale {
            id: d.source.clone(),
            tag: d.source.replace('_', "-"),
            name: d.source.clone(),
        });
        for (tid, tf) in objects(m, TABLE) {
            for (kid, kf) in sub_objects(&tf, "key") {
                let at = format!("{TABLE}.{tid}.key.{kid}");
                let mut texts = BTreeMap::new();
                for (k, v) in &kf {
                    let Some(l) = k.strip_prefix("text.") else {
                        continue;
                    };
                    match v {
                        Value::Text(t) => {
                            texts.insert(l.to_string(), t.clone());
                        }
                        other => p.push(format!("{at}.{k}: expected text, found {}", other.kind())),
                    }
                }
                let max = match int(&kf, "max", -1, p, &at) {
                    n if n > 0 => Some(n as usize),
                    _ => None,
                };
                d.entries.insert(
                    (tid.to_string(), kid.to_string()),
                    Entry {
                        table: tid.to_string(),
                        key: kid.to_string(),
                        texts,
                        note: text(&kf, "note", "", p, &at),
                        max,
                    },
                );
            }
        }
        d
    }

    /// The source locale's tag.
    pub fn source_tag(&self) -> String {
        self.locales
            .get(&self.source)
            .map_or_else(|| self.source.replace('_', "-"), |l| l.tag.clone())
    }

    /// The locales other than the source, by id.
    pub fn targets(&self) -> Vec<&Locale> {
        self.locales
            .values()
            .filter(|l| l.id != self.source)
            .collect()
    }

    /// Translated share of `locale` (0..=1; 1 for an empty project).
    pub fn coverage(&self, locale: &str) -> f64 {
        let with_source = self
            .entries
            .values()
            .filter(|e| e.texts.contains_key(&self.source))
            .count();
        if with_source == 0 {
            return 1.0;
        }
        let done = self
            .entries
            .values()
            .filter(|e| e.texts.contains_key(&self.source) && e.texts.contains_key(locale))
            .count();
        done as f64 / with_source as f64
    }

    /// Every problem (see the module docs), in key order.
    pub fn issues(&self) -> Vec<Issue> {
        let mut out = Vec::new();
        for e in self.entries.values() {
            let key = e.full_key();
            let Some(src) = e.texts.get(&self.source) else {
                out.push(Issue::Missing {
                    key: key.clone(),
                    locale: self.source_tag(),
                });
                continue;
            };
            if let Some(max) = e.max {
                let pseudo = pseudo_localise(src).chars().count();
                if pseudo > max {
                    out.push(Issue::TooLong {
                        key: key.clone(),
                        locale: PSEUDO_LOCALE.into(),
                        len: pseudo,
                        max,
                    });
                }
            }
            for l in self.targets() {
                match e.texts.get(&l.id) {
                    None => out.push(Issue::Missing {
                        key: key.clone(),
                        locale: l.tag.clone(),
                    }),
                    Some(t) => {
                        let (missing, extra) = placeholder_mismatch(src, t);
                        if !missing.is_empty() || !extra.is_empty() {
                            out.push(Issue::Placeholders {
                                key: key.clone(),
                                locale: l.tag.clone(),
                                missing,
                                extra,
                            });
                        }
                        if let Some(max) = e.max {
                            let len = t.chars().count();
                            if len > max {
                                out.push(Issue::TooLong {
                                    key: key.clone(),
                                    locale: l.tag.clone(),
                                    len,
                                    max,
                                });
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// The runtime lookup these tables compile to (locale **tags**, as the game asks).
    pub fn localiser(&self) -> Localiser {
        let tag = |id: &str| {
            self.locales
                .get(id)
                .map_or_else(|| id.to_string(), |l| l.tag.clone())
        };
        let mut l = Localiser::new(&tag(&self.source));
        for e in self.entries.values() {
            for (loc, t) in &e.texts {
                l.insert(&tag(loc), &e.full_key(), t);
            }
        }
        l
    }

    /// Everything as the translator's CSV: `key`, then the source, then each locale (by tag).
    pub fn export_csv(&self) -> String {
        let mut order: Vec<&Locale> = vec![];
        if let Some(s) = self.locales.get(&self.source) {
            order.push(s);
        }
        order.extend(self.targets());
        let tags: Vec<&str> = order.iter().map(|l| l.tag.as_str()).collect();
        let rows: Vec<(String, Vec<Option<String>>)> = self
            .entries
            .values()
            .map(|e| {
                (
                    e.full_key(),
                    order.iter().map(|l| e.texts.get(&l.id).cloned()).collect(),
                )
            })
            .collect();
        to_csv(&tags, &rows)
    }
}

fn entry_key(t: &str, k: &str, f: &str) -> String {
    format!("{TABLE}.{t}.key.{k}.{f}")
}

/// `table.key` split and checked (both parts are identifiers).
pub fn split_key(full: &str) -> Result<(String, String), String> {
    let (t, k) = full
        .split_once('.')
        .ok_or_else(|| format!("{full:?} is not table.key (e.g. menu.play)"))?;
    if !is_ident(t) || !is_ident(k) {
        return Err(format!(
            "{full:?}: a table and a key are letters, digits and _ (e.g. menu.play)"
        ));
    }
    Ok((t.to_string(), k.to_string()))
}

/// A new string `table.key` with its source text.
pub fn add_string(
    doc: &StringsDoc,
    full: &str,
    source_text: &str,
) -> Result<Vec<EditorCommand>, String> {
    let (t, k) = split_key(full)?;
    if doc.entries.contains_key(&(t.clone(), k.clone())) {
        return Err(format!("{full} already exists"));
    }
    Ok(vec![set(
        entry_key(&t, &k, &format!("text.{}", doc.source)),
        Value::Text(source_text.into()),
    )])
}

/// Set (or, with an empty text, clear) one translation.
pub fn set_text(
    doc: &StringsDoc,
    e: &Entry,
    locale: &str,
    t: &str,
) -> Result<Vec<EditorCommand>, String> {
    if !doc.locales.contains_key(locale) {
        return Err(format!("no locale {locale}"));
    }
    let k = entry_key(&e.table, &e.key, &format!("text.{locale}"));
    Ok(vec![if t.is_empty() {
        crate::domain::clear(k)
    } else {
        set(k, Value::Text(t.into()))
    }])
}

pub fn set_note(e: &Entry, note: &str) -> Vec<EditorCommand> {
    let k = entry_key(&e.table, &e.key, "note");
    vec![if note.is_empty() {
        crate::domain::clear(k)
    } else {
        set(k, Value::Text(note.into()))
    }]
}

pub fn set_max(e: &Entry, max: Option<usize>) -> Vec<EditorCommand> {
    let k = entry_key(&e.table, &e.key, "max");
    vec![match max {
        Some(n) if n > 0 => set(k, Value::Int(n as i64)),
        _ => crate::domain::clear(k),
    }]
}

pub fn delete_string(m: &ProjectMirror, e: &Entry) -> Vec<EditorCommand> {
    crate::domain::clear_under(m, &format!("{TABLE}.{}.key.{}", e.table, e.key))
}

/// A locale from its BCP-47 tag (`fr-FR`) and endonym: `(id, commands)`.
pub fn add_locale(
    doc: &StringsDoc,
    tag: &str,
    name: &str,
) -> Result<(String, Vec<EditorCommand>), String> {
    let tag = tag.trim();
    let ok = !tag.is_empty()
        && tag.len() <= 35
        && tag
            .split('-')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()));
    if !ok {
        return Err(format!(
            "{tag:?} is not a language tag (e.g. fr-FR, de, pt-BR)"
        ));
    }
    let id = ident_from(tag);
    if doc.locales.contains_key(&id) || tag == PSEUDO_LOCALE {
        return Err(format!("{tag} is already a locale"));
    }
    Ok((
        id.clone(),
        vec![
            set(format!("{LOCALE}.{id}.tag"), Value::Text(tag.into())),
            set(
                format!("{LOCALE}.{id}.name"),
                Value::Text(name.trim().into()),
            ),
        ],
    ))
}

/// What a CSV import will do, before it is sent (the panel shows the counts).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportPlan {
    pub commands: Vec<EditorCommand>,
    pub changed: usize,
    pub added_keys: usize,
    pub unchanged: usize,
    /// Columns whose tag is not a locale of the project (skipped, named).
    pub unknown_locales: Vec<String>,
    /// Rows refused, with the reason.
    pub refused: Vec<String>,
}

/// Plan a translator's CSV import (see [`forge_ui::l10n::parse_csv`]): each cell that
/// differs from the project becomes one `SetSetting`; the whole import is one transaction.
/// Empty cells change nothing (a translator's blank is not a deletion).
pub fn plan_import(doc: &StringsDoc, csv: &str) -> Result<ImportPlan, String> {
    let (tags, rows): CsvTable = parse_csv(csv)?;
    let by_tag: BTreeMap<&str, &str> = doc
        .locales
        .values()
        .map(|l| (l.tag.as_str(), l.id.as_str()))
        .collect();
    let mut plan = ImportPlan::default();
    let cols: Vec<Option<&str>> = tags
        .iter()
        .map(|t| {
            let id = by_tag.get(t.as_str()).copied();
            if id.is_none() {
                plan.unknown_locales.push(t.clone());
            }
            id
        })
        .collect();
    for (full, texts) in rows {
        let (t, k) = match split_key(&full) {
            Ok(x) => x,
            Err(e) => {
                plan.refused.push(e);
                continue;
            }
        };
        let existing = doc.entries.get(&(t.clone(), k.clone()));
        if existing.is_none() {
            plan.added_keys += 1;
        }
        for (col, text) in cols.iter().zip(texts) {
            let (Some(loc), Some(text)) = (col, text) else {
                continue;
            };
            if existing.and_then(|e| e.texts.get(*loc)) == Some(&text) {
                plan.unchanged += 1;
                continue;
            }
            plan.changed += 1;
            plan.commands.push(set(
                entry_key(&t, &k, &format!("text.{loc}")),
                Value::Text(text),
            ));
        }
    }
    Ok(plan)
}

/// The game's string runtime (Ch.28): what compiles the tables into what ships. The real
/// one is `forge-play` (M5-8); [`MemoryStrings`] compiles to `forge_ui::l10n::Localiser`
/// in memory — the lookup the shipped game's UI performs — and is labelled (D-4).
pub trait StringTables {
    fn backend(&self) -> BackendInfo;
    /// Compile the tables into the runtime lookup.
    fn compile(&self, doc: &StringsDoc) -> Localiser;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryStrings;

impl StringTables for MemoryStrings {
    fn backend(&self) -> BackendInfo {
        BackendInfo {
            name: "in-memory string runtime".into(),
            in_memory: true,
            note: "Tables compile to forge-ui's runtime lookup in memory; packing them into a shipped game is forge-play (M5-8), not built yet."
                .into(),
        }
    }
    fn compile(&self, doc: &StringsDoc) -> Localiser {
        doc.localiser()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> StringsDoc {
        let mut d = StringsDoc {
            source: "en".into(),
            ..StringsDoc::default()
        };
        for (id, tag) in [("en", "en"), ("fr_fr", "fr-FR")] {
            d.locales.insert(
                id.into(),
                Locale {
                    id: id.into(),
                    tag: tag.into(),
                    name: id.into(),
                },
            );
        }
        let mut e = Entry {
            table: "menu".into(),
            key: "score".into(),
            max: Some(12),
            ..Entry::default()
        };
        e.texts.insert("en".into(), "Score: {n}".into());
        e.texts.insert("fr_fr".into(), "Le score est : {m}".into());
        d.entries.insert(("menu".into(), "score".into()), e);
        let mut e = Entry {
            table: "menu".into(),
            key: "play".into(),
            ..Entry::default()
        };
        e.texts.insert("en".into(), "Play".into());
        d.entries.insert(("menu".into(), "play".into()), e);
        d
    }

    #[test]
    fn issues_name_missing_placeholders_and_length() {
        let d = doc();
        let issues: Vec<String> = d.issues().iter().map(ToString::to_string).collect();
        assert!(
            issues.contains(&"menu.play: no fr-FR text".to_string()),
            "{issues:?}"
        );
        assert!(
            issues.contains(&"menu.score (fr-FR): drops {n}; invents {m}".to_string()),
            "{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.starts_with("menu.score (fr-FR): 18 characters, room for 12")),
            "{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.starts_with("menu.score (qps-ploc):")),
            "the pseudo text does not fit either: {issues:?}"
        );
        assert_eq!(d.coverage("fr_fr"), 0.5);
    }

    #[test]
    fn compiled_lookup_uses_tags_and_falls_back() {
        let d = doc();
        let mut l = d.localiser();
        assert_eq!(l.get("menu.play", &[]), "Play");
        assert!(l.set_current("fr-FR"));
        assert_eq!(l.get("menu.play", &[]), "Play", "falls back to the source");
        assert_eq!(l.get("menu.score", &[("m", "3")]), "Le score est : 3");
    }

    #[test]
    fn csv_round_trip_plans_only_changes() {
        let d = doc();
        let csv = d.export_csv();
        assert!(csv.starts_with("key,en,fr-FR\n"), "{csv}");
        let plan = plan_import(&d, &csv).expect("parses");
        assert_eq!(plan.changed, 0, "{plan:?}");
        assert!(plan.commands.is_empty());
        let plan = plan_import(
            &d,
            "key,fr-FR,de\nmenu.play,Jouer,Spielen\nnew.key,Nouveau,\nbad key,x,\n",
        )
        .expect("parses");
        assert_eq!(plan.changed, 2, "{plan:?}");
        assert_eq!(plan.added_keys, 1);
        assert_eq!(plan.unknown_locales, vec!["de".to_string()]);
        assert_eq!(plan.refused.len(), 1);
    }

    #[test]
    fn keys_and_tags_are_checked() {
        assert!(split_key("menu.play").is_ok());
        assert!(split_key("menu").is_err());
        assert!(split_key("menu.play now").is_err());
        let d = doc();
        assert!(add_locale(&d, "de-DE", "Deutsch").is_ok());
        assert!(add_locale(&d, "fr-FR", "x").is_err(), "exists");
        assert!(add_locale(&d, "not a tag", "x").is_err());
        assert!(add_string(&d, "menu.play", "x").is_err(), "exists");
    }
}
