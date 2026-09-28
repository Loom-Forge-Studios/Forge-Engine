//! The **Localisation** editor (`forge.localisation`, Ch.21 §21.21, DoD M2-65; Ch.28) over
//! `forge_editor::authoring::strings`:
//!
//! * **String tables**: keys (`menu.play`), the source text, a translation per locale, a
//!   note for translators and the room the UI has (`max` characters). The list is
//!   virtualised, so a table of thousands of strings costs what is on screen.
//! * **Locales** by BCP-47 tag with their coverage; the pseudo-locale is always there.
//! * **Pseudo-locale preview**: every string pseudo-localised (accented, ~40 % longer,
//!   bracketed, placeholders intact) beside its source, and a toggle that shows the whole
//!   table as the pseudo-locale would — text that does not fit is listed before it ships.
//! * **The game's view**: the selected string as the compiled runtime lookup returns it in
//!   the chosen locale (the same `forge_ui::l10n` the shipped game UI uses), formatted with
//!   sample arguments.
//! * **Problems**: missing translations, dropped or invented `{placeholders}`, text over its
//!   `max`; activating one selects its string.
//! * **CSV hand-off**: export the tables, import a translator's file — one transaction,
//!   one undo entry; unknown locales and bad keys are named, blank cells change nothing.
//!
//! Every project edit is a command (I7).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use forge_editor::authoring::strings::{self as ls, Entry, StringsDoc};
use forge_editor::mirror::ProjectMirror;
use forge_editor::panels::PanelCx;
use forge_editor::services::EditorServices;
use forge_ui::l10n::{PSEUDO_LOCALE, placeholders, pseudo_localise};
use forge_ui::widgets::{
    Button, Container, IntegerCommitted, IntegerField, Label, LabelKind, MultilineEditor, Pressed,
    RowActivated, RowItem, RowsDeleteRequested, SelectionChanged, Submitted, TextField,
    VirtualTree,
};
use forge_ui::{NodeStyle, Role, Signal, Ui, WidgetId};

use crate::common::{Row, Rows, key_of, refuse, select, show_rows, watch};

const PREFIX: &str = "loc.";

struct Lz {
    doc: StringsDoc,
    seen: u64,
    sel: Option<(String, String)>,
    pending: Option<(String, String)>,
    /// The locale the translation field and the game view show.
    locale: Option<String>,
    pseudo: bool,
    string_rows: Rows<(String, String)>,
    locale_rows: Rows<String>,
    issue_rows: Rows<(String, String)>,
    strings: WidgetId,
    /// The strings list's empty state (M2-70).
    strings_empty: WidgetId,
    locales: WidgetId,
    issues: WidgetId,
    source: Signal<String>,
    translation: Signal<String>,
    translation_label: Signal<String>,
    note: Signal<String>,
    max: Signal<i128>,
    pseudo_text: Signal<String>,
    game_view: Signal<String>,
    status: Signal<String>,
    summary: Signal<String>,
    pseudo_label: Signal<String>,
    csv: Signal<String>,
}

impl Lz {
    fn say(&self, ui: &mut Ui, s: impl Into<String>) {
        self.status.set(ui.rt_mut(), s.into());
    }
    fn entry(&self) -> Option<&Entry> {
        self.sel.as_ref().and_then(|k| self.doc.entries.get(k))
    }

    fn reread(&mut self, ui: &mut Ui, m: &ProjectMirror, sv: &EditorServices) {
        self.doc = StringsDoc::read(m);
        if let Some(k) = self.pending.take()
            && self.doc.entries.contains_key(&k)
        {
            self.sel = Some(k);
        }
        if self
            .sel
            .as_ref()
            .is_none_or(|k| !self.doc.entries.contains_key(k))
        {
            self.sel = self.doc.entries.keys().next().cloned();
        }
        if self
            .locale
            .as_ref()
            .is_none_or(|l| !self.doc.locales.contains_key(l) || *l == self.doc.source)
        {
            self.locale = self.doc.targets().first().map(|l| l.id.clone());
        }
        self.show(ui, sv);
    }

