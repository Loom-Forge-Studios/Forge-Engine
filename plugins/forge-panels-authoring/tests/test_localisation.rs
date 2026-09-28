//! The localisation editor (`forge.localisation`, DoD M2-65) through the running shell:
//! strings and locales as commands; translations, notes and room; problems (missing, a
//! dropped placeholder, too long — the pseudo text included); the pseudo-locale view and
//! the game's own lookup; a CSV import as one transaction, one undo entry.

mod common;

use common::{
    history_len, last_notice, part, press, raise, rows_with, select_row, setting, text_of,
    type_into, undo,
};
use forge_cmd::Value;
use forge_editor::testing::Rig;
use forge_ui::WidgetId;
use forge_ui::l10n::is_pseudo;
use forge_ui::widgets::{IntegerCommitted, RowActivated, Submitted};

const P: &str = "forge.localisation";

fn w(rig: &Rig, path: &[&str]) -> WidgetId {
    part(rig, P, path)
}

fn tap(rig: &mut Rig, path: &[&str]) {
    let b = w(rig, path);
    press(rig, b);
}

fn add_string(rig: &mut Rig, key: &str, text: &str) {
    let k = w(rig, &["bar", "key"]);
    type_into(rig, k, key);
    let t = w(rig, &["bar", "text"]);
    type_into(rig, t, text);
    tap(rig, &["bar", "add"]);
}

fn add_locale(rig: &mut Rig, tag: &str, name: &str) {
    let t = w(rig, &["bar", "tag"]);
    type_into(rig, t, tag);
    let n = w(rig, &["bar", "locale_name"]);
    type_into(rig, n, name);
    tap(rig, &["bar", "add_locale"]);
}

fn select_string(rig: &mut Rig, key: &str) {
    let list = w(rig, &["body", "strings"]);
    let row = rows_with(rig, list, &format!("{key} \u{2014}"));
    select_row(rig, list, row[0]);
}

fn submit(rig: &mut Rig, path: &[&str], text: &str) {
    let f = w(rig, path);
    raise(
        rig,
        f,
        Submitted {
            field: f,
            text: text.into(),
        },
    );
}

fn setup() -> Rig {
    let mut rig = common::rig(&[P]);
    add_locale(&mut rig, "fr-FR", "Fran\u{e7}ais");
    add_string(&mut rig, "menu.play", "Play");
    add_string(&mut rig, "hud.score", "Score: {score}");
    rig
}

#[test]
fn strings_locales_and_translations_are_commands() {
    let mut rig = setup();
    assert_eq!(
        setting(&rig, "loc.locale.fr_fr.tag"),
        Some(Value::Text("fr-FR".into()))
    );
    assert_eq!(
        setting(&rig, "loc.table.menu.key.play.text.en"),
        Some(Value::Text("Play".into()))
    );
    // A bad key is refused with the reason.
    let before = history_len(&rig);
    add_string(&mut rig, "no dot", "x");
    assert_eq!(history_len(&rig), before);
    let (_, why) = last_notice(&rig).unwrap_or_default();
    assert!(why.contains("table.key"), "{why}");
    // Translate: the selected string in the selected locale (fr-FR is the first target).
    select_string(&mut rig, "menu.play");
    let before = history_len(&rig);
    submit(&mut rig, &["body", "detail", "translation"], "Jouer");
    assert_eq!(history_len(&rig), before + 1);
    assert_eq!(
        setting(&rig, "loc.table.menu.key.play.text.fr_fr"),
        Some(Value::Text("Jouer".into()))
    );
    // The game's view is the compiled runtime lookup in that locale.
    let game = text_of(&rig, w(&rig, &["body", "detail", "game_view"]));
    assert!(game.contains("In the game (fr-FR): Jouer"), "{game}");
    // Note and room.
    submit(&mut rig, &["body", "detail", "note"], "Main menu button");
    assert_eq!(
        setting(&rig, "loc.table.menu.key.play.note"),
        Some(Value::Text("Main menu button".into()))
    );
    let max = w(&rig, &["body", "detail", "max"]);
    raise(
        &mut rig,
        max,
        IntegerCommitted {
            field: max,
            value: 6,
        },
    );
    assert_eq!(
        setting(&rig, "loc.table.menu.key.play.max"),
        Some(Value::Int(6))
    );
    // Undo takes the room back off.
    undo(&mut rig);
    assert!(setting(&rig, "loc.table.menu.key.play.max").is_none());
}

