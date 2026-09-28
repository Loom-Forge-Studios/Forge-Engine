//! **Plugin manager** (`forge.plugins`, Ch.21 §21.21, Ch.32, Ch.38; DoD M2-52).
//!
//! * Installed plugins — every plugin the loader installed or left out, first-party ones
//!   too (I16) — and the plugins the project's set names that are not installed here.
//! * Capabilities **requested vs granted** per plugin; Grant and Revoke are the human-only
//!   security commands `forge.plugin.grant` / `revoke` (§21.18): a grant reaches a hosted
//!   WASM plugin at its next call, and undo never grants.
//! * Enable, disable and remove are plugin-set commands (undoable project state).
//! * `replaces` shown with **both names** (who replaces what, from whom), and every conflict
//!   naming both plugins — for an index entry too, before it is added.
//! * Browse the index and add a plugin: the `forge add` equivalent (fetch, sandbox-check,
//!   cache — the one direct write, user config — then the `forge.plugin.add` command).
//! * **Held security settings** (WP-21, ADR 0043 Amendment 2): plugin grants and default
//!   automation capabilities a project carried that did not take effect because an automation
//!   session or a script opened, cloned or pulled it — listed here as elsewhere, with the same
//!   human-only Accept and Discard (`forge.security.accept_held` / `discard_held`), which the core
//!   checks against the proposal it holds. A plugin's grants are this panel's business, so the
//!   person managing plugins sees what is waiting for them.
//! * **Project trust** (WP-34, `forge_editor::trust`): when the open project carries plugins
//!   or plugin grants, what it carries and whether it is trusted, with *Trust this project* /
//!   *Don't trust*. Until it is trusted its plugins are not loaded and its grants are held.
//!   The answer is the person's user config (not a bus command, so no automation session can give
//!   it); trusting also accepts the grants a person's open held, through the human-only command.

use std::cell::RefCell;
use std::rc::Rc;

use forge_editor::connect::capability_label;
use forge_editor::connect::plugins::{IndexEntry, PluginRow};
use forge_editor::panels::PanelCx;
use forge_editor::security;
use forge_plugin::Capability;
use forge_ui::widgets::{LabelKind, Pressed, SearchChanged, SearchField, SelectionChanged};
use forge_ui::{NodeStyle, Role, Ui, WidgetId};

use crate::ui::{bar, button, fill, group, list, selected, set, show, text, warn};

#[derive(Default)]
struct State {
    rows: Vec<PluginRow>,
    index: Vec<IndexEntry>,
    /// Security settings a non-human's project load or pull held back (WP-21).
    held: Option<security::HeldSecurity>,
}

/// The key of plugin row `i`.
fn row_key(i: usize) -> u64 {
    (i as u64 + 1) << 16
}

fn row_of(key: u64) -> Option<usize> {
    ((key >> 16) as usize).checked_sub(1)
}

/// A capability's key in the capability list.
fn cap_key(c: Capability) -> u64 {
    Capability::ALL
        .iter()
        .position(|x| *x == c)
        .map_or(0, |i| i as u64 + 1)
}

fn cap_of(key: u64) -> Option<Capability> {
    (key as usize)
        .checked_sub(1)
        .and_then(|i| Capability::ALL.get(i).copied())
}

struct Parts {
    installed: WidgetId,
    caps: WidgetId,
    detail: forge_ui::Signal<String>,
    index: WidgetId,
    preview: forge_ui::Signal<String>,
    held: WidgetId,
}

/// Fill the capability list and the detail line for the selected plugin.
fn show_detail(ui: &mut Ui, st: &State, p: &Parts) {
    let sel = selected(ui, p.installed).and_then(row_of);
    let Some(r) = sel.and_then(|i| st.rows.get(i)) else {
        fill(ui, p.caps, Vec::new());
        set(
            ui,
            p.detail,
            forge_ui::tr!("Select a plugin to see and change its capabilities.").into(),
        );
        return;
    };
    let mut rows = Vec::new();
    for c in r.requested.iter().chain(&r.extra_granted) {
        let granted = r.granted.contains(c) || r.extra_granted.contains(c);
        let state = match (granted, r.requested.contains(c)) {
            (true, true) => forge_ui::tr!("granted"),
            (false, _) => forge_ui::tr!("requested \u{2014} not granted"),
            (true, false) => forge_ui::tr!("granted but no longer requested (revoke it)"),
        };
        let what = capability_label(*c);
        rows.push((
            cap_key(*c),
            format!("{what} \u{2014} {state}"),
            !granted,
            None,
        ));
    }
    fill(ui, p.caps, rows);
    let mut lines = vec![format!(
        "{} {}: {}{}",
        r.id,
        r.version,
        match &r.in_project {
            Some(e) => forge_ui::trf!(
                "in the project ({state}, from {source})",
                state = if e.enabled {
                    forge_ui::tr!("enabled")
                } else {
                    forge_ui::tr!("disabled")
                },
                source = if e.source.is_empty() {
                    forge_ui::tr!("this editor")
                } else {
                    &e.source
                }
            ),
            None => forge_ui::tr!("compiled into this editor (not in the project's set)").into(),
        },
        if r.pending_restart() {
            forge_ui::tr!(" \u{b7} takes effect when the editor next starts")
        } else {
            ""
        }
    )];
    if r.requested.is_empty() {
        lines.push(forge_ui::tr!("It requests no capabilities.").into());
    }
    set(ui, p.detail, lines.join("\n"));
}