    fn show(&mut self, ui: &mut Ui, sv: &EditorServices) {
        let src = self.doc.source.clone();
        // The strings: the source text, or its pseudo form in the pseudo view.
        let mut rows: Vec<Row> = Vec::with_capacity(self.doc.entries.len());
        let mut map = HashMap::with_capacity(self.doc.entries.len());
        for (k, e) in &self.doc.entries {
            let text = e.texts.get(&src).map(String::as_str).unwrap_or("");
            let shown = if self.pseudo {
                pseudo_localise(text)
            } else {
                text.to_string()
            };
            let missing = self
                .doc
                .targets()
                .iter()
                .filter(|l| !e.texts.contains_key(&l.id))
                .count();
            let rk = key_of(&["str", &k.0, &k.1]);
            let mut item = RowItem::new(format!("{} \u{2014} {shown}", e.full_key()));
            if missing > 0 {
                item.note = Some(forge_ui::trf!("{missing} untranslated", missing));
            }
            rows.push((None, rk, item));
            map.insert(rk, k.clone());
        }
        let no_strings = rows.is_empty();
        show_rows(ui, self.strings, rows, map, &mut self.string_rows);
        let _ = ui.set_hidden(self.strings_empty, !no_strings);
        if let Some(k) = &self.sel {
            select(ui, self.strings, key_of(&["str", &k.0, &k.1]));
        }
        // Locales with coverage.
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        for l in self.doc.locales.values() {
            let k = key_of(&["loc", &l.id]);
            let label = if l.id == src {
                forge_ui::trf!("{tag} \u{2014} {name} (source)", tag = l.tag, name = l.name)
            } else {
                format!(
                    "{} \u{2014} {} ({:.0}%)",
                    l.tag,
                    l.name,
                    self.doc.coverage(&l.id) * 100.0
                )
            };
            rows.push((None, k, RowItem::new(label)));
            map.insert(k, l.id.clone());
        }
        let pk = key_of(&["loc", PSEUDO_LOCALE]);
        rows.push((
            None,
            pk,
            RowItem::new(forge_ui::trf!(
                "{PSEUDO_LOCALE} \u{2014} pseudo-locale (generated)",
                PSEUDO_LOCALE
            )),
        ));
        map.insert(pk, PSEUDO_LOCALE.to_string());
        show_rows(ui, self.locales, rows, map, &mut self.locale_rows);
        if let Some(l) = &self.locale {
            select(ui, self.locales, key_of(&["loc", l]));
        }
        // Problems.
        let issues = self.doc.issues();
        let mut rows: Vec<Row> = Vec::new();
        let mut map = HashMap::new();
        for (i, is) in issues.iter().enumerate() {
            let k = key_of(&["issue", &i.to_string(), &is.to_string()]);
            let key = match is {
                ls::Issue::Missing { key, .. }
                | ls::Issue::Placeholders { key, .. }
                | ls::Issue::TooLong { key, .. } => key.clone(),
            };
            if let Ok(tk) = ls::split_key(&key) {
                map.insert(k, tk);
            }
            rows.push((None, k, RowItem::new(forge_ui::trf!("\u{26a0} {is}", is))));
        }
        show_rows(ui, self.issues, rows, map, &mut self.issue_rows);
        let targets = self.doc.targets();
        let cov: Vec<String> = targets
            .iter()
            .map(|l| format!("{} {:.0}%", l.tag, self.doc.coverage(&l.id) * 100.0))
            .collect();
        self.summary.set(
            ui.rt_mut(),
            forge_ui::trf!(
                "{entries_count} string(s), source {src}; {items}; {issues_count} problem(s){problems}",
                entries_count = self.doc.entries.len(),
                src,
                items = if cov.is_empty() {
                    forge_ui::tr!("no translations yet").to_string()
                } else {
                    cov.join(", ")
                },
                issues_count = issues.len(),
                problems = if self.doc.problems.is_empty() {
                    String::new()
                } else {
                    forge_ui::trf!(
                        "; \u{26a0} {problems_count} value(s) skipped: {join}",
                        problems_count = self.doc.problems.len(),
                        join = self.doc.problems.join("; ")
                    )
                }
            ),
        );
        self.show_entry(ui, sv);
    }