#[test]
fn problems_are_listed_and_the_pseudo_locale_shows_every_string() {
    let mut rig = setup();
    select_string(&mut rig, "hud.score");
    submit(
        &mut rig,
        &["body", "detail", "translation"],
        "Points : {points}",
    );
    let issues = w(&rig, &["issues"]);
    let rows = common::labels(&mut rig, issues);
    assert!(
        rows.iter()
            .any(|r| r.contains("hud.score (fr-FR): drops {score}; invents {points}")),
        "{rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("menu.play: no fr-FR text")),
        "{rows:?}"
    );
    // Room for 5 characters: the pseudo text of "Play" does not fit, and is listed.
    select_string(&mut rig, "menu.play");
    let max = w(&rig, &["body", "detail", "max"]);
    raise(
        &mut rig,
        max,
        IntegerCommitted {
            field: max,
            value: 5,
        },
    );
    let rows = common::labels(&mut rig, issues);
    assert!(
        rows.iter().any(|r| r.contains("menu.play (qps-ploc):")),
        "{rows:?}"
    );
    // Activating a problem selects its string.
    let row = rows_with(&mut rig, issues, "hud.score (fr-FR)");
    raise(
        &mut rig,
        issues,
        RowActivated {
            view: issues,
            key: row[0],
        },
    );
    let src = w(&rig, &["body", "detail", "source"]);
    rig.h.ui.a11y_activate();
    rig.settle();
    let shown = rig
        .h
        .ui
        .a11y_node(src)
        .and_then(|n| n.value().map(str::to_string))
        .unwrap_or_default();
    assert_eq!(shown, "Score: {score}");
    // The pseudo-locale view: every string transformed, placeholders intact.
    tap(&mut rig, &["bar", "pseudo"]);
    let list = w(&rig, &["body", "strings"]);
    let rows = common::labels(&mut rig, list);
    assert_eq!(rows.len(), 2);
    for r in &rows {
        let text = r.split(" \u{2014} ").nth(1).unwrap_or_default();
        assert!(is_pseudo(text), "{r}");
    }
    assert!(rows.iter().any(|r| r.contains("{score}")), "{rows:?}");
    // The game's view under the pseudo-locale.
    let locales = w(&rig, &["body", "detail", "locales"]);
    let row = rows_with(&mut rig, locales, "qps-ploc");
    select_row(&mut rig, locales, row[0]);
    let game = text_of(&rig, w(&rig, &["body", "detail", "game_view"]));
    let shown = game.trim_start_matches("In the game (qps-ploc): ");
    assert!(
        is_pseudo(shown.split("  \u{26a0}").next().unwrap_or_default()),
        "{game}"
    );
}

#[test]
fn a_csv_import_is_one_transaction_and_names_what_it_skipped() {
    let mut rig = setup();
    tap(&mut rig, &["csv_bar", "export"]);
    let csv = w(&rig, &["csv"]);
    let exported = rig
        .h
        .ui
        .widget::<forge_ui::widgets::MultilineEditor>(csv)
        .map(|m| m.model().get(rig.h.ui.rt()))
        .unwrap_or_default();
    assert!(exported.starts_with("key,en,fr-FR\n"), "{exported}");
    assert!(exported.contains("menu.play,Play,\n"), "{exported}");
    // A translator's file: two translations, one unknown locale, one bad key.
    let file = "key,fr-FR,de-DE\nmenu.play,Jouer,Spielen\nhud.score,Score : {score},\nbad key,x,\n";
    let model = rig
        .h
        .ui
        .widget::<forge_ui::widgets::MultilineEditor>(csv)
        .map(forge_ui::widgets::MultilineEditor::model)
        .unwrap_or_else(|| panic!("csv editor"));
    model.set(rig.h.ui.rt_mut(), file.to_string());
    let before = history_len(&rig);
    tap(&mut rig, &["csv_bar", "import"]);
    assert_eq!(history_len(&rig), before + 1, "one import, one undo entry");
    assert_eq!(
        setting(&rig, "loc.table.hud.key.score.text.fr_fr"),
        Some(Value::Text("Score : {score}".into()))
    );
    let status = text_of(&rig, w(&rig, &["status"]));
    assert!(status.contains("2 change(s)"), "{status}");
    assert!(status.contains("unknown locale(s) de-DE"), "{status}");
    assert!(status.contains("refused"), "{status}");
    undo(&mut rig);
    assert!(setting(&rig, "loc.table.menu.key.play.text.fr_fr").is_none());
    assert!(setting(&rig, "loc.table.hud.key.score.text.fr_fr").is_none());
}
