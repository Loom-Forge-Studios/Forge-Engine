//! The command palette (Ch.21 §21.17; DoD M2-45): every action, every panel and every
//! command is listed and reachable by keyboard; fuzzy search; chords shown; recently used
//! first; picking returns what the shell must do and changes nothing itself (I7).
//!
//! Positive control (W2): `positive_control_a_missing_variant_fails_coverage` — a variant
//! table that forgets one `EditorCommand` variant fails the coverage check, which reads
//! the variants `EditorCommand` really has (reflection), not a hand-kept list.

mod common;

use std::rc::Rc;
use std::sync::Arc;

use bevy_reflect::{TypeInfo, Typed};
use forge_cmd::{EditorCommand, EntityKey};
use forge_editor::actions::{ActionCx, Invocation, SessionOp, builtin_actions, builtin_registry};
use forge_editor::keymap::{KeyMap, default_keymap};
use forge_editor::palette::{
    CommandInfo, Palette, PaletteOutcome, VARIANTS, VariantReach, commands_from_registry,
};
use forge_editor::panels::{EditorPanelHost, PanelCatalog};
use forge_plugin::points::{Command, CommandItem, PresetKind};
use forge_plugin::{Order, PluginId, Registry};
use forge_ui::dock::PanelId;
use forge_ui::testing::Harness;
use forge_ui::widgets::{CommandInvoked, open_command_palette_ui};
use forge_ui::{KeyCode, Modifiers, UiConfig};

fn keymap() -> KeyMap {
    let (m, errs) = KeyMap::from_layers(&default_keymap().unwrap_or_else(|e| panic!("{e}")), &[]);
    assert!(errs.is_empty(), "{errs:?}");
    m
}

fn plugin_commands() -> Registry<Command> {
    let mut r = Registry::<Command>::new();
    let owner = PluginId::new("com.example.rivers").unwrap_or_else(|e| panic!("{e}"));
    r.add(
        owner,
        "rivers.regenerate",
        CommandItem::new(|_b, _a| Ok(())),
        Order::Last,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    r
}

fn commands() -> Vec<CommandInfo> {
    let mut c = commands_from_registry(&plugin_commands());
    c.push(CommandInfo {
        target: "terrain.raise".into(),
        fields: Some(vec!["entity".into(), "height".into()]),
    });
    c
}

fn palette(cx: &ActionCx) -> Palette {
    let x = common::load(&[]);
    let host = EditorPanelHost::new(
        common::panels(&x),
        PresetKind::ThreeD,
        PanelCatalog::default(),
    );
    let actions = builtin_registry().unwrap_or_else(|e| panic!("{e}"));
    Palette::build(&actions, &keymap(), &host.panels(), &commands(), cx)
}

/// Every variant `EditorCommand` has, read by reflection.
fn real_variants() -> Vec<String> {
    match EditorCommand::type_info() {
        TypeInfo::Enum(e) => e.variant_names().iter().map(|s| s.to_string()).collect(),
        other => panic!("EditorCommand is not an enum: {other:?}"),
    }
}

fn coverage_problems(table: &[(&str, VariantReach)]) -> Vec<String> {
    real_variants()
        .into_iter()
        .filter(|v| !table.iter().any(|(n, _)| n == v))
        .map(|v| format!("EditorCommand::{v} is not reachable from the palette"))
        .collect()
}

#[test]
fn palette_reaches_every_editor_command_variant() {
    let p = coverage_problems(VARIANTS);
    assert!(p.is_empty(), "{p:#?}");
    // And the table names no variant that does not exist.
    let real = real_variants();
    for (n, _) in VARIANTS {
        assert!(
            real.iter().any(|r| r == n),
            "{n} is not an EditorCommand variant"
        );
    }
}

#[test]
fn positive_control_a_missing_variant_fails_coverage() {
    let partial: Vec<(&str, VariantReach)> = VARIANTS
        .iter()
        .copied()
        .filter(|(n, _)| *n != "SetSetting")
        .collect();
    let p = coverage_problems(&partial);
    assert_eq!(
        p,
        vec!["EditorCommand::SetSetting is not reachable from the palette".to_string()]
    );
}

#[test]
fn every_action_panel_and_command_is_listed_with_its_chord() {
    let p = palette(&ActionCx::default());
    let has = |key: &str| p.entries().iter().find(|e| e.key == key);
    for (id, _) in builtin_actions() {
        assert!(
            has(&format!("action:{id}")).is_some(),
            "action {id} missing"
        );
    }
    for (id, title, ..) in forge_editor::stand_in::PANELS {
        let e = has(&format!("panel:{id}")).unwrap_or_else(|| panic!("panel {id} missing"));
        assert_eq!(e.label, format!("Open: {title}"));
    }
    for (name, reach) in VARIANTS {
        if !matches!(reach, VariantReach::PerHandler) {
            assert!(has(&format!("command:{name}")).is_some(), "{name} missing");
        }
    }
    assert!(has("invoke:rivers.regenerate").is_some());
    assert!(has("invoke:terrain.raise").is_some_and(|e| e.label.ends_with('\u{2026}')));
    // Chords are shown.
    let pal = has("action:forge.palette.open").unwrap_or_else(|| panic!("no palette action"));
    assert_eq!(pal.chord.as_deref(), Some("Ctrl+Shift+P"));
    let console = has("panel:forge.console").unwrap_or_else(|| panic!("no console"));
    assert_eq!(console.chord.as_deref(), Some("Ctrl+`"));
    // Unavailable entries are listed, marked, and ranked last.
    let del = has("action:forge.edit.delete").unwrap_or_else(|| panic!("no delete"));
    assert!(!del.enabled, "nothing selected: delete is unavailable");
    let items = p.pick_items();
    let d = items
        .iter()
        .find(|i| i.id == "action:forge.edit.delete")
        .unwrap_or_else(|| panic!("no item"));
    assert_eq!(d.detail, "not available here");
    assert!(d.boost < 0);
}

#[test]
fn fuzzy_search_finds_by_initials_and_recent_entries_rank_first() {
    let mut p = palette(&ActionCx::default());
    let top = |p: &Palette, q: &str| {
        p.search(q)
            .first()
            .map(|i| p.entries()[*i].key.clone())
            .unwrap_or_default()
    };
    assert_eq!(top(&p, "open console"), "panel:forge.console");
    assert_eq!(top(&p, "opprof"), "panel:forge.profiler");
    assert!(p.search("zzzqqq").is_empty());
    // Use the light theme once: it now leads the list, and leads "theme".
    let actions = builtin_registry().unwrap_or_else(|e| panic!("{e}"));
    let out = p
        .pick("action:forge.theme.light", &actions, &ActionCx::default())
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        out,
        PaletteOutcome::Run(Invocation::Session(SessionOp::SetTheme(
            "forge.light".into()
        )))
    );
    assert_eq!(top(&p, ""), "action:forge.theme.light");
    assert_eq!(top(&p, "theme"), "action:forge.theme.light");
    assert_eq!(p.recent(), ["action:forge.theme.light".to_string()]);
}