    fn show_entry(&mut self, ui: &mut Ui, sv: &EditorServices) {
        let src = self.doc.source.clone();
        let loc = self.locale.clone();
        let (source, translation, note, max, pseudo, game) = match self.entry() {
            Some(e) => {
                let s = e.texts.get(&src).cloned().unwrap_or_default();
                let t = loc
                    .as_ref()
                    .and_then(|l| e.texts.get(l))
                    .cloned()
                    .unwrap_or_default();
                // The game's view: the compiled lookup, in the chosen locale, with sample
                // arguments for the placeholders.
                let mut rt = sv.strings.compile(&self.doc);
                let tag = loc
                    .as_ref()
                    .map(|l| {
                        if l == PSEUDO_LOCALE {
                            l.clone()
                        } else {
                            self.doc
                                .locales
                                .get(l)
                                .map_or_else(|| l.clone(), |x| x.tag.clone())
                        }
                    })
                    .unwrap_or_else(|| src.clone());
                rt.set_current(&tag);
                let args: Vec<(String, String)> = placeholders(&s)
                    .into_iter()
                    .enumerate()
                    .map(|(i, p)| (p.to_string(), format!("{}", (i + 1) * 7)))
                    .collect();
                let a: Vec<(&str, &str)> =
                    args.iter().map(|(x, y)| (x.as_str(), y.as_str())).collect();
                let shown = rt.get(&e.full_key(), &a);
                let fit = match e.max {
                    Some(m) if shown.chars().count() > m => forge_ui::trf!(
                        "  \u{26a0} {n} characters, room for {m}",
                        n = shown.chars().count(),
                        m
                    ),
                    _ => String::new(),
                };
                (
                    s.clone(),
                    t,
                    e.note.clone(),
                    e.max.map_or(0, |m| m as i128),
                    pseudo_localise(&s),
                    forge_ui::trf!("In the game ({tag}): {shown}{fit}", tag, shown, fit),
                )
            }
            None => (
                String::new(),
                String::new(),
                String::new(),
                0,
                String::new(),
                forge_ui::tr!("Add a string to see it as the game shows it.").into(),
            ),
        };
        self.source.set(ui.rt_mut(), source);
        self.translation.set(ui.rt_mut(), translation);
        let tl = match loc.as_deref() {
            Some(PSEUDO_LOCALE) => {
                forge_ui::tr!("Pseudo-locale (generated, read-only)").to_string()
            }
            Some(l) => forge_ui::trf!(
                "Translation ({tag})",
                tag = self.doc.locales.get(l).map_or(l, |x| x.tag.as_str())
            ),
            None => forge_ui::tr!("Translation (add a locale)").to_string(),
        };
        self.translation_label.set(ui.rt_mut(), tl);
        self.note.set(ui.rt_mut(), note);
        self.max.set(ui.rt_mut(), max);
        self.pseudo_text
            .set(ui.rt_mut(), forge_ui::trf!("Pseudo: {pseudo}", pseudo));
        self.game_view.set(ui.rt_mut(), game);
    }
}