fn fill_installed(ui: &mut Ui, st: &State, p: &Parts) {
    let mut rows = Vec::new();
    for (i, r) in st.rows.iter().enumerate() {
        let k = row_key(i);
        rows.push((k, r.label(), false, None));
        let mut j = 0u64;
        let mut child = |label: String, muted: bool, rows: &mut Vec<_>| {
            j += 1;
            rows.push((k | j, label, muted, Some(k)));
        };
        for line in &r.replaces {
            child(line.clone(), false, &mut rows);
        }
        for line in &r.conflicts {
            child(
                forge_ui::trf!("\u{26a0} conflict: {line}", line),
                false,
                &mut rows,
            );
        }
        if !r.provides.is_empty() {
            child(
                forge_ui::trf!("provides {points}", points = r.provides.join(", ")),
                true,
                &mut rows,
            );
        }
    }
    fill(ui, p.installed, rows);
}

fn fill_index(ui: &mut Ui, st: &State, p: &Parts) {
    let rows = st
        .index
        .iter()
        .enumerate()
        .map(|(i, e)| (i as u64 + 1, e.label(), false, None))
        .collect();
    fill(ui, p.index, rows);
}

pub fn build(cx: &mut PanelCx) {
    cx.never_empty(forge_ui::tr!(
        "the loaded plugins, first-party ones included"
    ));
    cx.add_live(|pb| {
        let services = pb.services();
        let space = pb.b.theme_ref().space;
        let root = pb.b.add(
            pb.parent,
            "content",
            NodeStyle::column(space[1])
                .padding(space[2])
                .grow(1.0)
                .scrollable(),
            forge_ui::widgets::Container::new(Role::Group).labelled(forge_ui::tr!("Plugins")),
        )?;
        let head = pb.b.signal(String::new());
        text(pb, root, "head", head, LabelKind::Heading)?;

        let ig = group(pb, root, "installed_group", forge_ui::tr!("Installed"))?;
        let installed = list(pb, ig, "installed", forge_ui::tr!("Installed plugins"), 220.0)?;
        let ibar = bar(pb, ig, "plugin_bar", forge_ui::tr!("Plugin set"))?;
        let enable = button(pb, ibar, "enable", forge_ui::tr!("Enable"))?;
        let disable = button(pb, ibar, "disable", forge_ui::tr!("Disable"))?;
        let remove = button(pb, ibar, "remove", forge_ui::tr!("Remove from project"))?;
        let detail = pb.b.signal(String::new());
        text(pb, ig, "detail", detail, LabelKind::Body)?;

        let cg = group(pb, root, "caps_group", forge_ui::tr!("Capabilities"))?;
        let caps = list(pb, cg, "caps", forge_ui::tr!("Capabilities requested and granted"), 140.0)?;
        let cbar = bar(pb, cg, "caps_bar", forge_ui::tr!("Grant or revoke"))?;
        let grant = pb.b.add(
            cbar,
            "grant",
            NodeStyle::leaf(),
            forge_ui::widgets::Button::new(forge_ui::tr!("Grant")).primary(),
        )?;
        let revoke = button(pb, cbar, "revoke", forge_ui::tr!("Revoke"))?;
        text(
            pb,
            cg,
            "caps_hint",
            forge_ui::tr!("Grants are yours alone to make: an automation session or a plugin can never grant itself, and undo never grants (an undone grant cannot be redone; a revoke cannot be undone)."),
            LabelKind::Small,
        )?;

        // WP-34: whether the open project is trusted to run its plugins with its grants.
        let tg = group(pb, root, "trust_group", forge_ui::tr!("Project trust"))?;
        let trust_head = pb.b.signal(String::new());
        text(pb, tg, "trust_head", trust_head, LabelKind::Small)?;
        let carried = list(pb, tg, "trust_list", forge_ui::tr!("What the project carries"), 60.0)?;
        let tbar = bar(pb, tg, "trust_bar", forge_ui::tr!("Trust or not"))?;
        let trust = pb.b.add(
            tbar,
            "trust",
            NodeStyle::leaf(),
            forge_ui::widgets::Button::new(forge_ui::tr!("Trust this project")),
        )?;
        let distrust = button(pb, tbar, "distrust", forge_ui::tr!("Don't trust"))?;
        // WP-21: what a non-human's load or pull held back, answered here as elsewhere.
        let hg = group(pb, root, "held_group", forge_ui::tr!("Held security settings"))?;
        let held_head = pb.b.signal(String::new());
        text(pb, hg, "held_head", held_head, LabelKind::Small)?;
        let held = list(
            pb,
            hg,
            "held",
            forge_ui::tr!("Security settings held until a person decides"),
            80.0,
        )?;
        let hbar = bar(pb, hg, "held_bar", forge_ui::tr!("Accept or discard"))?;
        let accept_held = pb.b.add(
            hbar,
            "accept_held",
            NodeStyle::leaf(),
            forge_ui::widgets::Button::new(forge_ui::tr!("Accept")),
        )?;
        let discard_held = button(pb, hbar, "discard_held", forge_ui::tr!("Discard"))?;

        let xg = group(pb, root, "index_group", forge_ui::tr!("Browse the index"))?;
        let query = pb.b.signal(String::new());
        pb.b.add(
            xg,
            "index_query",
            NodeStyle::leaf().width(280.0),
            SearchField::new(query, forge_ui::tr!("Search the plugin index")),
        )?;
        let index = list(pb, xg, "index", forge_ui::tr!("Index"), 120.0)?;
        let preview = pb.b.signal(String::new());
        text(pb, xg, "preview", preview, LabelKind::Body)?;
        let xbar = bar(pb, xg, "index_bar", forge_ui::tr!("Add"))?;
        let add = pb.b.add(
            xbar,
            "add",
            NodeStyle::leaf(),
            forge_ui::widgets::Button::new(forge_ui::tr!("Add to project")).primary(),
        )?;
        let status = pb.b.signal(String::new());
        text(pb, root, "status", status, LabelKind::Muted)?;
        // The id of the trust question the group shows (WP-36): an answer is to that question,
        // refused if it changed since (a pull landed between the look and the press).
        let shown_trust: Rc<std::cell::Cell<Option<u64>>> = Rc::default();
        for (btn, answer) in [
            (trust, forge_editor::trust::Trust::Trusted),
            (distrust, forge_editor::trust::Trust::Untrusted),
        ] {
            let shown = shown_trust.clone();
            pb.on(btn, move |act, _: &Pressed| {
                let Some(core) = act.services.trust_core() else {
                    set(act.ui, status, forge_ui::tr!("This editor runs no project of its own.").into());
                    return;
                };
                let Some(seen) = shown.get() else {
                    set(
                        act.ui,
                        status,
                        forge_ui::tr!(
                            "The open project carries no plugins, plugin grants or wider automation capabilities: nothing to trust."
                        )
                        .into(),
                    );
                    return;
                };
                let user = match forge_editor::shell::gui_issuer() {
                    forge_cmd::Issuer::Human { user } => user,
                    other => other.tag(),
                };
                match forge_editor::core::EditorCore::decide_trust_seen(&core, &user, answer, seen) {
                    Ok(line) => set(act.ui, status, format!("{line}.")),
                    Err(e) => {
                        set(act.ui, status, forge_ui::trf!("Not changed: {e}", e = e));
                        act.session.refuse(forge_ui::tr!("The project's trust was not changed"), &e);
                    }
                }
                act.want_turn();
            });
        }

        let footer = forge_ui::trf!(
            "Index: {index} \u{b7} downloads: {downloads} \u{b7} an added WASM plugin installs while the editor runs, a disable or a removal when it next starts; a grant or a revoke at once.",
            index = forge_ui::l10n::tr_str(&services.connect.index.backend()),
            downloads = services
                .connect
                .cache
                .borrow()
                .dir()
                .map_or(forge_ui::tr!("kept in memory (no user config directory)").to_string(), |d| {
                    d.display().to_string()
                })
        );
        text(pb, root, "footer", footer, LabelKind::Small)?;

        let parts = Rc::new(Parts {
            installed,
            caps,
            detail,
            index,
            preview,
            held,
        });
        let st = Rc::new(RefCell::new(State {
            rows: Vec::new(),
            index: services.connect.index.search(""),
            held: None,
        }));

        let selected_row = |ui: &mut Ui, st: &State, p: &Parts| -> Option<PluginRow> {
            selected(ui, p.installed)
                .and_then(row_of)
                .and_then(|i| st.rows.get(i).cloned())
        };

        {
            let (st, p) = (st.clone(), parts.clone());
            pb.on(installed, move |act, _: &SelectionChanged| {
                show_detail(act.ui, &st.borrow(), &p);
            });
        }
        for (btn, on) in [(enable, true), (disable, false)] {
            let (st, p) = (st.clone(), parts.clone());
            pb.on(btn, move |act, _: &Pressed| {
                if let Some(r) = selected_row(act.ui, &st.borrow(), &p) {
                    act.cmd.emit(security::plugin_enabled_command(&r.id, on));
                }
            });
        }
        {
            let (st, p) = (st.clone(), parts.clone());
            pb.on(remove, move |act, _: &Pressed| {
                if let Some(r) = selected_row(act.ui, &st.borrow(), &p) {
                    if r.in_project.is_some() {
                        act.cmd.emit(security::plugin_remove_command(&r.id));
                    } else {
                        act.session.notify(forge_editor::notify::Level::Info, forge_ui::tr!("Not in the project"), &forge_ui::trf!("{id} is compiled into this editor; disable it instead.", id = r.id));
                    }
                }
            });
        }
        for (btn, is_grant) in [(grant, true), (revoke, false)] {
            let (st, p) = (st.clone(), parts.clone());
            pb.on(btn, move |act, _: &Pressed| {
                let row = selected_row(act.ui, &st.borrow(), &p);
                let cap = selected(act.ui, p.caps).and_then(cap_of);
                let (Some(r), Some(cap)) = (row, cap) else {
                    act.session.notify(forge_editor::notify::Level::Info, forge_ui::tr!("Pick a plugin and a capability"), forge_ui::tr!("Select a plugin, then one of its capabilities."));
                    return;
                };
                act.cmd.emit(if is_grant {
                    security::plugin_grant_command(&r.id, cap)
                } else {
                    security::plugin_revoke_command(&r.id, cap)
                });
            });
        }
        for (btn, ok) in [(accept_held, true), (discard_held, false)] {
            let st = st.clone();
            pb.on(btn, move |act, _: &Pressed| {
                let Some(h) = st.borrow().held.clone() else {
                    set(act.ui, status, forge_ui::tr!("No security settings are held.").into());
                    return;
                };
                // Human-only commands: the core checks they answer the proposal it holds.
                act.cmd.emit(if ok {
                    security::accept_held_command(&h)
                } else {
                    security::discard_held_command(&h)
                });
                set(
                    act.ui,
                    status,
                    forge_ui::trf!("{outcome} {items_count} held security setting(s).", outcome = if ok { forge_ui::tr!("Accepted") } else { forge_ui::tr!("Discarded") }, items_count = h.items.len()),
                );
                act.want_turn();
            });
        }
        {
            let (st, p) = (st.clone(), parts.clone());
            pb.on(index, move |act, _: &SelectionChanged| {
                let st = st.borrow();
                let e = selected(act.ui, p.index)
                    .and_then(|k| (k as usize).checked_sub(1))
                    .and_then(|i| st.index.get(i));
                let text = match e {
                    None => String::new(),
                    Some(e) => {
                        let m = act.services.connect.plugins.borrow();
                        let mut lines = vec![e.label()];
                        lines.extend(m.replaces_of(&e.manifest));
                        let c = m.conflicts_with(&e.manifest);
                        if c.is_empty() {
                            lines.push(forge_ui::tr!("No conflict with the installed plugins.").into());
                        } else {
                            lines.extend(
                                c.into_iter()
                                    .map(|c| forge_ui::trf!("\u{26a0} conflict: {c}", c)),
                            );
                        }
                        lines.join("\n")
                    }
                };
                set(act.ui, p.preview, text);
            });
        }
        {
            let (st, p) = (st.clone(), parts.clone());
            pb.on(add, move |act, _: &Pressed| {
                let entry = {
                    let st = st.borrow();
                    selected(act.ui, p.index)
                        .and_then(|k| (k as usize).checked_sub(1))
                        .and_then(|i| st.index.get(i).cloned())
                };
                let Some(e) = entry else {
                    set(act.ui, status, forge_ui::tr!("Select a plugin in the index first.").into());
                    return;
                };
                let (id, version) = (e.manifest.id.to_string(), e.manifest.version.to_string());
                let c = &act.services.connect;
                let r = c.plugins.borrow().prepare_add(
                    &*c.index,
                    &mut c.cache.borrow_mut(),
                    &id,
                    &version,
                );
                match r {
                    Ok(cmd) => {
                        act.cmd.emit(cmd);
                        set(
                            act.ui,
                            status,
                            forge_ui::trf!("Added {id} {version} to the project. A WASM plugin installs the next time the editor wakes (a source plugin at the next start); grant what it requests below.", id, version),
                        );
                    }
                    Err(e) => {
                        set(act.ui, status, forge_ui::trf!("{id} was not added: {e}", id, e));
                        warn(act.session, forge_ui::tr!("The plugin was not added"), &e);
                    }
                }
            });
        }
        {
            let (st, p) = (st.clone(), parts.clone());
            pb.on(xg, move |act, e: &SearchChanged| {
                st.borrow_mut().index = act.services.connect.index.search(&e.query);
                fill_index(act.ui, &st.borrow(), &p);
            });
        }

        let mut seen = (u64::MAX, u64::MAX, u64::MAX, u64::MAX);
        let mut first = true;
        pb.sync(root, move |s| {
            // A discard of held settings changes no project state: the proposal's id is part
            // of what the panel follows (its id only; the proposal is read when it changed).
            // So is the open project's trust question (WP-34), which is user config.
            let held_id = s.services.connect.held_security_id();
            let trust_core = s.services.trust_core();
            let trust_id = trust_core
                .as_ref()
                .and_then(forge_editor::core::EditorCore::project_trust_id);
            let rev = (
                s.mirror.settings_revision(),
                s.services.connect.plugins.borrow().revision(),
                held_id.unwrap_or(0),
                trust_id.unwrap_or(0),
            );
            if first {
                first = false;
                fill_index(s.ui, &st.borrow(), &parts);
            }
            if rev == seen {
                return Ok(());
            }
            seen = rev;
            let rows = s.services.connect.plugins.borrow().rows(s.mirror);
            let in_project = rows.iter().filter(|r| r.in_project.is_some()).count();
            let hosted = rows
                .iter()
                .filter(|r| {
                    matches!(
                        r.state,
                        Some(forge_editor::connect::plugins::InstallState::Hosted { .. })
                    )
                })
                .count();
            set(
                s.ui,
                head,
                forge_ui::trf!("{rows_count} plugin(s) \u{b7} {in_project} in the project's set \u{b7} {hosted} hosted in the WASM sandbox", rows_count = rows.len(), in_project, hosted),
            );
            st.borrow_mut().rows = rows;
            let held_now = s.services.connect.held_security();
            set(
                s.ui,
                held_head,
                held_now.as_ref().map_or_else(
                    || forge_ui::tr!("Nothing held: plugin grants a project carries take effect when a person opens it; when an automation session or a script opens or pulls it, new ones wait here.").to_string(),
                    security::HeldSecurity::label,
                ),
            );
            fill(
                s.ui,
                parts.held,
                held_now
                    .iter()
                    .flat_map(|h| h.items.iter())
                    .enumerate()
                    .map(|(i, item)| (i as u64 + 1, item.line(), false, None))
                    .collect(),
            );
            st.borrow_mut().held = held_now;
            let trust_now = trust_core
                .as_ref()
                .and_then(forge_editor::core::EditorCore::project_trust);
            set(
                s.ui,
                trust_head,
                match (&trust_now, &trust_core) {
                    (Some(t), Some(core)) => forge_ui::trf!(
                        "{label} Your answer is {answer}.",
                        label = t.label(),
                        answer = forge_editor::core::EditorCore::trust_book_describe(core)
                    ),
                    _ => forge_ui::tr!(
                        "The open project carries no plugins, plugin grants or wider automation capabilities: nothing to trust."
                    )
                    .to_string(),
                },
            );
            fill(
                s.ui,
                carried,
                trust_now
                    .iter()
                    .flat_map(forge_editor::trust::ProjectTrust::lines)
                    .enumerate()
                    .map(|(i, line)| (i as u64 + 1, line, false, None))
                    .collect(),
            );
            // Nothing carried, nothing to answer: no buttons (an answer then would record a
            // decision about nothing).
            shown_trust.set(trust_now.as_ref().map(|t| t.id));
            show(s.ui, tbar, trust_now.is_some());
            show(s.ui, carried, trust_now.is_some());
            fill_installed(s.ui, &st.borrow(), &parts);
            show_detail(s.ui, &st.borrow(), &parts);
            Ok(())
        });
        pb.want_turn();
        Ok(())
    });
}