#[test]
fn picking_returns_what_to_do_and_changes_nothing() {
    let cx = ActionCx {
        selection: vec![EntityKey(7), EntityKey(9)],
        ..ActionCx::default()
    };
    let mut p = palette(&cx);
    let actions = builtin_registry().unwrap_or_else(|e| panic!("{e}"));
    let run = |p: &mut Palette, key: &str| {
        p.pick(key, &actions, &cx)
            .unwrap_or_else(|e| panic!("{key}: {e}"))
    };
    let cmds = |o: PaletteOutcome| match o {
        PaletteOutcome::Run(Invocation::Commands { commands, .. }) => commands,
        other => panic!("not commands: {other:?}"),
    };
    assert_eq!(
        cmds(run(&mut p, "command:Spawn")),
        vec![EditorCommand::Spawn {
            name: "Entity".into(),
            parent: Some(EntityKey(7))
        }]
    );
    assert_eq!(
        cmds(run(&mut p, "command:Despawn")),
        vec![
            EditorCommand::Despawn {
                entity: EntityKey(7)
            },
            EditorCommand::Despawn {
                entity: EntityKey(9)
            }
        ]
    );
    assert_eq!(
        cmds(run(&mut p, "action:forge.edit.delete")).len(),
        2,
        "the delete action builds the same commands"
    );
    assert_eq!(
        run(&mut p, "command:Rename"),
        PaletteOutcome::NeedsArguments {
            command: "Rename".into(),
            fields: vec!["entity".into(), "name".into()]
        }
    );
    assert_eq!(
        cmds(run(&mut p, "invoke:rivers.regenerate")),
        vec![EditorCommand::Invoke {
            target: "rivers.regenerate".into(),
            args: "{}".into()
        }]
    );
    assert!(matches!(
        run(&mut p, "invoke:terrain.raise"),
        PaletteOutcome::NeedsArguments { .. }
    ));
    assert_eq!(
        run(&mut p, "panel:forge.profiler"),
        PaletteOutcome::OpenPanel(PanelId::new("forge.profiler"))
    );
    let e = p
        .pick("action:forge.edit.undo", &actions, &cx)
        .expect_err("nothing to undo");
    assert_eq!(e.code(), "EDITOR-0004");
    // What the palette builds are ordinary bus commands: only the bus changes the project
    // (I7), and the change is one undoable transaction.
    let empty = ActionCx::default();
    let mut p2 = palette(&empty);
    let commands = match p2
        .pick("command:Spawn", &actions, &empty)
        .unwrap_or_else(|e| panic!("{e}"))
    {
        PaletteOutcome::Run(Invocation::Commands { commands, .. }) => commands,
        other => panic!("{other:?}"),
    };
    let mut bus = forge_cmd::Bus::new();
    let issuer = forge_cmd::Issuer::Human {
        user: "test".into(),
    };
    let txn = bus.begin("Create entity", issuer.clone());
    for c in commands {
        let env = bus.envelope_in(txn, issuer.clone(), c);
        forge_cmd::CommandSink::apply(&mut bus, env).unwrap_or_else(|e| panic!("{e:?}"));
    }
    bus.commit(txn).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(
        bus.project().len(),
        1,
        "the palette's command created one entity"
    );
    let _ = Arc::new(());
}

#[test]
fn the_palette_popup_runs_an_action_by_keyboard() {
    let cx = ActionCx::default();
    let mut p = palette(&cx);
    let mut h = Harness::with_host()
        .map(|(h, _)| h)
        .unwrap_or_else(|e| panic!("{e}"));
    let _ = UiConfig::default();
    let root = h.ui.root();
    let popup = open_command_palette_ui(&mut h.ui, root, Rc::new(p.pick_items()));
    assert!(popup.is_some());
    h.settle();
    h.type_text("save layout");
    h.press(KeyCode::Enter, Modifiers::NONE);
    let picked = h.take::<CommandInvoked>();
    assert_eq!(
        picked,
        vec![CommandInvoked {
            id: "action:forge.layout.save_as".into()
        }]
    );
    let actions = builtin_registry().unwrap_or_else(|e| panic!("{e}"));
    let out = p
        .pick(&picked[0].id, &actions, &cx)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        out,
        PaletteOutcome::Run(Invocation::Session(SessionOp::SaveLayoutAs))
    );
}