pub fn build(cx: &mut PanelCx) {
    cx.add_live(|pb| {
        let space = pb.b.theme_ref().space[1];
        pb.want_turn();
        let sv = pb.services();
        let bar = pb.b.add(
            pb.parent,
            "bar",
            NodeStyle::row(space).padding(space).wrap(),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Localisation actions")),
        )?;
        let new_key = pb.b.signal(String::new());
        pb.b.add(
            bar,
            "key",
            NodeStyle::leaf().width(140.0),
            TextField::new(new_key, forge_ui::tr!("New key"))
                .placeholder(forge_ui::tr!("menu.play")),
        )?;
        let new_text = pb.b.signal(String::new());
        pb.b.add(
            bar,
            "text",
            NodeStyle::leaf().width(180.0),
            TextField::new(new_text, forge_ui::tr!("Source text"))
                .placeholder(forge_ui::tr!("Play")),
        )?;
        let add = pb.b.add(
            bar,
            "add",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Add string")),
        )?;
        let tag = pb.b.signal(String::new());
        pb.b.add(
            bar,
            "tag",
            NodeStyle::leaf().width(90.0),
            TextField::new(tag, forge_ui::tr!("Locale tag")).placeholder(forge_ui::tr!("fr-FR")),
        )?;
        let lname = pb.b.signal(String::new());
        pb.b.add(
            bar,
            "locale_name",
            NodeStyle::leaf().width(120.0),
            TextField::new(lname, forge_ui::tr!("Locale name"))
                .placeholder(forge_ui::tr!("Fran\u{e7}ais")),
        )?;
        let add_locale = pb.b.add(
            bar,
            "add_locale",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Add locale")),
        )?;
        let delete = pb.b.add(
            bar,
            "delete",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Delete string")),
        )?;
        let pseudo_label =
            pb.b.signal(forge_ui::tr!("Pseudo-locale view: off").to_string());
        let pseudo_b =
            pb.b.add(bar, "pseudo", NodeStyle::leaf(), Button::new(pseudo_label))?;
        let summary = pb.b.signal(String::new());
        pb.b.add(
            pb.parent,
            "summary",
            NodeStyle::leaf().padding(space),
            Label::new(summary).wrapping(),
        )?;
        let body = pb.b.add(
            pb.parent,
            "body",
            crate::common::body_row(space),
            Container::new(Role::Group).labelled(forge_ui::tr!("String tables")),
        )?;
        let strings_empty = pb.b.add(
            pb.parent,
            "strings_empty",
            NodeStyle::leaf(),
            forge_ui::widgets::EmptyState::new(forge_ui::tr!(
                "No strings yet: type a key above and press Add string."
            )),
        )?;
        let strings = pb.b.add(
            body,
            "strings",
            crate::common::fill(260.0, 160.0),
            VirtualTree::list(forge_ui::tr!("Strings")).single_select(),
        )?;
        let detail = pb.b.add(
            body,
            "detail",
            NodeStyle::column(space).width(340.0),
            Container::new(Role::Group).labelled(forge_ui::tr!("String")),
        )?;
        let locales = pb.b.add(
            detail,
            "locales",
            NodeStyle::leaf().min_size(0.0, 90.0),
            VirtualTree::list(forge_ui::tr!("Locales")).single_select(),
        )?;
        let source = pb.b.signal(String::new());
        pb.b.add(
            detail,
            "source_label",
            NodeStyle::leaf(),
            Label::new(forge_ui::tr!("Source text (Enter applies)")).kind(LabelKind::Small),
        )?;
        let source_f = pb.b.add(
            detail,
            "source",
            NodeStyle::leaf(),
            TextField::new(source, forge_ui::tr!("Source text")),
        )?;
        let translation_label = pb.b.signal(forge_ui::tr!("Translation").to_string());
        pb.b.add(
            detail,
            "translation_label",
            NodeStyle::leaf(),
            Label::new(translation_label).kind(LabelKind::Small),
        )?;
        let translation = pb.b.signal(String::new());
        let translation_f = pb.b.add(
            detail,
            "translation",
            NodeStyle::leaf(),
            TextField::new(translation, forge_ui::tr!("Translation")),
        )?;
        pb.b.add(
            detail,
            "note_label",
            NodeStyle::leaf(),
            Label::new(forge_ui::tr!("Note for translators")).kind(LabelKind::Small),
        )?;
        let note = pb.b.signal(String::new());
        let note_f = pb.b.add(
            detail,
            "note",
            NodeStyle::leaf(),
            TextField::new(note, forge_ui::tr!("Note for translators")),
        )?;
        let max = pb.b.signal(0i128);
        pb.b.add(
            detail,
            "max_label",
            NodeStyle::leaf(),
            Label::new(forge_ui::tr!(
                "Room in characters (0: no limit; Enter applies)"
            ))
            .kind(LabelKind::Small),
        )?;
        let max_f = pb.b.add(
            detail,
            "max",
            NodeStyle::leaf().width(120.0),
            IntegerField::new(max, forge_ui::tr!("Room in characters (0: unlimited)")),
        )?;
        let pseudo_text = pb.b.signal(String::new());
        pb.b.add(
            detail,
            "pseudo_text",
            NodeStyle::leaf(),
            Label::new(pseudo_text).wrapping(),
        )?;
        let game_view = pb.b.signal(String::new());
        pb.b.add(
            detail,
            "game_view",
            NodeStyle::leaf(),
            Label::new(game_view).wrapping(),
        )?;
        let issues = pb.b.add(
            pb.parent,
            "issues",
            NodeStyle::leaf().min_size(0.0, 80.0).padding(space),
            VirtualTree::list(forge_ui::tr!("Problems (Enter selects the string)"))
                .read_only()
                .single_select(),
        )?;
        let csv_bar = pb.b.add(
            pb.parent,
            "csv_bar",
            NodeStyle::row(space).padding(space),
            Container::new(Role::Toolbar).labelled(forge_ui::tr!("Translator hand-off")),
        )?;
        let export = pb.b.add(
            csv_bar,
            "export",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Export CSV")),
        )?;
        let import = pb.b.add(
            csv_bar,
            "import",
            NodeStyle::leaf(),
            Button::new(forge_ui::tr!("Import CSV")),
        )?;
        let csv = pb.b.signal(String::new());
        pb.b.add(
            pb.parent,
            "csv",
            NodeStyle::leaf().min_size(0.0, 90.0).padding(space),
            MultilineEditor::new(
                csv,
                forge_ui::tr!("CSV (key, then one column per locale tag)"),
            )
            .mono(),
        )?;
        let status = pb.b.signal(
            forge_ui::tr!("String tables for the game; the pseudo-locale checks they fit.")
                .to_string(),
        );
        pb.b.add(
            pb.parent,
            "status",
            NodeStyle::leaf().padding(space),
            Label::new(status).kind(LabelKind::Warning).wrapping(),
        )?;
        pb.b.add(
            pb.parent,
            "backend",
            NodeStyle::leaf().padding(space),
            Label::new(forge_ui::l10n::tr_str(&sv.strings.backend().note).into_owned())
                .kind(LabelKind::Small)
                .wrapping(),
        )?;

        let st = Rc::new(RefCell::new(Lz {
            doc: StringsDoc::default(),
            seen: u64::MAX,
            sel: None,
            pending: None,
            locale: None,
            pseudo: false,
            string_rows: Rows::default(),
            locale_rows: Rows::default(),
            issue_rows: Rows::default(),
            strings,
            strings_empty,
            locales,
            issues,
            source,
            translation,
            translation_label,
            note,
            max,
            pseudo_text,
            game_view,
            status,
            summary,
            pseudo_label,
            csv,
        }));

        let s = st.clone();
        pb.on(add, move |act, _: &Pressed| {
            let mut lz = s.borrow_mut();
            let k = new_key.get(act.ui.rt());
            let t = new_text.get(act.ui.rt());
            match ls::add_string(&lz.doc, k.trim(), &t) {
                Ok(cmds) => {
                    act.cmd.emit_all(forge_ui::tr!("Add string"), cmds);
                    lz.pending = ls::split_key(k.trim()).ok();
                    lz.say(act.ui, forge_ui::trf!("Added {text}.", text = k.trim()));
                }
                Err(why) => {
                    refuse(act.session, forge_ui::tr!("Add string"), &why);
                    lz.say(act.ui, format!("\u{26a0} {why}"));
                }
            }
        });
        let s = st.clone();
        pb.on(add_locale, move |act, _: &Pressed| {
            let mut lz = s.borrow_mut();
            let t = tag.get(act.ui.rt());
            let n = lname.get(act.ui.rt());
            let n = if n.trim().is_empty() {
                t.trim().to_string()
            } else {
                n
            };
            match ls::add_locale(&lz.doc, &t, &n) {
                Ok((id, cmds)) => {
                    act.cmd.emit_all(forge_ui::tr!("Add locale"), cmds);
                    lz.locale = Some(id.clone());
                    lz.say(
                        act.ui,
                        forge_ui::trf!("Added locale {text}.", text = t.trim()),
                    );
                }
                Err(why) => {
                    refuse(act.session, forge_ui::tr!("Add locale"), &why);
                    lz.say(act.ui, format!("\u{26a0} {why}"));
                }
            }
        });
        let s = st.clone();
        let del = move |act: &mut forge_editor::panel_rt::PanelAct| {
            let lz = s.borrow();
            if let Some(e) = lz.entry() {
                act.cmd.emit_all(
                    forge_ui::tr!("Delete string"),
                    ls::delete_string(act.mirror, e),
                );
            }
        };
        let d = Rc::new(del);
        let d2 = d.clone();
        pb.on(delete, move |act, _: &Pressed| d2(act));
        pb.on(strings, move |act, _: &RowsDeleteRequested| d(act));
        let s = st.clone();
        pb.on(pseudo_b, move |act, _: &Pressed| {
            let mut lz = s.borrow_mut();
            lz.pseudo = !lz.pseudo;
            let on = lz.pseudo;
            lz.pseudo_label.set(
                act.ui.rt_mut(),
                forge_ui::trf!(
                    "Pseudo-locale view: {state}",
                    state = if on {
                        forge_ui::tr!("on")
                    } else {
                        forge_ui::tr!("off")
                    }
                ),
            );
            let sv = act.services;
            lz.show(act.ui, sv);
        });
        let s = st.clone();
        pb.on(strings, move |act, e: &SelectionChanged| {
            let mut lz = s.borrow_mut();
            if let Some(k) = e.keys.first().and_then(|k| lz.string_rows.get(*k))
                && lz.sel.as_ref() != Some(&k)
            {
                lz.sel = Some(k);
                let sv = act.services;
                lz.show_entry(act.ui, sv);
            }
        });
        let s = st.clone();
        pb.on(locales, move |act, e: &SelectionChanged| {
            let mut lz = s.borrow_mut();
            if let Some(l) = e.keys.first().and_then(|k| lz.locale_rows.get(*k))
                && lz.locale.as_ref() != Some(&l)
            {
                lz.locale = Some(l);
                let sv = act.services;
                lz.show_entry(act.ui, sv);
            }
        });
        let s = st.clone();
        pb.on(issues, move |act, e: &RowActivated| {
            let mut lz = s.borrow_mut();
            if let Some(k) = lz.issue_rows.get(e.key) {
                lz.sel = Some(k.clone());
                let rk = key_of(&["str", &k.0, &k.1]);
                select(act.ui, lz.strings, rk);
                let sv = act.services;
                lz.show_entry(act.ui, sv);
            }
        });
        let s = st.clone();
        pb.on(source_f, move |act, e: &Submitted| {
            let lz = s.borrow();
            let Some(en) = lz.entry() else { return };
            let src = lz.doc.source.clone();
            match ls::set_text(&lz.doc, en, &src, &e.text) {
                Ok(cmds) => {
                    act.cmd.emit_all(forge_ui::tr!("Edit source text"), cmds);
                }
                Err(why) => refuse(act.session, forge_ui::tr!("Source text"), &why),
            }
        });
        let s = st.clone();
        pb.on(translation_f, move |act, e: &Submitted| {
            let lz = s.borrow();
            let Some(en) = lz.entry() else { return };
            let Some(l) = lz.locale.clone() else {
                refuse(
                    act.session,
                    forge_ui::tr!("Translation"),
                    forge_ui::tr!("Add a locale first."),
                );
                return;
            };
            if l == PSEUDO_LOCALE {
                refuse(
                    act.session,
                    forge_ui::tr!("Translation"),
                    forge_ui::tr!(
                        "The pseudo-locale is generated from the source; it is not edited."
                    ),
                );
                return;
            }
            match ls::set_text(&lz.doc, en, &l, &e.text) {
                Ok(cmds) => {
                    act.cmd.emit_all(forge_ui::tr!("Edit translation"), cmds);
                }
                Err(why) => refuse(act.session, forge_ui::tr!("Translation"), &why),
            }
        });
        let s = st.clone();
        pb.on(note_f, move |act, e: &Submitted| {
            let lz = s.borrow();
            if let Some(en) = lz.entry() {
                act.cmd
                    .emit_all(forge_ui::tr!("Edit note"), ls::set_note(en, &e.text));
            }
        });
        let s = st.clone();
        pb.on(max_f, move |act, e: &IntegerCommitted| {
            let lz = s.borrow();
            if let Some(en) = lz.entry() {
                let m = usize::try_from(e.value).ok().filter(|m| *m > 0);
                act.cmd
                    .emit_all(forge_ui::tr!("String room"), ls::set_max(en, m));
            }
        });
        let s = st.clone();
        pb.on(export, move |act, _: &Pressed| {
            let lz = s.borrow();
            let text = lz.doc.export_csv();
            let n = lz.doc.entries.len();
            lz.csv.set(act.ui.rt_mut(), text.clone());
            act.ui.clipboard().set_text(&text);
            lz.say(
                act.ui,
                forge_ui::trf!("Exported {n} string(s) (also on the clipboard).", n),
            );
        });
        let s = st.clone();
        pb.on(import, move |act, _: &Pressed| {
            let lz = s.borrow();
            let text = lz.csv.get(act.ui.rt());
            match ls::plan_import(&lz.doc, &text) {
                Ok(plan) => {
                    let mut msg = forge_ui::trf!(
                        "Import: {changed} change(s), {added} new key(s), {unchanged} unchanged",
                        changed = plan.changed,
                        added = plan.added_keys,
                        unchanged = plan.unchanged
                    );
                    if !plan.unknown_locales.is_empty() {
                        msg.push_str(&forge_ui::trf!(
                            "; skipped unknown locale(s) {locales}",
                            locales = plan.unknown_locales.join(", ")
                        ));
                    }
                    if !plan.refused.is_empty() {
                        msg.push_str(&forge_ui::trf!(
                            "; refused: {why}",
                            why = plan.refused.join("; ")
                        ));
                    }
                    if !plan.commands.is_empty() {
                        act.cmd
                            .emit_all(forge_ui::tr!("Import translations"), plan.commands);
                    }
                    lz.say(act.ui, msg);
                }
                Err(why) => {
                    refuse(act.session, forge_ui::tr!("Import CSV"), &why);
                    lz.say(act.ui, format!("\u{26a0} {why}"));
                }
            }
        });

        let s = st;
        pb.sync(strings, move |sy| {
            let rev = watch(sy.mirror, PREFIX);
            let mut lz = s.borrow_mut();
            if rev != lz.seen {
                lz.seen = rev;
                lz.reread(sy.ui, sy.mirror, sy.services);
            }
            Ok(())
        });
        Ok(())
    });
}
